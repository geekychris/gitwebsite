// The 3-D models gallery (/models/ or a post with models::gallery[], and /models/<project>/<slug>/). Everything here is an
// enhancement: without it the grid links to each model's page and the views link to the images.
// - a model's detail: the view thumbnails (and ← →) swap the big image; "3-D" shows the
//   interactive <model-viewer>, whose script is fetched only then
// - the gallery: project chips filter the grid (#<project> in the URL); a card opens the
//   model's detail in a dialog (fetched from its page) and the address becomes the model's URL
'use strict';
(() => {
  const VIEWER_JS = 'https://cdn.jsdelivr.net/npm/@google/model-viewer@4/dist/model-viewer.min.js';

  function loadViewer() {
    if (window.customElements && customElements.get('model-viewer')) return;
    if (document.querySelector('script[data-mg-viewer], script[src*="model-viewer"]')) return;
    const s = document.createElement('script');
    s.type = 'module'; s.src = VIEWER_JS; s.dataset.mgViewer = '';
    document.head.appendChild(s);
  }

  /** Wire up one [data-model-detail] article; returns {step(n)} for the arrow keys. */
  function enhance(detail) {
    const big = detail.querySelector('[data-mg-big]');
    const box3d = detail.querySelector('[data-mg-3d]');
    const tab3d = detail.querySelector('[data-mg-3d-tab]');
    const hint = detail.querySelector('[data-mg-hint]');
    if (tab3d && box3d) tab3d.hidden = false;
    const views = [...detail.querySelectorAll('[data-mg-view]')].filter(v => !v.closest('[hidden]'));
    if (hint && views.length > 1) hint.hidden = false;
    let cur = 0;
    function show(i) {
      if (!views.length) return;
      cur = (i + views.length) % views.length;
      const v = views[cur];
      views.forEach(x => x === v ? x.setAttribute('aria-current', 'true') : x.removeAttribute('aria-current'));
      if (v.dataset.mgView === '3d') {
        loadViewer();
        box3d.hidden = false; big.hidden = true;
      } else {
        if (box3d) box3d.hidden = true;
        big.hidden = false;
        big.src = v.dataset.src; big.alt = v.dataset.alt || '';
      }
    }
    views.forEach((v, i) => v.addEventListener('click', e => {
      if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button) return;
      e.preventDefault(); show(i);
    }));
    // The 3-D view first when there is one (its poster is the hero render until it loads).
    show(0);
    return { step: n => show(cur + n) };
  }

  const typing = t => t && (t.isContentEditable || /^(input|textarea|select)$/i.test(t.tagName));
  const arrows = (ctl) => e => {
    if (e.defaultPrevented || typing(e.target) || (e.target.closest && e.target.closest('model-viewer'))) return;
    if (e.key === 'ArrowRight') { ctl.step(1); e.preventDefault(); }
    else if (e.key === 'ArrowLeft') { ctl.step(-1); e.preventDefault(); }
  };

  // ---- a model's own page
  const index = document.querySelector('[data-mg-index]');
  if (!index) {
    const d = document.querySelector('[data-model-detail]');
    if (d) document.addEventListener('keydown', arrows(enhance(d)));
    return;
  }

  // ---- the gallery: filter chips
  const cards = [...index.querySelectorAll('.mg-card')];
  const chips = index.querySelector('[data-mg-chips]');
  const count = index.querySelector('[data-mg-count]');
  const none = index.querySelector('[data-mg-none]');
  const projects = new Set(cards.map(c => c.dataset.project));
  function filter(p, push) {
    if (!projects.has(p)) p = '';
    let n = 0;
    cards.forEach(c => { const on = !p || c.dataset.project === p; c.hidden = !on; if (on) n++; });
    if (chips) chips.querySelectorAll('[data-mg-project]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.mgProject === p)));
    if (count) count.textContent = n;
    if (none) none.hidden = n > 0;
    if (push !== false) history.replaceState(history.state, '', p ? '#' + p : location.pathname);
  }
  if (chips) {
    chips.hidden = false;
    chips.addEventListener('click', e => { const b = e.target.closest('[data-mg-project]'); if (b) filter(b.dataset.mgProject); });
  }
  const fromHash = () => decodeURIComponent(location.hash.slice(1));
  if (fromHash()) filter(fromHash(), false);

  // ---- the gallery: a model in a dialog
  const dialog = document.querySelector('[data-mg-dialog]');
  if (!dialog || typeof dialog.showModal !== 'function' || !window.fetch || !window.DOMParser) return;
  const body = dialog.querySelector('[data-mg-dialog-body]');
  const indexTitle = document.title;
  const indexPath = location.pathname;   // /models/, or the post that shows the gallery
  let ctl = null, openUrl = null, opened = 0;

  async function open(url, push) {
    let html;
    try {
      const r = await fetch(url, { credentials: 'same-origin' });
      if (!r.ok) throw new Error(r.status);
      html = await r.text();
    } catch (_) { location.href = url; return; }
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const detail = doc.querySelector('[data-model-detail]');
    if (!detail) { location.href = url; return; }
    body.replaceChildren(document.importNode(detail, true));
    const d = body.querySelector('[data-model-detail]');
    ctl = enhance(d);
    openUrl = url;
    document.title = doc.title || indexTitle;
    if (push) { history.pushState({ mg: url }, '', url); opened++; }
    if (!dialog.open) { dialog.showModal(); document.documentElement.classList.add('mg-lock'); }
    dialog.scrollTop = 0;
    const h = d.querySelector('.mg-title'); if (h) { h.tabIndex = -1; h.focus({ preventScroll: true }); }
  }
  function hide() {
    if (dialog.open) dialog.close();
    document.documentElement.classList.remove('mg-lock');
    body.replaceChildren(); ctl = null; openUrl = null;
    document.title = indexTitle;
  }
  function close() {
    if (opened > 0 && history.state && history.state.mg) { history.go(-opened); opened = 0; return; }   // popstate hides it
    hide();
    history.replaceState(null, '', indexPath + location.hash);
  }

  index.addEventListener('click', e => {
    const a = e.target.closest('a[data-mg-open]');
    if (!a || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button) return;
    e.preventDefault();
    open(a.getAttribute('href'), true);
  });
  // Previous / next inside the dialog stay in the dialog.
  body.addEventListener('click', e => {
    const a = e.target.closest('a[data-mg-nav]');
    if (!a || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button) return;
    e.preventDefault();
    history.replaceState({ mg: a.getAttribute('href') }, '', a.getAttribute('href'));
    open(a.getAttribute('href'), false);
  });
  dialog.querySelector('[data-mg-close]').addEventListener('click', close);
  dialog.addEventListener('cancel', e => { e.preventDefault(); close(); });
  dialog.addEventListener('click', e => { if (e.target === dialog) close(); });
  dialog.addEventListener('keydown', e => { if (ctl) arrows(ctl)(e); });
  window.addEventListener('popstate', () => {
    const p = location.pathname;
    if (/^\/models\/[^/]+\/[^/]+\/$/.test(p)) { if (p !== openUrl) open(p, false); }
    else { opened = 0; hide(); filter(fromHash(), false); }
  });
})();
