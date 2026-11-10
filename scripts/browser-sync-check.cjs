// End-to-end check of sync in a real Chromium, on the site as Pages
// serves it, against a fake Google: its sign-in says yes at once (a
// redirect to oauth.html with a token, as Google's token flow sends), and
// Drive's REST API is an in-memory app data folder here. Two browser
// contexts are two devices, each with storage of its own, on one Google
// account (src/sync, docs/sync.md).
//
//   node scripts/browser-sync-check.cjs <site dir> [base path]
//
// It checks what the privacy policy says of sync, in the browser:
// nothing reaches Google until sync is turned on; what the drive holds
// says nothing in the clear; the passphrase is never kept; and the
// history, the settings and the theme of one device reach the other. A build made
// without a web client ID offers no sync, and the check says so and passes.
// The fake Google answers on Google's own addresses, so the pages' content
// security policies hold as they would with the real one: the app's page
// refuses a connection anywhere else, and no page, the sign-in page among
// them, breaches its policy.
//
// Playwright is found the way Node finds any package, NODE_PATH included.
'use strict';

const fs = require('fs');
const http = require('http');
const path = require('path');
const assert = require('assert');
const { chromium } = require('playwright');

const SITE = path.resolve(process.argv[2] || 'site');
const BASE = `/${(process.argv[3] || '').replace(/^\/+|\/+$/g, '')}/`.replace('//', '/');
const SCOPE = 'https://www.googleapis.com/auth/drive.appdata';
const PASSPHRASE = 'correct horse battery';

// A backup of two sessions, and settings a fresh install does not have,
// for the first device to restore: what the second is to take up.
const SESSIONS = [{"date":"2026-11-01","timestamp":1793500000000,"groups":[{"sent":"KMURE","received":"KMURE","correct":true},{"sent":"PAXTO","received":"PAXTE","correct":false}],"groupTimings":[{"timeToCompleteMs":500.0,"charWpm":40.0,"effectiveWpm":40.0,"sends":1},{"timeToCompleteMs":500.0,"charWpm":40.0,"effectiveWpm":40.0,"sends":1}],"accuracy":0.9,"letterAccuracy":{"A":{"correct":1,"total":1},"E":{"correct":1,"total":1},"K":{"correct":1,"total":1},"M":{"correct":1,"total":1},"O":{"correct":0,"total":1},"P":{"correct":1,"total":1},"R":{"correct":1,"total":1},"T":{"correct":1,"total":1},"U":{"correct":1,"total":1},"X":{"correct":1,"total":1}},"avgResponseMs":100.0,"totalChars":10,"score":211.21,"level":1,"digitsLevel":1,"charSetMode":"koch","charWpm":40.0,"effectiveWpm":40.0,"alphabetFingerprint":"KMURESNAPTLWI.JZ=FOY,VG5/Q92H38B?47C1D60X","typingLocked":true},{"date":"2026-11-02","timestamp":1793590000000,"groups":[{"sent":"QZLRV","received":"QZLRV","correct":true}],"groupTimings":[{"timeToCompleteMs":500.0,"charWpm":40.0,"effectiveWpm":40.0,"sends":1}],"accuracy":1.0,"letterAccuracy":{"L":{"correct":1,"total":1},"Q":{"correct":1,"total":1},"R":{"correct":1,"total":1},"V":{"correct":1,"total":1},"Z":{"correct":1,"total":1}},"avgResponseMs":100.0,"totalChars":5,"score":123.35,"level":1,"digitsLevel":1,"charSetMode":"koch","charWpm":40.0,"effectiveWpm":40.0,"alphabetFingerprint":"KMURESNAPTLWI.JZ=FOY,VG5/Q92H38B?47C1D60X","typingLocked":true}];
const BACKUP = JSON.stringify({ dustBackup: 1, savedAt: 1793600000000, settings: { numGroups: 7 }, sessions: SESSIONS });

const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css', '.json': 'application/json', '.png': 'image/png', '.svg': 'image/svg+xml' };

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
    if (!fs.existsSync(file) || fs.statSync(file).isDirectory()) file = path.join(SITE, 'index.html');
    res.writeHead(200, { 'content-type': TYPES[path.extname(file)] || 'application/octet-stream' });
    fs.createReadStream(file).pipe(res);
  });
  return new Promise(ok => server.listen(0, '127.0.0.1', () => ok(server)));
}

