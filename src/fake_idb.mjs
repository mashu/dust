// A browser's IndexedDB, BroadcastChannel and localStorage, made up in memory
// for the history's page script (src/persist/idb.js) to run on in Node, as
// the tests have no browser (`crate::testing::on_fake_page`, which puts them
// on the page). Only what that script uses, and as a browser does it:
//
// - every request answers a moment later, one at a time, in the order asked;
// - a transaction takes requests only while it is active: in the moment it
//   was made, and in its requests' callbacks; it commits once the last
//   request is answered and nothing more was asked;
// - a transaction that writes waits for every one made before it on the
//   same stores, and is made whole or not at all: an error, an abort, or no
//   room left for the site at commit leaves nothing of it;
// - keys are strings, and sorted as a browser sorts them.
//
// `fakeIdb`, on the page, is what a test does to it from outside: the room
// the site has (`quota`, in characters), an open that fails (`failOpen`, the
// error's name) or never answers (`hangOpen`), the browser clearing the
// site's data (`evict`), a newer Dust opening it in another tab (`upgrade`),
// what it holds (`contents`, `plant`), and how many changes it took
// (`committed`).

const later = (run) => setImmediate(run);

const fault = (name, message) => new DOMException(message, name);

class KeyRange {
  constructor(lower, lowerOpen) {
    this.lower = lower;
    this.lowerOpen = lowerOpen;
  }

  static lowerBound(lower, open = false) {
    return new KeyRange(lower, open);
  }

  includes(key) {
    return this.lowerOpen ? key > this.lower : key >= this.lower;
  }
}

// Whether `key` is what `query` asks for: everything, one key, or a range.
const asked = (query, key) => {
  if (query === undefined || query === null) {
    return true;
  }
  if (query instanceof KeyRange) {
    return query.includes(key);
  }
  return key === query;
};

class Request {
  constructor(transaction) {
    this.transaction = transaction;
    this.result = undefined;
    this.error = null;
    this.readyState = "pending";
    this.onsuccess = null;
    this.onerror = null;
  }
}

// One database: its version, its stores (each a Map of key to value), the
// connections open to it, and its transactions, oldest first.
class Database {
  constructor(name) {
    this.name = name;
    this.version = 0;
    this.stores = new Map();
    this.connections = new Set();
    this.transactions = [];
  }

  // Start each transaction nothing made before it holds up.
  schedule() {
    for (const [at, tx] of this.transactions.entries()) {
      if (tx.state !== "waiting") {
        continue;
      }
      const held = this.transactions
        .slice(0, at)
        .some(
          (before) =>
            (before.mode === "readwrite" || tx.mode === "readwrite") &&
            before.scope.some((store) => tx.scope.includes(store)),
        );
      if (!held) {
        tx.start();
      }
    }
  }

  finished(tx) {
    this.transactions = this.transactions.filter((held) => held !== tx);
    this.schedule();
  }
}

class Store {
  constructor(tx, name) {
    this.transaction = tx;
    this.name = name;
  }

  ask(operation, writes = false) {
    return this.transaction.ask(this.name, operation, writes);
  }

  get(key) {
    return this.ask((data) => data.get(key));
  }

  count(query) {
    return this.ask((data) => [...data.keys()].filter((key) => asked(query, key)).length);
  }

  getAll(query, count) {
    return this.ask((data) => this.#entries(data, query, count).map(([, value]) => value));
  }

  getAllKeys(query, count) {
    return this.ask((data) => this.#entries(data, query, count).map(([key]) => key));
  }

  put(value, key) {
    return this.ask((data) => {
      data.set(key, value);
      return key;
    }, true);
  }

  add(value, key) {
    return this.ask((data) => {
      if (data.has(key)) {
        throw fault("ConstraintError", `there is a record ${key} already`);
      }
      data.set(key, value);
      return key;
    }, true);
  }

  delete(key) {
    return this.ask((data) => {
      data.delete(key);
    }, true);
  }

  #entries(data, query, count) {
    const sorted = [...data.entries()]
      .filter(([key]) => asked(query, key))
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    return count ? sorted.slice(0, count) : sorted;
  }
}

class Transaction {
  constructor(connection, scope, mode, factory) {
    this.db = connection;
    this.database = connection.database;
    this.factory = factory;
    this.scope = scope;
    this.mode = mode;
    this.state = "waiting";
    this.error = null;
    this.oncomplete = null;
    this.onabort = null;
    this.onerror = null;
    this.requests = [];
    // What it changed, by store, until it commits.
    this.working = new Map();
    // Requests are taken only in the moment it was made, and in callbacks.
    this.active = true;
    later(() => {
      this.active = false;
    });
  }

