// gitwebsite management UI — talks to the REST API at /api. No build step.
'use strict';

const $ = (sel, el = document) => el.querySelector(sel);
const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];
const main = $('[data-main]');
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const enc = id => id.split('/').map(encodeURIComponent).join('/');

async function api(method, path, body) {
  const r = await fetch('/api' + path, {
    method, headers: body ? { 'content-type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data.error || `${r.status} ${r.statusText}`);
  return data;
}
const get = p => api('GET', p);

function toast(msg, err = false) {
  const t = $('[data-toast]');
  t.textContent = msg; t.className = 'toast' + (err ? ' err' : ''); t.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (t.hidden = true), err ? 7000 : 3000);
}
async function guard(fn) {
  try { return await fn(); } catch (e) { toast(e.message, true); }
}

// ---------------------------------------------------------------- modal forms

/** Show a form; fields: [{name, label, type, value, options, multiple, rows, full}]. Resolves to values or null. */
/** Files dropped anywhere in the app. On a story page they go to that story (the dashed box
 *  handles its own drops first); elsewhere they're ignored. Never let the browser open a dropped
 *  file in place of the app. */
let storyDrop = null;   // { slug, send } while a story page is showing
const hasFiles = e => e.dataTransfer && [...e.dataTransfer.types].includes('Files');
document.addEventListener('dragover', e => { if (hasFiles(e)) { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; } });
document.addEventListener('drop', e => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  if (e.target.closest && e.target.closest('[data-drop]')) return;   // the dashed box took it
  const onStory = storyDrop && location.hash === '#story/' + storyDrop.slug;
  if (!onStory) return toast('Open a story, then drop the photos on it', true);
  if (e.dataTransfer.files.length) storyDrop.send(e.dataTransfer.files);
  else toast('Nothing came through with that drop. From Apple Photos, use “From Apple Photos…” (or drag the photos to Finder first).', true);
});

/** Bring photos in from Apple Photos (macOS): the ones selected in Photos, or an album. */
let photosAlbums = null;
async function fromApplePhotos(slug, reload) {
  const v = await form('Add from Apple Photos', [
    { type: 'note', text: 'Photos exports the current versions (with your edits) as JPEG. The first time, macOS asks whether gitwebsite may control Photos — allow it.' },
    { name: 'from', label: 'Which photos', type: 'select', value: 'selection', options: [['selection', 'The photos selected in Photos now'], ['album', 'An album…']] },
  ], 'Next');
  if (!v) return;
  const base = `/stories/${encodeURIComponent(slug)}/add-photos`;
  if (v.from === 'selection') {
    return guard(async () => {
      toast('Exporting from Photos…');
      const r = await api('POST', base, {});
      toast(`Added ${r.added} photo${r.added === 1 ? '' : 's'} from Photos${r.videos_skipped ? ` (${r.videos_skipped} video${r.videos_skipped === 1 ? '' : 's'} skipped)` : ''} — give each a comment`);
      reload();
    });
  }
  if (!photosAlbums) {
    toast('Reading your albums from Photos…');
    try { photosAlbums = (await api('GET', '/photos/albums')).albums; } catch (e) { return toast(e.message, true); }
  }
  if (!photosAlbums.length) return toast('No albums in Photos', true);
  const a = await form('Add an album from Apple Photos', [
    { name: 'album', label: 'Album', type: 'select', value: photosAlbums[0].id, options: photosAlbums.map(x => [x.id, `${x.name} (${x.count})`]) },
    { name: 'candidate', label: 'Offer them as candidates to choose from (untick to add them all as chosen photos)', type: 'checkbox', value: true },
    { name: 'limit', label: 'At most this many photos', type: 'number', value: '300' },
  ], 'Add');
  if (!a) return;
  guard(async () => {
    toast('Exporting the album from Photos…');
    const r = await api('POST', base, { album: a.album, candidate: a.candidate, limit: Math.max(1, parseInt(a.limit, 10) || 300) });
    toast(`Added ${r.added} ${r.as === 'candidates' ? 'candidate' : 'photo'}${r.added === 1 ? '' : 's'}${r.already_in_story ? `, ${r.already_in_story} already in the story` : ''}${r.videos_skipped ? `, ${r.videos_skipped} video${r.videos_skipped === 1 ? '' : 's'} skipped` : ''}`);
    reload();
  });
}

function form(title, fields, okLabel = 'Save') {
  const dlg = $('[data-modal]');
  const f = $('[data-modal-form]');
  const field = x => {
    const v = x.value ?? '';
    let input;
    if (x.type === 'select') {
      input = `<select name="${x.name}" ${x.multiple ? 'multiple' : ''}>${(x.options || []).map(o => {
        const [val, lab] = Array.isArray(o) ? o : [o, o];
        const sel = x.multiple ? (v || []).includes(val) : v === val;
        return `<option value="${esc(val)}" ${sel ? 'selected' : ''}>${esc(lab)}</option>`;
      }).join('')}</select>`;
    } else if (x.type === 'textarea') {
      input = `<textarea name="${x.name}" rows="${x.rows || 4}" class="${x.mono ? 'mono' : ''}">${esc(v)}</textarea>`;
    } else if (x.type === 'note') {
      return `<p class="muted">${esc(x.text)}</p>`;
    } else if (x.type === 'checkbox') {
      return `<label style="flex-direction:row;align-items:center;gap:.5rem"><input type="checkbox" name="${x.name}" ${v ? 'checked' : ''}> ${esc(x.label)}</label>`;
    } else {
      input = `<input name="${x.name}" type="${x.type || 'text'}" value="${esc(v)}" ${x.required ? 'required' : ''} placeholder="${esc(x.placeholder || '')}">`;
    }
    return `<label>${esc(x.label)}${input}</label>`;
  };
  f.innerHTML = `<h2>${esc(title)}</h2>${fields.map(field).join('')}
    <div class="actions"><button value="cancel" type="button" data-cancel>Cancel</button><button class="primary" value="ok">${esc(okLabel)}</button></div>`;
  return new Promise(resolve => {
    $('[data-cancel]', f).onclick = () => { dlg.close(); resolve(null); };
    f.onsubmit = e => {
      e.preventDefault();
      const out = {};
      for (const x of fields) {
        if (x.type === 'note') continue;
        const el = f.elements[x.name];
        if (x.type === 'checkbox') out[x.name] = el.checked;
        else if (x.multiple) out[x.name] = [...el.selectedOptions].map(o => o.value);
        else out[x.name] = el.value;
      }
      dlg.close(); resolve(out);
    };
    dlg.showModal();
  });
}
/** Yes/no question in the app's own dialog. (The macOS app's web view shows nothing for
 *  window.confirm() and treats it as "no", so never use confirm/alert/prompt here.) */
async function ask(title, text, okLabel = 'OK') {
  return (await form(title, text ? [{ type: 'note', text }] : [], okLabel)) !== null;
}
const csv = s => String(s || '').split(',').map(x => x.trim()).filter(Boolean);

// ---------------------------------------------------------------- shared data

let areasCache = null;
async function areas() {
  if (!areasCache) areasCache = await get('/areas');
  return areasCache;
}
const KINDS = ['article', 'project', 'update', 'page', 'app', 'playground', 'tool'];

function statusPill(it) {
  if (it.draft) return '<span class="pill draft">draft</span>';
  if (it.unlisted) return '<span class="pill unlisted">unlisted</span>';
  return '<span class="pill published">published</span>';
}

// ---------------------------------------------------------------- Content

const contentFilter = { kind: '', area: '', status: '', q: '' };

async function viewContent() {
  const [items, ar] = await Promise.all([get('/items'), areas()]);
  main.innerHTML = `
    <h1>Content</h1><p class="sub">Articles, projects and their updates, pages, apps, playgrounds, and tools.</p>
    <div class="bar">
      <input type="search" placeholder="Filter by title…" data-f="q" value="${esc(contentFilter.q)}">
      <select data-f="kind"><option value="">All kinds</option>${KINDS.map(k => `<option ${contentFilter.kind === k ? 'selected' : ''}>${k}</option>`).join('')}</select>
      <select data-f="area"><option value="">All areas</option>${ar.map(a => `<option value="${esc(a.path)}" ${contentFilter.area === a.path ? 'selected' : ''}>${esc(a.path)}</option>`).join('')}</select>
      <select data-f="status"><option value="">Any status</option>${['draft', 'published', 'unlisted'].map(s => `<option ${contentFilter.status === s ? 'selected' : ''}>${s}</option>`).join('')}</select>
      <span class="grow"></span>
      <button class="primary" data-new>New item</button>
    </div>
    <table><thead><tr><th>Title</th><th>Kind</th><th>Date</th><th>Areas</th><th>Status</th></tr></thead><tbody data-rows></tbody></table>`;
  const render = () => {
    const f = contentFilter;
    const rows = items.filter(it =>
      (!f.kind || it.kind === f.kind) &&
      (!f.area || (it.areas || []).some(a => a === f.area || f.area.endsWith('/' + a) || a.startsWith(f.area + '/') || a === f.area.split('/').pop())) &&
      (!f.status || (f.status === 'draft' ? it.draft : f.status === 'unlisted' ? it.unlisted : !it.draft && !it.unlisted)) &&
      (!f.q || it.title.toLowerCase().includes(f.q.toLowerCase())));
    $('[data-rows]').innerHTML = rows.map(it => `
      <tr class="clickable" data-id="${esc(it.id)}">
        <td><strong>${esc(it.title)}</strong><br><span class="muted mono">${esc(it.url)}</span></td>
        <td>${esc(it.kind)}</td><td>${esc(it.date || '')}</td>
        <td>${(it.areas || []).map(a => `<span class="pill">${esc(a)}</span>`).join(' ')}</td>
        <td>${statusPill(it)}</td></tr>`).join('') || '<tr><td colspan="5" class="empty">No matching content.</td></tr>';
    $$('[data-rows] tr[data-id]').forEach(tr => tr.onclick = () => (location.hash = '#edit/' + tr.dataset.id));
  };
  $$('[data-f]').forEach(el => el.oninput = () => { contentFilter[el.dataset.f] = el.value; render(); });
  $('[data-new]').onclick = () => newItem();
  render();
}

async function newItem(preset = {}) {
  const ar = await areas();
  const projects = (await get('/items?kind=project')).map(p => [p.slug, p.title]);
  const v = await form('New item', [
    { name: 'kind', label: 'Kind', type: 'select', options: KINDS, value: preset.kind || 'article' },
    { name: 'title', label: 'Title', required: true, value: preset.title || '' },
    { name: 'summary', label: 'Summary (one line for cards and feeds)', value: preset.summary || '' },
    { name: 'areas', label: 'Areas (Cmd-click for several)', type: 'select', multiple: true, options: ar.map(a => [a.path, a.path + ' — ' + a.title]), value: preset.areas || [] },
    { name: 'tags', label: 'Tags (comma separated)' },
    { name: 'project', label: 'Project (for updates)', type: 'select', options: [['', '—'], ...projects] },
    { name: 'url', label: 'App URL (apps only)', placeholder: 'https://…' },
    { name: 'repo', label: 'Repository URL (optional)' },
    { name: 'bundle', label: 'Bundle directory (keep images beside the text)', type: 'checkbox' },
  ], 'Create draft');
  if (!v) return null;
  return guard(async () => {
    const r = await api('POST', '/items', {
      kind: v.kind, title: v.title, summary: v.summary, areas: v.areas, tags: csv(v.tags),
      project: v.project || null, url: v.url || null, repo: v.repo || null, bundle: v.bundle,
    });
    toast('Created ' + r.created);
    areasCache = null;
    location.hash = '#edit/' + r.item.id;
    return r;
  });
}

async function viewEditor(id) {
  const it = await get('/items/' + enc(id));
  main.innerHTML = `
    <h1>${esc(it.title)} ${statusPill(it)}</h1>
    <p class="sub mono">${esc(it.path)}</p>
    <div class="bar">
      <a class="btn" href="#content">← Content</a>
      <button class="primary" data-save>Save</button>
      <button data-release>${it.draft ? 'Release (publish at next deploy)' : 'Back to draft'}</button>
      <button data-refresh>Rebuild preview</button>
      <a class="btn" href="${esc(it.url)}" target="_blank" data-open>Open preview ↗</a>
      ${String(it.id).startsWith('ext:') ? '' : '<button data-enhance title="Rework this post with Claude: answer a few questions, Claude reads the code repository, and proposes a revision you review">✦ Enhance with Claude</button>'}
      <span class="grow"></span><span class="muted" data-dirty></span>
    </div>
    <div class="editor"><textarea spellcheck="false" data-src></textarea><iframe data-frame title="Preview"></iframe></div>`;
  const ta = $('[data-src]');
  ta.value = it.source_text;
  const frame = $('[data-frame]');
  const showPreview = () => { frame.src = it.url + '?t=' + Date.now(); };
  ta.oninput = () => ($('[data-dirty]').textContent = 'Unsaved changes');
  ta.onkeydown = e => {
    if (e.key === 'Tab') { e.preventDefault(); ta.setRangeText('  ', ta.selectionStart, ta.selectionEnd, 'end'); }
    if ((e.metaKey || e.ctrlKey) && e.key === 's') { e.preventDefault(); save(); }
  };
  const rebuild = async () => { await api('POST', '/build', { drafts: true }); showPreview(); };
  const save = () => guard(async () => {
    const r = await api('PUT', '/items/' + enc(id) + '/source', { text: ta.value });
    $('[data-dirty]').textContent = 'Saved';
    toast('Saved');
    if (r.item.id !== id) { location.hash = '#edit/' + r.item.id; return; }
    await rebuild();
  });
  $('[data-save]').onclick = save;
  $('[data-refresh]').onclick = () => guard(rebuild);
  const enh = $('[data-enhance]');
  if (enh) enh.onclick = () => guard(async () => {
    if ($('[data-dirty]').textContent === 'Unsaved changes' && !(await ask('Unsaved changes', 'You have unsaved changes. Enhance the saved version anyway?', 'Enhance the saved version'))) return;
    const st = await api('POST', '/items/' + enc(id) + '/enhance');
    toast(st.created ? 'Enhancement started' : 'Reopened the enhancement');
    location.hash = '#story/' + st.slug;
  });
  $('[data-release]').onclick = () => guard(async () => {
    await api('POST', '/items/' + enc(id) + '/status', { status: it.draft ? 'published' : 'draft' });
    toast(it.draft ? 'Released: it will go live at the next publish' : 'Back to draft');
    viewEditor(id);
  });
  showPreview();
}

// ---------------------------------------------------------------- Ideas

const STATUSES = ['idea', 'planned', 'drafting', 'review', 'published', 'dropped'];

async function viewIdeas() {
  const ideas = await get('/ideas');
  main.innerHTML = `
    <h1>Ideas</h1><p class="sub">Things worth writing up or building. Drag cards between columns.</p>
    <div class="bar"><button class="primary" data-add>Add idea</button></div>
    <div class="kanban">${STATUSES.map(s => `
      <div class="col" data-col="${s}"><h3>${s}<span>${ideas.filter(i => i.status === s).length}</span></h3>
        ${ideas.filter(i => i.status === s).map(i => `
          <div class="idea" draggable="true" data-idea="${i.id}">
            <strong>#${i.id} ${esc(i.title)}</strong>
            ${i.notes ? `<p>${esc(i.notes)}</p>` : ''}
            <p>${(i.areas || []).map(a => `<span class="pill">${esc(a)}</span>`).join(' ')} ${i.kind ? `<span class="pill">${esc(i.kind)}</span>` : ''}</p>
            ${i.content ? `<p><a href="#edit/${esc(i.content)}">→ ${esc(i.content)}</a></p>` : ''}
            <div class="acts"><button data-edit="${i.id}">Edit</button>${i.content ? '' : `<button data-draft="${i.id}">Draft this</button>`}</div>
          </div>`).join('')}
      </div>`).join('')}</div>`;
  $('[data-add]').onclick = async () => {
    const ar = await areas();
    const v = await form('New idea', [
      { name: 'title', label: 'Title', required: true },
      { name: 'notes', label: 'Notes', type: 'textarea' },
      { name: 'kind', label: 'Becomes a…', type: 'select', options: ['', ...KINDS] },
      { name: 'areas', label: 'Areas', type: 'select', multiple: true, options: ar.map(a => a.path) },
      { name: 'source', label: 'Source (repo path, URL, album…)' },
    ], 'Add');
    if (v) guard(async () => { await api('POST', '/ideas', v); viewIdeas(); });
  };
  $$('[data-edit]').forEach(b => b.onclick = async () => {
    const i = ideas.find(x => x.id === +b.dataset.edit);
    const v = await form(`Idea #${i.id}`, [
      { name: 'title', label: 'Title', value: i.title },
      { name: 'notes', label: 'Notes', type: 'textarea', value: i.notes },
      { name: 'status', label: 'Status', type: 'select', options: STATUSES, value: i.status },
      { name: 'content', label: 'Linked content id', value: i.content },
    ]);
    if (v) guard(async () => { await api('PATCH', '/ideas/' + i.id, v); viewIdeas(); });
  });
  $$('[data-draft]').forEach(b => b.onclick = async () => {
    const i = ideas.find(x => x.id === +b.dataset.draft);
    const ar = await areas();
    const r = await newItem({ kind: KINDS.includes(i.kind) ? i.kind : 'article', title: i.title, summary: '',
      areas: (i.areas || []).map(a => (ar.find(x => x.path === a || x.id === a) || {}).path).filter(Boolean) });
    if (r) await guard(() => api('PATCH', '/ideas/' + i.id, { content: r.item.id, status: 'drafting' }));
  });
  let dragging = null;
  $$('.idea').forEach(c => { c.ondragstart = () => (dragging = c.dataset.idea); });
  $$('.col').forEach(col => {
    col.ondragover = e => { e.preventDefault(); col.classList.add('drop'); };
    col.ondragleave = () => col.classList.remove('drop');
    col.ondrop = e => {
      e.preventDefault(); col.classList.remove('drop');
      if (dragging) guard(async () => { await api('PATCH', '/ideas/' + dragging, { status: col.dataset.col }); viewIdeas(); });
    };
  });
}

// ---------------------------------------------------------------- Registry

async function viewRegistry() {
  const entries = await get('/registry');
  main.innerHTML = `
    <h1>Registry</h1><p class="sub">Where every piece of work came from. Entries of kind <em>publication</em> make the publications page.</p>
    <div class="bar"><button class="primary" data-add>Add entry</button></div>
    <table><thead><tr><th>Id</th><th>Kind</th><th>Title</th><th>Visibility</th><th>Origin</th></tr></thead>
    <tbody>${entries.map(e => `<tr class="clickable" data-id="${esc(e.id)}">
      <td class="mono">${esc(e.id)}</td><td>${esc(e.kind)}</td>
      <td><strong>${esc(e.title)}</strong>${e.url ? `<br><a href="${esc(e.url)}" target="_blank">${esc(e.url)}</a>` : ''}</td>
      <td><span class="pill ${e.visibility === 'unlisted' ? 'unlisted' : 'published'}">${esc(e.visibility)}</span></td>
      <td class="muted">${esc(e.origin || '')}</td></tr>`).join('') || '<tr><td colspan="5" class="empty">Empty.</td></tr>'}</tbody></table>`;
  const edit = async e => {
    const isNew = !e;
    e = e || { id: '', kind: 'publication', title: '', visibility: 'unlisted', links: [], meta: [], areas: [] };
    const v = await form(isNew ? 'New registry entry' : 'Edit ' + e.id, [
      { name: 'id', label: 'Id', value: e.id, required: true },
      { name: 'kind', label: 'Kind', type: 'select', options: ['publication', 'repo', 'wordpress', 'smugmug', 'app', 'external', 'document'], value: e.kind },
      { name: 'group', label: 'Group (publications page heading)', value: e.group || '' },
      { name: 'title', label: 'Title', value: e.title, required: true },
      { name: 'subtitle', label: 'Subtitle', value: e.subtitle || '' },
      { name: 'url', label: 'URL', value: e.url || '' },
      { name: 'origin', label: 'Origin (repo, path, post id…)', value: e.origin || '' },
      { name: 'description', label: 'Description', type: 'textarea', value: e.description || '' },
      { name: 'meta', label: 'Meta chips (comma separated)', value: (e.meta || []).join(', ') },
      { name: 'areas', label: 'Areas (comma separated)', value: (e.areas || []).join(', ') },
      { name: 'visibility', label: 'Visibility', type: 'select', options: ['public', 'unlisted'], value: e.visibility || 'public' },
      { name: 'links', label: 'Links — one per line: Label | URL | note | primary', type: 'textarea', mono: true, rows: 5,
        value: (e.links || []).map(l => [l.label, l.url, l.note || '', l.primary ? 'primary' : ''].join(' | ').replace(/( \| )+$/, '')).join('\n') },
    ]);
    if (!v) return;
    const entry = {
      ...e, ...v, meta: csv(v.meta), areas: csv(v.areas),
      links: v.links.split('\n').map(l => l.split('|').map(x => x.trim())).filter(p => p[0] && p[1])
        .map(([label, url, note, primary]) => ({ label, url, note: note || '', primary: primary === 'primary' })),
    };
    if (!entry.added) entry.added = new Date().toISOString().slice(0, 10);
    guard(async () => {
      if (!isNew && entry.id !== e.id) await api('DELETE', '/registry/' + encodeURIComponent(e.id));
      await api('POST', '/registry', entry); toast('Saved'); viewRegistry();
    });
  };
  $('[data-add]').onclick = () => edit(null);
  $$('tr[data-id]').forEach(tr => tr.onclick = e => { if (e.target.tagName !== 'A') edit(entries.find(x => x.id === tr.dataset.id)); });
}

// ---------------------------------------------------------------- Media

async function viewMedia(filter) {
  filter = filter || sessionStorage.getItem('gw-media-filter') || 'all';
  const [lib, usage, album] = await Promise.all([get('/media'), get('/media/usage').catch(() => ({})), get('/smugmug/album').catch(() => ({}))]);
  const keys = Object.keys(lib).sort();
  const pick = { all: () => true, model: m => m.kind === 'model', video: m => m.kind === 'video', smugmug: m => m.provider === 'smugmug', unused: (m, k) => !(usage[k] || []).length };
  const shown = keys.filter(k => pick[filter](lib[k], k));
  const count = f => keys.filter(k => pick[f](lib[k], k)).length;
  const chip = (f, label) => `<button class="chip${f === filter ? ' on' : ''}" data-filter="${f}">${label} <span class="muted">${count(f)}</span></button>`;
  main.innerHTML = `
    <h1>Media</h1><p class="sub">Images, video and 3-D models that content uses as <code>media:&lt;key&gt;</code> — files in the site, any URL, or SmugMug. Add a 3-D model from <a href="#models">Models</a> (Add to site).</p>
    <div class="bar"><button class="primary" data-add>Add by URL</button>
      ${album.album_key ? `<span class="muted">Site media album on SmugMug (unlisted): ${album.web ? `<a href="${esc(album.web)}" target="_blank" rel="noopener">${esc(album.album_key)} ↗</a>` : esc(album.album_key)}</span>` : ''}
      <span class="muted">Upload: <code>gw smugmug upload &lt;file&gt;</code> or the MCP tools.</span></div>
    <div class="bar">${chip('all', 'All')}${chip('video', 'Video')}${chip('model', '3-D models')}${chip('smugmug', 'SmugMug')}${chip('unused', 'Not used yet')}</div>
    <div class="media-grid">${shown.map(k => {
      const m = lib[k], ids = m.ids || {};
      const processing = m.provider === 'smugmug' && m.kind === 'video' && ids.note;
      const src = m.thumb || m.url;
      const thumb = m.kind === 'video' && !processing ? `<video src="${esc(m.url)}" poster="${esc(m.thumb || '')}" muted controls preload="none"></video>` : `<img src="${esc(src)}" alt="${esc(m.alt)}" loading="lazy">`;
      const snippet = m.kind === 'video' ? `video::media:${k}[]` : m.kind === 'model' ? `model::media:${k}[${m.caption || ''}${m.download ? ', download=true' : ''}]` : `image::media:${k}[${m.alt || ''}]`;
      const modelFacts = m.kind === 'model' ? `<br><span class="muted small">${[m.triangles && `${Number(m.triangles).toLocaleString()} triangles`, m.bytes && `GLB ${fmtBytes(m.bytes)}`, m.ios && `USDZ ${fmtBytes(m.ios_bytes || 0)}`, m.download && `download ${fmtBytes(m.download_bytes || 0)}`, (m.ids || {}).model].filter(Boolean).map(esc).join(' · ')}</span>` : '';
      const used = usage[k] || [];
      const page = ids.web_uri || (m.provider === 'smugmug' ? m.url : '');
      return `<div class="tile"><div class="thumb">${thumb}</div><div class="meta"><strong>${esc(k)}</strong>
        <span class="pill">${esc(m.provider)}</span> <span class="pill">${esc(m.kind)}</span>${processing ? ' <span class="pill warn">processing on SmugMug</span>' : ''}<br>
        <code>${esc(snippet)}</code>${modelFacts}<br><button data-copy="${esc(snippet)}">Copy snippet</button>
        ${page ? ` <a href="${esc(page)}" target="_blank" rel="noopener">SmugMug page ↗</a>` : ''}
        ${m.provider === 'smugmug' ? ` <button data-refresh="${esc(k)}" title="Look it up on SmugMug again (e.g. once a video has finished processing)">Refresh</button>` : ''}
        <div class="muted">${used.length ? 'Used in ' + used.map(u => `<a href="#edit/${encodeURIComponent(u.id)}">${esc(u.title)}</a>${u.draft ? ' (draft)' : ''}`).join(', ') : 'Not used in any page yet'}</div></div></div>`;
    }).join('') || '<p class="empty">No media here.</p>'}</div>`;
  $$('[data-filter]').forEach(b => b.onclick = () => { sessionStorage.setItem('gw-media-filter', b.dataset.filter); viewMedia(b.dataset.filter); });
  $$('[data-copy]').forEach(b => b.onclick = () => navigator.clipboard.writeText(b.dataset.copy).then(() => toast('Copied ' + b.dataset.copy), () => toast(b.dataset.copy)));
  $$('[data-refresh]').forEach(b => b.onclick = () => guard(async () => {
    b.disabled = true;
    const r = await api('POST', `/media/${encodeURIComponent(b.dataset.refresh)}/refresh`);
    toast(r.playable === false ? 'Still processing on SmugMug — try again in a minute' : 'Refreshed ' + b.dataset.refresh);
    viewMedia(filter);
  }));
  $('[data-add]').onclick = async () => {
    const v = await form('Add media', [
      { name: 'key', label: 'Key (letters, digits, - _ .)', required: true },
      { name: 'url', label: 'URL or site path', required: true, placeholder: 'https://… or /images/x.jpg' },
      { name: 'kind', label: 'Kind', type: 'select', options: ['image', 'video'] },
      { name: 'alt', label: 'Alt text' },
      { name: 'caption', label: 'Caption' },
    ], 'Add');
    if (v) guard(async () => {
      await api('POST', '/media', { ...v, provider: v.url.startsWith('/') ? 'local' : 'external' });
      viewMedia(filter);
    });
  };
}

// ---------------------------------------------------------------- Models (Meshy.ai)

const meshyFile = p => '/api/meshy/file?path=' + encodeURIComponent(p);
let meshyPoll = null;   // the live-log timer while a sync runs
function stopMeshyPoll() { if (meshyPoll) { clearTimeout(meshyPoll); meshyPoll = null; } }

/** "3 h ago", "in 20 h" (and the local date/time as a tooltip). */
function relTime(iso) {
  if (!iso) return '—';
  const t = new Date(iso), d = (Date.now() - t) / 60000;
  const a = Math.abs(d);
  const s = a < 1 ? 'now' : a < 90 ? `${Math.round(a)} min` : a < 48 * 60 ? `${Math.round(a / 60)} h` : `${Math.round(a / 1440)} days`;
  return s === 'now' ? 'just now' : d >= 0 ? `${s} ago` : `in ${s}`;
}
const fmtTris = n => n == null ? '' : n >= 1e6 ? `${(n / 1e6).toFixed(1)} M` : n >= 1e3 ? `${Math.round(n / 1e3)} k` : String(n);
const fmtDims = d => Array.isArray(d) ? d.map(x => Math.round(x)).join(' × ') + ' mm' : '';
const fmtBytes = n => n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : n >= 1e6 ? `${Math.round(n / 1e6)} MB` : n >= 1e3 ? `${Math.round(n / 1e3)} kB` : `${n} B`;

/** The model READMEs are simple Markdown (headings, paragraphs, lists, tables, images, links,
 *  `code`, **bold**). Images and links inside the model folder are resolved against it. */
function miniMarkdown(md, dir) {
  const inline = s => esc(s)
    .replace(/!\[([^\]]*)\]\(([^)\s]+)\)/g, (_, alt, src) => /^https?:/.test(src) ? `<img src="${src}" alt="${alt}">`
      : `<img src="${meshyFile(dir + '/' + src.replace(/^\.\//, ''))}" alt="${alt}" loading="lazy">`)
    .replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, text, href) => /^https?:/.test(href) ? `<a href="${href}" target="_blank" rel="noopener">${text}</a>` : text)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[\s(])_([^_]+)_(?=[\s).,;:]|$)/g, '$1<em>$2</em>');
  const out = [], lines = md.split('\n');
  for (let i = 0; i < lines.length;) {
    const l = lines[i];
    if (!l.trim()) { i++; continue; }
    let m;
    if ((m = l.match(/^(#{1,4})\s+(.*)/))) { const n = Math.min(m[1].length + 1, 4); out.push(`<h${n}>${inline(m[2])}</h${n}>`); i++; continue; }
    if (l.startsWith('|')) {
      const rows = [];
      while (i < lines.length && lines[i].startsWith('|')) rows.push(lines[i++]);
      const cells = r => r.replace(/^\||\|$/g, '').split('|').map(c => c.trim());
      const body = rows.filter(r => !/^\|[\s:|-]+\|$/.test(r));
      const head = cells(body.shift() || '');
      const hasHead = head.some(Boolean);
      out.push(`<div class="md-table"><table>${hasHead ? `<tr>${head.map(c => `<th>${inline(c)}</th>`).join('')}</tr>` : ''}${body.map(r => `<tr>${cells(r).map(c => `<td>${inline(c)}</td>`).join('')}</tr>`).join('')}</table></div>`);
      continue;
    }
    if (/^\s*[-*]\s/.test(l)) {
      const items = [];
      while (i < lines.length && /^\s*[-*]\s/.test(lines[i])) items.push(lines[i++].replace(/^\s*[-*]\s/, ''));
      out.push(`<ul>${items.map(x => `<li>${inline(x)}</li>`).join('')}</ul>`);
      continue;
    }
    if (l.startsWith('```')) {
      const code = []; i++;
      while (i < lines.length && !lines[i].startsWith('```')) code.push(lines[i++]);
      i++; out.push(`<pre class="mono">${esc(code.join('\n'))}</pre>`); continue;
    }
    if (l.startsWith('>')) {
      const q = [];
      while (i < lines.length && lines[i].startsWith('>')) q.push(lines[i++].replace(/^>\s?/, ''));
      out.push(`<blockquote>${inline(q.join(' '))}</blockquote>`); continue;
    }
    const p = [];
    while (i < lines.length && lines[i].trim() && !/^(#|\||\s*[-*]\s|```|>)/.test(lines[i])) p.push(lines[i++]);
    out.push(`<p>${inline(p.join(' '))}</p>`);
  }
  return out.join('\n');
}

/** The website gallery's selection (GET /api/site-models), as last loaded. */
let siteModels = null;

/** Switch models on/off the website (writes the site's models.yaml and copies the pictures). */
async function setOnSite(changes, el, after) {
  if (el) el.disabled = true;
  try {
    const r = await api('PUT', '/site-models', { changes });
    siteModels = r;
    const s = r.sync || {};
    toast(`${r.count} model${r.count === 1 ? '' : 's'} on the website${(s.problems || []).length ? ' · ' + s.problems.join(' · ') : ''}`, (s.problems || []).length > 0);
    if (after) after(r);
    return r;
  } catch (e) {
    toast(e.message, true);
    if (el && el.type === 'checkbox') el.checked = !el.checked;
  } finally { if (el) el.disabled = false; }
}

async function viewModels(rest = []) {
  stopMeshyPoll();
  const filter = sessionStorage.getItem('gw-models-project') || 'all';
  const webFilter = sessionStorage.getItem('gw-models-web') || 'all';   // all | on | off
  const [st, lib, web] = await Promise.all([
    get('/meshy/status'),
    get('/meshy/models').catch(e => ({ error: e.message, models: [], projects: [] })),
    get('/site-models').catch(e => ({ error: e.message, models: [], web_models: {} })),
  ]);
  markModelsNav(st);
  siteModels = web;
  const onSite = new Map((web.models || []).map(x => [x.key, x]));
  const keyOf = m => `${m.project}/${m.slug}`;
  const inProject = lib.models.filter(m => filter === 'all' || m.project === filter);
  const shown = inProject.filter(m => webFilter === 'all' || (webFilter === 'on') === onSite.has(keyOf(m)));
  const projOn = inProject.filter(m => onSite.has(keyOf(m))).length;
  const last = st.last_sync, run = st.running;
  const lastText = last ? `${relTime(last.finished || last.started)}` : 'never';
  const lastNote = last ? (last.ok ? `ok · ${(last.summary || {}).new || 0} new · ${esc(last.trigger)}` : `<span class="bad">${esc(last.error || 'failed')}</span>`) : 'no sync recorded yet';
  const next = !st.enabled ? 'off' : !st.repo_ok ? '—' : (new Date(st.next_due) <= Date.now() ? 'due now' : relTime(st.next_due));
  const chip = (id, label, n) => `<button class="chip${id === filter ? ' on' : ''}" data-project="${esc(id)}">${esc(label)} <span class="muted">${n}</span></button>`;
  const needs = st.expired_needing_remesh || 0, cost = st.recover_cost || 0;
  main.innerHTML = `
    <h1>Models</h1>
    <p class="sub">3D models made with Meshy.ai, kept in the git repository <span class="mono">${esc(st.repo || '(none)')}</span>.
      New models are downloaded free while Meshy keeps them (about 3 days); the app checks every ${esc(st.interval_hours)} h.</p>
    ${st.repo_ok ? '' : `<p class="list problems">${esc(st.repo_error || 'No models repository.')} — set one with <code>gw meshy config --repo &lt;path&gt;</code>.</p>`}
    <div class="cards meshy-stats">
      <div class="stat"><b title="${esc(last ? new Date(last.finished || last.started).toLocaleString() : '')}">${lastText}</b>Last sync — ${lastNote}</div>
      <div class="stat"><b title="${esc(st.next_due ? new Date(st.next_due).toLocaleString() : '')}">${next}</b>Next scheduled sync${st.enabled ? ` (every ${esc(st.interval_hours)} h)` : ' — <code>gw meshy config --enable</code>'}</div>
      <div class="stat"><b>${st.balance ?? '?'}</b>${st.balance == null ? `Credit balance unknown <span class="muted small">${esc(st.balance_error || '')}</span>` : 'Meshy credits'}</div>
      <div class="stat"><b>${st.models}</b>models${st.new_available ? ` · <strong>${st.new_available}</strong> new on Meshy` : ''}</div>
    </div>
    <div class="bar">
      <button class="primary" data-sync ${run || !st.repo_ok ? 'disabled' : ''}>${run ? 'Syncing…' : 'Sync now'}</button>
      <button data-dry ${run || !st.repo_ok ? 'disabled' : ''} title="List what is waiting on Meshy; change nothing">Check Meshy</button>
      <span class="muted small">Free: only downloads models still inside Meshy's download window.</span>
      <span class="grow"></span>
      <button data-log>Last log</button>
    </div>
    ${st.unseen_new ? `<div class="meshy-banner ok-banner"><div><strong>${st.unseen_new} new model${st.unseen_new === 1 ? '' : 's'} arrived</strong>
      <div class="muted small">${st.unseen_models.map(m => `<a href="#models/${encodeURIComponent(m.project)}/${encodeURIComponent(m.slug)}">${esc(m.name)}</a>`).join(', ')}</div></div>
      <button data-seen>Dismiss</button></div>` : ''}
    ${st.expired_unrecovered ? `<div class="meshy-banner warn-banner"><div><strong>Expired — ${st.expired_unrecovered} model${st.expired_unrecovered === 1 ? '' : 's'} not recovered</strong>
      <div class="muted small">Meshy's free download links ran out before these were synced. ${(st.expired_models || []).map(m => esc(m.name)).join(', ')}</div></div>
      ${needs ? `<button data-recover>Recover ${needs} model${needs === 1 ? '' : 's'} (${needs}×${st.remesh_cost} = ${cost} credits)</button>` : `<button data-recover-free title="These already have a paid conversion; collecting them is free">Collect them (free)</button>`}</div>` : ''}
    <div data-progress ${run ? '' : 'hidden'}><h2>Sync in progress</h2><pre class="list claude-log" data-logtext></pre></div>
    ${lib.error ? `<p class="list problems">${esc(lib.error)}</p>` : ''}
    <div class="bar">${chip('all', 'All', lib.models.length)}${lib.projects.map(p => chip(p.id, p.id, p.count)).join('')}</div>
    ${filter !== 'all' ? `<p class="muted small">${esc((lib.projects.find(p => p.id === filter) || {}).description || '')}</p>` : ''}
    <div class="bar site-bar">
      <span class="site-count" data-site-count><strong>${onSite.size}</strong> on the website${web.bytes ? ` <span class="muted small">(${fmtBytes(web.bytes)} of pictures in the site)</span>` : ''}</span>
      <span class="muted small">Website:</span>
      ${[['all', 'All'], ['on', 'On'], ['off', 'Off']].map(([id, label]) => `<button class="chip${id === webFilter ? ' on' : ''}" data-webfilter="${id}">${label}</button>`).join('')}
      <span class="grow"></span>
      ${filter !== 'all' ? `<button data-site-project="on" ${projOn === inProject.length ? 'disabled' : ''} title="Put every ${esc(filter)} model on the website">Show all ${inProject.length} on website</button>
        <button data-site-project="off" ${projOn ? '' : 'disabled'}>Hide all</button>` : '<span class="muted small">Pick a project to show or hide all of it.</span>'}
      <a class="btn" href="/preview/models/" target="_blank" title="The gallery in the preview build (Build &amp; Publish → Build preview to refresh)">Gallery preview ↗</a>
    </div>
    ${web.error ? `<p class="list problems">${esc(web.error)}</p>` : ''}
    <div class="model-grid">${shown.map(m => {
      const k = keyOf(m), w = onSite.get(k);
      return `
      <div class="model-card${w ? ' on-site' : ''}">
        <a class="model-link" href="#models/${encodeURIComponent(m.project)}/${encodeURIComponent(m.slug)}">
        <div class="thumb">${m.thumb ? `<img src="${meshyFile(m.thumb)}" alt="" loading="lazy">` : '<span>no image</span>'}</div>
        <div class="body"><strong>${esc(m.name)}</strong>
          <p>${esc(m.summary || m.first_line || '')}</p>
          <div class="facts"><span>${esc(fmtDims(m.dimensions_mm))}</span>${m.triangles ? `<span>${fmtTris(m.triangles)} tris</span>` : ''}
            ${m.multi_part ? `<span class="pill warn">${esc(m.parts)} parts</span>` : ''}${filter === 'all' ? `<span class="pill">${esc(m.project)}</span>` : ''}
            ${w && w.viewer ? '<span class="pill published" title="Interactive 3-D viewer on the website">3-D</span>' : ''}</div></div>
        </a>
        <label class="site-toggle" title="Show this model in the website's /models/ gallery"><input type="checkbox" data-site-toggle="${esc(k)}" ${w ? 'checked' : ''}> Show on website</label>
      </div>`;
    }).join('') || '<p class="empty">No models here.</p>'}</div>
    <div data-detail></div>`;
  $$('[data-project]').forEach(b => b.onclick = () => { sessionStorage.setItem('gw-models-project', b.dataset.project); viewModels(rest); });
  $$('[data-webfilter]').forEach(b => b.onclick = () => { sessionStorage.setItem('gw-models-web', b.dataset.webfilter); viewModels(rest); });
  $$('[data-site-toggle]').forEach(c => c.onchange = () => setOnSite([{ key: c.dataset.siteToggle, on: c.checked }], c, () => viewModels(rest)));
  $$('[data-site-project]').forEach(b => b.onclick = () => setOnSite([{ project: filter, on: b.dataset.siteProject === 'on' }], b, () => viewModels(rest)));
  const start = async (body, what) => guard(async () => {
    await api('POST', '/meshy/sync', body);
    toast(what);
    viewModels(rest);
  });
  $('[data-sync]').onclick = () => start({}, 'Sync started');
  $('[data-dry]').onclick = () => start({ dry_run: true }, 'Checking Meshy…');
  $('[data-log]').onclick = () => showMeshyLog();
  const seen = $('[data-seen]');
  if (seen) seen.onclick = () => guard(async () => { await api('POST', '/meshy/seen'); viewModels(rest); });
  const rec = $('[data-recover]');
  if (rec) rec.onclick = async () => {
    const bal = st.balance;
    const ok = await form(`Spend ${cost} Meshy credits?`, [
      { type: 'note', text: `Recovering ${needs} expired model${needs === 1 ? '' : 's'} costs ${st.remesh_cost} credits each: ${cost} credits in all. ` +
        `${bal != null ? `Your balance is ${bal} credits; afterwards about ${bal - cost}. ` : ''}` +
        `The sync never spends more than ${cost} credits in this run and stops before the balance would fall below 250. Credits spent are not refunded.` },
    ], `Spend up to ${cost} credits`);
    if (ok) start({ recover: true, max_credits: cost }, `Recovering — up to ${cost} credits`);
  };
  const free = $('[data-recover-free]');
  if (free) free.onclick = () => start({}, 'Sync started');
  if (run) followMeshyLog(rest);
  if (rest.length >= 2) showModel(decodeURIComponent(rest[0]), decodeURIComponent(rest[1]));
}

/** While a sync runs, append its log to the page; reload the page when it ends. */
async function followMeshyLog(rest, offset = 0, runId = null) {
  const box = $('[data-logtext]');
  if (!box || !location.hash.startsWith('#models')) return stopMeshyPoll();
  try {
    const l = await get(`/meshy/log?offset=${offset}${runId ? '&run=' + encodeURIComponent(runId) : ''}`);
    if (l.text) { box.textContent += l.text; box.scrollTop = box.scrollHeight; }
    if (!l.running) {
      const r = l.record;
      const n = r && r.summary ? r.summary.new : 0;
      toast(r ? (r.ok ? (r.dry_run ? `Checked: ${r.summary ? r.summary.available_free : 0} new on Meshy, ${r.summary ? r.summary.expired : 0} expired` : `Sync done: ${n} new model${n === 1 ? '' : 's'}`) : 'Sync failed: ' + (r.error || '')) : 'Sync finished', r && !r.ok);
      return viewModels(rest);
    }
    meshyPoll = setTimeout(() => followMeshyLog(rest, l.offset, l.run), 1500);
  } catch (e) { meshyPoll = setTimeout(() => followMeshyLog(rest, offset, runId), 4000); }
}

async function showMeshyLog() {
  const l = await get('/meshy/log').catch(e => ({ text: e.message }));
  const box = $('[data-detail]');
  box.innerHTML = `<div class="model-detail" role="dialog" aria-label="Sync log"><div class="md-head"><h2>Sync log ${l.run ? `<span class="muted small mono">${esc(l.run)}</span>` : ''}</h2><button data-close>Close</button></div>
    <pre class="list claude-log meshy-log">${esc(l.text || 'No sync has run yet.')}</pre></div>`;
  $('[data-close]', box).onclick = () => (box.innerHTML = '');
}

async function showModel(project, slug) {
  const box = $('[data-detail]');
  if (!box) return;
  const m = await get(`/meshy/models/${encodeURIComponent(project)}/${encodeURIComponent(slug)}`).catch(e => ({ error: e.message }));
  if (m.error) return toast(m.error, true);
  const imgs = m.images.filter(i => i.name !== 'thumb');
  const main0 = m.hero || (imgs[0] || {}).path;
  const local = (m.files || []).filter(f => !f.in_git);
  box.innerHTML = `<div class="model-backdrop" data-close></div><aside class="model-detail" role="dialog" aria-label="${esc(m.name)}">
    <div class="md-head"><div><h2>${esc(m.name)}</h2><div class="muted small">${esc(m.project)} · <span class="mono">${esc(m.slug)}</span>${m.created ? ` · made ${esc(String(m.created).slice(0, 10))}` : ''}</div></div>
      <button data-close aria-label="Close">Close</button></div>
    <div class="md-hero">${main0 ? `<img data-big src="${meshyFile(main0)}" alt="${esc(m.name)}">` : ''}</div>
    <div class="md-views">${imgs.map(i => `<button data-view="${esc(i.path)}" class="${i.path === main0 ? 'on' : ''}" title="${esc(i.name)}"><img src="${meshyFile(i.path)}" alt="${esc(i.name)}" loading="lazy"><span>${esc(i.name)}</span></button>`).join('')}</div>
    <div class="facts big"><span>${esc(fmtDims(m.dimensions_mm))}</span>${m.triangles ? `<span>${Number(m.triangles).toLocaleString()} triangles</span>` : ''}
      ${m.multi_part ? `<span class="pill warn">${esc(m.parts)} parts on ${esc(m.plates || 1)} plate${m.plates > 1 ? 's' : ''}</span>` : '<span class="pill">single part</span>'}
      ${m.textured ? '<span class="pill">textured</span>' : ''}${m.watertight === false ? '<span class="pill">not watertight</span>' : ''}</div>
    <div class="bar site-detail" data-site-detail></div>
    <div class="bar"><button data-reveal>Show in Finder</button>
      <button class="primary" data-add-site title="Make a web model (GLB + USDZ for AR + poster) in the site and register it as media:&lt;key&gt; for model::media:&lt;key&gt;[] — needs Blender">Add to site</button>
      <span class="muted small">Print: ${(m.print_files || []).map(f => `<code>${esc(f)}</code>`).join(', ') || '—'}${m.full_res_stl ? ` · full resolution (local only): <code>${esc(m.full_res_stl)}</code>` : ''}</span></div>
    ${local.length ? `<p class="muted small">Local only (not in git): ${local.map(f => `<code>${esc(f.name)}</code> ${fmtBytes(f.bytes)}`).join(', ')}</p>` : ''}
    <div class="md-readme">${miniMarkdown(m.readme || m.description || '', m.dir)}</div>
  </aside>`;
  const close = () => { box.innerHTML = ''; if (location.hash !== '#models') history.replaceState(null, '', '#models'); };
  $$('[data-close]', box).forEach(b => b.onclick = close);
  document.onkeydown = e => { if (e.key === 'Escape') close(); };
  $$('[data-view]', box).forEach(b => b.onclick = () => {
    $('[data-big]', box).src = meshyFile(b.dataset.view);
    $$('[data-view]', box).forEach(x => x.classList.toggle('on', x === b));
  });
  $('[data-reveal]', box).onclick = () => guard(async () => { await api('POST', '/meshy/reveal', { project, slug }); });
  renderSiteDetail(box, project, slug);
  $('[data-add-site]', box).onclick = async () => {
    const hasStl = (m.print_files || []).some(f => f.endsWith('.stl'));
    const v = await form(`Add “${m.name}” to the site`, [
      { type: 'note', text: 'Makes a decimated, textured web model (a few MB), a USDZ for AR on iPhone/iPad and a poster in static/models/<key>/ with Blender (10–60 s), and registers media:<key>.' },
      { name: 'key', label: 'Media key', value: slug, required: true },
      { name: 'download', label: 'Offer readers a download', type: 'select', value: hasStl ? 'stl' : 'none',
        options: [['stl', 'The print STL' + (hasStl ? '' : ' (none in the repository)')], ['3mf', 'The 3MF'], ['none', 'No download']] },
      { name: 'download_polycount', label: 'Shrink the STL to this many triangles (optional; ≈50 bytes each)', placeholder: 'e.g. 400000' },
      { name: 'polycount', label: 'Web model triangles', value: '150000' },
    ], 'Add to site');
    if (!v) return;
    const b = $('[data-add-site]', box);
    b.disabled = true; b.textContent = 'Working…';
    guard(async () => {
      try {
        const r = await api('POST', '/media/model', {
          model: `${project}/${slug}`, key: v.key.trim(), download: v.download,
          polycount: Number(v.polycount) || 150000,
          download_polycount: v.download_polycount.trim() ? Number(v.download_polycount) : null,
        });
        const f = r.files || {};
        const sizes = [['web GLB', f.glb], ['USDZ', f.usdz], ['download', f.download]].filter(([, x]) => x && x.bytes).map(([n, x]) => `${n} ${fmtBytes(x.bytes)}`).join(', ');
        const snippet = `model::media:${r.key}[${r.media.caption || ''}]`;
        navigator.clipboard?.writeText(snippet).catch(() => {});
        toast(`Added media:${r.key} (${sizes}) — copied ${snippet}${(r.warnings || []).length ? ' · ' + r.warnings.join(' · ') : ''}`);
      } finally { b.disabled = false; b.textContent = 'Add to site'; }
    });
  };
}

/** "Show on website" and "Interactive 3-D" in a model's detail panel. Turning 3-D on makes the
 *  web model first (gw media add-model, no download) when the site has none for this model. */
async function renderSiteDetail(box, project, slug) {
  const el = $('[data-site-detail]', box);
  if (!el) return;
  const web = siteModels || await get('/site-models').catch(() => null);
  if (!web) return (el.hidden = true);
  const key = `${project}/${slug}`;
  const w = (web.models || []).find(x => x.key === key);
  const mk = (web.web_models || {})[key];
  el.innerHTML = `
    <label class="site-toggle big"><input type="checkbox" data-d-on ${w ? 'checked' : ''}> Show on website</label>
    <label class="site-toggle big" title="${mk ? `Uses media:${esc(mk)}` : 'Makes a web model with Blender first (10–60 s): media:' + esc(slug)}"><input type="checkbox" data-d-viewer ${w && w.viewer ? 'checked' : ''}> Interactive 3-D</label>
    <span class="muted small" data-d-note>${mk ? `web model: <code>media:${esc(mk)}</code>` : 'no web model yet'}</span>
    ${w ? `<a class="btn" href="/preview/models/${encodeURIComponent(project)}/${encodeURIComponent(slug)}/" target="_blank">Preview page ↗</a>` : ''}`;
  const refresh = () => {
    renderSiteDetail(box, project, slug);
    if (location.hash.startsWith('#models')) refreshModelCard(key);
  };
  $('[data-d-on]', el).onchange = e => setOnSite([{ key, on: e.target.checked }], e.target, refresh);
  $('[data-d-viewer]', el).onchange = async e => {
    const cb = e.target, want = cb.checked;
    if (want && !mk) {
      cb.disabled = true;
      const note = $('[data-d-note]', el);
      const t0 = Date.now();
      const tick = setInterval(() => { note.textContent = `Making the web model with Blender… ${Math.round((Date.now() - t0) / 1000)} s (10–60 s)`; }, 500);
      try {
        const r = await api('POST', '/media/model', { model: key, key: slug, download: 'none' });
        toast(`Made media:${r.key}${(r.warnings || []).length ? ' · ' + r.warnings.join(' · ') : ''}`);
      } catch (err) {
        clearInterval(tick); cb.checked = false; cb.disabled = false;
        note.textContent = 'no web model yet';
        return toast(err.message, true);
      }
      clearInterval(tick);
      cb.disabled = false;
    }
    setOnSite([{ key, viewer: want }], cb, refresh);
  };
}

/** Update one card's toggle after the detail panel changed it. */
function refreshModelCard(key) {
  const c = $(`[data-site-toggle="${CSS.escape(key)}"]`);
  const w = (siteModels && siteModels.models || []).find(x => x.key === key);
  if (c) { c.checked = !!w; c.closest('.model-card').classList.toggle('on-site', !!w); }
  const n = $('[data-site-count] strong');
  if (n && siteModels) n.textContent = siteModels.count;
}

/** A count of newly arrived models beside "Models" in the sidebar. */
function markModelsNav(st) {
  const a = $('[data-section="models"]');
  if (!a) return;
  const n = st && st.unseen_new || 0;
  a.innerHTML = 'Models' + (n ? ` <span class="nav-badge" title="${n} new model${n === 1 ? '' : 's'} from Meshy">${n}</span>` : '');
}

// ---------------------------------------------------------------- Build & publish

async function viewPublish() {
  const site = await get('/site');
  main.innerHTML = `
    <h1>Build &amp; Publish</h1>
    <p class="sub">Publishing builds without drafts and pushes to <span class="mono">${esc(site.publish.remote || site.publish.pages_repo)}</span> → ${esc(site.base_url)}</p>
    <div class="bar">
      <button data-check>Check</button>
      <button data-build>Build preview (with drafts)</button>
      <button data-plan>Publish plan</button>
      <a class="btn" href="/preview/" target="_blank">Open preview ↗</a>
    </div>
    <div data-out></div>`;
  const out = $('[data-out]');
  $('[data-check]').onclick = () => guard(async () => {
    const r = await get('/check');
    out.innerHTML = `<h2>Check</h2>` + (r.ok ? '<div class="list ok">No problems.</div>'
      : `<ul class="list problems">${r.problems.map(p => `<li>${esc(p)}</li>`).join('')}</ul>`);
  });
  $('[data-build]').onclick = () => guard(async () => {
    out.innerHTML = '<p class="muted">Building…</p>';
    const r = await api('POST', '/build', { drafts: true });
    out.innerHTML = `<h2>Preview build</h2><div class="cards">
      <div class="stat"><b>${r.pages}</b>pages</div><div class="stat"><b>${r.items}</b>items (drafts included)</div>
      <div class="stat"><b>${r.files}</b>files</div></div>
      ${r.warnings.length ? `<h2>Warnings</h2><ul class="list problems">${r.warnings.map(w => `<li>${esc(w)}</li>`).join('')}</ul>` : ''}`;
  });
  $('[data-plan]').onclick = () => guard(async () => {
    out.innerHTML = '<p class="muted">Building and comparing with the Pages repo…</p>';
    const p = await get('/publish/plan');
    const section = (name, list) => `<h2>${name} (${list.length})</h2>` +
      (list.length ? `<div class="list">${list.map(esc).join('<br>')}</div>` : '<p class="empty">None.</p>');
    out.innerHTML = `<div class="cards">
        <div class="stat"><b>${p.added.length}</b>added</div><div class="stat"><b>${p.updated.length}</b>updated</div>
        <div class="stat"><b>${p.removed.length}</b>removed</div><div class="stat"><b>${p.unchanged}</b>unchanged</div></div>
      ${section('Added', p.added)}${section('Updated', p.updated)}${section('Removed', p.removed)}
      <h2>Publish</h2>
      <p class="sub">This commits to <span class="mono">${esc(p.pages_repo)}</span> and pushes to the live site. Type <strong>publish</strong> to confirm.</p>
      <div class="bar"><input data-confirm placeholder="publish" autocapitalize="off" autocorrect="off" autocomplete="off" spellcheck="false"><input data-msg placeholder="Commit message (optional)" style="min-width:320px">
        <button class="primary" data-go disabled>Publish to the live site</button></div>`;
    const c = $('[data-confirm]'), go = $('[data-go]');
    c.oninput = () => (go.disabled = c.value.trim().toLowerCase() !== 'publish');
    c.onkeydown = e => { if (e.key === 'Enter' && !go.disabled) go.click(); };
    go.onclick = () => guard(async () => {
      go.disabled = true; go.textContent = 'Publishing…';
      const r = await api('POST', '/publish', { confirm: true, push: true, message: $('[data-msg]').value || null });
      out.innerHTML = `<h2>Published</h2><div class="list ok">Commit ${esc(r.commit || '(no changes)')} · ${r.added} added, ${r.updated} updated, ${r.removed} removed · pushed: ${r.pushed}<br>
        <a href="${esc(r.url)}" target="_blank">${esc(r.url)}</a> (GitHub Pages takes a minute or two to deploy, and browsers may show the old page for up to 10 minutes — press Cmd+Shift+R to reload)</div>`;
    });
  });
}

function viewPreview() {
  main.innerHTML = `<h1>Preview</h1><p class="sub">The site with drafts included. <a href="/preview/" target="_blank">Open in a tab ↗</a></p>
    <div class="bar"><button data-rebuild>Rebuild</button></div>
    <iframe class="preview-frame" src="/preview/" title="Site preview"></iframe>`;
  $('[data-rebuild]').onclick = () => guard(async () => { await api('POST', '/build', { drafts: true }); $('iframe').src = '/preview/?t=' + Date.now(); toast('Rebuilt'); });
}

// ---------------------------------------------------------------- Stories
//
// A story is a workspace: photos (dropped in, linked, or picked from SmugMug), the author's
// notes and per-photo comments, then an interview in Claude Code (/project-story <slug>) that
// turns it all into a post.

const STORY_STATUSES = ['collecting', 'interviewing', 'drafting', 'drafted', 'released'];
const sourceKind = src => (src || '').split(':')[0];

async function storiesApi(fn) {
  try { return await fn(); } catch (e) {
    if (/no route .*\/api\/(stories|smugmug)/.test(e.message)) {
      main.innerHTML = `<h1>Stories</h1><div class="list problems">This gitwebsite app was started before Stories existed.
        Quit it (menu bar icon → Quit) and open it again to enable them.</div>`;
      return null;
    }
    throw e;
  }
}

function copyText(text, label = 'Copied') {
  const done = () => toast(label + ': ' + text);
  if (navigator.clipboard && window.ClipboardItem) {
    navigator.clipboard.write([new ClipboardItem({ 'text/plain': new Blob([text], { type: 'text/plain' }) })]).then(done, () => fallback());
  } else fallback();
  function fallback() {
    const ta = document.createElement('textarea');
    ta.value = text; ta.style.position = 'fixed'; ta.style.opacity = '0';
    document.body.append(ta); ta.select();
    try { document.execCommand('copy'); done(); } catch { toast(text); }
    ta.remove();
  }
}

async function viewStories() {
  const list = await storiesApi(() => get('/stories'));
  if (!list) return;
  main.innerHTML = `
    <h1>Stories</h1>
    <p class="sub">Turn photos and a few notes into a project story. Add pictures here — from disk or SmugMug, each with a comment —
      then run <code>/project-story &lt;slug&gt;</code> in Claude Code to be interviewed and get a draft.
      To rework an existing post, open it under Content and choose <em>✦ Enhance with Claude</em>.</p>
    <div class="bar"><button class="primary" data-new>New story</button></div>
    <div class="story-cards">${list.map(s => `
      <a class="story-card" href="#story/${esc(s.slug)}">
        <div class="strip">${(s.thumbs || []).map(t => `<img src="${esc(t)}" alt="" loading="lazy">`).join('') || '<span class="muted">No photos yet</span>'}</div>
        <div class="body"><strong>${esc(s.title)}</strong>
          <span><span class="pill st-${esc(s.status)}">${esc(s.status)}</span> <span class="pill">${esc(s.target)}</span> ${s.enhances ? `<span class="pill enh">enhancement</span>${s.has_proposal ? ' <span class="pill need">proposal to review</span>' : ''}` : `${s.images} photo${s.images === 1 ? '' : 's'}`}</span>
          <span class="muted mono">${esc(s.slug)}</span></div>
      </a>`).join('') || '<p class="empty">No stories yet. Start one for something you made, an event you ran, or a build you finished.</p>'}</div>`;
  $('[data-new]').onclick = () => newStory();
}

async function storyForm(title, s, okLabel) {
  const [ar, projects] = await Promise.all([areas(), get('/items?kind=project')]);
  return form(title, [
    { name: 'title', label: 'Title', required: true, value: s.title || '', placeholder: 'Creating the Amiga 40th exhibit' },
    { name: 'target', label: 'Becomes a…', type: 'select', options: [['project', 'Project page'], ['article', 'Article'], ['update', 'Update to an existing project']], value: s.target || 'project' },
    { name: 'project', label: 'Project (for updates)', type: 'select', options: [['', '—'], ...projects.map(p => [p.slug, p.title])], value: s.project || '' },
    { name: 'areas', label: 'Areas (Cmd-click for several)', type: 'select', multiple: true, options: ar.map(a => [a.path, a.path + ' — ' + a.title]), value: s.areas || [] },
    { name: 'tags', label: 'Tags (comma separated)', value: (s.tags || []).join(', ') },
    ...(s.slug ? [{ name: 'status', label: 'Status', type: 'select', options: STORY_STATUSES, value: s.status }] : [
      { name: 'notes', label: 'Notes — a few lines in your own words (what, when, why)', type: 'textarea', rows: 5, value: s.notes || '' }]),
  ], okLabel);
}

async function newStory() {
  const v = await storyForm('New story', {}, 'Create story');
  if (!v) return;
  guard(async () => {
    const s = await api('POST', '/stories', { title: v.title, target: v.target, project: v.project || null, areas: v.areas, tags: csv(v.tags), notes: v.notes });
    location.hash = '#story/' + s.slug;
  });
}

function uploadFile(slug, file, onProgress) {
  return new Promise((resolve, reject) => {
    const x = new XMLHttpRequest();
    x.open('POST', `/api/stories/${encodeURIComponent(slug)}/upload?name=${encodeURIComponent(file.name)}`);
    x.upload.onprogress = e => e.lengthComputable && onProgress(e.loaded / e.total);
    x.onload = () => {
      let d = {}; try { d = JSON.parse(x.responseText); } catch {}
      x.status < 300 ? resolve(d) : reject(new Error(d.error || `${x.status} ${x.statusText}`));
    };
    x.onerror = () => reject(new Error('upload failed'));
    x.send(file);
  });
}

/** The story page from before candidates and live sync (an app started from an older build). */
async function viewStoryLegacy(slug) {
  const s = await storiesApi(() => get('/stories/' + encodeURIComponent(slug)));
  if (!s) return;
  const cmd = s.interview_command;
  const d = s.draft_item;
  main.innerHTML = `
    <h1>${esc(s.title)} <span class="pill st-${esc(s.status)}">${esc(s.status)}</span></h1>
    <p class="sub">${esc(s.target)}${s.project ? ' for ' + esc(s.project) : ''} · ${(s.areas || []).map(a => `<span class="pill">${esc(a)}</span>`).join(' ') || 'no areas yet'} · <span class="mono">${esc(slug)}</span></p>
    <div class="bar">
      <a class="btn" href="#stories">← Stories</a>
      <button data-settings>Settings</button>
      ${d ? `<a class="btn" href="#edit/${esc(d.id)}">Open the draft</a><a class="btn" href="${esc(d.url)}" target="_blank">Preview ↗</a>` : ''}
      <span class="grow"></span><button class="danger" data-delete>Delete story</button>
    </div>
    <div class="interview-panel">
      <div><strong>Start the interview</strong>
        <p>In Claude Code, run the command below. Claude looks at every photo and your comments, asks you questions a few at a time,
          proposes an outline, and writes the draft — you approve each step. Nothing is published unless you ask.</p></div>
      <div class="cmd"><code>${esc(cmd)}</code><button class="primary" data-copy-cmd>Copy</button></div>
    </div>
    <h2>Notes</h2>
    <textarea class="story-notes" data-notes rows="5" placeholder="A few lines in your own words: what it is, when, who it was for, what you're proud of…">${esc(s.notes)}</textarea>
    <div class="bar"><button data-save-notes>Save notes</button><span class="muted" data-notes-state></span></div>
    <h2>Photos <span class="muted">(${s.images.length})</span></h2>
    <div class="drop" data-drop tabindex="0">
      <strong>Drop photos here</strong>, paste them, or <button data-pick-files>choose files</button>
      · <a class="btn primary" href="#story/${esc(slug)}/smugmug">Pick from SmugMug</a>
      · <button data-photos>From Apple Photos…</button>
      · <button data-add-links>Add links…</button>
      <input type="file" accept="image/*,.heic,.heif" multiple hidden data-files>
      <div class="uploads" data-uploads></div>
    </div>
    <p class="muted">Drag photos to reorder. Untick a photo to leave it out of the post. Comments are your words — Claude builds the interview on them.</p>
    <div class="story-grid" data-grid>${s.images.map(im => `
      <div class="story-img ${im.include ? '' : 'excluded'}" draggable="true" data-img="${esc(im.id)}">
        <div class="thumb"><img src="${esc(im.thumb_url)}" alt="" loading="lazy"><span class="badge">${esc(im.id)} · ${esc(sourceKind(im.source))}</span></div>
        <div class="meta">
          <div class="muted small">${esc(im.filename || im.source)}${im.taken ? ' · ' + esc(im.taken) : ''}</div>
          <label class="inline"><input type="checkbox" data-include ${im.include ? 'checked' : ''}> include in the post</label>
          <input data-caption placeholder="Caption" value="${esc(im.caption)}">
          <textarea data-comment rows="3" placeholder="Comment: what's happening here, why it matters">${esc(im.notes)}</textarea>
          <div class="acts"><span class="muted small" data-state></span><span class="grow"></span><button class="danger" data-remove>Remove</button></div>
        </div>
      </div>`).join('') || '<p class="empty">No photos yet.</p>'}</div>
    <div class="two">
      <div><h2>Interview</h2>${s.interview.trim() ? `<pre class="transcript">${esc(s.interview)}</pre>` : `<p class="empty">Not started. Run <code>${esc(cmd)}</code> in Claude Code.</p>`}</div>
      <div><h2>Outline</h2>${s.outline.trim() ? `<pre class="transcript">${esc(s.outline)}</pre>` : '<p class="empty">Agreed during the interview.</p>'}</div>
    </div>`;
  main.insertAdjacentHTML('afterbegin', `<div class="list problems">This gitwebsite app was started before candidates, contact sheets, and live sync with Claude
    existed. Quit it (menu bar icon → Quit) and open it again to get them.</div>`);
  const reload = () => viewStoryLegacy(slug);
  const base = '/stories/' + encodeURIComponent(slug);
  $('[data-copy-cmd]').onclick = () => copyText(cmd, 'Copied — paste it into Claude Code');
  $('[data-settings]').onclick = async () => {
    const v = await storyForm('Story settings', s, 'Save');
    if (v) guard(async () => { await api('PATCH', base, { title: v.title, target: v.target, project: v.project, areas: v.areas, tags: csv(v.tags), status: v.status }); reload(); });
  };
  $('[data-delete]').onclick = async () => {
    const v = await form(`Delete “${s.title}”?`, [{ name: 'confirm', label: `This removes the story's photos (copies), notes, and interview. ${d ? 'The drafted post is kept.' : ''} Type the slug to confirm: ${slug}` }], 'Delete');
    if (v && v.confirm.trim() === slug) guard(async () => { await api('DELETE', base); toast('Deleted'); location.hash = '#stories'; });
    else if (v) toast('Not deleted: the slug did not match', true);
  };
  const notes = $('[data-notes]');
  notes.oninput = () => ($('[data-notes-state]').textContent = 'Unsaved');
  $('[data-save-notes]').onclick = () => guard(async () => { await api('POST', base + '/notes', { text: notes.value, replace: true }); $('[data-notes-state]').textContent = 'Saved'; });

  // uploads: drop, paste, pick
  const drop = $('[data-drop]'), files = $('[data-files]'), ups = $('[data-uploads]');
  const send = async list => {
    const imgs = [...list].filter(f => f.type.startsWith('image/') || /\.(heic|heif)$/i.test(f.name));
    if (!imgs.length) return toast('No image files there', true);
    let failed = 0;
    for (const f of imgs) {
      const row = document.createElement('div');
      row.innerHTML = `<span>${esc(f.name)}</span><progress max="1" value="0"></progress>`;
      ups.append(row);
      try { await uploadFile(slug, f, p => (row.querySelector('progress').value = p)); row.querySelector('progress').value = 1; }
      catch (e) { failed++; row.innerHTML = `<span class="bad">${esc(f.name)}: ${esc(e.message)}</span>`; }
    }
    if (!failed) reload(); else toast(`${failed} file(s) could not be added`, true);
  };
  $('[data-pick-files]').onclick = e => { e.preventDefault(); files.click(); };
  $('[data-photos]').onclick = e => { e.preventDefault(); fromApplePhotos(slug, reload); };
  storyDrop = { slug, send };
  files.onchange = () => send(files.files);
  drop.ondragover = e => { if ([...e.dataTransfer.types].includes('Files')) { e.preventDefault(); drop.classList.add('over'); } };
  drop.ondragleave = () => drop.classList.remove('over');
  drop.ondrop = e => {
    e.preventDefault(); drop.classList.remove('over');
    if (e.dataTransfer.files.length) send(e.dataTransfer.files);
    else toast('Nothing came through with that drop. From Apple Photos, use “From Apple Photos…” (or drag the photos to Finder first).', true);
  };
  document.onpaste = e => { if (location.hash === '#story/' + slug && e.clipboardData.files.length) send(e.clipboardData.files); };
  $('[data-add-links]').onclick = async () => {
    const v = await form('Add photos by link', [
      { name: 'sources', label: 'One per line: image URLs, SmugMug image or album links, smugmug:<ImageKey>, media:<key>, or file paths', type: 'textarea', rows: 6, mono: true },
      { name: 'caption', label: 'Caption (optional, applies to all)' },
      { name: 'comment', label: 'Comment — what these show and why they matter (required for SmugMug)', type: 'textarea', rows: 3 },
    ], 'Add');
    if (!v) return;
    guard(async () => {
      toast('Adding…');
      const r = await api('POST', base + '/images', { sources: v.sources.split('\n').map(x => x.trim()).filter(Boolean), caption: v.caption || null, comment: v.comment || null });
      toast(`Added ${r.added.length}${r.skipped.length ? `, skipped ${r.skipped.length}: ` + r.skipped.map(x => x.reason).join('; ') : ''}`);
      reload();
    });
  };

  // per-image edits (saved when a field loses focus)
  $$('[data-img]').forEach(card => {
    const id = card.dataset.img, state = $('[data-state]', card);
    const patch = body => guard(async () => { state.textContent = 'Saving…'; await api('PATCH', `${base}/images/${id}`, body); state.textContent = 'Saved'; });
    $('[data-caption]', card).onchange = e => patch({ caption: e.target.value });
    $('[data-comment]', card).onchange = e => patch({ notes: e.target.value });
    $('[data-include]', card).onchange = e => { card.classList.toggle('excluded', !e.target.checked); patch({ include: e.target.checked }); };
    $('[data-remove]', card).onclick = async () => {
      const v = await form(`Remove ${id}?`, [{ name: 'x', label: 'It is taken out of the story (a photo you dropped in is deleted from the story folder; SmugMug is not touched).', type: 'checkbox', value: true }], 'Remove');
      if (v) guard(async () => { await api('DELETE', `${base}/images/${id}`); reload(); });
    };
  });
  // drag to reorder
  let dragId = null;
  $$('[data-img]').forEach(card => {
    card.ondragstart = e => { if (e.target.closest('input,textarea')) return e.preventDefault(); dragId = card.dataset.img; card.classList.add('dragging'); };
    card.ondragend = () => card.classList.remove('dragging');
    card.ondragover = e => { if (dragId) { e.preventDefault(); card.classList.add('drop-target'); } };
    card.ondragleave = () => card.classList.remove('drop-target');
    card.ondrop = e => {
      e.preventDefault(); card.classList.remove('drop-target');
      if (!dragId || dragId === card.dataset.img) return;
      const ids = $$('[data-img]').map(c => c.dataset.img).filter(x => x !== dragId);
      ids.splice(ids.indexOf(card.dataset.img), 0, dragId);
      dragId = null;
      guard(async () => { await api('POST', base + '/order', { ids }); reload(); });
    };
  });
}


// ---------------------------------------------------------------- Story page (candidates, live sync)
//
// The story page stays in sync with Claude: the CLI, the MCP server, and this app all write the
// same story under a lock, and the server streams every change (/events). Edits here never
// clobber edits made elsewhere: a refresh waits while you are typing, per-photo fields are saved
// one at a time, and the story text is saved against the revision you started from (a conflict
// asks which version to keep).

let live = null;           // { slug, source, poll, rev, queued, narrativeBase, narrativeDirty }
const candSel = {};        // slug → { id: { number, comment } } — photos being chosen on this page

function stopLive() {
  if (live) {
    if (live.source) live.source.close();
    clearInterval(live.poll);
    clearInterval(live.claudePoll);
  }
  live = null;
}

const WHO = { cli: 'Claude / the terminal', mcp: 'Claude (MCP)', app: 'this app', claude: 'Claude' };
const who = by => WHO[by] || by;

/** Typing in a field on the story page: hold refreshes until it loses focus. */
function editing() {
  const a = document.activeElement;
  return !!(a && main.contains(a) && a.matches('input:not([type=checkbox]), textarea, select'));
}

function startLive(slug, rev) {
  stopLive();
  live = { slug, rev, queued: false, source: null, poll: null };
  const onData = d => {
    if (!live || live.slug !== slug || d.hello) return;
    live.rev = d.rev;
    const theirs = (d.changes || []).filter(c => c.by !== 'app');
    if (theirs.length) flashChanges(theirs);
    refreshStory(slug, (d.changes || []).flatMap(c => c.ids || []));
  };
  const poll = () => {
    clearInterval(live.poll);
    live.poll = setInterval(async () => {
      if (!live || live.slug !== slug) return;
      try {
        const r = await get(`/stories/${encodeURIComponent(slug)}/changes?since=${live.rev}`);
        if (r.rev !== live.rev) onData(r);
      } catch {}
    }, 3000);
  };
  if (window.EventSource) {
    const es = new EventSource(`/api/stories/${encodeURIComponent(slug)}/events?since=${rev}`);
    live.source = es;
    es.addEventListener('change', e => { try { onData(JSON.parse(e.data)); } catch {} });
    es.addEventListener('gone', () => { stopLive(); toast('This story no longer exists', true); });
    es.onerror = () => { if (live && live.source === es && es.readyState === EventSource.CLOSED) { live.source = null; poll(); } };
  } else poll();
  setLive(`Live — changes from Claude appear here as they happen (rev ${rev})`);
}

function setLive(text, cls = '') {
  const el = $('[data-live]');
  if (el) { el.textContent = text; el.className = 'live ' + cls; }
}

function flashChanges(list) {
  // Kept on `live` so the refresh that follows (which re-renders the page) shows it again.
  if (live) live.flash = { html: list.slice(-4).map(c => `<div><strong>${esc(who(c.by))}</strong> · ${esc(String(c.at).slice(11, 16))} — ${esc(c.what)}</div>`).join(''), until: Date.now() + 12000, fresh: true };
  showFlash();
}

function showFlash() {
  const el = $('[data-flash]');
  if (!el || !live || !live.flash || Date.now() > live.flash.until) return;
  el.innerHTML = live.flash.html;
  el.hidden = false;
  if (live.flash.fresh) { el.classList.remove('fade'); void el.offsetWidth; el.classList.add('fade'); live.flash.fresh = false; }
  clearTimeout(showFlash.t);
  showFlash.t = setTimeout(() => { const e = $('[data-flash]'); if (e) e.hidden = true; }, live.flash.until - Date.now());
}

async function refreshStory(slug, changedIds = []) {
  if (!live || live.slug !== slug || location.hash.split('/')[1] !== slug) return;
  if (editing()) {
    live.queued = true;
    setLive('Updated elsewhere — the page refreshes when you finish typing in this field', 'waiting');
    return;
  }
  live.queued = false;
  const y = main.scrollTop;
  try {
    const s = await get('/stories/' + encodeURIComponent(slug));
    renderStory(slug, s);
    main.scrollTop = y;
    for (const id of changedIds) {
      const el = $(`[data-img="${CSS.escape(id)}"], [data-cand="${CSS.escape(id)}"]`);
      if (el) { el.classList.remove('pulse'); void el.offsetWidth; el.classList.add('pulse'); }
    }
    setLive(`Live — up to date (rev ${s.rev})`);
  } catch (e) { setLive('Live sync: ' + e.message, 'waiting'); }
}

// Apply a refresh that waited for the user to leave a field.
main.addEventListener('focusout', () => setTimeout(() => {
  if (live && live.queued && !editing()) refreshStory(live.slug);
}, 250));

async function viewStory(slug) {
  stopLive();
  const s = await storiesApi(() => get('/stories/' + encodeURIComponent(slug)));
  if (!s) return;
  if (s.rev === undefined) return viewStoryLegacy(slug);
  startLive(slug, s.rev);   // first, so the page records which story text it started from
  renderStory(slug, s);
}

function renderStory(slug, s) {
  if (s.is_enhancement) return renderEnhancement(slug, s);
  const base = '/stories/' + encodeURIComponent(slug);
  const cmd = s.interview_command;
  const d = s.draft_item;
  const c = s.counts || {};
  const sel = candSel[slug] || (candSel[slug] = {});
  const showRejected = !!renderStory.rejected;
  const selected = s.images.filter(im => im.state === 'selected');
  const cands = s.images.filter(im => im.state === (showRejected ? 'rejected' : 'candidate'));
  for (const id of Object.keys(sel)) if (!s.images.some(im => im.id === id && im.state !== 'selected')) delete sel[id];
  const narrativeLocal = live && live.slug === slug && live.narrativeDirty ? $('[data-narrative]')?.value : null;

  main.innerHTML = `
    <h1>${esc(s.title)} <span class="pill st-${esc(s.status)}">${esc(s.status)}</span></h1>
    <p class="sub">${esc(s.target)}${s.project ? ' for ' + esc(s.project) : ''} · ${(s.areas || []).map(a => `<span class="pill">${esc(a)}</span>`).join(' ') || 'no areas yet'} · <span class="mono">${esc(slug)}</span></p>
    <div class="bar">
      <a class="btn" href="#stories">← Stories</a>
      <button data-settings>Settings</button>
      ${d ? `<a class="btn" href="#edit/${esc(d.id)}">Open the draft</a><a class="btn" href="${esc(d.url)}" target="_blank">Preview ↗</a>` : ''}
      <span class="grow"></span><button class="danger" data-delete>Delete story</button>
    </div>
    <div class="live" data-live>Live</div>
    <div class="live-flash" data-flash hidden></div>
    ${(s.pending || []).map(q => `
      <div class="pending-q" data-q="${q.id}">
        <div><strong>Claude is asking</strong> <span class="muted small">Q${q.id}</span></div>
        <p>${esc(q.q)}</p>
        <textarea data-ans rows="3" placeholder="Answer here — or in the terminal, whichever is handier"></textarea>
        <div class="bar"><button class="primary" data-send>Send answer</button><span class="muted small">Claude picks it up before its next question.</span></div>
      </div>`).join('')}
    <div class="interview-panel">
      <div><strong>The interview</strong>
        <p>In Claude Code, say <em>“let's do a project interview”</em> or run the command. Claude looks at your chosen photos and comments,
          asks a few questions at a time (answer there or here), proposes an outline, and writes a draft. Nothing is published unless you ask.</p></div>
      <div class="cmd"><code>${esc(cmd)}</code><button class="primary" data-copy-cmd>Copy</button></div>
    </div>
    <div class="two">
      <div><h2>The story so far</h2>
        <textarea class="story-notes" data-narrative rows="7" placeholder="Tell it roughly, in your own words: what happened, when, who, what you're proud of. Claude builds the interview on this.">${esc(narrativeLocal ?? s.narrative ?? '')}</textarea>
        <div class="bar"><span class="muted small" data-nstate>${narrativeLocal != null ? 'Unsaved' : 'Saved automatically'}</span></div>
        <div class="conflict" data-conflict hidden></div></div>
      <div><h2>Notes</h2>
        <textarea class="story-notes" data-notes rows="7" placeholder="Facts and reminders: dates, names, links, numbers…">${esc(s.notes)}</textarea>
        <div class="bar"><button data-save-notes>Save notes</button><span class="muted small" data-notes-state></span></div></div>
    </div>

    <h2>Chosen photos <span class="muted">(${c.selected ?? selected.length})</span>
      ${c.needs_comment ? `<span class="pill need">${c.needs_comment} need${c.needs_comment === 1 ? 's' : ''} a comment</span>` : ''}</h2>
    <p class="muted">Each chosen photo needs a comment in your words. Drag to reorder; untick to leave a photo out of the post.</p>
    <div class="story-grid" data-grid>${selected.map(im => `
      <div class="story-img ${im.include ? '' : 'excluded'} ${im.needs_comment ? 'needs' : ''}" draggable="true" data-img="${esc(im.id)}">
        <div class="thumb"><img src="${esc(im.thumb_url)}" alt="" loading="lazy"><span class="badge">#${im.number} · ${esc(sourceKind(im.source))}</span></div>
        <div class="meta">
          <div class="muted small">${esc(im.filename || im.source)}${im.taken ? ' · ' + esc(im.taken) : ''}</div>
          <label class="inline"><input type="checkbox" data-include ${im.include ? 'checked' : ''}> include in the post</label>
          <input data-caption placeholder="Caption" value="${esc(im.caption)}">
          <textarea data-comment rows="3" placeholder="${im.needs_comment ? 'Comment needed: what’s happening here, why it matters' : 'Comment'}">${esc(im.notes)}</textarea>
          <div class="acts"><span class="muted small" data-state></span><span class="grow"></span>
            <button data-unselect title="Back to the candidates">↩ Candidates</button><button class="danger" data-remove>Remove</button></div>
        </div>
      </div>`).join('') || '<p class="empty">None yet — choose from the candidates below, drop photos here, or pick from SmugMug.</p>'}</div>
    <div class="drop" data-drop tabindex="0">
      <strong>Drop photos here</strong> to add them straight to the story, paste them, or <button data-pick-files>choose files</button>
      · <a class="btn primary" href="#story/${esc(slug)}/smugmug">Pick from SmugMug</a> · <button data-photos>From Apple Photos…</button> · <button data-add-links>Add links…</button>
      <input type="file" accept="image/*,.heic,.heif" multiple hidden data-files>
      <div class="uploads" data-uploads></div>
    </div>

    <h2>Candidates <span class="muted">(${c.candidates ?? 0}${c.rejected ? `, ${c.rejected} rejected` : ''})</span></h2>
    <div class="bar cand-add">
      <input data-dir placeholder="A folder on this Mac, e.g. ~/Pictures/Amiga 40th">
      <label class="inline"><input type="checkbox" data-recursive> subfolders</label>
      <button data-add-dir>Offer folder</button>
      <input data-smug placeholder="SmugMug album or gallery link">
      <button data-add-smug>Offer album</button>
    </div>
    <div class="cand-wrap">
      <div class="cand-main">
        <div class="bar"><span class="seg"><button data-show="candidate" class="${showRejected ? '' : 'on'}">Candidates</button><button data-show="rejected" class="${showRejected ? 'on' : ''}">Rejected</button></span>
          <span class="grow"></span><button data-cand-all>Choose all</button><button data-cand-none>None</button></div>
        <p class="muted small">Click to choose · Shift-click for a range · double-click to enlarge (arrow keys, space to choose)</p>
        <div class="pick-grid" data-cgrid>${cands.map((im, i) => `
          <div class="pick ${sel[im.id] ? 'on' : ''}" data-cand="${esc(im.id)}" data-i="${i}">
            <img src="${esc(im.thumb_url)}" alt="" loading="lazy"><span class="check">✓</span><span class="num">#${im.number}</span>
            <span class="name">${esc(im.filename || im.source)}${im.taken ? ' · ' + esc(String(im.taken).slice(0, 10)) : ''}</span></div>`).join('')
          || `<p class="empty">${showRejected ? 'Nothing rejected.' : 'No candidates. Offer a folder or a SmugMug album above — or ask Claude to (“here are the photos: ~/Pictures/…”).'}</p>`}</div>
      </div>
      <aside class="picker-side cand-side">
        <h2>Choosing <span data-ccount>0</span></h2>
        <div data-csel></div>
        <p class="muted small" data-cwhy></p>
        <button class="primary wide" data-cselect disabled>Add to the story</button>
        <button class="wide" data-creject disabled>Reject</button>
      </aside>
    </div>
    <div class="two">
      <div><h2>Interview</h2>${s.interview.trim() ? `<pre class="transcript">${esc(s.interview)}</pre>` : `<p class="empty">Not started. Say “let's do a project interview” in Claude Code, or run <code>${esc(cmd)}</code>.</p>`}</div>
      <div><h2>Outline</h2>${s.outline.trim() ? `<pre class="transcript">${esc(s.outline)}</pre>` : '<p class="empty">Agreed during the interview.</p>'}</div>
    </div>
    <div class="lightbox" data-lightbox hidden><button class="close" data-lb-close aria-label="Close">×</button>
      <button class="nav prev" data-lb-prev aria-label="Previous">‹</button><img data-lb-img alt=""><button class="nav next" data-lb-next aria-label="Next">›</button>
      <div class="lb-meta" data-lb-meta></div></div>`;

  if (live && live.slug === slug) { setLive(`Live — up to date (rev ${s.rev})`); showFlash(); }
  const reload = () => refreshStory(slug);
  $('[data-copy-cmd]').onclick = () => copyText(cmd, 'Copied — paste it into Claude Code');
  $('[data-settings]').onclick = async () => {
    const v = await storyForm('Story settings', s, 'Save');
    if (v) guard(async () => { await api('PATCH', base, { title: v.title, target: v.target, project: v.project, areas: v.areas, tags: csv(v.tags), status: v.status }); reload(); });
  };
  $('[data-delete]').onclick = async () => {
    const v = await form(`Delete “${s.title}”?`, [{ name: 'confirm', label: `This removes the story's photos (copies), notes, and interview. ${d ? 'The drafted post is kept.' : ''} Type the slug to confirm: ${slug}` }], 'Delete');
    if (v && v.confirm.trim() === slug) guard(async () => { stopLive(); await api('DELETE', base); toast('Deleted'); location.hash = '#stories'; });
    else if (v) toast('Not deleted: the slug did not match', true);
  };

  // pending questions: answer here instead of the terminal
  $$('[data-q]').forEach(box => {
    $('[data-send]', box).onclick = () => guard(async () => {
      const a = $('[data-ans]', box).value.trim();
      if (!a) return toast('Write an answer first', true);
      await api('POST', base + '/answer', { id: +box.dataset.q, a });
      toast('Sent — Claude will pick it up');
      reload();
    });
  });

  // the story so far: autosaved against the revision it started from
  if (live && live.slug === slug && live.narrativeBase === undefined) live.narrativeBase = s.narrative_rev;
  if (live && live.slug === slug && !live.narrativeDirty) { live.narrativeBase = s.narrative_rev; live.narrativeBaseText = s.narrative || ''; }
  const nar = $('[data-narrative]'), nstate = $('[data-nstate]'), conflict = $('[data-conflict]');
  const saveNarrative = async (force = false) => {
    if (!live) return;
    try {
      // Never save blind: without a known starting revision, treat it as a conflict to resolve.
      const r = await api('PUT', base + '/narrative', { text: nar.value, base: force ? null : (live.narrativeBase ?? -1) });
      live.narrativeBase = r.narrative_rev; live.narrativeDirty = false;
      nstate.textContent = 'Saved'; conflict.hidden = true;
    } catch (e) {
      if (!/^conflict:/.test(e.message)) return toast(e.message, true);
      const theirs = (await get('/stories/' + encodeURIComponent(slug))).narrative;
      conflict.hidden = false;
      conflict.innerHTML = `<strong>Claude changed the story text while you were typing.</strong> Theirs:<pre>${esc(theirs)}</pre>
        <div class="bar"><button data-keep-mine>Keep mine</button><button data-keep-theirs>Use theirs</button><button class="primary" data-keep-both>Keep both</button></div>`;
      $('[data-keep-mine]', conflict).onclick = () => saveNarrative(true);
      $('[data-keep-theirs]', conflict).onclick = () => { live.narrativeDirty = false; conflict.hidden = true; reload(); };
      // Keep both: their version plus only what you added (your text minus what it started from).
      $('[data-keep-both]', conflict).onclick = () => {
        const start = (live && live.narrativeBaseText) || '';
        const mine = nar.value;
        const added = start && mine.startsWith(start) ? mine.slice(start.length) : '\n\n' + mine.trim();
        nar.value = theirs.trimEnd() + (/^\s/.test(added) ? added : ' ' + added);
        saveNarrative(true);
      };
      nstate.textContent = 'Not saved — see below';
    }
  };
  const debounced = (() => { let t; return () => { clearTimeout(t); t = setTimeout(() => saveNarrative(), 900); }; })();
  nar.oninput = () => { if (live) live.narrativeDirty = true; nstate.textContent = 'Unsaved…'; debounced(); };
  nar.onblur = () => { if (live && live.narrativeDirty) saveNarrative(); };

  const notes = $('[data-notes]');
  notes.oninput = () => ($('[data-notes-state]').textContent = 'Unsaved');
  $('[data-save-notes]').onclick = () => guard(async () => { await api('POST', base + '/notes', { text: notes.value, replace: true }); $('[data-notes-state]').textContent = 'Saved'; });

  // uploads: drop, paste, pick — straight into the chosen photos
  const drop = $('[data-drop]'), files = $('[data-files]'), ups = $('[data-uploads]');
  const send = async list => {
    const imgs = [...list].filter(f => f.type.startsWith('image/') || /\.(heic|heif)$/i.test(f.name));
    if (!imgs.length) return toast('No image files there', true);
    let failed = 0;
    for (const f of imgs) {
      const row = document.createElement('div');
      row.innerHTML = `<span>${esc(f.name)}</span><progress max="1" value="0"></progress>`;
      ups.append(row);
      try { await uploadFile(slug, f, p => (row.querySelector('progress').value = p)); row.querySelector('progress').value = 1; }
      catch (e) { failed++; row.innerHTML = `<span class="bad">${esc(f.name)}: ${esc(e.message)}</span>`; }
    }
    if (!failed) reload(); else toast(`${failed} file(s) could not be added`, true);
  };
  $('[data-pick-files]').onclick = e => { e.preventDefault(); files.click(); };
  $('[data-photos]').onclick = e => { e.preventDefault(); fromApplePhotos(slug, reload); };
  storyDrop = { slug, send };
  files.onchange = () => send(files.files);
  drop.ondragover = e => { if ([...e.dataTransfer.types].includes('Files')) { e.preventDefault(); drop.classList.add('over'); } };
  drop.ondragleave = () => drop.classList.remove('over');
  drop.ondrop = e => {
    e.preventDefault(); drop.classList.remove('over');
    if (e.dataTransfer.files.length) send(e.dataTransfer.files);
    else toast('Nothing came through with that drop. From Apple Photos, use “From Apple Photos…” (or drag the photos to Finder first).', true);
  };
  document.onpaste = e => { if (location.hash === '#story/' + slug && e.clipboardData.files.length) send(e.clipboardData.files); };
  $('[data-add-links]').onclick = async () => {
    const v = await form('Add photos by link', [
      { name: 'sources', label: 'One per line: image URLs, SmugMug image or album links, smugmug:<ImageKey>, media:<key>, or file paths', type: 'textarea', rows: 6, mono: true },
      { name: 'candidate', label: 'Offer them as candidates to choose from (no comment needed yet)', type: 'checkbox', value: true },
      { name: 'comment', label: 'Comment — what these show and why they matter (needed to add SmugMug photos directly)', type: 'textarea', rows: 3 },
    ], 'Add');
    if (!v) return;
    guard(async () => {
      toast('Adding…');
      const r = await api('POST', base + '/images', { sources: v.sources.split('\n').map(x => x.trim()).filter(Boolean), comment: v.comment || null, candidate: v.candidate });
      toast(`Added ${r.added.length}${r.skipped.length ? `, skipped ${r.skipped.length}: ` + r.skipped.map(x => x.reason).join('; ') : ''}`);
      reload();
    });
  };

  // offering candidates: a folder on this Mac, or a SmugMug album
  $('[data-add-dir]').onclick = () => guard(async () => {
    const dir = $('[data-dir]').value.trim();
    if (!dir) return toast('Type a folder path', true);
    toast('Looking for photos…');
    const r = await api('POST', base + '/add-dir', { dirs: [dir], recursive: $('[data-recursive]').checked });
    toast(`Offered ${r.added} photo${r.added === 1 ? '' : 's'} ${r.numbers || ''}${r.already_in_story ? ` (${r.already_in_story} already here)` : ''}${r.videos_not_added.length ? `; ${r.videos_not_added.length} video(s) not added` : ''}`);
    reload();
  });
  $('[data-dir]').onkeydown = e => { if (e.key === 'Enter') $('[data-add-dir]').click(); };
  $('[data-add-smug]').onclick = () => guard(async () => {
    const t = $('[data-smug]').value.trim();
    if (!t) return toast('Paste a SmugMug album or gallery link', true);
    toast('Asking SmugMug…');
    const r = await api('POST', base + '/images', { sources: [t], candidate: true });
    toast(`Offered ${r.added.length} photo${r.added.length === 1 ? '' : 's'}${r.skipped.length ? `, ${r.skipped.length} skipped` : ''}`);
    reload();
  });

  // chosen photos: per-field saves
  $$('[data-img]').forEach(card => {
    const id = card.dataset.img, state = $('[data-state]', card);
    const patch = body => guard(async () => { state.textContent = 'Saving…'; await api('PATCH', `${base}/images/${id}`, body); state.textContent = 'Saved'; });
    $('[data-caption]', card).onchange = e => patch({ caption: e.target.value });
    $('[data-comment]', card).onchange = e => patch({ notes: e.target.value }).then(() => { if (e.target.value.trim()) card.classList.remove('needs'); });
    $('[data-include]', card).onchange = e => { card.classList.toggle('excluded', !e.target.checked); patch({ include: e.target.checked }); };
    $('[data-unselect]', card).onclick = () => guard(async () => { await api('POST', base + '/unselect', { refs: id }); reload(); });
    $('[data-remove]', card).onclick = async () => {
      const v = await form(`Remove ${id}?`, [{ name: 'x', label: 'It is taken out of the story (a photo you dropped in is deleted from the story folder; SmugMug and your folders are not touched).', type: 'checkbox', value: true }], 'Remove');
      if (v) guard(async () => { await api('DELETE', `${base}/images/${id}`); reload(); });
    };
  });
  let dragId = null;
  $$('[data-img]').forEach(card => {
    card.ondragstart = e => { if (e.target.closest('input,textarea')) return e.preventDefault(); dragId = card.dataset.img; card.classList.add('dragging'); };
    card.ondragend = () => card.classList.remove('dragging');
    card.ondragover = e => { if (dragId) { e.preventDefault(); card.classList.add('drop-target'); } };
    card.ondragleave = () => card.classList.remove('drop-target');
    card.ondrop = e => {
      e.preventDefault(); card.classList.remove('drop-target');
      if (!dragId || dragId === card.dataset.img) return;
      const ids = $$('[data-img]').map(c => c.dataset.img).filter(x => x !== dragId);
      ids.splice(ids.indexOf(card.dataset.img), 0, dragId);
      dragId = null;
      guard(async () => { await api('POST', base + '/order', { ids }); reload(); });
    };
  });

  // candidates: choose (each needs a comment), reject
  $$('[data-show]').forEach(b => b.onclick = () => { renderStory.rejected = b.dataset.show === 'rejected'; renderStory(slug, s); });
  let last = -1;
  const byId = Object.fromEntries(cands.map(im => [im.id, im]));
  const toggle = (id, on) => {
    const im = byId[id];
    if (!im) return;
    on = on ?? !sel[id];
    if (on) sel[id] = sel[id] || { number: im.number, comment: im.notes || '' }; else delete sel[id];
    const t = $(`[data-cand="${CSS.escape(id)}"]`);
    if (t) t.classList.toggle('on', !!sel[id]);
  };
  const renderSide = () => {
    const ids = Object.keys(sel);
    $('[data-ccount]').textContent = ids.length;
    $('[data-csel]').innerHTML = ids.map(id => {
      const x = sel[id], im = byId[id];
      return `<div class="sel ${(x.comment || '').trim() ? '' : 'missing'}" data-s="${esc(id)}"><img src="${esc(im ? im.thumb_url : '')}" alt="">
        <div><div class="small"><strong>#${x.number}</strong> ${esc(im ? im.filename : '')}</div>
        <textarea data-sc rows="2" placeholder="Required: what's happening here, why it matters">${esc(x.comment || '')}</textarea></div>
        <button class="x" data-sx title="Don't choose">×</button></div>`;
    }).join('') || `<p class="empty">${showRejected ? 'Click rejected photos to bring them back.' : 'Click photos to choose them. Each needs a comment before it joins the story.'}</p>`;
    $$('[data-s]').forEach(el => {
      const id = el.dataset.s;
      $('[data-sc]', el).oninput = e => { sel[id].comment = e.target.value; el.classList.toggle('missing', !e.target.value.trim()); why(); };
      $('[data-sx]', el).onclick = () => { toggle(id, false); renderSide(); };
    });
    why();
  };
  const why = () => {
    const ids = Object.keys(sel);
    const missing = ids.filter(id => !(sel[id].comment || '').trim());
    $('[data-cselect]').disabled = !ids.length || missing.length > 0;
    $('[data-cselect]').textContent = ids.length ? `Add ${ids.length} photo${ids.length === 1 ? '' : 's'} to the story` : 'Add to the story';
    $('[data-creject]').disabled = !ids.length || showRejected;
    $('[data-cwhy]').textContent = missing.length ? `Still needing a comment: ${missing.map(id => '#' + sel[id].number).join(', ')}` : '';
  };
  $$('[data-cand]').forEach(t => {
    t.onclick = e => {
      const i = +t.dataset.i;
      if (e.shiftKey && last >= 0) {
        const on = !sel[cands[i].id];
        for (let j = Math.min(last, i); j <= Math.max(last, i); j++) toggle(cands[j].id, on);
      } else toggle(t.dataset.cand);
      last = i; renderSide();
    };
    t.ondblclick = () => lightbox(+t.dataset.i);
  });
  $('[data-cand-all]').onclick = () => { cands.forEach(im => toggle(im.id, true)); renderSide(); };
  $('[data-cand-none]').onclick = () => { cands.forEach(im => toggle(im.id, false)); renderSide(); };
  $('[data-cselect]').onclick = () => guard(async () => {
    const ids = Object.keys(sel);
    $('[data-cselect]').disabled = true; $('[data-cselect]').textContent = 'Adding…';
    for (const id of ids) {
      await api('POST', base + '/select', { refs: id, comment: sel[id].comment.trim() });
      delete sel[id];
    }
    toast(`Added ${ids.length} photo${ids.length === 1 ? '' : 's'} to the story`);
    reload();
  });
  $('[data-creject]').onclick = () => guard(async () => {
    const ids = Object.keys(sel);
    await api('POST', base + '/reject', { refs: ids.join(',') });
    ids.forEach(id => delete sel[id]);
    reload();
  });
  renderSide();

  // lightbox over the candidates (arrow keys, space to choose)
  const lb = $('[data-lightbox]');
  let lbI = 0;
  function lightbox(i) {
    if (!cands.length) return;
    lbI = (i + cands.length) % cands.length;
    const im = cands[lbI];
    $('[data-lb-img]').src = im.thumb_url;
    $('[data-lb-meta]').innerHTML = `<strong>#${im.number}</strong> ${esc(im.filename)} ${im.taken ? '· ' + esc(im.taken) : ''} · ${lbI + 1}/${cands.length}
      <button data-lb-sel class="${sel[im.id] ? 'primary' : ''}">${sel[im.id] ? 'Chosen ✓' : 'Choose (space)'}</button>`;
    $('[data-lb-sel]').onclick = () => { toggle(im.id); renderSide(); lightbox(lbI); };
    lb.hidden = false;
  }
  $('[data-lb-close]').onclick = () => (lb.hidden = true);
  $('[data-lb-prev]').onclick = () => lightbox(lbI - 1);
  $('[data-lb-next]').onclick = () => lightbox(lbI + 1);
  lb.onclick = e => { if (e.target === lb) lb.hidden = true; };
  document.onkeydown = e => {
    if (lb.hidden) return;
    if (e.key === 'Escape') lb.hidden = true;
    else if (e.key === 'ArrowLeft') lightbox(lbI - 1);
    else if (e.key === 'ArrowRight') lightbox(lbI + 1);
    else if (e.key === ' ') { e.preventDefault(); toggle(cands[lbI].id); renderSide(); lightbox(lbI); }
  };
}

// ---------------------------------------------------------------- SmugMug picker

// ---------------------------------------------------------------- Enhancements
// An enhancement reworks an existing post: the author answers a few standard questions, Claude
// reads the post and the code repository behind it, interviews (questions appear here), and
// proposes a revision. The post changes only when the author accepts the proposal.

const keyDraft = {};       // slug → { questionId: text being typed }

/** Fold long unchanged stretches of a diff, keeping `ctx` lines around each change. */
function foldDiff(diff, ctx = 3) {
  const keep = diff.map(() => false);
  diff.forEach((d, i) => { if (d.op !== ' ') for (let j = Math.max(0, i - ctx); j <= Math.min(diff.length - 1, i + ctx); j++) keep[j] = true; });
  const out = [];
  let skipped = 0;
  diff.forEach((d, i) => {
    if (keep[i]) { if (skipped) out.push({ fold: skipped }); skipped = 0; out.push(d); } else skipped++;
  });
  if (skipped) out.push({ fold: skipped });
  return out;
}

function renderEnhancement(slug, s) {
  const base = '/stories/' + encodeURIComponent(slug);
  const it = s.enhances_item || {};
  const cmd = s.interview_command;
  const repo = s.repo || {};
  const drafts = keyDraft[slug] || (keyDraft[slug] = {});
  const answered = (s.key_questions || []).filter(q => q.a).length;
  const run = s.claude || null;

  main.innerHTML = `
    <h1><span class="muted">Enhance:</span> ${esc(it.title || s.title)} <span class="pill st-${esc(s.status)}">${esc(s.status)}</span></h1>
    <p class="sub">${esc(it.kind || s.target)} · ${it.missing ? `<span class="pill need">the post ${esc(it.id)} is missing</span>` : `<a href="#edit/${esc(it.id)}">${esc(it.id)}</a>`}
      · ${(s.areas || []).map(a => `<span class="pill">${esc(a)}</span>`).join(' ')} · <span class="mono">${esc(slug)}</span></p>
    <div class="bar">
      <a class="btn" href="#stories">← Stories</a>
      ${it.id && !it.missing ? `<a class="btn" href="#edit/${esc(it.id)}">Open the post</a><a class="btn" href="${esc(it.url)}" target="_blank">Preview ↗</a>` : ''}
      <span class="grow"></span><button class="danger" data-delete>Delete enhancement</button>
    </div>
    <div class="live" data-live>Live</div>
    <div class="live-flash" data-flash hidden></div>

    <div class="enh-steps">
      <span class="${answered ? 'done' : ''}">1 · Answer what you can below</span>
      <span class="${run ? 'done' : ''}">2 · Start Claude (it reads the post and the repo, then asks)</span>
      <span class="${s.has_proposal ? 'done' : ''}">3 · Review the proposal: accept, or ask for changes</span>
    </div>

    ${(s.pending || []).map(q => `
      <div class="pending-q" data-q="${q.id}">
        <div><strong>Claude is asking</strong> <span class="muted small">Q${q.id}</span></div>
        <p>${esc(q.q)}</p>
        <textarea data-ans rows="3" placeholder="Answer here — Claude is waiting for it"></textarea>
        <div class="bar"><button class="primary" data-send>Send answer</button><span class="muted small">Claude picks it up within a few seconds.</span></div>
      </div>`).join('')}

    <div class="proposal" data-proposal ${s.has_proposal ? '' : 'hidden'}><p class="muted">Loading the proposal…</p></div>

    <div class="interview-panel claude-panel">
      <div><strong>Claude</strong> <span class="pill ${s.claude_running ? 'need' : ''}" data-cstate>${s.claude_running ? 'working' : run ? (run.exit_code === 0 ? 'finished' : run.finished ? 'stopped' : 'finished') : 'not started'}</span>
        <p>Start Claude here and answer its questions on this page, or run the command in Claude Code yourself.
          It reads the post${repo.path ? ' and the repository' : ''}, asks a few questions at a time, and writes a proposal. It never changes the post or publishes.</p></div>
      <div class="cmd">
        <button class="primary" data-cstart ${s.claude_running ? 'hidden' : ''}>${run ? 'Start Claude again' : 'Start Claude'}</button>
        <button data-cstop ${s.claude_running ? '' : 'hidden'}>Stop</button>
        <code>${esc(cmd)}</code><button data-copy-cmd>Copy</button></div>
    </div>
    <pre class="transcript claude-log" data-clog ${run ? '' : 'hidden'}></pre>

    <div class="two">
      <div>
        <h2>Key questions <span class="muted">(${answered}/${(s.key_questions || []).length})</span></h2>
        <p class="muted small">Answer in your own words — skip any. Each answer is saved when you leave the box and goes into the interview.</p>
        <div class="keyq">${(s.key_questions || []).map(q => `
          <label>${esc(q.q)}<textarea rows="3" data-key="${esc(q.id)}" placeholder="Optional">${esc(drafts[q.id] ?? q.a)}</textarea>
            <span class="muted small" data-kstate="${esc(q.id)}">${drafts[q.id] !== undefined && drafts[q.id] !== q.a ? 'Unsaved' : q.a ? 'Saved' : ''}</span></label>`).join('')}</div>
      </div>
      <div>
        <h2>Repository</h2>
        <div class="repo-box">${repo.url || repo.path ? `
          ${repo.url ? `<div><a href="${esc(repo.url)}" target="_blank">${esc(repo.url)}</a></div>` : ''}
          <div class="mono small">${repo.path ? esc(repo.path) : '<span class="pill need">no local checkout found</span>'}</div>`
          : '<p class="empty">None detected. Set one so Claude can read the code.</p>'}
          <div class="bar"><button data-repo>Change…</button></div></div>
        <h2>Anything else Claude should know</h2>
        <textarea class="story-notes" data-notes rows="6" placeholder="Facts, links, what you want emphasised or cut, the tone you want…">${esc(s.notes)}</textarea>
        <div class="bar"><button data-save-notes>Save</button><span class="muted small" data-notes-state></span></div>
      </div>
    </div>
    <h2>Interview</h2>${s.interview.trim() ? `<pre class="transcript">${esc(s.interview)}</pre>` : '<p class="empty">Nothing yet — your answers and Claude’s questions appear here.</p>'}`;

  if (live && live.slug === slug) { setLive(`Live — up to date (rev ${s.rev})`); showFlash(); }
  const reload = () => refreshStory(slug);
  $('[data-copy-cmd]').onclick = () => copyText(cmd, 'Copied — paste it into Claude Code');
  $('[data-delete]').onclick = async () => {
    const v = await form(`Delete this enhancement?`, [{ name: 'confirm', label: `This removes the answers, the interview, and any proposal. The post itself is kept. Type the slug to confirm: ${slug}` }], 'Delete');
    if (v && v.confirm.trim() === slug) guard(async () => { stopLive(); await api('DELETE', base); toast('Deleted'); location.hash = '#stories'; });
    else if (v) toast('Not deleted: the slug did not match', true);
  };
  $$('[data-q]').forEach(box => {
    $('[data-send]', box).onclick = () => guard(async () => {
      const a = $('[data-ans]', box).value.trim();
      if (!a) return toast('Write an answer first', true);
      await api('POST', base + '/answer', { id: +box.dataset.q, a });
      toast('Sent — Claude will pick it up');
      reload();
    });
  });

  // key questions: kept locally while typing, saved one at a time on leaving the box
  $$('[data-key]').forEach(ta => {
    const id = ta.dataset.key;
    const st = $(`[data-kstate="${CSS.escape(id)}"]`);
    ta.oninput = () => { drafts[id] = ta.value; st.textContent = 'Unsaved'; };
    ta.onchange = () => guard(async () => {
      await api('PUT', base + '/key-answers', { answers: { [id]: ta.value } });
      delete drafts[id];
      st.textContent = 'Saved';
    });
  });

  $('[data-repo]').onclick = async () => {
    const v = await form('Repository', [
      { name: 'url', label: 'URL (e.g. https://github.com/you/thing)', value: repo.url || '' },
      { name: 'path', label: 'Local checkout (leave empty to look for one)', value: repo.path || '' },
    ]);
    if (v) guard(async () => { await api('PUT', base + '/repo', v); reload(); });
  };
  const notes = $('[data-notes]');
  notes.oninput = () => ($('[data-notes-state]').textContent = 'Unsaved');
  $('[data-save-notes]').onclick = () => guard(async () => { await api('POST', base + '/notes', { text: notes.value, replace: true }); $('[data-notes-state]').textContent = 'Saved'; });

  // Claude: start / stop / log
  const clog = $('[data-clog]');
  const pollClaude = async () => {
    try {
      const r = await get(base + '/claude?lines=60');
      if (!r.claude) return;
      clog.hidden = false;
      const atEnd = clog.scrollTop + clog.clientHeight >= clog.scrollHeight - 8;
      clog.textContent = (r.log || []).join('\n') || 'Starting…';
      if (atEnd) clog.scrollTop = clog.scrollHeight;
      const cs = $('[data-cstate]');
      if (cs) { cs.textContent = r.running ? 'working' : 'finished'; cs.className = 'pill ' + (r.running ? 'need' : ''); }
      $('[data-cstart]').hidden = r.running; $('[data-cstop]').hidden = !r.running;
      if (!r.running && live) clearInterval(live.claudePoll);
    } catch {}
  };
  if (live && live.slug === slug) {
    clearInterval(live.claudePoll);
    if (run) { pollClaude(); if (s.claude_running) live.claudePoll = setInterval(pollClaude, 2500); }
  }
  $('[data-cstart]').onclick = () => guard(async () => {
    await api('POST', base + '/claude');
    toast('Claude is starting — its questions will appear here');
    reload();
  });
  $('[data-cstop]').onclick = () => guard(async () => { await api('DELETE', base + '/claude'); toast('Stopped'); reload(); });

  // the proposal: a folded line diff with accept / request changes / discard
  if (s.has_proposal) guard(async () => {
    const p = await get(base + '/proposal');
    const box = $('[data-proposal]');
    if (!box) return;
    const rows = foldDiff(p.diff).map(d => d.fold
      ? `<div class="d-fold">… ${d.fold} unchanged line${d.fold === 1 ? '' : 's'} …</div>`
      : `<div class="d-${d.op === '+' ? 'add' : d.op === '-' ? 'del' : 'ctx'}"><span>${d.op === ' ' ? '&nbsp;' : esc(d.op)}</span>${esc(d.text) || '&nbsp;'}</div>`).join('');
    box.innerHTML = `
      <h2>Proposal ${p.proposal && p.proposal.round > 1 ? `<span class="muted">(revision ${p.proposal.round})</span>` : ''}
        <span class="pill">+${p.added} −${p.removed}</span></h2>
      <p class="muted small">From ${esc(who(p.proposal?.by))} at ${esc(String(p.proposal?.at || '').replace('T', ' ').slice(0, 16))}. Red is the post now, green is what accepting writes:
        the proposal with the post's address, status, and date kept, and <code>:updated:</code> stamped.</p>
      <div class="bar"><span class="seg"><button data-pv="diff" class="on">Changes</button><button data-pv="full">Full proposal</button></span></div>
      <div class="diff" data-pdiff>${rows}</div>
      <pre class="transcript" data-pfull hidden>${esc(p.result || p.text)}</pre>
      <div class="bar">
        <button class="primary" data-accept>Accept into the post</button>
        <button data-discard class="danger">Discard</button>
      </div>
      <label>Ask for changes<textarea rows="3" data-feedback placeholder="What should change? Claude revises the proposal."></textarea></label>
      <div class="bar"><button data-request>Send to Claude</button>${s.claude_running ? '' : '<span class="muted small">Claude isn’t running: start it again after sending, or run the command.</span>'}</div>`;
    $$('[data-pv]', box).forEach(b => b.onclick = () => {
      $$('[data-pv]', box).forEach(x => x.classList.toggle('on', x === b));
      $('[data-pdiff]', box).hidden = b.dataset.pv !== 'diff';
      $('[data-pfull]', box).hidden = b.dataset.pv !== 'full';
    });
    $('[data-accept]', box).onclick = () => guard(async () => {
      if (!(await ask('Accept the proposal?', 'Write the proposal over the post? It keeps its address, status, and date.', 'Accept into the post'))) return;
      const r = await api('POST', base + '/accept');
      toast(`Accepted into ${r.url}` + (r.status === 'published' ? ' — it goes live at the next publish' : ''));
      for (const w of r.warnings || []) toast(w, true);
      reload();
    });
    $('[data-discard]', box).onclick = () => guard(async () => {
      if (!(await ask('Discard the proposal?', 'Throw this proposal away?', 'Discard'))) return;
      await api('POST', base + '/discard'); toast('Discarded'); reload();
    });
    $('[data-request]', box).onclick = () => guard(async () => {
      const t = $('[data-feedback]', box).value.trim();
      if (!t) return toast('Say what should change first', true);
      await api('POST', base + '/request-changes', { text: t });
      toast('Sent — Claude revises the proposal'); reload();
    });
  });
}

async function viewSmugmugPicker(slug, initialTarget) {
  const s = await storiesApi(() => get('/stories/' + encodeURIComponent(slug)));
  if (!s) return;
  const base = '/stories/' + encodeURIComponent(slug);
  // What is already in the story, by ImageKey.
  const inStory = {};
  for (const im of s.images) if (im.source.startsWith('smugmug:')) inStory[im.source.slice(8)] = im;
  // Selection: ImageKey → {image, comment, caption}; restored from this tab's session.
  const memKey = 'gw-picker:' + slug;
  let sel = {};
  try { sel = JSON.parse(sessionStorage.getItem(memKey) || '{}'); } catch {}
  for (const [k, im] of Object.entries(inStory)) if (!sel[k]) sel[k] = { image: { image_key: k, filename: im.filename, small: im.thumb_url, caption: im.caption }, comment: im.notes, caption: im.caption, existing: true, keep: true };
  const remember = () => { try { sessionStorage.setItem(memKey, JSON.stringify(sel)); } catch {} };

  main.innerHTML = `
    <h1>Pick from SmugMug</h1>
    <p class="sub">For <a href="#story/${esc(slug)}">${esc(s.title)}</a>. Open an album or gallery page, select the photos you want, and say a few words about each —
      what's happening and why it matters. Claude uses your comments in the interview.</p>
    <div class="bar picker-bar">
      <input data-target placeholder="SmugMug page link (album, gallery, folder, or photo) or AlbumKey" value="${esc(initialTarget || '')}">
      <button class="primary" data-open>Open</button>
      <select data-albums><option value="">…or choose one of your albums</option></select>
      <input data-album-filter placeholder="Filter albums" size="14">
    </div>
    <div class="picker">
      <div class="picker-main" data-pane><p class="empty">Paste a link to a SmugMug album or gallery page, or choose an album.</p></div>
      <aside class="picker-side">
        <h2>Selected <span data-count>0</span></h2>
        <div class="bar"><button data-all>Select all</button><button data-none>Select none</button></div>
        <div data-selected></div>
        <p class="muted small" data-why></p>
        <button class="primary wide" data-apply disabled>Add to story</button>
        <a class="btn wide" href="#story/${esc(slug)}">Back to the story</a>
      </aside>
    </div>
    <div class="lightbox" data-lightbox hidden><button class="close" data-lb-close aria-label="Close">×</button>
      <button class="nav prev" data-lb-prev aria-label="Previous">‹</button><img data-lb-img alt=""><button class="nav next" data-lb-next aria-label="Next">›</button>
      <div class="lb-meta" data-lb-meta></div></div>`;

  let images = [], lastClicked = -1, loadToken = 0;
  const pane = $('[data-pane]');

  // album list for the dropdown
  storiesApi(() => get('/smugmug/albums')).then(list => {
    if (!list) return;
    const albSel = $('[data-albums]'), filt = $('[data-album-filter]');
    const fill = () => {
      const q = filt.value.toLowerCase();
      albSel.innerHTML = '<option value="">…or choose one of your albums</option>' + list
        .filter(a => !q || (a.name || '').toLowerCase().includes(q) || (a.web || '').toLowerCase().includes(q))
        .map(a => `<option value="${esc(a.web || a.album_key)}">${esc(a.name)} (${a.images ?? '?'})</option>`).join('');
    };
    fill();
    filt.oninput = fill;
    albSel.onchange = () => { if (albSel.value) { $('[data-target]').value = albSel.value; open(); } };
  }).catch(e => toast('SmugMug: ' + e.message, true));

  const thumbOf = im => im.small || im.thumb;
  function renderSide() {
    const keys = Object.keys(sel).filter(k => sel[k].keep !== false);
    const removing = Object.keys(sel).filter(k => sel[k].existing && sel[k].keep === false);
    $('[data-count]').textContent = keys.length;
    $('[data-selected]').innerHTML = keys.map(k => {
      const x = sel[k], missing = !x.comment || !x.comment.trim();
      return `<div class="sel ${missing ? 'missing' : ''}" data-sel="${esc(k)}">
        <img src="${esc(thumbOf(x.image) || '')}" alt="">
        <div><div class="small"><strong>${esc(x.image.filename || k)}</strong>${x.existing ? ' <span class="pill">in story</span>' : ''}</div>
          <textarea data-c rows="2" placeholder="Required: what's happening here, why it matters">${esc(x.comment || '')}</textarea>
          <input data-cap placeholder="Caption (optional)" value="${esc(x.caption ?? x.image.caption ?? '')}"></div>
        <button class="x" data-unsel title="Deselect">×</button></div>`;
    }).join('') + (removing.length ? `<p class="small bad">${removing.length} photo(s) already in the story will be removed.</p>` : '') ||
      '<p class="empty">Click photos to select them.</p>';
    $$('[data-sel]').forEach(el => {
      const k = el.dataset.sel;
      $('[data-c]', el).oninput = e => { sel[k].comment = e.target.value; el.classList.toggle('missing', !e.target.value.trim()); remember(); updateApply(); };
      $('[data-cap]', el).oninput = e => { sel[k].caption = e.target.value; remember(); };
      $('[data-unsel]', el).onclick = () => toggle(k);
    });
    updateApply();
  }
  function updateApply() {
    const kept = Object.keys(sel).filter(k => sel[k].keep !== false);
    const missing = kept.filter(k => !(sel[k].comment || '').trim());
    const adding = kept.filter(k => !sel[k].existing).length;
    const updating = kept.length - adding;
    const removing = Object.keys(sel).filter(k => sel[k].existing && sel[k].keep === false).length;
    const btn = $('[data-apply]');
    btn.disabled = missing.length > 0 || (adding + updating + removing === 0);
    btn.textContent = adding && !updating && !removing ? `Add ${adding} photo${adding === 1 ? '' : 's'} to story`
      : `Apply: add ${adding}, update ${updating}${removing ? `, remove ${removing}` : ''}`;
    $('[data-why]').innerHTML = missing.length
      ? `Each selected photo needs a comment. Missing: ${missing.map(k => esc(sel[k].image.filename || k)).join(', ')}` : '';
  }
  function toggle(k, force) {
    const im = images.find(i => i.image_key === k) || (sel[k] && sel[k].image);
    const on = force ?? !(sel[k] && sel[k].keep !== false);
    if (on) {
      if (sel[k]) sel[k].keep = true;
      else sel[k] = { image: im, comment: '', caption: im.caption || '', keep: true };
    } else if (sel[k]) {
      if (sel[k].existing) sel[k].keep = false; else delete sel[k];
    }
    const tile = $(`[data-k="${CSS.escape(k)}"]`);
    if (tile) tile.classList.toggle('on', !!(sel[k] && sel[k].keep !== false));
    remember(); renderSide();
  }

  function renderGrid(meta) {
    pane.innerHTML = `<div class="bar"><strong>${esc(meta.title)}</strong><span class="muted" data-progress>${esc(meta.note || '')}</span>
        <span class="grow"></span><span class="muted small">Click to select · Shift-click for a range · double-click or 🔍 to enlarge</span></div>
      <div class="pick-grid" data-grid>${images.map((im, i) => tile(im, i)).join('')}</div>`;
    bindTiles();
  }
  const tile = (im, i) => `<div class="pick ${sel[im.image_key] && sel[im.image_key].keep !== false ? 'on' : ''} ${inStory[im.image_key] ? 'had' : ''}" data-k="${esc(im.image_key)}" data-i="${i}">
      <img src="${esc(thumbOf(im))}" alt="" loading="lazy">
      <span class="check">✓</span>${im.is_video ? '<span class="vid">video</span>' : ''}${inStory[im.image_key] ? '<span class="instory">in story</span>' : ''}
      <button class="zoom" data-zoom title="Enlarge">🔍</button>
      <span class="name">${esc(im.filename)}</span></div>`;
  function bindTiles() {
    $$('.pick', pane).forEach(t => {
      if (t.dataset.bound) return; t.dataset.bound = 1;
      t.onclick = e => {
        const i = +t.dataset.i;
        if (e.target.closest('[data-zoom]')) return lightbox(i);
        if (e.shiftKey && lastClicked >= 0) {
          const [a, b] = [Math.min(lastClicked, i), Math.max(lastClicked, i)];
          const on = !(sel[images[i].image_key] && sel[images[i].image_key].keep !== false);
          for (let j = a; j <= b; j++) toggle(images[j].image_key, on);
        } else toggle(t.dataset.k);
        lastClicked = i;
      };
      t.ondblclick = () => lightbox(+t.dataset.i);
    });
  }

  async function open(target) {
    target = (target ?? $('[data-target]').value).trim();
    if (!target) return;
    const token = ++loadToken;
    history.replaceState(null, '', `#story/${slug}/smugmug?target=${encodeURIComponent(target)}`);
    pane.innerHTML = '<p class="muted">Looking it up on SmugMug…</p>';
    try {
      let page = await get('/smugmug/browse?target=' + encodeURIComponent(target));
      if (token !== loadToken) return;
      if (page.kind === 'folder') {
        pane.innerHTML = `<h2>${esc(page.folder.name || 'Folder')}</h2>
          ${page.folders.length ? `<p class="muted">Folders: ${page.folders.map(f => `<a href="#" data-go="${esc(f.web)}">${esc(f.name)}</a>`).join(' · ')}</p>` : ''}
          <div class="album-list">${page.albums.map(a => `<a href="#" class="album" data-go="${esc(a.web || a.album_key)}"><strong>${esc(a.name)}</strong><span class="muted">${a.images ?? '?'} photos</span></a>`).join('') || '<p class="empty">No albums here.</p>'}</div>`;
        $$('[data-go]', pane).forEach(a => a.onclick = e => { e.preventDefault(); $('[data-target]').value = a.dataset.go; open(); });
        return;
      }
      images = page.images;
      const title = page.kind === 'album' ? `${page.album.name} — ${page.total} photo${page.total === 1 ? '' : 's'}` : 'One photo';
      renderGrid({ title, note: page.next_start ? `loaded ${images.length} of ${page.total}…` : '' });
      while (page.next_start && token === loadToken) {
        page = await get(`/smugmug/browse?target=${encodeURIComponent(target)}&start=${page.next_start}`);
        if (token !== loadToken) return;
        const from = images.length;
        images = images.concat(page.images);
        $('[data-grid]', pane).insertAdjacentHTML('beforeend', page.images.map((im, j) => tile(im, from + j)).join(''));
        bindTiles();
        $('[data-progress]', pane).textContent = page.next_start ? `loaded ${images.length} of ${page.total}…` : '';
      }
    } catch (e) {
      if (token === loadToken) pane.innerHTML = `<div class="list problems">${esc(e.message)}</div>`;
    }
  }

  // lightbox with arrow keys
  const lb = $('[data-lightbox]');
  let lbIndex = 0;
  function lightbox(i) {
    lbIndex = (i + images.length) % images.length;
    const im = images[lbIndex];
    $('[data-lb-img]').src = im.xlarge || im.large || im.medium;
    const on = sel[im.image_key] && sel[im.image_key].keep !== false;
    $('[data-lb-meta]').innerHTML = `<strong>${esc(im.filename)}</strong> ${im.date ? '· ' + esc(String(im.date).slice(0, 10)) : ''} ${im.caption ? '· ' + esc(im.caption) : ''}
      ${im.is_video ? '· <em>video</em>' : ''} · ${lbIndex + 1}/${images.length}
      <button data-lb-sel class="${on ? 'primary' : ''}">${on ? 'Selected ✓' : 'Select (space)'}</button>`;
    $('[data-lb-sel]').onclick = () => { toggle(im.image_key); lightbox(lbIndex); };
    lb.hidden = false;
  }
  $('[data-lb-close]').onclick = () => (lb.hidden = true);
  $('[data-lb-prev]').onclick = () => lightbox(lbIndex - 1);
  $('[data-lb-next]').onclick = () => lightbox(lbIndex + 1);
  lb.onclick = e => { if (e.target === lb) lb.hidden = true; };
  document.onkeydown = e => {
    if (lb.hidden || !location.hash.startsWith('#story/')) return;
    if (e.key === 'Escape') lb.hidden = true;
    else if (e.key === 'ArrowLeft') lightbox(lbIndex - 1);
    else if (e.key === 'ArrowRight') lightbox(lbIndex + 1);
    else if (e.key === ' ') { e.preventDefault(); toggle(images[lbIndex].image_key); lightbox(lbIndex); }
  };

  $('[data-open]').onclick = () => open();
  $('[data-target]').onkeydown = e => { if (e.key === 'Enter') open(); };
  $('[data-all]').onclick = () => images.forEach(im => toggle(im.image_key, true));
  $('[data-none]').onclick = () => images.forEach(im => toggle(im.image_key, false));
  $('[data-apply]').onclick = () => guard(async () => {
    const btn = $('[data-apply]');
    btn.disabled = true; btn.textContent = 'Saving…';
    const picks = Object.entries(sel).map(([k, x]) => ({
      image_key: k, album_key: x.image.album_key || null, caption: x.caption ?? null,
      comment: x.comment || '', remove: x.keep === false,
    })).filter(p => !p.remove || inStory[p.image_key]);
    const r = await api('POST', base + '/smugmug-pick', { picks });
    sessionStorage.removeItem(memKey);
    toast(`Added ${r.added.length}, updated ${r.updated.length}, removed ${r.removed.length}${r.failed.length ? `, ${r.failed.length} failed` : ''}`, r.failed.length > 0);
    location.hash = '#story/' + slug;
  }).finally(() => updateApply());

  renderSide();
  if (initialTarget) open(initialTarget);
}

// ---------------------------------------------------------------- routing

// ---------------------------------------------------------------- sites: setup and switching

const NO_SITE = 'no site configured';
const isNoSite = e => String(e && e.message || '').startsWith(NO_SITE);

/** The Sites page: the first-run setup (open or create a site), switching, and publish target. */
async function viewSites() {
  const { showing, config } = await get('/sites');
  const site = showing ? await get('/site').catch(() => null) : null;
  const sites = config.sites || [];
  main.innerHTML = `
    <h1>${showing ? 'Sites' : 'Set up gitwebsite'}</h1>
    ${showing ? '' : `<p class="lede">gitwebsite builds a website from AsciiDoc files in a folder (usually a git repository) and publishes it to GitHub Pages.
      Open a site you already have, or create a new one.</p>`}
    ${sites.length ? `<div class="list">${sites.map(s => `
      <div class="row-item"><div><strong>${esc(s.name)}</strong>${s.path === showing ? ' <span class="pill">open</span>' : ''}
        <div class="muted small mono">${esc(s.path)}${s.exists ? '' : ' — missing'}</div></div>
        ${s.path === showing || !s.exists ? '' : `<button data-use="${esc(s.path)}">Switch to it</button>`}</div>`).join('')}</div>` : ''}
    <div class="cols">
      <form class="panel" data-open-site>
        <h2>Open an existing site</h2>
        <p class="muted small">A folder holding <code>site.yaml</code>, e.g. <code>~/sites/my-site</code>.</p>
        <label>Folder<input name="path" required placeholder="~/sites/my-site"></label>
        <button class="primary">Open</button>
      </form>
      <form class="panel" data-new-site>
        <h2>Create a new site</h2>
        <label>Folder (new or empty)<input name="path" required placeholder="~/sites/my-site"></label>
        <label>Title<input name="title" required placeholder="My notes"></label>
        <label>Author<input name="author" required placeholder="Your name"></label>
        <label>Tagline<input name="tagline" placeholder="One line about the site (optional)"></label>
        <label>GitHub Pages repository<input name="pages_remote" placeholder="you/you.github.io (optional)"></label>
        <label>Address<input name="base_url" placeholder="https://you.github.io (worked out from the repository if blank)"></label>
        <label style="flex-direction:row;align-items:center;gap:.5rem"><input type="checkbox" name="repo" checked> Make the folder a git repository</label>
        <button class="primary">Create</button>
      </form>
    </div>
    ${site ? `<form class="panel" data-publish-target>
      <h2>Publishing for “${esc(site.title)}”</h2>
      <p class="muted small">Where <em>Build &amp; Publish</em> pushes the built site. Turn on GitHub Pages for this branch in the repository's settings.</p>
      <label>GitHub Pages repository (owner/name)<input name="remote" value="${esc(site.publish.remote || '')}"></label>
      <label>Branch<input name="branch" value="${esc(site.publish.branch || 'main')}"></label>
      <label>Local checkout<input name="pages_repo" value="${esc(site.publish.pages_repo || '')}"></label>
      <button>Save</button>
    </form>` : ''}`;
  const after = async () => { await showSiteHeader(); await refreshSiteSwitch(); location.hash = '#content'; };
  $$('[data-use]').forEach(b => b.onclick = () => guard(async () => {
    toast('Opening…'); await api('POST', '/sites/use', { path: b.dataset.use }); await after();
  }));
  $('[data-open-site]').onsubmit = e => { e.preventDefault(); guard(async () => {
    toast('Opening and building the preview…');
    await api('POST', '/sites/open', { path: e.target.elements.path.value }); await after();
  }); };
  $('[data-new-site]').onsubmit = e => { e.preventDefault(); guard(async () => {
    const f = e.target.elements;
    const body = { path: f.path.value, title: f.title.value, author: f.author.value, tagline: f.tagline.value,
      pages_remote: f.pages_remote.value.trim(), base_url: f.base_url.value.trim(), repo: f.repo.checked, copy_theme: true };
    toast('Creating the site…');
    const r = await api('POST', '/sites/init', body);
    toast(`Created ${r.created}`); await after();
  }); };
  const pt = $('[data-publish-target]');
  if (pt) pt.onsubmit = e => { e.preventDefault(); guard(async () => {
    const f = e.target.elements;
    await api('PATCH', '/site/publish', { remote: f.remote.value.trim(), branch: f.branch.value.trim(), pages_repo: f.pages_repo.value.trim() });
    toast('Saved');
  }); };
}

/** The site switcher at the foot of the sidebar. */
async function refreshSiteSwitch() {
  const box = $('[data-site-switch]');
  if (!box) return;
  const { showing, config } = await get('/sites').catch(() => ({ config: { sites: [] } }));
  const sites = (config.sites || []).filter(s => s.exists);
  box.innerHTML = sites.length > 1 || !showing
    ? `<select aria-label="Site">${showing ? '' : '<option value="">Choose a site…</option>'}${sites.map(s =>
        `<option value="${esc(s.path)}" ${s.path === showing ? 'selected' : ''}>${esc(s.name)}</option>`).join('')}<option value="#sites">Other sites…</option></select>`
    : '';
  const sel = $('select', box);
  if (sel) sel.onchange = () => guard(async () => {
    if (sel.value === '#sites') { location.hash = '#sites'; return; }
    if (!sel.value) return;
    toast('Switching…');
    await api('POST', '/sites/use', { path: sel.value });
    await showSiteHeader();
    route();
  });
  return showing;
}

async function showSiteHeader() {
  try {
    const s = await get('/site');
    $('[data-site-title]').textContent = s.title;
    $('[data-site-url]').textContent = s.base_url;
    document.title = 'gitwebsite · ' + s.title;
  } catch {
    $('[data-site-title]').textContent = 'No site yet';
    $('[data-site-url]').textContent = '';
    document.title = 'gitwebsite';
  }
}

// ---------------------------------------------------------------- companion

/** Companion: the local gateway the site's tools use for relays, plus the tool windows. Shows
 *  status, pairing requests, paired origins (revocable), relay toggles and allow-lists, supplier
 *  secrets (present or absent — values are write-only) and the audit log. */
async function viewCompanion() {
  const st = await get('/companion');
  const s = st.settings || {};
  const relays = s.relays || {};
  const lines = a => (a || []).join('\n');
  const yes = b => b ? '<span class="pill">yes</span>' : '<span class="pill bad">no</span>';
  const kube = st.kube || {};
  const audit = st.running ? await get('/companion/audit?n=100').catch(() => ({ entries: [] })) : { entries: [] };
  main.innerHTML = `
    <h1>Companion</h1>
    <p class="sub">A local gateway the site's tools use for what a browser can't do alone — feeds without CORS, TCP (Postgres, Redis, Mode S), Docker, Kubernetes, git clone, devices on your LAN (HTTP and WebSocket), S3 storage.
      ${st.running ? `Running on <span class="mono">${esc(st.url)}</span>.` : '<b>Not running</b> (disabled, or another copy holds the port).'}
      Tool windows: ${st.view === 'window' ? 'native windows' : 'the default browser'}. Settings: <span class="mono">${esc(st.settings_file || '')}</span></p>
    ${(st.pending || []).length ? `<div class="panel"><h2>Waiting for approval</h2>${st.pending.map(p => `
      <div class="row-item"><b>${esc(p.tool || 'a page')}</b> at <span class="mono">${esc(p.origin || '(no origin)')}</span> wants ${esc(p.scopes.join(', '))} — code <b class="mono">${esc(p.code)}</b>
        <button class="primary" data-allow="${esc(p.id)}">Allow</button> <button class="danger" data-deny="${esc(p.id)}">Deny</button></div>`).join('')}</div>` : ''}
    ${(st.approvals || []).length ? `<div class="panel"><h2>Waiting for approval: first use</h2>${st.approvals.map(a => `
      <div class="row-item"><b>${esc(a.tool || 'a page')}</b> at <span class="mono">${esc(a.origin || '(no origin)')}</span> wants
        ${a.kind === 'lan' ? `the LAN device <b class="mono">${esc(a.target)}</b>` : a.kind === 'tcp' ? `TCP to <b class="mono">${esc(a.target)}</b>` : `to delete <b class="mono">s3://${esc(a.target)}</b>`}
        <span class="small muted mono">${esc(a.detail)}</span> — code <b class="mono">${esc(a.code)}</b>
        <button class="primary" data-approve="${esc(a.id)}">${a.kind === 's3-delete' ? 'Delete' : 'Allow'}</button> <button class="danger" data-disapprove="${esc(a.id)}">Deny</button></div>`).join('')}
      <p class="small muted">Allowing a LAN device or TCP target adds it to its allow-list below.</p></div>` : ''}
    <p class="small muted">Chrome asks before an https page may reach this computer's 127.0.0.1 (Local Network Access): when a tool on the site first connects, allow “access other apps and services on this device” in Chrome's prompt, or the tool will report that the app isn't running.</p>
    <div class="panel"><h2>Paired</h2>
      ${(st.paired || []).length ? `<table><tr><th>Origin</th><th>Tool</th><th>Scopes</th><th>Last used</th><th></th></tr>${st.paired.map(t => `
        <tr><td class="mono">${esc(t.origin || '(no origin)')}</td><td>${esc(t.tool)}</td><td>${esc((t.scopes || []).join(', '))}</td><td>${esc(t.last_used || '—')}</td>
        <td><button class="danger" data-revoke="${esc(t.id)}">Revoke</button></td></tr>`).join('')}</table>` : '<p class="muted">Nothing paired yet. Pages from this app\'s preview pair automatically; others ask here and in a dialog.</p>'}
    </div>
    <div class="panel"><h2>Relays</h2>
      <div class="bar">${['http', 'tcp', 'docker', 'kube', 'git', 'lan', 'ws', 's3'].map(r => `<label class="inline"><input type="checkbox" data-relay="${r}" ${relays[r] ? 'checked' : ''}> ${r}</label>`).join(' ')}</div>
      <label>TCP allow-list (host:port, one per line; a paired page's first connection to a new LAN target asks you) <textarea rows="3" data-set="tcp_allow">${esc(lines(s.tcp_allow))}</textarea></label>
      <label>LAN hosts for relay/lan and LAN WebSockets (host = any port, or host:port; IPs, names, .local — LAN addresses only; a paired page's first request to a new one asks you) <textarea rows="3" data-set="lan_hosts" placeholder="homeassistant.local&#10;192.168.1.20:8123">${esc(lines(s.lan_hosts))}</textarea></label>
      <label>Public wss:// hosts for relay/ws (host or host:port; never asked for, only added here) <textarea rows="2" data-set="ws_allow" placeholder="mqtt.meshtastic.org">${esc(lines(s.ws_allow))}</textarea></label>
      <label>Extra page origins allowed (one per line; the site and http://127.0.0.1:* / localhost:* always are) <textarea rows="2" data-set="extra_origins">${esc(lines(s.extra_origins))}</textarea></label>
      <label>ADS-B receiver (relay/http/receiver) <input data-set="receiver" value="${esc(s.receiver)}" placeholder="http://piaware.local:8080"></label>
      <label>Kafka REST proxy (relay/http/kafka-rest) <input data-set="kafka_rest" value="${esc(s.kafka_rest)}" placeholder="http://localhost:8082"></label>
      <label>Redpanda HTTP Proxy (relay/http/redpanda-proxy) <input data-set="redpanda_proxy" value="${esc(s.redpanda_proxy)}" placeholder="http://localhost:8082"></label>
      <label>Redpanda Admin API (relay/http/redpanda-admin, GET only) <input data-set="redpanda_admin" value="${esc(s.redpanda_admin)}" placeholder="http://localhost:9644"></label>
      <label>Extra git hosts (one per line) <textarea rows="2" data-set="git_hosts">${esc(lines(s.git_hosts))}</textarea></label>
      <label class="inline"><input type="checkbox" data-flag="git_allow_push" ${s.git_allow_push ? 'checked' : ''}> Allow git push through the relay</label>
      <label class="inline"><input type="checkbox" data-flag="s3_confirm_delete" ${s.s3_confirm_delete !== false ? 'checked' : ''}> Ask before each S3 delete</label>
      <label>Docker socket (empty: auto-detect) <input data-set="docker_host" value="${esc(s.docker_host)}" placeholder="unix:///Users/you/.rd/docker.sock"></label>
      <label>kubeconfig (empty: $KUBECONFIG or ~/.kube/config; read, never written) <input data-set="kubeconfig" value="${esc(s.kubeconfig)}"></label>
      <button class="primary" data-save>Save settings</button>
      <p class="small muted">Docker: ${st.docker ? `<span class="mono">${esc(st.docker)}</span>` : 'no socket found'}.
        Kubernetes: ${kube.ok ? `${(kube.contexts || []).length} context(s) — ${(kube.contexts || []).map(c => esc(c.name)).join(', ')}` : esc(kube.error || 'none')}.
        HTTP upstreams: ${esc((st.upstreams || []).join(', '))}.</p>
    </div>
    <div class="panel"><h2>S3 profiles</h2>
      <p class="small muted">S3-compatible storage for <span class="mono">relay/s3/&lt;profile&gt;/…</span> (AWS, MinIO, R2, …). The companion signs each request; the access key and secret are kept with the other secrets and never shown or sent to a page.</p>
      ${(st.s3 || []).length ? `<table><tr><th>Profile</th><th>Endpoint</th><th>Region</th><th>Style</th><th>Credentials</th><th></th></tr>${st.s3.map(p => `
        <tr><td class="mono">${esc(p.name)}</td><td class="mono small">${esc(p.endpoint)}</td><td>${esc(p.region)}</td><td>${p.path_style ? 'path' : 'virtual-host'}${p.insecure ? ', self-signed OK' : ''}</td><td>${yes(p.credentials)}</td>
        <td><button data-s3cred="${esc(p.name)}">Set credentials…</button> <button class="danger" data-s3rm="${esc(p.name)}">Remove</button></td></tr>`).join('')}</table>` : '<p class="muted">No profiles.</p>'}
      <button data-s3add>Add profile…</button>
    </div>
    <div class="panel"><h2>Supplier API secrets</h2>
      <p class="small muted">${String(st.secrets_store || '').startsWith('file:') ? `Kept in <span class="mono">${esc(String(st.secrets_store).slice(5))}</span> (a non-default config directory never uses the Keychain)` : 'Kept in the macOS Keychain'} and added to parts-supplier requests by the companion; pages never see them. Values are never shown.</p>
      <table><tr><th>Secret</th><th>Present</th><th></th></tr>${Object.entries(st.secrets || {}).map(([k, v]) => `
        <tr><td class="mono">${esc(k)}</td><td>${yes(v)}</td><td><button data-secret="${esc(k)}">Set…</button> ${v ? `<button class="danger" data-unsecret="${esc(k)}">Remove</button>` : ''}</td></tr>`).join('')}</table>
    </div>
    <div class="panel"><h2>Audit log</h2>
      ${audit.entries.length ? `<table><tr><th>When</th><th>Origin</th><th>Tool</th><th>Request</th><th>Status</th></tr>${audit.entries.map(e => `
        <tr><td class="small">${esc(e.at)}</td><td class="mono small">${esc(e.origin || '—')}</td><td>${esc(e.tool || '')}</td><td class="mono small">${e.event ? auditEvent(e) : `${esc(e.method)} ${esc(e.path)}`}${e.note ? ` <span class="muted">(${esc(e.note)})</span>` : ''}</td><td>${e.event ? esc(e.decision || e.action || '') : esc(e.status)}${e.close ? ` → close ${esc(e.close)}` : ''}</td></tr>`).join('')}</table>` : '<p class="muted">No requests yet.</p>'}
    </div>`;
  const refresh = () => viewCompanion();
  $$('[data-allow]').forEach(b => b.onclick = () => guard(async () => { await api('POST', '/companion/pair/' + b.dataset.allow, { allow: true }); toast('Allowed'); refresh(); }));
  $$('[data-deny]').forEach(b => b.onclick = () => guard(async () => { await api('POST', '/companion/pair/' + b.dataset.deny, { allow: false }); toast('Denied'); refresh(); }));
  $$('[data-revoke]').forEach(b => b.onclick = () => guard(async () => { await api('DELETE', '/companion/tokens/' + encodeURIComponent(b.dataset.revoke)); toast('Revoked'); refresh(); }));
  $$('[data-approve]').forEach(b => b.onclick = () => guard(async () => { await api('POST', '/companion/approvals/' + b.dataset.approve, { allow: true }); toast('Allowed'); refresh(); }));
  $$('[data-disapprove]').forEach(b => b.onclick = () => guard(async () => { await api('POST', '/companion/approvals/' + b.dataset.disapprove, { allow: false }); toast('Denied'); refresh(); }));
  const s3creds = async name => {
    const v = await form('Credentials for S3 profile ' + name, [
      { type: 'note', text: 'Write-only: stored with the companion secrets, never shown again or sent to a page. Leave a field empty to keep it.' },
      { name: 'access_key_id', label: 'Access key id' }, { name: 'secret_access_key', label: 'Secret access key', type: 'password' }], 'Save');
    if (!v) return false;
    for (const f of ['access_key_id', 'secret_access_key']) if (v[f]) await api('PUT', '/companion/secrets/' + encodeURIComponent(`s3:${name}:${f}`), { value: v[f] });
    return true;
  };
  $$('[data-s3cred]').forEach(b => b.onclick = () => guard(async () => { if (await s3creds(b.dataset.s3cred)) { toast('Saved'); refresh(); } }));
  $$('[data-s3rm]').forEach(b => b.onclick = () => guard(async () => {
    if (!confirm(`Remove the S3 profile ${b.dataset.s3rm} and its credentials?`)) return;
    const profiles = { ...(s.s3_profiles || {}) };
    delete profiles[b.dataset.s3rm];
    await api('PUT', '/companion/settings', { s3_profiles: profiles });
    for (const f of ['access_key_id', 'secret_access_key']) await api('DELETE', '/companion/secrets/' + encodeURIComponent(`s3:${b.dataset.s3rm}:${f}`)).catch(() => {});
    toast('Removed'); refresh();
  }));
  $('[data-s3add]').onclick = () => guard(async () => {
    const v = await form('Add an S3 profile', [
      { name: 'name', label: 'Name (letters, digits, - and _)', required: true, placeholder: 'minio' },
      { name: 'endpoint', label: 'Endpoint (scheme://host[:port])', required: true, placeholder: 'http://nas.local:9000 or https://s3.us-east-1.amazonaws.com' },
      { name: 'region', label: 'Region', value: 'us-east-1', required: true },
      { name: 'path_style', type: 'checkbox', label: 'Path-style URLs (MinIO and most self-hosted stores)', value: true },
      { name: 'insecure', type: 'checkbox', label: 'Accept a self-signed certificate (LAN endpoints only)' },
      { name: 'access_key_id', label: 'Access key id' }, { name: 'secret_access_key', label: 'Secret access key (write-only)', type: 'password' }], 'Add');
    if (!v) return;
    const profiles = { ...(s.s3_profiles || {}), [v.name.trim()]: { endpoint: v.endpoint.trim().replace(/\/+$/, ''), region: v.region.trim(), path_style: v.path_style, insecure: v.insecure } };
    await api('PUT', '/companion/settings', { s3_profiles: profiles });
    for (const f of ['access_key_id', 'secret_access_key']) if (v[f]) await api('PUT', '/companion/secrets/' + encodeURIComponent(`s3:${v.name.trim()}:${f}`), { value: v[f] });
    toast('Added'); refresh();
  });
  $$('[data-unsecret]').forEach(b => b.onclick = () => guard(async () => { await api('DELETE', '/companion/secrets/' + b.dataset.unsecret); toast('Removed'); refresh(); }));
  $$('[data-secret]').forEach(b => b.onclick = async () => {
    const v = await form('Set ' + b.dataset.secret, [{ name: 'value', label: 'Value (stored in the Keychain; not shown again)', required: true }], 'Save');
    if (v) guard(async () => { await api('PUT', '/companion/secrets/' + b.dataset.secret, { value: v.value }); toast('Saved'); refresh(); });
  });
  $('[data-save]').onclick = () => guard(async () => {
    const patch = { relays: {} };
    $$('[data-relay]').forEach(c => (patch.relays[c.dataset.relay] = c.checked));
    $$('[data-flag]').forEach(c => (patch[c.dataset.flag] = c.checked));
    $$('[data-set]').forEach(el => {
      const k = el.dataset.set;
      patch[k] = el.tagName === 'TEXTAREA' ? el.value.split('\n').map(x => x.trim()).filter(Boolean) : el.value.trim();
    });
    // Relay bases are scheme://host[:port] only; the page's path is joined on.
    const warn = [];
    for (const k of ['receiver', 'kafka_rest', 'redpanda_proxy', 'redpanda_admin']) {
      const v = patch[k];
      if (!v) continue;
      let u;
      try { u = new URL(v); } catch { throw new Error(`${k}: not a URL — use http://host:port`); }
      if (!/^https?:$/.test(u.protocol)) throw new Error(`${k}: must start with http:// or https://`);
      if (u.search || u.hash || u.username || u.password) throw new Error(`${k}: use scheme://host[:port] only (no query, fragment or user:password@)`);
      if (u.pathname !== '/') warn.push(`${k} has a path (${u.pathname}): bases should be scheme://host[:port]; the companion won't double it if the tool's path starts with it`);
    }
    const r = await api('PUT', '/companion/settings', patch);
    toast((r.restart_needed ? 'Saved — restart the app for the port change' : 'Saved') + (warn.length ? '. Note: ' + warn.join('; ') : ''));
    refresh();
  });
}

/** An audit-log event (approval decided, settings or secret changed) as one line. */
function auditEvent(e) {
  if (e.event === 'approval') return `approval ${esc(e.kind)} <b>${esc(e.target)}</b> by ${esc(e.by || '')}`;
  if (e.event === 'settings') return `settings changed: ${esc((e.keys || []).join(', '))} (${esc(e.by || '')})`;
  if (e.event === 'secret') return `secret ${esc((e.names || []).join(', '))} (${esc(e.by || '')})`;
  return esc(e.event);
}

const routes = { companion: viewCompanion, content: viewContent, stories: viewStories, ideas: viewIdeas, registry: viewRegistry, media: viewMedia, models: viewModels, publish: viewPublish, preview: viewPreview, sites: viewSites };

async function route() {
  const h = location.hash.replace(/^#/, '') || 'content';
  const [section, ...rest] = h.split('/');
  const nav = section === 'edit' ? 'content' : section === 'story' ? 'stories' : section;
  $$('[data-section]').forEach(a => a.classList.toggle('active', a.dataset.section === nav));
  document.onpaste = null;
  document.onkeydown = null;
  if (!(section === 'story' && live && live.slug === rest[0] && !rest[1])) stopLive();
  if (section !== 'models') stopMeshyPoll();
  main.innerHTML = '<p class="muted">Loading…</p>';
  try {
    if (section === 'edit') await viewEditor(decodeURIComponent(rest.join('/')));
    else if (section === 'story') {
      // #story/<slug> or #story/<slug>/smugmug?target=<url>
      const [slug, sub] = rest;
      const [subName, query] = (sub || '').split('?');
      if (subName === 'smugmug') await viewSmugmugPicker(slug, new URLSearchParams(query || '').get('target') || '');
      else await viewStory(slug);
    }
    else if (section === 'models') await viewModels(rest);
    else await (routes[section] || viewContent)();
  } catch (e) {
    if (isNoSite(e) && section !== 'sites') { location.hash = '#sites'; return; }
    main.innerHTML = `<h1>Something went wrong</h1><p class="list problems">${esc(e.message)}</p>`;
  }
}

window.addEventListener('hashchange', route);
// Newly arrived Meshy models show as a count beside Models (checked now and every 10 minutes).
const refreshModelsNav = () => get('/meshy/status').then(markModelsNav).catch(() => {});
refreshModelsNav(); setInterval(refreshModelsNav, 600000);
showSiteHeader();
// With no site yet, start on the setup screen.
refreshSiteSwitch().then(showing => { if (!showing && location.hash !== '#sites') location.hash = '#sites'; else route(); });

// macOS web views auto-capitalise and autocorrect text boxes; every field here holds slugs,
// keys, paths, code, or a confirmation word, so turn that off everywhere.
document.addEventListener('focusin', e => {
  const el = e.target;
  if (el.matches && el.matches('input:not([type=checkbox]):not([type=radio]), textarea')) {
    el.setAttribute('autocapitalize', 'off');
    el.setAttribute('autocorrect', 'off');
    el.spellcheck = false;
  }
});

// macOS dictation (in the desktop app only): a 🎤 button beside every text box focuses it and
// starts Edit ▸ Start Dictation… there. The app installs the native hook behind /api/dictate;
// in a plain browser the server reports it unavailable and no buttons appear.
const MIC_FIELDS = 'textarea, input[type=text], input[type=search], input:not([type])';
function addMics(root = document) {
  for (const el of root.querySelectorAll(MIC_FIELDS)) {
    if (el.dataset.mic || el.readOnly || el.disabled || el.type === 'hidden' || el.closest('.mic-wrap, .no-mic')) continue;
    el.dataset.mic = '1';
    const wrap = document.createElement('span');
    wrap.className = 'mic-wrap' + (el.tagName === 'TEXTAREA' ? ' area' : '');
    const cs = getComputedStyle(el);
    if (cs.display === 'block' || el.tagName === 'TEXTAREA' || cs.width === getComputedStyle(el.parentElement).width) wrap.classList.add('block');
    el.replaceWith(wrap);
    wrap.append(el);
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'mic';
    b.title = 'Dictate into this box (macOS dictation; press the button again or Esc to stop)';
    b.setAttribute('aria-label', 'Dictate');
    b.textContent = '🎤';
    b.addEventListener('mousedown', e => e.preventDefault()); // keep the caret where it is
    b.addEventListener('click', async e => {
      e.preventDefault();
      e.stopPropagation();
      el.focus();
      try { await api('POST', '/dictate'); } catch (err) { toast('Dictation: ' + err.message, true); }
    });
    wrap.append(b);
  }
}
get('/dictate').then(d => {
  if (!d.available) return;
  document.body.classList.add('can-dictate');
  addMics();
  new MutationObserver(muts => {
    for (const m of muts) for (const n of m.addedNodes) if (n.nodeType === 1) addMics(n.matches?.(MIC_FIELDS) ? n.parentElement : n);
  }).observe(document.body, { childList: true, subtree: true });
}).catch(() => {});

// ⌘R / Ctrl+R reloads the window: the desktop app has no View ▸ Reload menu item, and the UI is
// served fresh from the repo (no-store), so a reload picks up a new app.js without a restart.
document.addEventListener('keydown', e => {
  if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'r') {
    e.preventDefault();
    location.reload();
  }
});
