// A page for the app's page scripts to run on in Node, as the tests have no
// webview: the window and the document they listen on, and the keys pressed
// there (`crate::testing::on_fake_page`).
//
// It reads the steps as JSON from stdin, takes them in order, and writes, for
// each, what the scripts sent the app during it and whether they kept the
// page's own default from happening. A script runs as Dioxus 0.7's
// `document::eval` runs one: as the body of an async function, `dioxus` its
// channel to the app. Dioxus closes that channel once the body is done, yet
// still delivers what is sent on it after, which is how a listener the body
// leaves behind talks to the app; so here a send is heard whenever it comes.
// A script waited on runs to its end before the next step is taken, as one
// that waits on the browser's database has to.
//
// The page has a browser's database, broadcast channels and page storage too,
// made up in memory (`src/fake_idb.mjs`), for the history's script.

import { readFileSync } from "node:fs";
import { install } from "./fake_idb.mjs";

install(globalThis);

class Target {
  #listeners = new Map();

  addEventListener(type, listener) {
    const listeners = this.#listeners.get(type) ?? [];
    if (!listeners.includes(listener)) {
      listeners.push(listener);
    }
    this.#listeners.set(type, listeners);
  }

  removeEventListener(type, listener) {
    const listeners = this.#listeners.get(type) ?? [];
    this.#listeners.set(
      type,
      listeners.filter((held) => held !== listener),
    );
  }

  dispatchEvent(event) {
    for (const listener of [...(this.#listeners.get(event.type) ?? [])]) {
      listener.call(this, event);
    }
  }
}

// Nothing has the focus: a key goes to the body, outside any field, and up
// through the document to the window.
const body = {
  tagName: "BODY",
  nodeName: "BODY",
  isContentEditable: false,
  closest: () => null,
  matches: () => false,
};
const window = new Target();
const document = new Target();
document.body = body;
document.activeElement = body;
document.fullscreenElement = null;
const navigator = {};
window.document = document;
window.navigator = navigator;

const AsyncFunction = (async () => {}).constructor;
let heard = null;
const failures = [];

const run = (script) => {
  const dioxus = {
    send: (value) => heard.sent.push(JSON.parse(JSON.stringify(value))),
    // The app sends these scripts nothing.
    recv: () => new Promise(() => {}),
    close: () => {},
  };
  return new AsyncFunction("dioxus", "window", "document", "navigator", script)(
    dioxus,
    window,
    document,
    navigator,
  ).then(
    () => dioxus.close(),
    (error) => failures.push(error),
  );
};

const event = (type, fields = {}) => ({
  type,
  target: body,
  key: "",
  code: "",
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  shiftKey: false,
  repeat: false,
  isComposing: false,
  ...fields,
  defaultPrevented: false,
  preventDefault() {
    this.defaultPrevented = true;
  },
});

const steps = JSON.parse(readFileSync(0, "utf8"));
const results = [];
for (const step of steps) {
  heard = { sent: [], prevented: false };
  if (step.step === "run") {
    run(step.script);
  } else if (step.step === "wait") {
    await run(step.script);
  } else if (step.step === "key") {
    const { step: _, ...fields } = step;
    const key = event(fields.type, fields);
    document.dispatchEvent(key);
    window.dispatchEvent(key);
    heard.prevented = key.defaultPrevented;
  } else if (step.step === "blur") {
    window.dispatchEvent(event("blur"));
  } else {
    throw new Error(`no such step: ${JSON.stringify(step)}`);
  }
  // What a script does once its body has run, as a page would by the next
  // thing to happen on it.
  await new Promise((resolve) => setImmediate(resolve));
  if (failures.length > 0) {
    throw failures[0];
  }
  results.push(heard);
}
process.stdout.write(JSON.stringify(results));