// The user's Drive: one app data folder, shared by both devices.
const drive = { files: new Map(), made: 0, tokens: new Set(), asked: [] };
const CORS = {
  'access-control-allow-origin': '*',
  'access-control-allow-headers': 'authorization, content-type',
  'access-control-allow-methods': 'GET, POST, PATCH, DELETE',
};
const json = (route, status, body) => route.fulfill({ status, headers: CORS, contentType: 'application/json', body: JSON.stringify(body) });
const fail = (route, status, reason) => json(route, status, { error: { code: status, message: reason, errors: [{ reason }] } });

function readMultipart(request) {
  const boundary = /boundary=(.+)$/.exec(request.headers()['content-type'])[1];
  const body = request.postDataBuffer();
  const opening = Buffer.from(`--${boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n`);
  assert(body.subarray(0, opening.length).equals(opening), 'multipart opening');
  const between = Buffer.from(`\r\n--${boundary}\r\nContent-Type: application/octet-stream\r\n\r\n`);
  const split = body.indexOf(between);
  const metadata = JSON.parse(body.subarray(opening.length, split).toString());
  assert.deepStrictEqual(metadata.parents, ['appDataFolder']);
  const closing = Buffer.from(`\r\n--${boundary}--\r\n`);
  const content = body.subarray(split + between.length, body.length - closing.length);
  return { name: metadata.name, bytes: Buffer.from(content) };
}

async function answerDrive(route) {
  const request = route.request();
  const url = new URL(request.url());
  const method = request.method();
  drive.asked.push(`${method} ${url.pathname}`);
  if (method === 'OPTIONS') return route.fulfill({ status: 204, headers: CORS });
  const token = (request.headers()['authorization'] || '').replace(/^Bearer /, '');
  if (!drive.tokens.has(token)) return fail(route, 401, 'authError');
  const files = '/drive/v3/files';
  const upload = '/upload/drive/v3/files';
  if (method === 'GET' && url.pathname === files) {
    assert.strictEqual(url.searchParams.get('spaces'), 'appDataFolder');
    const listed = [...drive.files].map(([id, f]) => ({ id, name: f.name, createdTime: f.created }));
    return json(route, 200, { files: listed });
  }
  if (method === 'GET' && url.pathname.startsWith(files + '/')) {
    assert.strictEqual(url.searchParams.get('alt'), 'media');
    const file = drive.files.get(url.pathname.slice(files.length + 1));
    if (!file) return fail(route, 404, 'notFound');
    return route.fulfill({ status: 200, headers: CORS, contentType: 'application/octet-stream', body: file.bytes });
  }
  if (method === 'POST' && url.pathname === upload) {
    assert.strictEqual(url.searchParams.get('uploadType'), 'multipart');
    const { name, bytes } = readMultipart(request);
    const id = `id${++drive.made}`;
    drive.files.set(id, { name, bytes, created: `2026-11-04T10:00:${String(drive.made).padStart(6, '0')}.000Z` });
    return json(route, 200, { id });
  }
  if (method === 'PATCH' && url.pathname.startsWith(upload + '/')) {
    const file = drive.files.get(url.pathname.slice(upload.length + 1));
    if (!file) return fail(route, 404, 'notFound');
    file.bytes = request.postDataBuffer();
    return json(route, 200, { id: 'x' });
  }
  if (method === 'DELETE' && url.pathname.startsWith(files + '/')) {
    drive.files.delete(url.pathname.slice(files.length + 1));
    return route.fulfill({ status: 204, headers: CORS });
  }
  return fail(route, 400, 'badRequest');
}

// Google's sign-in: says yes at once, as a user who has done it before.
let signIns = 0;
function answerSignIn(route) {
  const url = new URL(route.request().url());
  assert.strictEqual(url.pathname, '/o/oauth2/v2/auth');
  const asked = Object.fromEntries(url.searchParams);
  assert(asked.client_id, 'a client ID');
  assert.strictEqual(asked.response_type, 'token');
  assert.strictEqual(asked.scope, SCOPE);
  assert(asked.redirect_uri.endsWith('/oauth.html'), asked.redirect_uri);
  assert(asked.state && asked.state.length === 32);
  const token = `token-${++signIns}`;
  drive.tokens.add(token);
  const back = `${asked.redirect_uri}#access_token=${token}&token_type=Bearer&expires_in=3599&scope=${encodeURIComponent(SCOPE)}&state=${asked.state}`;
  return route.fulfill({ status: 302, headers: { location: back } });
}

