// The history in a browser's database (src/persist/idb.rs): the body of a
// function that opens IndexedDB `dust_history` for one tab, and hands back
// what the app does with it.
//
// Three stores, and every key and value in them a string:
// - `sessions`: each record by its name, as the build that stored it wrote
//   it, never written over;
// - `deleted`: each tombstone, by the name of the record deleted by hand;
// - `meta`: `layout`, how the history is kept, and `told`, the records this
//   build could not read and has said so.
// Strings, so that whatever a build writes comes back byte for byte, with
// every number and every key it does not know of.
//
// Each change is one transaction, made whole or not at all, and once it is
// made every other tab hears of it: on the BroadcastChannel `dust_history`
// at once, and from a stamp in `localStorage`, which a tab that slept
// through the word reads as it wakes.
//
// Every answer that waits is `{ ok: <what was asked> }`, or why not:
// `{ why, message }`, `why` being "full" (the browser has no room left for
// the site), "newer" (a newer Dust keeps the history, so this one only reads
// it), "unread" (how the history is kept does not read), "raced" (another tab
// changed how it is kept first, so nothing was), "replaced" (a newer Dust
// opened the database in another tab, and this page has to be reloaded to
// write to it) or "failed".
//
// `options.openWaitMs`, for a test: how long the database has to open.

const NAME = "dust_history";
const VERSION = 1;
const SESSIONS = "sessions";
const DELETED = "deleted";
const META = "meta";
const STAMP = "dust_history_stamp";
// The oldest layout a build has to know to write to the history
// (src/persist/legacy.rs, `COMPATIBLE`).
const COMPATIBLE = 1;
// How long the database has to open before the page goes on without it: a
// browser that never answers, as some have done on a first open, holds the
// app up no longer.
const OPEN_WAIT_MS = (options && options.openWaitMs) || 5000;
// How many names past its own a record is tried under, where other bytes
// have its name already.
const BUMPS = 64;

// The connection, while it is open.
let db = null;
// Opened at whatever version a newer Dust left it at: read, never written.
let readOnly = false;
// Why the connection closed under the page: "replaced", as a newer Dust asked
// for it, or "lost", as a browser closes it when the site's data is cleared.
let closed = null;
// How many changes other tabs have told of, and who waits on the next.
let news = 0;
const waiting = [];

// Word of a change: the app looks at the history again.
const heard = () => {
  news += 1;
  for (const wake of waiting.splice(0)) {
    wake();
  }
};

const channel = typeof BroadcastChannel === "function" ? new BroadcastChannel(NAME) : null;
if (channel) {
  channel.onmessage = heard;
}

const refusal = (why, message) => ({ why, message: String(message || why) });

// Why an error stopped what was asked: no room is "full", anything else
// "failed", in the browser's own words.
const failed = (error) => {
  const name = (error && error.name) || "";
  const message = (error && (error.message || error.name)) || String(error);
  const answer = refusal(name === "QuotaExceededError" ? "full" : "failed", message);
  answer.name = name;
  return answer;
};

// A value as it is stored, as a string: none where there is none. What a
// later build stored that is no string is handed over as its JSON.
const asText = (value) => {
  if (value === undefined || value === null) {
    return null;
  }
  if (typeof value === "string") {
    return value;
  }
  try {
    return JSON.stringify(value);
  } catch (error) {
    return "";
  }
};

// Whether this build may write to a history kept as `layout` says, by its
// `compatible` alone: "open", "newer" (a later build's, which this one would
// harm), or "unread".
const gateOf = (layout) => {
  let held;
  try {
    held = JSON.parse(layout);
  } catch (error) {
    return "unread";
  }
  if (held === null || typeof held !== "object" || Array.isArray(held)) {
    return "unread";
  }
  const compatible = held.compatible;
  if (!Number.isInteger(compatible) || compatible < 0) {
    return "unread";
  }
  return compatible > COMPATIBLE ? "newer" : "open";
};

