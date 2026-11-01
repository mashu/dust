#!/usr/bin/env node
// Sessions keyed along, and copied after each group, in a real browser,
// every character timed.
//
//   node scripts/browser-timing-check.cjs <site dir> [base path]
//
// Serves the bundled web app (dx bundle's output, as Pages serves it, under
// its base path), opens it in headless Chromium with Playwright, and runs
// two group sessions with Lock typing while sending off, each in a fresh
// browser profile. The Morse is read from the very samples the page hands
// its AudioContext, and keyed in the answer box:
//
// - keyed along: each character a quarter of a second after it reached the
//   ear, then Enter. Results must time every character keyed right under
//   "Recognised in", and show no copy time;
// - copied after: each group half a second after it ended, a letter every
//   fifth of a second, then Enter, as most learners copy. Results must give
//   every letter keyed right a copy time, and show no recognition time.
//
// Either way there is no "not timed" line, and the session the browser's
// database keeps has every group's character times and no reason it went
// untimed.
//
// It is the regression check for characters left untimed that the Rust
// tests cannot make: the browser's own audio clock, stream, event loop and
// database (src/timing_tests.rs runs the same sessions in the app's test
// harness). Playwright is found the way Node finds any package, NODE_PATH
// included.
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
// Copied after: the first key this long after the group's end reached the
// ear, each later one this long after the one before.
const COPY_FIRST_MS = 500;
const COPY_GAP_MS = 200;
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

// In the page: read the Morse as it sounds, and key it along, or copy each
// group once it has ended.
const KEY_ALONG = `((wpm, reactMs, after, firstMs, gapMs) => {
  const A = window.__ear;
  const MORSE = {'.-':'A','-...':'B','-.-.':'C','-..':'D','.':'E','..-.':'F','--.':'G','....':'H','..':'I','.---':'J','-.-':'K','.-..':'L','--':'M','-.':'N','---':'O','.--.':'P','--.-':'Q','.-.':'R','...':'S','-':'T','..-':'U','...-':'V','.--':'W','-..-':'X','-.--':'Y','--..':'Z','-----':'0','.----':'1','..---':'2','...--':'3','....-':'4','.....':'5','-....':'6','--...':'7','---..':'8','----.':'9','.-.-.-':'.','--..--':',','..--..':'?','-..-.':'/','-...-':'=','.-.-.':'+','-....-':'-'};
  const dit = 1.2 / wpm;
  const keyed = A.keyed = [];
  let scanned = 0, on = false, onAt = 0, offAt = 0, elements = '', heard = [], groupEnd = null, copying = null, deafUntil = 0;
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
      if (t < deafUntil) {
        // A group just answered, sent again and still dying away: never the
        // next group's.
        on = false;
        elements = '';
        groupEnd = null;
        scanned += step;
        continue;
      }
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
    const enter = () => {
      const field = box();
      if (field) field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true }));
      keyed.push({ ch: 'Enter', typed: !!field });
    };
    if (after) {
      // The group over, its letters typed from memory once it has ended.
      if (copying === null && groupEnd !== null && heard.length && now - toEar > groupEnd + 6 * dit) {
        copying = { at: groupEnd + toEar + firstMs / 1000, letters: heard.splice(0) };
        groupEnd = null;
      }
      if (copying !== null && now >= copying.at) {
        if (copying.letters.length) {
          const { ch } = copying.letters.shift();
          keyed.push({ ch, typed: type(ch) });
          copying.at = now + gapMs / 1000;
        } else {
          // Whatever was heard since, a group sent again for some of it
          // unheard, is that group's: dropped, and the ear deaf until it
          // has stopped, short of when the next group could sound.
          enter();
          copying = null;
          heard = [];
          elements = '';
          groupEnd = null;
          on = false;
          deafUntil = now + 0.3;
        }
      }
      return;
    }
    while (heard.length && now >= heard[0].end + toEar + reactMs / 1000) {
      const { ch } = heard.shift();
      keyed.push({ ch, typed: type(ch) });
    }
    if (groupEnd !== null && !heard.length && now - toEar > groupEnd + 6 * dit) {
      enter();
      groupEnd = null;
    }
  }, 10);
})`;