/// Every breach of a page's content security policy in `context`, its
/// popups included, said into `breaches`: what was refused, under which
/// directive, and where.
async function watchPolicy(context, breaches) {
  await context.exposeBinding('__dustBreach', ({ page }, breach) => {
    breaches.push(`${page ? page.url() : 'a page'}: ${breach}`);
  });
  await context.addInitScript(() => {
    document.addEventListener('securitypolicyviolation', e => {
      window.__dustBreach(`${e.effectiveDirective} refused ${e.blockedURI || 'inline'} at ${e.sourceFile || 'the page'}:${e.lineNumber}`);
    });
  });
}

/// Where the check asks the page's policy to refuse a connection.
const ELSEWHERE = 'https://example.invalid/';

/// The breaches of `d`'s pages, but for the one the check asked for.
const breached = d => d.breaches.filter(breach => !breach.includes(ELSEWHERE));

async function device(browser, origin, name) {
  const context = await browser.newContext();
  const breaches = [];
  await watchPolicy(context, breaches);
  const elsewhere = [];
  await context.route(() => true, route => {
    const url = new URL(route.request().url());
    if (url.origin === origin) return route.continue();
    if (url.origin === 'https://accounts.google.com') return answerSignIn(route);
    if (url.origin === 'https://www.googleapis.com') return answerDrive(route);
    elsewhere.push(url.origin);
    return route.abort();
  });
  const page = await context.newPage();
  page.on('pageerror', err => console.log(`[${name}] page error: ${err.message}`));
  await page.addInitScript(() => {
    localStorage.setItem('dust_tutorial_done', 'done');
    localStorage.setItem('dust_asked_about_lock', 'asked');
  });
  await page.goto(`${origin}${BASE}`);
  await page.click('#nav-stats');
  await page.click('#seg-history');
  return { context, page, elsewhere, breaches };
}

async function turnOn(page, repeat) {
  const popup = page.waitForEvent('popup');
  await page.click('#btn-turn-on-sync');
  await popup;
  await page.locator('#sync-passphrase').waitFor({ timeout: 20000 });
  await page.fill('#sync-passphrase', PASSPHRASE);
  if (repeat) await page.fill('#sync-passphrase-again', PASSPHRASE);
  await page.click('#btn-open-sync');
  await page.getByText(/Synced today at/).waitFor({ timeout: 60000 });
}