// The next name in the same place, for a record whose own name other bytes
// have already: its nonce, the sixteen hex digits after the dash, one more.
const bumped = (key) => {
  const digits = key.slice(21).split("");
  for (let at = digits.length - 1; at >= 0; at -= 1) {
    const next = (parseInt(digits[at], 16) + 1) % 16;
    digits[at] = next.toString(16);
    if (next !== 0) {
      break;
    }
  }
  return key.slice(0, 21) + digits.join("");
};

// Open the database at `version`, or at whatever version it is: null once
// it is open, or why not.
const connect = (version) =>
  new Promise((resolve) => {
    let request;
    try {
      request = version === undefined ? indexedDB.open(NAME) : indexedDB.open(NAME, version);
    } catch (error) {
      resolve(failed(error));
      return;
    }
    let settled = false;
    let timer = null;
    const settle = (answer) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        resolve(answer);
      }
    };
    timer = setTimeout(() => settle(refusal("failed", "it did not answer")), OPEN_WAIT_MS);
    request.onupgradeneeded = () => {
      const made = request.result;
      for (const store of [SESSIONS, DELETED, META]) {
        if (!made.objectStoreNames.contains(store)) {
          made.createObjectStore(store);
        }
      }
    };
    request.onsuccess = () => {
      const opened = request.result;
      // Too late: the page went on without it.
      if (settled) {
        opened.close();
        return;
      }
      // A newer Dust in another tab asks for the database, to change how it
      // is kept: let go of, so it can.
      opened.onversionchange = () => {
        opened.close();
        if (db === opened) {
          db = null;
          closed = "replaced";
        }
      };
      // Closed by the browser, as it clears the site's data: the app looks
      // at the history again at once, and stores again what it finds gone.
      opened.onclose = () => {
        if (db === opened) {
          db = null;
          closed = "lost";
          heard();
        }
      };
      db = opened;
      closed = null;
      settle(null);
    };
    request.onerror = (event) => {
      if (event && event.preventDefault) {
        event.preventDefault();
      }
      settle(failed(request.error));
    };
  });

// Do `run` on the open connection: opened again first, where the browser
// closed it under the page, and once more should it turn out closed as
// `run` starts.
const withDb = async (run) => {
  if (!db) {
    if (closed === "replaced") {
      return refusal("replaced", "a newer Dust opened it in another tab");
    }
    if (closed !== "lost") {
      return refusal("failed", "it is not open");
    }
    const reopened = await connect(VERSION);
    if (reopened !== null) {
      return reopened;
    }
  }
  try {
    return await run(db);
  } catch (error) {
    if (!(error && error.name === "InvalidStateError") || readOnly || closed === "replaced") {
      return failed(error);
    }
    db = null;
    closed = "lost";
    const reopened = await connect(VERSION);
    if (reopened !== null) {
      return reopened;
    }
    try {
      return await run(db);
    } catch (again) {
      return failed(again);
    }
  }
};

// One transaction over `stores`: `make` asks what it asks of it, and says
// what to answer once it is done; the answer comes once it is committed,
// or why it was not.
const transact = (base, stores, mode, make) =>
  new Promise((resolve) => {
    const tx =
      mode === "readwrite"
        ? base.transaction(stores, mode, { durability: "strict" })
        : base.transaction(stores, mode);
    let answer = { ok: null };
    let refused = null;
    const said = {
      answer: (value) => {
        answer = { ok: value };
      },
      refuse: (why, message) => {
        refused = refusal(why, message);
        try {
          tx.abort();
        } catch (error) {
          // Over already.
        }
      },
    };
    tx.oncomplete = () => resolve(answer);
    tx.onabort = () => resolve(refused || failed(tx.error));
    make(tx, said);
  });

// The last of `requests`, answered: every one before it is too.
const lastOf = (requests, then) => {
  if (requests.length > 0) {
    requests[requests.length - 1].onsuccess = then;
  }
};