/// One session in a fresh profile, keyed along or copied after: what went
/// wrong, if anything.
async function session(browser, url, after) {
  const context = await browser.newContext();
  const page = await context.newPage();
  const log = [];
  page.on('console', m => log.push(`${m.type()}: ${m.text()}`));
  page.on('pageerror', e => log.push(`page error: ${e.message}`));
  const way = after ? 'copied after each group' : 'keyed along';
  const problems = [];
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
    await page.evaluate(`${KEY_ALONG}(${WPM}, ${REACT_MS}, ${after}, ${COPY_FIRST_MS}, ${COPY_GAP_MS})`);
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
      // As written, not as styled: the stylesheet sets some labels in
      // capitals, which innerText would give. Each tile by its id, so a
      // figure in one never answers for the other.
      const tile = id => {
        const found = document.querySelector(`#${id}`);
        return found ? found.textContent : null;
      };
      const hint = document.querySelector('#untimed-hint');
      return {
        recognised: tile('kpi-recognised'),
        copy: tile('kpi-copy'),
        hint: hint && hint.innerText,
        keyed: window.__ear.keyed,
        stored,
      };
    });
    const kept = seen.stored.sort((a, b) => a.timestamp - b.timestamp).pop();
    if (!kept) problems.push('no session in the browser database');
    if (seen.hint) problems.push(`Results says: ${seen.hint}`);
    // Characters in groups the ear here keyed exactly right: every one is
    // timed. A group it misread may be timed in part, as the app times
    // only keys that line up with what was sent. A headless browser's
    // audio now and then holds a send up on its way, and the app rightly
    // leaves that group untimed: one such group is the browser's, not a
    // fault in the timing, which would leave every group untimed; more is
    // a fault.
    let right = 0;
    let exact = 0;
    const heldUp = [];
    for (const [i, group] of (kept ? kept.groups : []).entries()) {
      const timing = kept.groupTimings[i] || {};
      if (timing.untimed === 'heldUp') {
        heldUp.push(`${i + 1} (${group.sent})`);
        continue;
      }
      if (timing.untimed) problems.push(`group ${i + 1} (${group.sent}) went untimed: ${timing.untimed}`);
      if (!timing.charTimes) problems.push(`group ${i + 1} (${group.sent}) kept no character times`);
      if (group.received === group.sent) {
        exact += 1;
        right += [...group.sent].length;
      }
    }
    if (heldUp.length > 1) problems.push(`groups ${heldUp.join(', ')} went untimed, held up on their way`);
    if (heldUp.length === 1) console.log(`${way}: group ${heldUp[0]} was held up on its way by the browser's audio, and is left untimed, as it should be.`);
    const groups = kept ? kept.groups.length : 0;
    const chars = kept ? kept.groups.reduce((n, g) => n + [...g.sent].length, 0) : 0;
    if (groups < GROUPS) problems.push(`only ${groups} of ${GROUPS} groups were kept`);
    if (exact < (groups - heldUp.length) * 0.8) problems.push(`the ear here keyed only ${exact} of ${groups} groups right`);
    const count = text => Number(((text || '').match(/(\d+)\s*timed/i) || [])[1] || 0);
    const [timing, other, figure] = after
      ? [seen.copy, seen.recognised, 'Copy time']
      : [seen.recognised, seen.copy, 'Recognised in'];
    const timed = count(timing);
    if (timed < right) problems.push(`${figure} times ${timed} characters, of ${right} in groups ${way} right`);
    if (other !== null && count(other) > 0) {
      problems.push(`${way}, yet timed the other way too: ${other}`);
    }
    console.log(`${way}: ${exact} of ${groups} groups (${right} of ${chars} characters) keyed right; ${figure}: ${timed} timed.`);
    if (problems.length) console.log(JSON.stringify({ seen: { ...seen, stored: undefined }, kept }, null, 1));
  } catch (e) {
    problems.push(String((e && e.stack) || e));
    console.log(await page.evaluate(() => document.body && document.body.innerText).catch(() => ''));
  } finally {
    await context.close();
  }
  if (problems.length) {
    console.error(`Characters ${way} went untimed in the browser:`);
    for (const problem of problems) console.error(`  ${problem}`);
    console.error(log.slice(-30).join('\n'));
  }
  return problems.length === 0;
}

async function main() {
  const server = await serve();
  const url = `http://127.0.0.1:${server.address().port}${BASE}`;
  const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  let passed = false;
  try {
    const along = await session(browser, url, false);
    const after = await session(browser, url, true);
    passed = along && after;
  } finally {
    await browser.close();
    server.close();
  }
  if (!passed) process.exit(1);
  console.log('Every character keyed right was timed, along or copied after.');
}

main();
