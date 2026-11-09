// What a tab does with the other tabs of the site (src/persist/lock.rs): the
// body of a function that hands back, for one tab, what the app asks of the
// Web Locks every tab of the site shares.
//
// `tryLock(name)` asks for the Web Lock `name` only if it is free, and never
// waits for it. It answers, a moment later, as the browser grants a lock:
// with a function that lets go of the lock once called, where it is this
// tab's now; with `false` where another tab holds it, or waits for it; and
// with `null` where the browser has no Web Locks (Safari before 15.4), or
// would not say. A lock is held until it is let go of, or until the tab
// closes, however it closes.
//
// `dropped(answer)` takes an answer of `tryLock` that the app no longer waits
// for: a lock it grants is let go of at once, rather than held by a tab that
// knows nothing of it.
//
// `options.locks`, for a test: the Web Locks to ask, in place of the page's.

const locks =
  (options && options.locks) || (typeof navigator === "object" && navigator ? navigator.locks : null);

const tryLock = (name) =>
  new Promise((answer) => {
    if (!locks || typeof locks.request !== "function") {
      answer(null);
      return;
    }
    try {
      locks
        .request(name, { ifAvailable: true }, (lock) => {
          if (!lock) {
            answer(false);
            return undefined;
          }
          // Held until this is called.
          return new Promise((release) => answer(() => release()));
        })
        .catch(() => answer(null));
    } catch (error) {
      answer(null);
    }
  });

const dropped = (asked) => {
  Promise.resolve(asked).then((release) => {
    if (typeof release === "function") {
      release();
    }
  });
};

return { tryLock, dropped };
