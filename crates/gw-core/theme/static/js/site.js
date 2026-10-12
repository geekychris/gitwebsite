// Site behaviour: code highlighting, search (press /), and runnable playgrounds.
(() => {
  // ---------------------------------------------------------------- highlighting
  window.addEventListener('load', () => {
    if (window.hljs) document.querySelectorAll('pre code').forEach(el => {
      if (!el.closest('.playground')) window.hljs.highlightElement(el);
    });
  });

  // ---------------------------------------------------------------- search
  const overlay = document.querySelector('[data-search]');
  const input = document.querySelector('[data-search-input]');
  const results = document.querySelector('[data-search-results]');
  let index = null;
  async function openSearch() {
    overlay.hidden = false;
    input.value = '';
    results.innerHTML = '';
    input.focus();
    if (!index) index = await fetch('/search.json').then(r => r.json()).catch(() => []);
  }
  function closeSearch() { overlay.hidden = true; }
  function score(item, terms) {
    const hay = [item.title, item.summary, item.kind, ...(item.areas || []), ...(item.tags || [])].join(' ').toLowerCase();
    let s = 0;
    for (const t of terms) {
      if (!hay.includes(t)) return 0;
      s += item.title.toLowerCase().includes(t) ? 3 : 1;
    }
    return s;
  }
  input && input.addEventListener('input', () => {
    const terms = input.value.toLowerCase().split(/\s+/).filter(Boolean);
    if (!terms.length || !index) { results.innerHTML = ''; return; }
    const hits = index.map(i => [score(i, terms), i]).filter(([s]) => s > 0).sort((a, b) => b[0] - a[0]).slice(0, 12);
    results.innerHTML = hits.map(([, i]) =>
      `<li><a href="${i.url}">${esc(i.title)}<small>${esc(i.kind)}${i.date ? ' · ' + i.date : ''}${i.summary ? ' — ' + esc(i.summary) : ''}</small></a></li>`
    ).join('') || '<li><a>No matches</a></li>';
  });
  document.querySelectorAll('[data-search-open]').forEach(b => b.addEventListener('click', openSearch));
  overlay && overlay.addEventListener('click', e => { if (e.target === overlay) closeSearch(); });
  document.addEventListener('keydown', e => {
    if (e.key === '/' && !/input|textarea/i.test(document.activeElement.tagName)) { e.preventDefault(); openSearch(); }
    if (e.key === 'Escape' && overlay && !overlay.hidden) closeSearch();
    if (e.key === 'Enter' && overlay && !overlay.hidden) { const a = results.querySelector('a[href]'); if (a) location.href = a.href; }
  });
  function esc(s) { return String(s || '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])); }

  // ---------------------------------------------------------------- area filter
  // Grouped listings (/projects/): the buttons show every card that has the area anywhere in
  // its list (data-areas holds each area path and its ancestors). ?area=making/3d-printing
  // opens filtered; the URL follows the choice without reloading.
  const bar = document.querySelector('[data-area-filter]');
  if (bar) {
    const cards = [...document.querySelectorAll('[data-areas]')];
    const groups = [...document.querySelectorAll('[data-group]')];
    const empty = document.querySelector('[data-filter-empty]');
    const apply = area => {
      for (const b of bar.querySelectorAll('[data-area]')) {
        const on = b.dataset.area === area;
        b.classList.toggle('on', on);
        b.setAttribute('aria-pressed', on ? 'true' : 'false');
      }
      for (const c of cards) c.hidden = !!area && !c.dataset.areas.split(' ').includes(area);
      let shown = 0;
      for (const g of groups) {
        const n = g.querySelectorAll('[data-areas]:not([hidden])').length;
        g.hidden = n === 0;
        shown += n;
      }
      if (empty) empty.hidden = shown > 0;
    };
    const fromUrl = () => new URLSearchParams(location.search).get('area') || '';
    bar.addEventListener('click', e => {
      const b = e.target.closest('[data-area]');
      if (!b) return;
      const area = b.dataset.area;
      const u = new URL(location.href);
      if (area) u.searchParams.set('area', area); else u.searchParams.delete('area');
      history.pushState(null, '', u);
      apply(area);
    });
    window.addEventListener('popstate', () => apply(fromUrl()));
    const start = fromUrl();
    if (start && bar.querySelector(`[data-area="${CSS.escape(start)}"]`)) apply(start);
  }

  // ---------------------------------------------------------------- playgrounds
  // AsciiDoc [source.playground,<lang>] blocks become editors with Run. The runtime (and each
  // language's interpreter or compiler) is loaded only on pages that have one: see playground.js.
  if (document.querySelector('.listingblock.playground')) {
    const s = document.createElement('script');
    s.src = '/js/playground.js';
    s.onload = () => window.GWPlay.enhanceAll();
    document.head.appendChild(s);
  }
})();