// Store `json` as the record `key`, unless it was deleted by hand and the
// change does not undo that; where other bytes have that name already, under
// the next free one. No record is ever written over.
const place = (stores, key, json, undoing, said) => {
  const store = (name, bumps) => {
    const there = stores.sessions.get(name);
    there.onsuccess = () => {
      const held = there.result;
      if (held === undefined) {
        stores.sessions.add(json, name);
      } else if (held === json || (undoing && bumps === 0)) {
        // Stored already; or put back where its record was never taken.
      } else if (bumps < BUMPS) {
        store(bumped(name), bumps + 1);
      } else {
        said.refuse("failed", `no name was free for the session stored as ${key}`);
      }
    };
  };
  if (undoing) {
    store(key, 0);
    return;
  }
  const gone = stores.deleted.count(key);
  gone.onsuccess = () => {
    if (gone.result === 0) {
      store(key, 0);
    }
  };
};

// Tell every other tab that the history changed.
const tell = () => {
  try {
    localStorage.setItem(STAMP, `${Date.now()}.${Math.random()}`);
  } catch (error) {
    // The word on the channel goes all the same.
  }
  if (channel) {
    channel.postMessage("changed");
  }
};

const pairs = (keys, values) => ({ keys: keys.map(String), values: values.map(asText) });

