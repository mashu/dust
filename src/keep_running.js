// Keep the app running while its window is out of sight.
//
// Dioxus desktop applies each render on the webview's next animation frame,
// and polls nothing else — no timer, no send, no session — until the webview
// says that render is done. WebKitGTK stops animation frames for a window
// that is minimised, covered or on another workspace, so the whole app
// stopped with it: the band played on from its own thread, and no new group
// was ever sent. Here a frame that does not come is stood in for by a timer,
// and a hidden page does not wait for the frame at all.
(() => {
  const frame = window.requestAnimationFrame.bind(window);
  const cancelFrame = window.cancelAnimationFrame.bind(window);
  // Long enough that a visible window always paints first.
  const FRAME_LATE_MS = 50;
  const pending = new Map();
  let next = 1;
  window.requestAnimationFrame = (callback) => {
    const id = next++;
    const run = () => {
      const entry = pending.get(id);
      if (!entry) return;
      pending.delete(id);
      cancelFrame(entry.frame);
      clearTimeout(entry.timer);
      callback(performance.now());
    };
    pending.set(id, {
      frame: frame(run),
      timer: setTimeout(run, document.hidden ? 0 : FRAME_LATE_MS),
    });
    return id;
  };
  window.cancelAnimationFrame = (id) => {
    const entry = pending.get(id);
    if (!entry) return;
    pending.delete(id);
    cancelFrame(entry.frame);
    clearTimeout(entry.timer);
  };
})();
