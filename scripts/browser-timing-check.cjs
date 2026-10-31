#!/usr/bin/env node
// A session keyed along in a real browser, every character timed.
//
//   node scripts/browser-timing-check.cjs <site dir> [base path]
//
// Serves the bundled web app (dx bundle's output, as Pages serves it, under
// its base path), opens it in headless Chromium with Playwright, and runs a
// group session with Lock typing while sending off, the way a learner keys
// along: the Morse is read from the very samples the page hands its
// AudioContext, and each character is keyed in the answer box a quarter of
// a second after it reached the ear, then Enter. The check passes only if
// the Results screen says every character keyed right was timed, gives no
// "not timed" line, and the session the browser's database keeps has every
// group's character times and no reason it went untimed.
//
// It is the regression check for characters keyed along and left untimed
// that the Rust tests cannot make: the browser's own audio clock, stream,
// event loop and database (src/timing_tests.rs runs the same session in
// the app's test harness). Playwright is found the way Node finds any
// package, NODE_PATH included.
'use strict';

const fs = require('fs');
const http = require('http');
const path = require('path');
const { chromium } = require('playwright');

const SITE = path.resolve(process.argv[2] || 'site');
const BASE = `/${(process.argv[3] || '').replace(/^\/+|\/+$/g, '')}/`.replace('//', '/');
const GROUPS = 6;
const WPM = 20;
// How long after a character reached the ear its key comes: a learner
// keeping up.
const REACT_MS = 250;
// What a learner keying along has, as an older save would hold it: groups
// of three at one speed, and no band noise for the ear here to read through.
const SETTINGS = {
  numGroups: GROUPS,
  minGroupSize: 3,
  maxGroupSize: 3,
  charWpmMin: WPM,
  charWpmMax: WPM,
  effectiveWpmMin: WPM,
  effectiveWpmMax: WPM,
  lockInputDuringGroupPlayback: false,
  noiseEnabled: false,
  qsbEnabled: false,
  qrnEnabled: false,
};

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.mjs': 'text/javascript',
  '.wasm': 'application/wasm',
  '.css': 'text/css',
  '.json': 'application/json',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.txt': 'text/plain',
};

/// The site, as Pages serves it: under its base path, and its index for
/// any page that is not a file.
function serve() {
  const server = http.createServer((req, res) => {
    let url = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    if (url.startsWith(BASE)) url = '/' + url.slice(BASE.length);
    let file = path.join(SITE, path.normalize(url));
    if (!file.startsWith(SITE)) {
      res.writeHead(403).end();
      return;
    }
    if (!fs.existsSync(file) || fs.statSync(file).isDirectory()) {
      const index = path.join(file, 'index.html');
      file = fs.existsSync(index) ? index : path.join(SITE, 'index.html');
    }
    res.writeHead(200, { 'content-type': TYPES[path.extname(file)] || 'application/octet-stream' });
    fs.createReadStream(file).pipe(res);
  });
  return new Promise(ok => server.listen(0, '127.0.0.1', () => ok(server)));
}

// Before the app: the samples each scheduled buffer plays, laid out on the
// AudioContext's own clock.
const LISTEN = `(() => {
  const A = window.__ear = { ctx: null, sr: 0, buf: null };
  const Context = window.AudioContext;
  window.AudioContext = class extends Context {
    constructor(...args) {
      super(...args);
      A.ctx = this;
      A.sr = this.sampleRate;
      A.buf = new Float32Array(this.sampleRate * 300);
    }
  };
  window.webkitAudioContext = window.AudioContext;
  const copy = AudioBuffer.prototype.copyToChannel;
  AudioBuffer.prototype.copyToChannel = function (source, channel, offset) {
    if (channel === 0) this.__samples = new Float32Array(source);
    return copy.call(this, source, channel, offset);
  };
  const start = AudioBufferSourceNode.prototype.start;
  AudioBufferSourceNode.prototype.start = function (when, ...rest) {
    const samples = this.buffer && this.buffer.__samples;
    if (samples && this.context === A.ctx) {
      const at = Math.round((when || this.context.currentTime) * A.sr);
      A.buf.set(samples.subarray(0, Math.max(0, A.buf.length - at)), at);
    }
    return start.call(this, when, ...rest);
  };
})();`;

// In the page: read the Morse as it sounds, and key it along.
const KEY_ALONG = `((wpm, reactMs) => {
  const A = window.__ear;
  const MORSE = {'.-':'A','-...':'B','-.-.':'C','-..':'D','.':'E','..-.':'F','--.':'G','....':'H','..':'I','.---':'J','-.-':'K','.-..':'L','--':'M','-.':'N','---':'O','.--.':'P','--.-':'Q','.-.':'R','...':'S','-':'T','..-':'U','...-':'V','.--':'W','-..-':'X','-.--':'Y','--..':'Z','-----':'0','.----':'1','..---':'2','...--':'3','....-':'4','.....':'5','-....':'6','--...':'7','---..':'8','----.':'9','.-.-.-':'.','--..--':',','..--..':'?','-..-.':'/','-...-':'=','.-.-.':'+','-....-':'-'};
  const dit = 1.2 / wpm;
  const keyed = A.keyed = [];
  let scanned = 0, on = false, onAt = 0, offAt = 0, elements = '', heard = [], groupEnd = null;
  const box = () => document.querySelector('input.answer:not(.locked)');
  const type = text => {
    const field = box();
    if (!field) return false;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(field, field.value + text);
    field.dispatchEvent(new InputEvent('input', { bubbles: true, data: text, inputType: 'insertText' }));
    return true;
  };
  const character = end => {
    heard.push({ ch: MORSE[elements] || '?', end });
    elements = '';
  };
  setInterval(() => {
    if (!A.ctx) return;
    const now = A.ctx.currentTime, step = Math.round(0.002 * A.sr);
    const toEar = (A.ctx.baseLatency || 0) + (A.ctx.outputLatency || 0);
    while (scanned + step <= Math.floor(now * A.sr)) {
      let peak = 0;
      for (let i = scanned; i < scanned + step; i++) peak = Math.max(peak, Math.abs(A.buf[i]));
      const t = scanned / A.sr;
      if (!on && peak > 0.02) {
        on = true;
        onAt = t;
        groupEnd = null;
      } else if (on && peak < 0.005) {
        on = false;
        offAt = t;
        if (t - onAt > 0.4 * dit) elements += t - onAt > 2 * dit ? '-' : '.';
      }
      if (!on && elements && t - offAt > 2 * dit) {
        character(offAt);
        groupEnd = offAt;
      }
      scanned += step;
    }
    while (heard.length && now >= heard[0].end + toEar + reactMs / 1000) {
      const { ch } = heard.shift();
      keyed.push({ ch, typed: type(ch) });
    }
    if (groupEnd !== null && !heard.length && now - toEar > groupEnd + 6 * dit) {
      const field = box();
      if (field) field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true }));
      keyed.push({ ch: 'Enter', typed: !!field });
      groupEnd = null;
    }
  }, 10);
})`;

