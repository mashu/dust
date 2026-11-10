// The marks of the sessions and runs the site's tabs have open
// (src/persist/marks.rs): the body of a function of `prefix`, which every
// mark's name starts with, and `options`, that hands back what one tab does
// with its own mark, and with those of tabs that have gone.
//
// A mark is a key of `localStorage`, `prefix` and an id of its tab's, held by
// a Web Lock of the same name for as long as its tab runs its session or run.
// A tab that goes, closed, reloaded or crashed, lets go of its locks, so a
// mark whose lock nobody holds was cut off, and one held belongs to a tab
// still running it, or to one that took it as cut off and holds it until it
// holds what it carried.
//
// `hold(name)` holds the Web Lock `name`, and keeps the mark of the same name
// in `localStorage` only while the lock is held. It hands back `write`, which
// takes the mark's text, or, given none, takes the mark away and lets go of
// the lock; and `placed`, which settles once the mark's text is in
// `localStorage`, every write after that going in at once, or once the mark
// is taken away. It hands back null in a browser without Web Locks, or one
// that keeps nothing for the site, where nothing is marked, so no running tab
// is ever taken for one cut off.
//
// A lock asked for is not held at once: the browser grants it a moment later,
// and only then does a page looking for marks see it, held or waiting. A mark
// written as the lock was asked for could be found in that moment without
// it, taken for one cut off, told of in the page that looked, and deleted
// from under this one as it ran on. So the mark goes in once the lock is
// granted, and under the lock pages look under (`take`): a look finds no
// mark, or one whose lock is held. It is taken away before the lock is let go
// of.
//
// `take()` takes every mark no tab holds, and hands back each one's name and
// what it said: one look at a time across tabs, so two opening at once tell
// of it once. A mark taken is held by this tab, under the lock of its name,
// and left in `localStorage`: a tab that goes before it holds what the mark
// carried lets go of it there, and the next look takes it again.
//
// `release(carried, left, placed)` lets go of marks this tab took. Those
// `left` are let go of as they are, for the next look to take. Those
// `carried`, whose sessions this tab's own mark holds now, or the history
// does, go once `placed`, its own mark's, has settled: from then on its own
// mark holds them in their place.
//
// `options.locks` and `options.storage`, for a test: the Web Locks and the
// page storage to use, in place of the page's.

const locks =
  (options && options.locks) || (typeof navigator === "object" && navigator ? navigator.locks : null);
// A browser that blocks the site's storage throws as `localStorage` is read.
const pageStorage = () => {
  try {
    return typeof localStorage === "object" ? localStorage : null;
  } catch (error) {
    return null;
  }
};
const storage = (options && options.storage) || pageStorage();
const look = prefix + "look";
// The marks this tab took, by name, each with what lets go of its lock.
const taken = new Map();

const hold = (name) => {
  if (!locks || !storage) return null;
  let release;
  const held = new Promise((done) => {
    release = done;
  });
  let settled;
  const inPlace = new Promise((done) => {
    settled = done;
  });
  let text = null;
  let placed = false;
  let gone = false;
  const place = () => {
    if (!gone && text !== null) storage.setItem(name, text);
    placed = true;
    settled();
  };
  locks
    .request(name, () => locks.request(look, place).then(() => held, () => held))
    .catch(() => {});
  const write = (raw) => {
    if (typeof raw !== "string") {
      gone = true;
      storage.removeItem(name);
      release();
      settled();
      return;
    }
    text = raw;
    if (placed && !gone) storage.setItem(name, raw);
  };
  return { write, placed: inPlace };
};

// The lock `name`, asked for only if no tab holds it nor waits for it, and
// held until what this hands back is called; or null, where it was not to be
// had.
const holdTaken = (name) =>
  new Promise((granted) => {
    try {
      locks
        .request(name, { ifAvailable: true }, (lock) => {
          if (!lock) {
            granted(null);
            return undefined;
          }
          return new Promise((letGo) => granted(letGo));
        })
        .catch(() => granted(null));
    } catch (error) {
      granted(null);
    }
  });

const take = () => {
  if (!locks || !storage) return Promise.resolve([]);
  return locks
    .request(look, async () => {
      const state = await locks.query();
      const live = new Set([...state.held, ...state.pending].map((lock) => lock.name));
      const keys = [];
      for (let i = 0; i < storage.length; i++) {
        const key = storage.key(i);
        if (key && key.startsWith(prefix) && !live.has(key) && !taken.has(key)) keys.push(key);
      }
      const marks = [];
      for (const key of keys) {
        // Held before the look is let go of: the next look, here or in
        // another tab, finds it held.
        const letGo = await holdTaken(key);
        if (letGo === null) continue;
        taken.set(key, letGo);
        marks.push([key, storage.getItem(key) ?? ""]);
      }
      return marks;
    })
    .catch(() => []);
};

const letGoOf = (key, remove) => {
  const letGo = taken.get(key);
  if (letGo === undefined) return;
  taken.delete(key);
  if (remove) storage.removeItem(key);
  letGo();
};

const release = async (carried, left, placed) => {
  for (const key of left) letGoOf(key, false);
  if (carried.length === 0) return;
  await placed;
  for (const key of carried) letGoOf(key, true);
};

return { hold, take, release };