return {
  // Open the database: `{ ok: "open" }`, or `{ ok: "newer" }` where a newer
  // Dust keeps it at a version past this one's, and this one may only read
  // it.
  open: async () => {
    if (typeof indexedDB === "undefined" || !indexedDB) {
      return refusal("failed", "this browser has none");
    }
    const answer = await connect(VERSION);
    if (answer === null) {
      return { ok: "open" };
    }
    if (answer.name === "VersionError") {
      const again = await connect(undefined);
      if (again === null) {
        readOnly = true;
        return { ok: "newer" };
      }
      return again;
    }
    return answer;
  },

  // How the history is kept, and the records told of: each null where none
  // is held.
  head: () =>
    withDb((base) =>
      transact(base, [META], "readonly", (tx, said) => {
        const meta = tx.objectStore(META);
        const layout = meta.get("layout");
        const told = meta.get("told");
        told.onsuccess = () => said.answer({ layout: asText(layout.result), told: asText(told.result) });
      }),
    ),

  // Up to `count` records after the name `after` (from the first, where it
  // is null), in the order of their names, which is the history's.
  page: (after, count) =>
    withDb((base) =>
      transact(base, [SESSIONS], "readonly", (tx, said) => {
        const sessions = tx.objectStore(SESSIONS);
        const range = after === null ? null : IDBKeyRange.lowerBound(after, true);
        const keys = sessions.getAllKeys(range, count);
        const values = sessions.getAll(range, count);
        values.onsuccess = () => said.answer(pairs(keys.result, values.result));
      }),
    ),

  // Every tombstone, and what each holds.
  tombstones: () =>
    withDb((base) =>
      transact(base, [DELETED], "readonly", (tx, said) => {
        const deleted = tx.objectStore(DELETED);
        const keys = deleted.getAllKeys();
        const values = deleted.getAll();
        values.onsuccess = () => said.answer(pairs(keys.result, values.result));
      }),
    ),

  // The name of every record, and of every tombstone.
  list: () =>
    withDb((base) =>
      transact(base, [SESSIONS, DELETED], "readonly", (tx, said) => {
        const sessions = tx.objectStore(SESSIONS).getAllKeys();
        const deleted = tx.objectStore(DELETED).getAllKeys();
        deleted.onsuccess = () =>
          said.answer({ sessions: sessions.result.map(String), deleted: deleted.result.map(String) });
      }),
    ),

  // What the records `keys` hold: null for one not there.
  read: (keys) =>
    withDb((base) =>
      transact(base, [SESSIONS], "readonly", (tx, said) => {
        const sessions = tx.objectStore(SESSIONS);
        const asked = keys.map((key) => sessions.get(key));
        said.answer([]);
        lastOf(asked, () => said.answer(asked.map((one) => asText(one.result))));
      }),
    ),

  // Which of `keys` were deleted by hand: those with a tombstone.
  deleted: (keys) =>
    withDb((base) =>
      transact(base, [DELETED], "readonly", (tx, said) => {
        const deleted = tx.objectStore(DELETED);
        const counted = keys.map((key) => deleted.count(key));
        said.answer([]);
        lastOf(counted, () => said.answer(keys.filter((_, at) => counted[at].result > 0)));
      }),
    ),

  // Make `change` in one transaction: `{ put: [[name, json]], forget:
  // [[name, tombstone]], unforget: [name], meta: [[key, value]], layout }`.
  // Deletions undone first, then the records, then the deletions: a record
  // deleted already stays so unless the change undoes that, and a tombstone
  // held already, which may hold what a later build added, stays as it is.
  //
  // `layout`, where given, is `{ write, over, expect }`: written where none
  // is held, as after the browser cleared the database; or, `over` being
  // true, written only where what is held is `expect` (null: none), and
  // otherwise nothing of the change is made, as another tab moved the
  // history first. A layout held that a newer Dust wrote, or that does not
  // read, lets nothing be written, but a layout over it.
  apply: (change) => {
    if (readOnly) {
      return Promise.resolve(refusal("newer", "a newer Dust keeps the history here"));
    }
    const guard = change.layout || null;
    const wrote =
      change.put.length > 0 ||
      change.forget.length > 0 ||
      change.unforget.length > 0 ||
      (guard !== null && guard.over);
    return withDb((base) =>
      transact(base, [SESSIONS, DELETED, META], "readwrite", (tx, said) => {
        const stores = {
          sessions: tx.objectStore(SESSIONS),
          deleted: tx.objectStore(DELETED),
          meta: tx.objectStore(META),
        };
        const held = stores.meta.get("layout");
        held.onsuccess = () => {
          const layout = asText(held.result);
          const over = guard !== null && guard.over;
          if (over && layout !== guard.expect) {
            said.refuse("raced", "another tab changed how the history is kept");
            return;
          }
          if (layout !== null) {
            const gate = gateOf(layout);
            if (gate === "newer") {
              said.refuse("newer", "a newer Dust keeps the history here");
              return;
            }
            if (gate === "unread" && !over) {
              said.refuse("unread", "how the history is kept could not be read");
              return;
            }
          }
          const undone = new Set(change.unforget);
          for (const key of change.unforget) {
            stores.deleted.delete(key);
          }
          for (const [key, json] of change.put) {
            place(stores, key, json, undone.has(key), said);
          }
          for (const [key, tombstone] of change.forget) {
            const there = stores.deleted.count(key);
            there.onsuccess = () => {
              if (there.result === 0) {
                stores.deleted.put(tombstone, key);
              }
            };
            stores.sessions.delete(key);
          }
          for (const [key, value] of change.meta) {
            stores.meta.put(value, key);
          }
          if (guard !== null && (over || layout === null)) {
            stores.meta.put(guard.write, "layout");
          }
        };
      }),
    ).then((answer) => {
      if (answer.ok !== undefined && wrote) {
        tell();
      }
      return answer;
    });
  },

  // The stamp the last change any tab made left: none where none has.
  stamp: () => {
    try {
      return localStorage.getItem(STAMP);
    } catch (error) {
      return null;
    }
  },

  // How many changes other tabs have told of this visit, and times the
  // browser closed the database under the page.
  news: () => news,

  // A wait that ends as another tab tells of a change.
  nextNews: () => new Promise((wake) => waiting.push(wake)),

  // How long the page storage's `key` is, in the browser's characters: -1
  // where it holds nothing, -2 where it cannot be read.
  legacyLength: (key) => {
    try {
      const raw = localStorage.getItem(key);
      return raw === null ? -1 : raw.length;
    } catch (error) {
      return -2;
    }
  },
};
