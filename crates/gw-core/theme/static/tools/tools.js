// Shared helpers for /tools/ pages.
//
// A tool script registers itself:
//
//   GW.tool('json', (root, k) => { root.append(k.h('div', {class: 't-panel'}, 'Hello')); });
//
// `k` is the kit below. Tools build their UI with k.h(), keep state in k.store(), and use the
// shared .t-* classes from tools.css so every tool looks and behaves alike.
(() => {
  const registry = {};

  /** Create an element: h('button', {class: 't-btn', onclick: fn}, 'Label', child, ...). */
  function h(tag, attrs, ...children) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === undefined || v === null || v === false) continue;
      if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
      else if (k === 'class') el.className = v;
      else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
      else if (k === 'dataset') Object.assign(el.dataset, v);
      else if (k in el && k !== 'list' && typeof v !== 'string') el[k] = v;
      else el.setAttribute(k, v === true ? '' : v);
    }
    for (const c of children.flat()) {
      if (c === undefined || c === null || c === false) continue;
      el.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return el;
  }

  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

  /** Per-tool persistent state in localStorage (survives reloads; never leaves the browser). */
  function store(ns) {
    const key = k => `gw-tool:${ns}:${k}`;
    return {
      get(k, def) { try { const v = localStorage.getItem(key(k)); return v === null ? def : JSON.parse(v); } catch { return def; } },
      set(k, v) { try { localStorage.setItem(key(k), JSON.stringify(v)); } catch {} },
      del(k) { try { localStorage.removeItem(key(k)); } catch {} },
    };
  }

  function debounce(fn, ms = 150) {
    let t;
    return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
  }

  let toastEl;
  function toast(msg, kind = '') {
    if (!toastEl) { toastEl = h('div', { class: 't-toast', role: 'status' }); document.body.append(toastEl); }
    toastEl.textContent = msg;
    toastEl.className = 't-toast show ' + kind;
    clearTimeout(toastEl._t);
    toastEl._t = setTimeout(() => (toastEl.className = 't-toast'), 1800);
  }

  async function copy(text) {
    try { await navigator.clipboard.writeText(text); toast('Copied'); }
    catch {
      const ta = h('textarea', { style: { position: 'fixed', opacity: 0 } }, text);
      document.body.append(ta); ta.select(); document.execCommand('copy'); ta.remove(); toast('Copied');
    }
  }

  /** A small "Copy" button that copies get() (a string or a function returning one). */
  const copyButton = (get, label = 'Copy') =>
    h('button', { class: 't-copy', type: 'button', onclick: () => copy(typeof get === 'function' ? get() : get) }, label);

  function download(name, data, type = 'text/plain') {
    const blob = data instanceof Blob ? data : new Blob([data], { type });
    const a = h('a', { href: URL.createObjectURL(blob), download: name });
    document.body.append(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(a.href), 2000);
  }

  function readFile(file, as = 'text') {
    return new Promise((ok, bad) => {
      const r = new FileReader();
      r.onload = () => ok(r.result); r.onerror = () => bad(r.error);
      if (as === 'arrayBuffer') r.readAsArrayBuffer(file);
      else if (as === 'dataURL') r.readAsDataURL(file);
      else r.readAsText(file);
    });
  }

  /** Open a file picker; resolves with the chosen File (or null). */
  function pickFile(accept = '') {
    return new Promise(ok => {
      const i = h('input', { type: 'file', accept, style: { display: 'none' } });
      i.onchange = () => { ok(i.files[0] || null); i.remove(); };
      document.body.append(i); i.click();
    });
  }

  /** Segmented control: seg([['a','A'],['b','B']], 'a', v => ...) */
  function seg(options, value, onchange) {
    const wrap = h('div', { class: 't-seg', role: 'group' });
    const set = v => { value = v; $$('button', wrap).forEach(b => b.classList.toggle('on', b.dataset.v === String(v))); };
    for (const [v, label] of options) {
      wrap.append(h('button', { type: 'button', dataset: { v: String(v) }, onclick: () => { set(v); onchange && onchange(v); } }, label));
    }
    set(value);
    wrap.value = () => value;
    wrap.set = set;
    return wrap;
  }

  /** Tabs: tabs([{id, label, render: el => ...}], storeObj). Renders lazily; remembers the tab. */
  function tabs(list, st) {
    const bar = h('div', { class: 't-tabs', role: 'tablist' });
    const body = h('div');
    const panes = {};
    const show = id => {
      $$('button', bar).forEach(b => b.classList.toggle('on', b.dataset.id === id));
      for (const t of list) {
        if (t.id === id && !panes[id]) { panes[id] = h('div'); t.render(panes[id]); body.append(panes[id]); }
        if (panes[t.id]) panes[t.id].hidden = t.id !== id;
      }
      st && st.set('tab', id);
      history.replaceState(null, '', '#' + id);
    };
    for (const t of list) bar.append(h('button', { type: 'button', role: 'tab', dataset: { id: t.id }, onclick: () => show(t.id) }, t.label));
    const fromHash = location.hash.slice(1);
    const start = list.find(t => t.id === fromHash) ? fromHash : (st && st.get('tab', null)) || list[0].id;
    const el = h('div', {}, bar, body);
    el.show = show;
    // A tab id in the hash can also match an element id in the help text (#crc on the CRC
    // page); the browser would then jump down the page. Keep the tool in view instead.
    if (fromHash && list.some(t => t.id === fromHash)) {
      const keep = () => { const other = document.getElementById(fromHash); if (other && !el.contains(other)) window.scrollTo(0, 0); };
      queueMicrotask(keep);
      addEventListener('load', () => setTimeout(keep, 0), { once: true });
    }
    queueMicrotask(() => show(list.find(t => t.id === start) ? start : list[0].id));
    return el;
  }

  /** Labelled field: field('Label', control, 'optional hint'). */
  const field = (label, control, hint) =>
    h('label', { class: 't-field', style: { display: 'block', minWidth: 0 } }, h('span', { class: 't-label' }, label), control, hint && h('p', { class: 't-hint' }, hint));

  const fmtBytes = n => {
    if (!isFinite(n)) return '—';
    const u = ['B', 'KB', 'MB', 'GB', 'TB']; let i = 0;
    while (Math.abs(n) >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return (i ? n.toFixed(n < 10 ? 2 : 1) : n) + ' ' + u[i];
  };

  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  /** Bind Cmd/Ctrl+Enter inside root to fn. */
  function onRun(root, fn) {
    root.addEventListener('keydown', e => { if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); fn(); } });
  }

  /** Textarea that keeps Tab as indentation. */
  function codeArea(attrs = {}) {
    const ta = h('textarea', Object.assign({ class: 't-textarea', spellcheck: false, autocapitalize: 'off', autocomplete: 'off' }, attrs));
    ta.addEventListener('keydown', e => {
      if (e.key === 'Tab' && !e.metaKey && !e.ctrlKey) {
        e.preventDefault();
        const s = ta.selectionStart;
        ta.setRangeText('  ', s, ta.selectionEnd, 'end');
        ta.dispatchEvent(new Event('input'));
      }
    });
    return ta;
  }

  // ------------------------------------------------------------------ deep links
  //
  // A tool opts in once its UI is built (call it last in the tool's setup):
  //
  //   k.share({
  //     state: () => ({ re: pattern.value, flags: flags.value }),  // JSON-serialisable
  //     apply: s => { pattern.value = s.re ?? ''; flags.value = s.flags ?? ''; render(); },
  //     readable: ['re', 'flags'],   // optional: emit these as plain ?re=…&flags=… when short
  //   });
  //
  // The kit adds a "Share link" button to the tool header, applies state from the page URL on
  // load, and builds links as /tools/<slug>/?<params>#<tab>. Short flat state listed in
  // `readable` stays human-readable (and hand-writable, e.g. /tools/railroad/?re=a%7Cb);
  // anything else is packed into ?s= (deflate-raw + base64url, or plain JSON as a fallback).
  // Mark the tool with `:deeplink: true` in its .adoc so it gets the link badge.

  const b64url = bytes => {
    let s = '';
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  };
  const unb64url = str => {
    const s = atob(str.replace(/-/g, '+').replace(/_/g, '/'));
    const out = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
    return out;
  };
  async function pipe(bytes, stream) {
    const out = new Response(new Blob([bytes]).stream().pipeThrough(stream));
    return new Uint8Array(await out.arrayBuffer());
  }
  /** Pack a state object for ?s=: 'z' + deflate-raw, or 'j' + plain JSON (both base64url). */
  async function packState(obj) {
    const json = new TextEncoder().encode(JSON.stringify(obj));
    if (typeof CompressionStream === 'function') {
      try { return 'z' + b64url(await pipe(json, new CompressionStream('deflate-raw'))); } catch {}
    }
    return 'j' + b64url(json);
  }
  async function unpackState(s) {
    const body = unb64url(s.slice(1));
    const bytes = s[0] === 'z' ? await pipe(body, new DecompressionStream('deflate-raw')) : body;
    return JSON.parse(new TextDecoder().decode(bytes));
  }
  const isFlat = v => v === null || ['string', 'number', 'boolean'].includes(typeof v);

  /** Build the query string for a state object (readable params when possible). */
  async function encodeState(obj, readable = []) {
    const keys = Object.keys(obj || {});
    const plain = readable.length && keys.every(k => readable.includes(k) && isFlat(obj[k]));
    if (plain) {
      const q = new URLSearchParams();
      for (const k of keys) if (obj[k] !== null && obj[k] !== undefined && obj[k] !== '') q.set(k, String(obj[k]));
      const str = q.toString();
      if (str.length <= 600) return str;
    }
    return 's=' + (await packState(obj));
  }
  /** Read state from a query string. Readable params come back as strings (tools coerce). */
  async function decodeState(search, readable = []) {
    const q = new URLSearchParams(search);
    if (q.has('s')) return unpackState(q.get('s'));
    const obj = {};
    let any = false;
    for (const k of readable) if (q.has(k)) { obj[k] = q.get(k); any = true; }
    return any ? obj : null;
  }

  const linkIcon = () => {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('width', '16'); svg.setAttribute('height', '16');
    svg.setAttribute('aria-hidden', 'true'); svg.setAttribute('fill', 'none'); svg.setAttribute('stroke', 'currentColor');
    svg.setAttribute('stroke-width', '2'); svg.setAttribute('stroke-linecap', 'round'); svg.setAttribute('stroke-linejoin', 'round');
    for (const d of ['M10 13a5 5 0 0 0 7.07 0l3-3a5 5 0 0 0-7.07-7.07l-1.5 1.5', 'M14 11a5 5 0 0 0-7.07 0l-3 3a5 5 0 0 0 7.07 7.07l1.5-1.5']) {
      const p = document.createElementNS(ns, 'path'); p.setAttribute('d', d); svg.append(p);
    }
    return svg;
  };

  /**
   * Copy text produced asynchronously. WebKit (Safari, and the gitwebsite app's preview) only
   * allows clipboard writes inside the click itself, so hand the clipboard a promise via
   * ClipboardItem while still in the gesture; fall back to writeText / execCommand.
   */
  async function copyLater(promiseOfText) {
    if (navigator.clipboard && window.ClipboardItem) {
      try {
        const blob = promiseOfText.then(t => new Blob([t], { type: 'text/plain' }));
        await navigator.clipboard.write([new ClipboardItem({ 'text/plain': blob })]);
        return true;
      } catch {}
    }
    const text = await promiseOfText;
    try { await navigator.clipboard.writeText(text); return true; } catch {}
    try {
      const ta = h('textarea', { style: { position: 'fixed', opacity: 0 } }, text);
      document.body.append(ta); ta.select();
      const ok = document.execCommand('copy'); ta.remove();
      return ok;
    } catch { return false; }
  }

  /** Make the tool's current configuration shareable as a URL (see the comment above). */
  function share({ state, apply, readable = [], label = 'Share link', autoApply = true }) {
    // Links always point at the public site, even from a local preview (127.0.0.1, the app).
    const slot = document.querySelector('[data-tool-actions]');
    const publicBase = (slot && slot.dataset.publicBase) || location.origin;
    const base = () => publicBase + location.pathname;
    const link = async () => {
      const q = await encodeState(state(), readable);
      return base() + (q ? '?' + q : '') + location.hash;
    };
    const btn = h('button', { type: 'button', class: 't-btn t-share', title: 'Copy a link that opens this tool exactly as configured now' },
      linkIcon(), h('span', {}, label));
    // The link is also shown in a field beside the button, selected, so it can be copied by hand
    // if the clipboard is unavailable.
    const box = h('input', { class: 't-input t-mono t-share-url', readonly: true, hidden: true, 'aria-label': 'Shareable link',
      onfocus: e => e.target.select() });
    btn.addEventListener('click', () => {
      const p = link();
      copyLater(p).then(async ok => {
        const url = await p;
        // Reflect the state in the address bar too (same origin only; the public URL may differ).
        try { history.replaceState(null, '', location.pathname + url.slice(base().length)); } catch {}
        box.value = url; box.hidden = false; box.focus(); box.select();
        const long = url.length > 8000 ? ' — it is long (' + url.length + ' chars); some apps may cut it off' : '';
        toast(ok ? 'Link copied' + long : 'Copy the selected link (⌘C / Ctrl+C)', ok ? '' : 'err');
      }, e => toast('Could not build a link: ' + e.message, 'err'));
    });
    if (slot) slot.append(btn, box);
    const ready = (async () => {
      if (!autoApply || !location.search) return false;
      try {
        const s = await decodeState(location.search, readable);
        if (s) { apply(s); return true; }
      } catch (e) { console.error(e); toast('This link could not be read: ' + e.message, 'err'); }
      return false;
    })();
    const handle = { button: btn, link, ready, state, apply };
    window.GW._share = handle;   // for tests: GW._share.state(), await GW._share.link()
    return handle;
  }

  // Values that look like credentials (password=…, "token": …, psk: …) never go into a link.
  const CREDENTIAL = /(\b[\w.-]*(?:passw(?:or)?d|secret|token|api_?key|psk)[\w.-]*\s*[=:]\s*)("[^"\n]*"|'[^'\n]*'|[^\s,;}]+)/gi;
  const redact = v => typeof v === 'string' ? v.replace(CREDENTIAL, '$1***') : Array.isArray(v) ? v.map(redact)
    : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).map(([a, b]) => [a, redact(b)])) : v;
  const isPlainObject = v => v !== null && typeof v === 'object' && !Array.isArray(v);

  /**
   * Share a tool's saved settings — its k.store entries — the common case for tools whose state
   * already lives in k.store. Await it FIRST, before building the UI, and build from the store as
   * usual:
   *
   *   GW.tool('gears', async (root, k) => {
   *     await k.shareStore('gears', { readable: { m: ['spur.m', 'n'], z1: ['spur.z1', 'n'] } });
   *     const st = k.store('gears');  …
   *   });
   *
   * Options:
   *   readable  short URL parameter → setting, so links can be written by hand: 'key',
   *             'key.field' (a field of an object setting), a shared 'ww:key', or [path, type]
   *             with type 'n' (number) or 'b' (boolean) to coerce hand-written values. An array
   *             ['data', 'dist'] maps each parameter to the setting of the same name. A link is
   *             written with these names when every setting maps onto one; otherwise all the
   *             settings are packed into ?s=.
   *   inbound   extra hand-written parameters: name → (value, allParams) => ({ key: value, … }).
   *   skip      keys never shared (history, personal checklists). 'tab' and keys starting with
   *             '_' are never shared, and credential-looking values are redacted.
   *   shared    { ns, prefix, keys }: settings kept in another tool namespace, carried as
   *             prefix + key (the workshop tools carry woodworking units as ww:units).
   *   scrub     settings => settings: drop tool-specific secrets before a link is built (MQTT
   *             passwords, Wi-Fi keys). Receives a fresh copy of the settings.
   *
   * Opening a link replaces the visitor's saved settings for the tool (a "Restore my settings"
   * button undoes it) and then removes the link from the address bar, so a refresh keeps later
   * edits. Resolves with the share handle.
   */
  async function shareStore(ns, opts = {}) {
    if (isPlainObject(ns)) { opts = ns; ns = opts.ns; }
    const { inbound = {}, skip = [], shared = null, scrub = x => x } = opts;
    const specs = Array.isArray(opts.readable) ? Object.fromEntries(opts.readable.map(p => [p, p])) : (opts.readable || {});
    const R = Object.entries(specs).map(([p, spec]) => {
      const [path, type] = Array.isArray(spec) ? spec : [spec];
      const [key, field] = path.split('.');
      return { p, key, field, type };
    });
    const pre = `gw-tool:${ns}:`, st = store(ns);
    const sh = shared && { st: store(shared.ns), prefix: shared.prefix || shared.ns + ':', keys: shared.keys || [] };
    const omit = n => n === 'tab' || n[0] === '_' || skip.includes(n);
    const own = () => {
      const out = [];
      try { for (let i = 0; i < localStorage.length; i++) { const n = localStorage.key(i); if (n && n.startsWith(pre) && !omit(n.slice(pre.length))) out.push(n.slice(pre.length)); } } catch {}
      return out.sort();
    };
    const read = () => {
      const o = {};
      for (const n of own()) o[n] = st.get(n);
      if (sh) for (const n of sh.keys) { const v = sh.st.get(n); if (v !== undefined) o[sh.prefix + n] = v; }
      return o;
    };
    const write = o => {
      for (const n of own()) st.del(n);
      if (sh) for (const n of sh.keys) sh.st.del(n);
      for (const [n, v] of Object.entries(o || {})) {
        if (sh && n.startsWith(sh.prefix)) { if (sh.keys.includes(n.slice(sh.prefix.length))) sh.st.set(n.slice(sh.prefix.length), v); }
        else if (!omit(n)) st.set(n, v);
      }
    };
    // Settings as readable parameters, or null when something can't be written that way.
    const flat = o => {
      const out = {};
      for (const [key, v] of Object.entries(o)) {
        if (isPlainObject(v)) {
          for (const [f, fv] of Object.entries(v)) {
            const r = R.find(r => r.key === key && r.field === f);
            if (!r || (fv !== null && typeof fv === 'object')) return null;
            out[r.p] = fv;
          }
        } else {
          const r = R.find(r => r.key === key && !r.field);
          if (!r || Array.isArray(v)) return null;
          out[r.p] = v;
        }
      }
      return out;
    };
    const coerce = (v, type) => typeof v !== 'string' ? v
      : type === 'n' || type === 'num' ? (v.trim() !== '' && isFinite(+v) ? +v : v)
      : type === 'b' || type === 'bool' ? v === 'true' || v === '1'
      : v;
    // Link contents (readable parameters, packed settings, or old links' aliases) → settings.
    const toStore = o => {
      const out = {};
      for (const [n, v] of Object.entries(isPlainObject(o) ? o : {})) {
        if (inbound[n]) { try { Object.assign(out, inbound[n](v, o)); } catch {} continue; }
        const r = R.find(r => r.p === n);
        if (!r) { out[n] = v; continue; }
        if (r.field) (out[r.key] = isPlainObject(out[r.key]) ? out[r.key] : {})[r.field] = coerce(v, r.type);
        else out[r.key] = coerce(v, r.type);
      }
      return out;
    };
    const params = [...R.map(r => r.p), ...Object.keys(inbound)];
    let mine = null;
    if (location.search) {
      try {
        const o = await decodeState(location.search, params);
        if (o) {
          const before = read();
          write(toStore(o));
          history.replaceState(null, '', location.pathname + location.hash);
          if (Object.keys(before).length && JSON.stringify(before) !== JSON.stringify(read())) mine = before;
        }
      } catch (e) { console.error(e); toast('This link could not be read: ' + e.message, 'err'); }
    }
    const handle = share({
      state: () => { const o = scrub(read()) || {}; return redact(flat(o) || o); },
      apply: o => { write(toStore(o)); location.reload(); },
      readable: params,
      autoApply: false,
    });
    const slot = document.querySelector('[data-tool-actions]');
    if (mine && slot) {
      slot.append(h('p', { class: 't-hint t-share-note' }, 'Opened from a shared link, which replaced your saved settings for this tool. ',
        h('button', { class: 't-btn small', type: 'button', onclick: () => { write(mine); location.reload(); } }, 'Restore my settings')));
    }
    return handle;
  }

  const kit = { h, $, $$, store, debounce, toast, copy, copyButton, download, readFile, pickFile, seg, tabs, field, fmtBytes, esc, onRun, codeArea, share, shareStore };
  kit._deeplink = { encodeState, decodeState, packState, unpackState };

  window.GW = {
    kit,
    tool(name, fn) {
      registry[name] = fn;
      const mount = () => {
        const root = document.querySelector(`#tool-root[data-tool="${name}"]`);
        if (!root || root.dataset.mounted) return;
        root.dataset.mounted = '1';
        root.innerHTML = '';
        const fail = e => { root.append(h('p', { class: 't-err' }, 'This tool failed to start: ' + e.message)); console.error(e); };
        try { const r = fn(root, kit); if (r && typeof r.then === 'function') r.catch(fail); }
        catch (e) { fail(e); }
      };
      if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', mount); else mount();
    },
  };
})();
