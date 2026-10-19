// The code catalog (/code/): the search box, the category chips and tags narrow the grid.
// State lives in the address (#<category>, #tag=<tag>, ?q=<text>) so a filtered view can be shared.
(() => {
  const root = document.querySelector('[data-code-index]');
  if (!root) return;
  const box = root.querySelector('[data-code-filter]');
  const chips = [...root.querySelectorAll('[data-cat]')];
  const groups = [...root.querySelectorAll('[data-code-group]')];
  const cards = [...root.querySelectorAll('[data-code-card]')];
  const tagLinks = [...root.querySelectorAll('[data-tag]')];
  const shown = root.querySelector('[data-code-shown]');
  const none = root.querySelector('[data-code-none]');
  const tagline = root.querySelector('[data-code-tagline]');
  let cat = '', tag = '';

  const readHash = () => {
    const h = decodeURIComponent(location.hash.slice(1));
    tag = h.startsWith('tag=') ? h.slice(4) : '';
    cat = !tag && groups.some(g => g.dataset.codeGroup === h) ? h : '';
  };
  const groupOf = c => c.closest('[data-code-group]').dataset.codeGroup;
  const run = () => {
    // `_` and `-` read as spaces, as in each card's text.
    const words = (box.value || '').toLowerCase().replace(/[_-]/g, ' ').trim().split(/\s+/).filter(Boolean);
    let n = 0, anywhere = 0;
    const perCat = {};
    cards.forEach(c => {
      const text = words.every(w => c.dataset.text.includes(w)) && (!tag || c.dataset.tags.split(' ').includes(tag));
      if (text) { anywhere++; perCat[groupOf(c)] = (perCat[groupOf(c)] || 0) + 1; }
      const ok = text && (!cat || groupOf(c) === cat);
      c.hidden = !ok;
      if (ok) n++;
    });
    groups.forEach(g => (g.hidden = !g.querySelector('[data-code-card]:not([hidden])')));
    // The chips count what the search finds in each category.
    chips.forEach(b => {
      b.setAttribute('aria-pressed', String(b.dataset.cat === cat));
      const k = b.dataset.cat ? perCat[b.dataset.cat] || 0 : anywhere;
      b.querySelector('span').textContent = k;
      b.classList.toggle('code-chip-empty', !k);
    });
    tagLinks.forEach(a => a.toggleAttribute('aria-current', a.dataset.tag === tag));
    if (shown) shown.textContent = n;
    if (none) {
      none.hidden = n > 0;
      // Nothing in the chosen category: offer what the search finds in the others.
      const title = cat ? (chips.find(b => b.dataset.cat === cat)?.firstChild.textContent.trim() || cat) : '';
      none.innerHTML = cat && anywhere
        ? `Nothing in ${title.replace(/[&<>]/g, '')} matches — <a href="#" data-code-allcats>show ${anywhere} match${anywhere === 1 ? '' : 'es'} in all categories</a>.`
        : 'No projects match.';
      const all = none.querySelector('[data-code-allcats]');
      if (all) all.addEventListener('click', e => { e.preventDefault(); setHash(tag ? 'tag=' + encodeURIComponent(tag) : ''); });
    }
    if (tagline) {
      tagline.hidden = !tag;
      tagline.querySelector('[data-code-tagname]').textContent = tag;
    }
  };
  const setHash = h => {
    history.replaceState(null, '', h ? '#' + h : location.pathname + location.search);
    readHash(); run();
  };

  chips.forEach(b => b.addEventListener('click', () => setHash(b.dataset.cat)));
  tagLinks.forEach(a => a.addEventListener('click', e => {
    e.preventDefault();
    setHash(a.dataset.tag === tag ? '' : 'tag=' + encodeURIComponent(a.dataset.tag));
    root.scrollIntoView({ behavior: 'smooth' });
  }));
  const clear = root.querySelector('[data-code-cleartag]');
  if (clear) clear.addEventListener('click', e => { e.preventDefault(); setHash(''); });
  box.addEventListener('input', run);
  window.addEventListener('hashchange', () => { readHash(); run(); });
  const q = new URLSearchParams(location.search).get('q');
  if (q) box.value = q;
  readHash();
  run();
})();
