#!/usr/bin/env node
// The site's content security policies, as Pages will serve them.
//
//   node scripts/check-site-policy.cjs <site dir> [page dx wrote...]
//
// Every page of the site sets its own policy, in a <meta> ahead of all it
// governs, as GitHub Pages sets no headers (index.html, docs/oauth.html and
// docs/privacy.html; src/app.rs's tests hold each to what it needs). The
// pages named after the site are those `dx bundle` wrote from index.html,
// which may put inline scripts of its own in, changing with dx's version:
// the hash of each inline script in them is worked out here and added to
// the policy where it is not listed yet. Then every page of the site must
// have all it holds inline covered by its policy, or this fails:
//
// - each inline script, by its hash, and no policy may let any inline
//   script run ('unsafe-inline');
// - no handler written into the markup, nor a `javascript:` address, which
//   no hash covers;
// - each inline style, and each style attribute: by 'unsafe-inline', with
//   no hash or nonce beside it, which would turn 'unsafe-inline' off (the
//   app's page, where Dioxus writes style attributes), or else each style
//   by its hash and no style attribute at all.
'use strict';

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');

const SITE = path.resolve(process.argv[2] || 'site');
const WRITTEN_BY_DX = new Set(process.argv.slice(3));

/// The tags of `page`, in order, with where each starts and ends, its
/// attributes, and what a script or a style holds. Comments are passed
/// over.
function tagsOf(page) {
  const tags = [];
  let at = 0;
  while ((at = page.indexOf('<', at)) !== -1) {
    const start = at;
    at += 1;
    if (page.startsWith('!--', at)) {
      const end = page.indexOf('-->', at);
      if (end === -1) throw new Error('a comment that never ends');
      at = end + 3;
      continue;
    }
    const name = (/^[A-Za-z][A-Za-z0-9]*/.exec(page.slice(at)) || [''])[0].toLowerCase();
    if (!name) continue;
    at += name.length;
    const attributes = [];
    for (;;) {
      while (/\s/.test(page[at] || '')) at += 1;
      if (at >= page.length) throw new Error(`<${name}> never ends`);
      if (page.startsWith('/>', at)) {
        at += 2;
        break;
      }
      if (page[at] === '>') {
        at += 1;
        break;
      }
      const key = (/^[^\s=>\/]+/.exec(page.slice(at)) || [page[at]])[0];
      at += key.length;
      while (/\s/.test(page[at] || '')) at += 1;
      let value = '';
      let valueAt = -1;
      if (page[at] === '=') {
        at += 1;
        while (/\s/.test(page[at] || '')) at += 1;
        const quote = page[at];
        if (quote === '"' || quote === "'") {
          const end = page.indexOf(quote, at + 1);
          if (end === -1) throw new Error(`<${name} ${key}> never ends`);
          valueAt = at + 1;
          value = page.slice(at + 1, end);
          at = end + 1;
        } else {
          const bare = /^[^\s>]*/.exec(page.slice(at))[0];
          valueAt = at;
          value = bare;
          at += bare.length;
        }
      }
      attributes.push({ key: key.toLowerCase(), value, valueAt });
    }
    const tag = { name, start, end: at, attributes, holds: null };
    if (name === 'script' || name === 'style') {
      const close = page.toLowerCase().indexOf(`</${name}`, at);
      if (close === -1) throw new Error(`<${name}> never closes`);
      tag.holds = page.slice(at, close);
      at = close;
    }
    tags.push(tag);
  }
  return tags;
}

const attribute = (tag, key) => {
  const found = tag.attributes.find(a => a.key === key);
  return found ? found.value : null;
};

const hashOf = text => `'sha256-${crypto.createHash('sha256').update(text, 'utf8').digest('base64')}'`;

/// A policy's directives, in order, each with its sources.
function parse(policy) {
  return policy
    .split(';')
    .map(directive => directive.trim().split(/\s+/).filter(Boolean))
    .filter(words => words.length)
    .map(([name, ...sources]) => ({ name: name.toLowerCase(), sources }));
}

const write = directives => directives.map(d => [d.name, ...d.sources].join(' ')).join('; ');

/// The directive that governs `kind` (script-src, style-src), falling back
/// to default-src as a browser does: None when neither is set, and then
/// anything goes.
const governing = (directives, kind) =>
  directives.find(d => d.name === kind) || directives.find(d => d.name === 'default-src') || null;