  objectStore(name) {
    if (!this.scope.includes(name)) {
      throw fault("NotFoundError", `${name} is not in this transaction`);
    }
    return new Store(this, name);
  }

  ask(store, operation, writes) {
    if (this.state === "done") {
      throw fault("TransactionInactiveError", "the transaction is over");
    }
    if (!this.active) {
      throw fault("TransactionInactiveError", "the transaction is not active");
    }
    if (writes && this.mode !== "readwrite") {
      throw fault("ReadOnlyError", "the transaction only reads");
    }
    const request = new Request(this);
    this.requests.push({ request, store, operation });
    return request;
  }

  start() {
    this.state = "running";
    later(() => this.next());
  }

  data(store) {
    if (this.mode === "readwrite") {
      if (!this.working.has(store)) {
        this.working.set(store, new Map(this.database.stores.get(store)));
      }
      return this.working.get(store);
    }
    return this.database.stores.get(store);
  }

  // Answer the next request, or commit once there are none.
  next() {
    if (this.state !== "running") {
      return;
    }
    const first = this.requests.shift();
    if (!first) {
      this.commit();
      return;
    }
    const { request, store, operation } = first;
    request.readyState = "done";
    let failure = null;
    try {
      request.result = operation(this.data(store));
    } catch (error) {
      failure = error;
    }
    this.active = true;
    if (failure === null) {
      request.onsuccess?.({ type: "success", target: request });
      this.active = false;
    } else {
      request.error = failure;
      let prevented = false;
      request.onerror?.({
        type: "error",
        target: request,
        preventDefault() {
          prevented = true;
        },
      });
      this.active = false;
      if (!prevented) {
        this.fail(failure);
        return;
      }
    }
    later(() => this.next());
  }

  commit() {
    if (this.mode === "readwrite") {
      const room = this.factory.quota;
      if (Number.isFinite(room) && this.factory.used(this) > room) {
        this.fail(fault("QuotaExceededError", "the site has no room left"));
        return;
      }
      for (const [store, data] of this.working) {
        this.database.stores.set(store, data);
      }
      this.factory.committed += 1;
    }
    this.state = "done";
    this.database.finished(this);
    later(() => this.oncomplete?.({ type: "complete", target: this }));
  }

  fail(error) {
    if (this.state === "done") {
      return;
    }
    this.state = "done";
    this.error = error;
    this.working.clear();
    this.database.finished(this);
    later(() => this.onabort?.({ type: "abort", target: this }));
  }

  abort() {
    if (this.state === "done") {
      throw fault("InvalidStateError", "the transaction is over");
    }
    this.fail(fault("AbortError", "the transaction was aborted"));
  }
}

class Connection {
  constructor(database, factory) {
    this.database = database;
    this.factory = factory;
    this.name = database.name;
    this.version = database.version;
    this.closed = false;
    this.upgrading = false;
    this.onversionchange = null;
    this.onclose = null;
    this.objectStoreNames = { contains: (name) => database.stores.has(name) };
  }

  createObjectStore(name) {
    if (!this.upgrading) {
      throw fault("InvalidStateError", "stores are made only as the database is upgraded");
    }
    this.database.stores.set(name, new Map());
  }

  transaction(names, mode = "readonly") {
    if (this.closed) {
      throw fault("InvalidStateError", "the connection is closing");
    }
    const scope = [].concat(names);
    for (const name of scope) {
      if (!this.database.stores.has(name)) {
        throw fault("NotFoundError", `there is no store ${name}`);
      }
    }
    const tx = new Transaction(this, scope, mode, this.factory);
    this.database.transactions.push(tx);
    this.database.schedule();
    return tx;
  }

  close() {
    this.closed = true;
    this.database.connections.delete(this);
  }
}

export class FakeIndexedDB {
  constructor() {
    this.databases = new Map();
    this.quota = Infinity;
    this.failOpen = null;
    this.hangOpen = false;
    this.committed = 0;
  }