async function main() {
  const server = await serve();
  const url = `http://127.0.0.1:${server.address().port}${BASE}`;
  const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  const page = await browser.newPage();
  const log = [];
  page.on('console', m => log.push(`${m.type()}: ${m.text()}`));
  page.on('pageerror', e => log.push(`page error: ${e.message}`));
  let failed = null;
  try {
    await page.addInitScript(LISTEN);
    await page.addInitScript(settings => {
      try {
        if (!localStorage.getItem('dust_settings')) {
          localStorage.setItem('dust_settings', settings);
          localStorage.setItem('dust_tutorial_done', 'done');
          localStorage.setItem('dust_asked_about_lock', 'asked');
        }
      } catch (e) {}
    }, JSON.stringify(SETTINGS));
    await page.goto(url);
    await page.getByText('Start training', { exact: true }).waitFor({ timeout: 120000 });
    await page.evaluate(`${KEY_ALONG}(${WPM}, ${REACT_MS})`);
    await page.getByText('Start training', { exact: true }).click();
    await page.waitForSelector('#screen-results', { timeout: 30000 * GROUPS });
    const seen = await page.evaluate(async () => {
      const read = (db, store) => new Promise((ok, no) => {
        const r = db.transaction(store).objectStore(store).getAll();
        r.onsuccess = () => ok(r.result);
        r.onerror = () => no(r.error);
      });
      const stored = [];
      for (const { name } of await indexedDB.databases()) {
        const db = await new Promise((ok, no) => {
          const r = indexedDB.open(name);
          r.onsuccess = () => ok(r.result);
          r.onerror = () => no(r.error);
        });
        if ([...db.objectStoreNames].includes('sessions')) {
          for (const raw of await read(db, 'sessions')) stored.push(typeof raw === 'string' ? JSON.parse(raw) : raw);
        }
      }
      const hint = document.querySelector('#untimed-hint');
      return {
        // As written, not as styled: the stylesheet sets some labels in
        // capitals, which innerText would give.
        text: document.querySelector('#screen-results').textContent,
        hint: hint && hint.innerText,
        keyed: window.__ear.keyed,
        stored,
      };
    });
    const session = seen.stored.sort((a, b) => a.timestamp - b.timestamp).pop();
    const problems = [];
    if (!session) problems.push('no session in the browser database');
    if (seen.hint) problems.push(`Results says: ${seen.hint}`);
    // Characters in groups the ear here keyed exactly right: every one is
    // timed. A group it misread may be timed in part, as the app times
    // only keys that line up with what was sent.
    let right = 0;
    let exact = 0;
    for (const [i, group] of (session ? session.groups : []).entries()) {
      const timing = session.groupTimings[i] || {};
      if (timing.untimed) problems.push(`group ${i + 1} (${group.sent}) went untimed: ${timing.untimed}`);
      if (!timing.charTimes) problems.push(`group ${i + 1} (${group.sent}) kept no character times`);
      if (group.received === group.sent) {
        exact += 1;
        right += [...group.sent].length;
      }
    }
    const groups = session ? session.groups.length : 0;
    const chars = session ? session.groups.reduce((n, g) => n + [...g.sent].length, 0) : 0;
    if (groups < GROUPS) problems.push(`only ${groups} of ${GROUPS} groups were kept`);
    if (exact < groups * 0.8) problems.push(`the ear here keyed only ${exact} of ${groups} groups right`);
    const timed = Number((seen.text.match(/(\d+)\s*timed/i) || [])[1] || 0);
    if (timed < right) problems.push(`Results times ${timed} characters, of ${right} in groups keyed right along`);
    console.log(`Keyed ${exact} of ${groups} groups (${right} of ${chars} characters) right along; Results: ${timed} timed.`);
    if (problems.length) {
      failed = problems;
      console.log(JSON.stringify({ results: seen.text, keyed: seen.keyed, session }, null, 1));
    }
  } catch (e) {
    failed = [String(e && e.stack || e)];
    console.log(await page.evaluate(() => document.body && document.body.innerText).catch(() => ''));
  } finally {
    await browser.close();
    server.close();
  }
  if (failed) {
    console.error('Characters keyed along went untimed in the browser:');
    for (const problem of failed) console.error(`  ${problem}`);
    console.error(log.slice(-30).join('\n'));
    process.exit(1);
  }
  console.log('Every character keyed right along was timed.');
}

main();