/// Check one page, completing its policy with the hashes of its inline
/// scripts first if `complete`: what is wrong with it, and the page as it
/// is to be served.
function check(file, complete) {
  let page = fs.readFileSync(file, 'utf8');
  const problems = [];
  let tags = tagsOf(page);
  const metas = tags.filter(t => (attribute(t, 'http-equiv') || '').toLowerCase() === 'content-security-policy');
  if (metas.length !== 1) return { problems: [`sets ${metas.length} policies of its own, not one`], page };
  const [meta] = metas;
  const governed = tags.find(t => ['script', 'style', 'link', 'iframe', 'img', 'object', 'embed'].includes(t.name));
  if (governed && governed.start < meta.start) problems.push(`its <${governed.name}> comes ahead of its policy`);
  const content = meta.attributes.find(a => a.key === 'content');
  if (!content) return { problems: ['its policy is empty'], page };
  const directives = parse(content.value);
  const scripts = tags.filter(t => t.name === 'script' && attribute(t, 'src') === null);
  const added = [];
  if (complete) {
    let script = directives.find(d => d.name === 'script-src');
    if (!script && scripts.length) {
      const fallback = directives.find(d => d.name === 'default-src');
      script = { name: 'script-src', sources: fallback ? [...fallback.sources] : [] };
      directives.splice(directives.findIndex(d => d.name === 'default-src') + 1, 0, script);
    }
    for (const tag of scripts) {
      const hash = hashOf(tag.holds);
      if (!script.sources.includes(hash)) {
        script.sources = script.sources.filter(s => s !== "'none'");
        script.sources.push(hash);
        added.push(hash);
      }
    }
    if (added.length) {
      const written = write(directives).replace(/"/g, '&quot;');
      page = page.slice(0, content.valueAt) + written + page.slice(content.valueAt + content.value.length);
      tags = tagsOf(page);
    }
  }
  const script = governing(directives, 'script-src');
  if (!script) problems.push('no script-src nor default-src: it lets any script run');
  else if (script.sources.includes("'unsafe-inline'")) problems.push(`${script.name} lets any inline script run ('unsafe-inline')`);
  for (const tag of scripts) {
    const hash = hashOf(tag.holds);
    if (script && !script.sources.includes(hash)) {
      problems.push(`an inline script its policy does not list: ${hash} (${JSON.stringify(tag.holds.trim().slice(0, 60))}…)`);
    }
  }
  for (const tag of tags) {
    for (const { key, value } of tag.attributes) {
      if (key.startsWith('on')) problems.push(`a handler written into the markup: <${tag.name} ${key}="${value}">`);
      if (/^\s*javascript:/i.test(value)) problems.push(`a javascript: address: <${tag.name} ${key}="${value}">`);
    }
  }
  const style = governing(directives, 'style-src');
  const styles = tags.filter(t => t.name === 'style');
  const styled = tags.filter(t => attribute(t, 'style') !== null);
  if (style) {
    const hashed = style.sources.some(s => /^'(sha(256|384|512)|nonce)-/.test(s));
    const inline = style.sources.includes("'unsafe-inline'") && !hashed;
    if (!inline) {
      for (const tag of styles) {
        const hash = hashOf(tag.holds);
        if (!style.sources.includes(hash)) problems.push(`an inline style its policy does not list: ${hash}`);
      }
      for (const tag of styled) {
        problems.push(`a style attribute its policy does not let apply: <${tag.name} style="${attribute(tag, 'style')}">`);
      }
    }
  }
  return { problems, page, added, policy: write(directives) };
}

function pages(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const at = path.join(dir, entry.name);
    if (entry.isDirectory()) return pages(at);
    return entry.name.endsWith('.html') ? [at] : [];
  });
}

let failed = false;
const found = pages(SITE);
for (const name of WRITTEN_BY_DX) {
  if (!found.includes(path.join(SITE, name))) {
    console.error(`${name}: no such page in ${SITE}`);
    failed = true;
  }
}
for (const file of found) {
  const name = path.relative(SITE, file);
  const complete = WRITTEN_BY_DX.has(name);
  const { problems, page, added = [], policy } = check(file, complete);
  if (added.length) {
    fs.writeFileSync(file, page);
    console.log(`${name}: added the hashes of the inline scripts dx bundle put in: ${added.join(' ')}`);
  }
  if (problems.length) {
    failed = true;
    console.error(`${name}:`);
    for (const problem of problems) console.error(`  ${problem}`);
  } else {
    console.log(`${name}: ${policy}`);
  }
}
if (!found.length) {
  console.error(`no page in ${SITE}`);
  failed = true;
}
if (failed) {
  console.error('A page of the site sets no policy of its own, or holds inline what its policy does not cover.');
  process.exit(1);
}