  open(name, version) {
    if (version !== undefined && (!Number.isInteger(version) || version < 1)) {
      throw new TypeError("a version is a whole number from 1");
    }
    const request = new Request(null);
    request.onupgradeneeded = null;
    request.onblocked = null;
    later(() => {
      if (this.hangOpen) {
        return;
      }
      const answer = (error) => {
        request.readyState = "done";
        if (error) {
          request.error = error;
          request.onerror?.({ type: "error", target: request, preventDefault() {} });
        } else {
          request.onsuccess?.({ type: "success", target: request });
        }
      };
      if (this.failOpen) {
        answer(fault(this.failOpen, "the browser would not open it"));
        return;
      }
      let database = this.databases.get(name);
      if (!database) {
        database = new Database(name);
        this.databases.set(name, database);
      }
      const wanted = version === undefined ? Math.max(database.version, 1) : version;
      if (wanted < database.version) {
        answer(fault("VersionError", `it is at version ${database.version}`));
        return;
      }
      if (wanted > database.version && !this.#others(database, wanted)) {
        request.onblocked?.({ type: "blocked", target: request });
        return;
      }
      const old = database.version;
      database.version = wanted;
      const connection = new Connection(database, this);
      request.result = connection;
      if (wanted > old) {
        connection.upgrading = true;
        request.onupgradeneeded?.({ type: "upgradeneeded", oldVersion: old, newVersion: wanted, target: request });
        connection.upgrading = false;
      }
      database.connections.add(connection);
      later(() => answer(null));
    });
    return request;
  }

  // Ask every connection open to `database` to let go, for it to go to
  // `version`: whether they all did.
  #others(database, version) {
    for (const other of [...database.connections]) {
      other.onversionchange?.({ type: "versionchange", oldVersion: database.version, newVersion: version, target: other });
    }
    return database.connections.size === 0;
  }

  // How much room the site's databases take, with `tx`'s changes made: the
  // characters of every key and value.
  used(tx) {
    let used = 0;
    for (const database of this.databases.values()) {
      for (const [name, data] of database.stores) {
        const held = tx.database === database && tx.working.has(name) ? tx.working.get(name) : data;
        for (const [key, value] of held) {
          used += String(key).length + (typeof value === "string" ? value.length : JSON.stringify(value).length);
        }
      }
    }
    return used;
  }

  // The browser clears the site's data: every connection is closed under
  // its page, and the database is gone.
  evict(name) {
    const database = this.databases.get(name);
    if (!database) {
      return;
    }
    this.databases.delete(name);
    for (const connection of [...database.connections]) {
      connection.closed = true;
      database.connections.delete(connection);
      later(() => connection.onclose?.({ type: "close", target: connection }));
    }
  }

  // A newer Dust opens `name` at `version` in another tab: whether every
  // page let go of it, so it could.
  upgrade(name, version) {
    const database = this.databases.get(name);
    if (!database || !this.#others(database, version)) {
      return false;
    }
    database.version = version;
    return true;
  }

  // What `store` of `name` holds, by key, in order.
  contents(name, store) {
    const data = this.databases.get(name)?.stores.get(store) ?? new Map();
    return Object.fromEntries([...data.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
  }

  // Put `value` under `key` in `store` of `name`, as another build might
  // have: the database and the store are made where they are not.
  plant(name, store, key, value) {
    let database = this.databases.get(name);
    if (!database) {
      database = new Database(name);
      database.version = 1;
      this.databases.set(name, database);
    }
    if (!database.stores.has(store)) {
      database.stores.set(store, new Map());
    }
    database.stores.get(store).set(key, value);
  }
}

// Each page's channels, by name: a word posted on one reaches every other.
const channels = new Map();

class FakeBroadcastChannel {
  constructor(name) {
    this.name = name;
    this.onmessage = null;
    const named = channels.get(name) ?? new Set();
    named.add(this);
    channels.set(name, named);
  }

  postMessage(data) {
    for (const other of channels.get(this.name) ?? []) {
      if (other !== this) {
        later(() => other.onmessage?.({ data }));
      }
    }
  }

  close() {
    channels.get(this.name)?.delete(this);
  }
}

class FakeStorage {
  #items = new Map();

  get length() {
    return this.#items.size;
  }

  key(at) {
    return [...this.#items.keys()][at] ?? null;
  }

  getItem(key) {
    return this.#items.has(String(key)) ? this.#items.get(String(key)) : null;
  }

  setItem(key, value) {
    this.#items.set(String(key), String(value));
  }

  removeItem(key) {
    this.#items.delete(String(key));
  }
}

// Put them on `page`, as a browser's page has them.
export const install = (page) => {
  const factory = new FakeIndexedDB();
  for (const [name, value] of [
    ["indexedDB", factory],
    ["fakeIdb", factory],
    ["IDBKeyRange", KeyRange],
    ["BroadcastChannel", FakeBroadcastChannel],
    ["localStorage", new FakeStorage()],
  ]) {
    Object.defineProperty(page, name, { value, configurable: true, writable: true });
  }
};