(async () => {
  const server = await serve();
  const origin = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch();
  try {
    // The phone: a history restored, with its settings, then sync on.
    const a = await device(browser, origin, 'phone');
    // The page's policy is in force: a connection anywhere but the site and
    // Google Drive is refused before it is made.
    const refused = await a.page.evaluate(
      elsewhere =>
        new Promise(ok => {
          document.addEventListener('securitypolicyviolation', e => {
            if (e.blockedURI.startsWith(elsewhere)) ok(e.effectiveDirective);
          });
          fetch(elsewhere).catch(() => {});
          setTimeout(() => ok(null), 10000);
        }),
      ELSEWHERE,
    );
    assert.strictEqual(refused, 'connect-src', "the page's policy refuses a connection elsewhere");
    if (!(await a.page.locator('#btn-turn-on-sync').count())) {
      assert.deepStrictEqual(breached(a), [], "nothing breaches the page's policy");
      console.log('This build offers no sync (it was made without a web client ID): nothing to check.');
      return;
    }
    await a.page.setInputFiles('#file-backup', {
      name: 'dust-backup.json',
      mimeType: 'application/json',
      buffer: Buffer.from(BACKUP),
    });
    await a.page.getByText(/Restored 2 sessions/).waitFor({ timeout: 20000 });
    // A theme chosen, to come across with the rest.
    await a.page.click('#btn-theme');
    const theme = await a.page.evaluate(() => localStorage.getItem('dust_theme'));
    assert(theme, 'a theme chosen');
    assert.strictEqual(drive.asked.length, 0, 'nothing reaches Google before sync is on');
    assert.strictEqual(signIns, 0);
    await turnOn(a.page, true);
    const names = [...drive.files.values()].map(f => f.name);
    console.log('drive after the phone:', names);
    assert(names.includes('dust-keyring'));
    assert(names.some(n => n.startsWith('dust-p-')), 'a pack');
    assert(names.some(n => n.startsWith('dust-s-')), 'a settings file');
    const everything = Buffer.concat([...drive.files.values()].map(f => Buffer.concat([Buffer.from(f.name), f.bytes])));
    for (const clear of ['KMURE', 'QZLRV', '2026-11-0', 'numGroups', 'timestamp']) {
      assert(!everything.includes(clear), `${clear} in the clear in the drive`);
    }
    const kept = await a.page.evaluate(() => localStorage.getItem('dust_sync'));
    assert(kept && !kept.includes(PASSPHRASE), 'the key kept, never the passphrase');

    // The computer: a fresh browser, the wrong passphrase first, then the
    // right one; it takes the history and the settings.
    const b = await device(browser, origin, 'computer');
    assert(await b.page.getByText('0 sessions on this device').isVisible());
    const popup = b.page.waitForEvent('popup');
    await b.page.click('#btn-turn-on-sync');
    await popup;
    await b.page.locator('#sync-passphrase').waitFor({ timeout: 20000 });
    assert(!(await b.page.locator('#sync-passphrase-again').count()), 'joining asks once');
    await b.page.fill('#sync-passphrase', 'wrong passphrase');
    await b.page.click('#btn-open-sync');
    await b.page.getByText(/does not open the synced copy/).waitFor({ timeout: 30000 });
    await b.page.fill('#sync-passphrase', PASSPHRASE);
    await b.page.click('#btn-open-sync');
    await b.page.getByText(/Synced today at/).waitFor({ timeout: 60000 });
    await b.page.getByText('2 sessions on your synced devices').waitFor({ timeout: 20000 });
    assert.strictEqual(await b.page.evaluate(() => localStorage.getItem('dust_theme')), theme, "the phone's theme");
    assert.strictEqual(
      await b.page.evaluate(() => document.documentElement.getAttribute('data-theme')),
      theme,
      'shown at once',
    );
    const settings = JSON.parse(await b.page.evaluate(() => localStorage.getItem('dust_settings')));
    assert.strictEqual(settings.numGroups, 7, "the phone's settings came across");

    // A sign-in run out: the card asks for one, and syncs once given it.
    drive.tokens.clear();
    await b.page.click('#btn-sync-now');
    await b.page.getByText('Sign in to Google again to sync.').waitFor({ timeout: 20000 });
    const again = b.page.waitForEvent('popup');
    await b.page.click('#btn-sign-in-to-sync');
    await again;
    await b.page.locator('#btn-sync-now').waitFor({ timeout: 20000 });

    // Turned off with the copy: the drive is emptied, and the phone finds
    // it gone.
    await b.page.click('#btn-turn-off-sync');
    await b.page.click('#btn-delete-synced-copy');
    await b.page.getByText('Turn on sync').waitFor({ timeout: 20000 });
    assert.strictEqual(drive.files.size, 0, 'the copy deleted');
    // The phone's sign-in ran out with the rest: it signs in again, and
    // then finds the copy gone, and turns sync off here too.
    await a.page.click('#btn-sync-now');
    await a.page.getByText('Sign in to Google again to sync.').waitFor({ timeout: 20000 });
    const phoneAgain = a.page.waitForEvent('popup');
    await a.page.click('#btn-sign-in-to-sync');
    await phoneAgain;
    await a.page.getByText(/gone from your Google Drive/).waitFor({ timeout: 20000 });
    assert.strictEqual(await a.page.evaluate(() => localStorage.getItem('dust_sync')), null);

    for (const d of [a, b]) {
      const off = d.elsewhere.filter(o => !o.includes('fonts.g'));
      assert.deepStrictEqual(off, [], 'nothing but the site and Google');
      assert.deepStrictEqual(breached(d), [], 'no page breaches its policy, the sign-in page among them');
    }
    console.log(`OK: ${signIns} sign-ins, ${drive.asked.length} Drive requests:`,
      [...new Set(drive.asked)].join(', '));
  } finally {
    await browser.close();
    server.close();
  }
})().catch(err => {
  console.error(err);
  process.exit(1);
});
