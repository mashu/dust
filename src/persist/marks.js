// The marks of the sessions and runs the site's tabs have open
// (src/persist/marks.rs): the body of a function of `prefix`, which every
// mark's name starts with, and `options`, that hands back what one tab does
// with its own mark, and with those of tabs that have gone.
//
// A mark is a key of `localStorage`, `prefix` and an id of its tab's, held by
// a Web Lock of the same name for as long as its tab runs its session or run.
// A tab that goes, closed, reloaded or crashed, lets go of its locks, so a
// mark whose lock nobody holds was cut off, and one held belongs to a tab
// still running it.
//
// `hold(name)` holds the Web Lock `name`, and keeps the mark of the same name
// in `localStorage` only while the lock is held. The function it hands back
// takes the mark's text, or, given none, takes the mark away and lets go of
// the lock. It hands back null in a browser without Web Locks, or one that
// keeps nothing for the site, where nothing is marked, so no running tab is
// ever taken for one cut off.
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
// `take()` takes every mark whose page has gone, and hands back what each
// said: one look at a time across tabs, so two opening at once tell of it
// once.
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

const hold = (name) => {
  if (!locks || !storage) return null;
  let release;
  const held = new Promise((done) => {
    release = done;
  });
  let text = null;
  let placed = false;
  let gone = false;
  const place = () => {
    if (!gone && text !== null) storage.setItem(name, text);
    placed = true;
  };
  locks
    .request(name, () => locks.request(look, place).then(() => held, () => held))
    .catch(() => {});
  return (raw) => {
    if (typeof raw !== "string") {
      gone = true;
      storage.removeItem(name);
      release();
      return;
    }
    text = raw;
    if (placed && !gone) storage.setItem(name, raw);
  };
};

const take = () => {
  if (!locks || !storage) return Promise.resolve([]);
  return locks
    .request(look, async () => {
      const state = await locks.query();
      const live = new Set([...state.held, ...state.pending].map((lock) => lock.name));
      const keys = [];
      for (let i = 0; i < storage.length; i++) {
        const key = storage.key(i);
        if (key && key.startsWith(prefix) && !live.has(key)) keys.push(key);
      }
      return keys.map((key) => {
        const mark = storage.getItem(key) ?? "";
        storage.removeItem(key);
        return mark;
      });
    })
    .catch(() => []);
};

return { hold, take };
