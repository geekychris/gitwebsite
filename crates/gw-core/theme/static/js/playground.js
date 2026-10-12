// Runnable code playgrounds. AsciiDoc `[source.playground,<lang>]` blocks become editors with Run.
//
// site.js loads this file only on pages that have a playground (the /tools/sandbox/ page loads it
// too). Every language except HTML, DuckDB, Rust, and Go runs in its own Web Worker, so Stop can
// terminate a runaway loop; runtimes are fetched from jsDelivr the first time a language is used
// and are cached by the browser afterwards. Rust and Go are compiled by their official online
// playgrounds (the only parts that send code anywhere).
//
//   window.GWPlay.enhanceAll()                       turn every .listingblock.playground into an editor
//   window.GWPlay.mount(el, {lang, code, onChange})  build one editor inside el (used by the sandbox)
//   window.GWPlay.languages                          [{id, label, note}] for pickers
(() => {
  'use strict';

  const CDN = 'https://cdn.jsdelivr.net/npm/';
  const PYODIDE = 'https://cdn.jsdelivr.net/pyodide/v0.26.4/full/';
  const CLANG = CDN + '@yowasp/clang@22.0.0-git20542-10/gen/bundle.js';
  const WASI_SHIM = CDN + '@bjorn3/browser_wasi_shim@0.4.2/dist/index.js';
  const WASMOON = CDN + 'wasmoon@1.16.0/dist/';
  const RUBY_API = CDN + '@ruby/wasm-wasi@2.10.1/dist/browser/+esm';
  const RUBY_WASM = CDN + '@ruby/3.4-wasm-wasi@2.10.1/dist/ruby+stdlib.wasm';
  const SQLJS = CDN + 'sql.js@1.14.2/dist/';
  const DUCKDB = CDN + '@duckdb/duckdb-wasm@1.32.0/+esm';
  const SWIPL = CDN + 'swipl-wasm@8.2.2/dist/swipl/swipl-bundle.js';
  const BIWA = CDN + 'biwascheme@0.8.3/release/biwascheme.mjs';

  // ------------------------------------------------------------------ pure helpers (unit tested)

  /** Split a SQL script into statements, respecting quotes and comments. */
  function splitSql(text) {
    const out = [];
    let cur = '';
    let i = 0;
    while (i < text.length) {
      const c = text[i], d = text[i + 1];
      if (c === '-' && d === '-') { const j = text.indexOf('\n', i); const end = j < 0 ? text.length : j; cur += text.slice(i, end); i = end; continue; }
      if (c === '/' && d === '*') { const j = text.indexOf('*/', i + 2); const end = j < 0 ? text.length : j + 2; cur += text.slice(i, end); i = end; continue; }
      if (c === "'" || c === '"' || c === '`') {
        let j = i + 1;
        while (j < text.length) { if (text[j] === c) { if (text[j + 1] === c) { j += 2; continue; } break; } j++; }
        cur += text.slice(i, j + 1); i = j + 1; continue;
      }
      if (c === ';') { if (stripSqlComments(cur).trim()) out.push(cur.trim()); cur = ''; i++; continue; }
      cur += c; i++;
    }
    if (stripSqlComments(cur).trim()) out.push(cur.trim());
    return out;
  }
  function stripSqlComments(s) { return s.replace(/--[^\n]*/g, '').replace(/\/\*[\s\S]*?\*\//g, ''); }

  /** Separate a Prolog playground into the program and its `?- goal.` queries. */
  function parseProlog(text) {
    const program = [];
    const queries = [];
    let q = null;
    for (const line of text.replace(/\r\n?/g, '\n').split('\n')) {
      if (q === null && /^\s*\?-/.test(line)) q = line.replace(/^\s*\?-\s*/, '');
      else if (q !== null) q += '\n' + line;
      else { program.push(line); continue; }
      program.push(''); // keep line numbers in error messages aligned
      const body = q.replace(/%[^\n]*$/gm, '').trim();
      if (/\.$/.test(body)) {
        queries.push(body.replace(/\.\s*$/, ''));
        q = null;
      }
    }
    if (q !== null && q.trim()) queries.push(q.trim().replace(/\.\s*$/, ''));
    return { program: program.join('\n'), queries };
  }

  /** A Prolog double-quoted string literal for s. */
  function prologString(s) {
    return '"' + s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\t/g, '\\t') + '"';
  }

  const ALIASES = {
    js: 'javascript', javascript: 'javascript', py: 'python', python: 'python', python3: 'python',
    c: 'c', cpp: 'cpp', 'c++': 'cpp', cxx: 'cpp', cc: 'cpp',
    lua: 'lua', rb: 'ruby', ruby: 'ruby', sql: 'sql', sqlite: 'sql', duckdb: 'duckdb',
    prolog: 'prolog', pl: 'prolog', swipl: 'prolog', scheme: 'scheme', scm: 'scheme', lisp: 'scheme',
    basic: 'basic', bas: 'basic', rust: 'rust', rs: 'rust', go: 'go', golang: 'go', html: 'html',
  };
  const langOf = s => ALIASES[String(s || '').toLowerCase()] || null;

  // ------------------------------------------------------------------ worker programs
  // Every worker gets COMMON first: the message loop, output helpers, and input requests.
  const COMMON = `
const send = m => postMessage(m);
const out = (text, stream) => send({ type: 'out', text: String(text), stream: stream || 'stdout' });
const status = text => send({ type: 'status', text });
const progress = (done, total, text) => send({ type: 'progress', done, total, text });
let ready = null, inputResolve = null;
const askInput = prompt => new Promise(r => { inputResolve = r; send({ type: 'input', prompt }); });
self.onmessage = async e => {
  const m = e.data;
  if (m.type === 'input') { const r = inputResolve; inputResolve = null; if (r) r(m.value); return; }
  if (m.type !== 'run') return;
  try {
    if (!ready) ready = init(m).catch(err => { ready = null; throw err; });
    await ready;
    status('');
    const t0 = performance.now();
    const r = await run(m.code, m);
    send({ type: 'done', ok: true, ms: r && r.ms !== undefined ? r.ms : performance.now() - t0 });
  } catch (err) {
    send({ type: 'done', ok: false, error: err && err.message !== undefined ? (err.name && err.name !== 'Error' && !String(err.message).startsWith(err.name) ? err.name + ': ' : '') + err.message : String(err) });
  }
};
`;

  const W = {
    javascript: `
const fmt = a => a.map(x => typeof x === 'string' ? x : (() => { try { return JSON.stringify(x, (k, v) => typeof v === 'bigint' ? v + 'n' : v); } catch { return String(x); } })()).join(' ');
async function init() {
  console.log = console.info = console.debug = (...a) => out(fmt(a) + '\\n');
  console.warn = console.error = (...a) => out(fmt(a) + '\\n', 'stderr');
  console.table = d => out(JSON.stringify(d, null, 2) + '\\n');
}
const AsyncFunction = (async () => {}).constructor;
async function run(code) { await new AsyncFunction(code)(); }
`,
    python: `
async function init() {
  status('Loading Python (Pyodide, about 10 MB the first time)…');
  importScripts('${PYODIDE}pyodide.js');
  self.py = await loadPyodide({ indexURL: '${PYODIDE}' });
}
async function run(code, m) {
  py.setStdout({ batched: s => out(s + '\\n') });
  py.setStderr({ batched: s => out(s + '\\n', 'stderr') });
  const lines = (m.stdin || '').split('\\n'); let i = 0;
  py.setStdin({ stdin: () => (i < lines.length && !(i === lines.length - 1 && lines[i] === '') ? lines[i++] : undefined) });
  const pips = [...code.matchAll(/^\\s*(?:#\\s*pip:|%pip install)\\s+(.+)$/gm)].flatMap(x => x[1].split(/[\\s,]+/)).filter(Boolean);
  if (pips.length) {
    status('Installing ' + pips.join(', ') + '…');
    await py.loadPackage('micropip');
    await py.pyimport('micropip').install(pips);
  }
  await py.loadPackagesFromImports(code, { messageCallback: s => status(s) });
  status('');
  const r = await py.runPythonAsync(code.replace(/^\\s*%pip install.*$/gm, ''));
  if (r !== undefined && r !== null) { out(String(r) + '\\n'); if (r.destroy) r.destroy(); }
}
`,
    clang: `
let clang, shim;
async function init() {
  status('Loading Clang (LLVM compiled to WebAssembly — about 50 MB the first time, cached afterwards)…');
  shim = await import('${WASI_SHIM}');
  clang = (await import('${CLANG}')).runClang;
  await clang(null, {}, { fetchProgress: ({ totalLength, doneLength }) => progress(doneLength, totalLength, 'Downloading Clang') });
}
async function run(code, m) {
  const cpp = m.lang === 'cpp';
  const src = cpp ? 'main.cpp' : 'main.c';
  const dec = new TextDecoder();
  const diag = b => { if (b) out(dec.decode(b, { stream: true }), 'stderr'); };
  status('Compiling…');
  const t0 = performance.now();
  let files;
  try {
    // WASI's libc++ is built without exception support, so C++ is compiled with -fno-exceptions.
    files = await clang([cpp ? 'clang++' : 'clang', '-O2', '-Wall', ...(cpp ? ['-std=c++20', '-fno-exceptions'] : ['-std=c17']), src, '-o', 'main.wasm'],
      { [src]: code }, { stdout: diag, stderr: diag, decodeASCII: false });
  } catch (e) {
    if (e && e.files !== undefined) throw new Error('Compilation failed (see the messages above).');
    throw e;
  }
  send({ type: 'phase', text: 'compiled in ' + Math.round(performance.now() - t0) + ' ms' });
  status('');
  const { WASI, File, OpenFile, ConsoleStdout } = shim;
  const fds = [
    new OpenFile(new File(new TextEncoder().encode(m.stdin || ''))),
    new ConsoleStdout(b => out(dec.decode(b, { stream: true }))),
    new ConsoleStdout(b => out(dec.decode(b, { stream: true }), 'stderr')),
  ];
  const wasi = new WASI(['main'], [], fds);
  const inst = await WebAssembly.instantiate(await WebAssembly.compile(files['main.wasm']), { wasi_snapshot_preview1: wasi.wasiImport });
  const t1 = performance.now();
  const rc = wasi.start(inst);
  const ms = performance.now() - t1;
  if (rc) out('\\n[exit code ' + rc + ']\\n', 'stderr');
  return { ms };
}
`,
    lua: `
async function init() {
  status('Loading Lua 5.4 (wasmoon, about 0.4 MB)…');
  importScripts('${WASMOON}index.js');
  self.factory = new wasmoon.LuaFactory('${WASMOON}glue.wasm');
}
async function run(code) {
  const lua = await factory.createEngine();
  try {
    lua.global.set('__pg_write', s => out(s));
    lua.global.set('__pg_err', s => out(s, 'stderr'));
    await lua.doString(\`
      local w, tostring, select = __pg_write, tostring, select
      function print(...)
        local t = {}
        for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
        w(table.concat(t, '\\\\t') .. '\\\\n')
      end
      io.write = function(...)
        for i = 1, select('#', ...) do w(tostring((select(i, ...)))) end
        return io
      end
      io.stderr = { write = function(self, ...) for i = 1, select('#', ...) do __pg_err(tostring((select(i, ...)))) end end }
    \`);
    const r = await lua.doString(code);
    if (r !== undefined && r !== null) out(String(r) + '\\n');
  } finally {
    lua.global.close();
  }
}
`,
    ruby: `
let vm;
async function init() {
  status('Loading Ruby 3.4 (ruby.wasm, about 8 MB the first time)…');
  // ruby.wasm prints each write() through console.log/console.warn, newlines included.
  console.log = (...a) => out(a.join(' '));
  console.warn = console.error = (...a) => out(a.join(' '), 'stderr');
  const { DefaultRubyVM } = await import('${RUBY_API}');
  const mod = await WebAssembly.compileStreaming(fetch('${RUBY_WASM}'));
  vm = (await DefaultRubyVM(mod, { consolePrint: true })).vm;
}
async function run(code) {
  try {
    const r = vm.eval(code);
    vm.eval('$stdout.flush; $stderr.flush');
    if (r && r.call('nil?').toString() === 'false') out('=> ' + r.call('inspect').toString() + '\\n');
  } catch (e) {
    try { vm.eval('$stdout.flush; $stderr.flush'); } catch {}
    throw new Error(String(e.message || e).replace(/^RbError: /, ''));
  }
}
`,
    sql: `
let db;
async function init() {
  status('Loading SQLite (sql.js, about 0.7 MB)…');
  importScripts('${SQLJS}sql-wasm.js');
  const SQL = await initSqlJs({ locateFile: f => '${SQLJS}' + f });
  db = new SQL.Database();
}
async function run(code) {
  const before = db.getRowsModified ? db.getRowsModified() : 0;
  const res = db.exec(code);
  for (const r of res) send({ type: 'table', columns: r.columns, rows: r.values });
  if (!res.length) out('OK — ' + db.getRowsModified() + ' row(s) changed by the last statement.\\n');
}
`,
    prolog: `
const HELPERS = \`
pg_query(S) :-
    format("?- ~s.~n", [S]),
    catch(term_string(G, S, [variable_names(Vs)]), E, (print_message(error, E), fail)), !,
    (   Vs == []
    ->  ( catch(G, E2, (print_message(error, E2), fail)) -> writeln(true) ; writeln(false) )
    ;   findall(Vs, limit(25, catch(G, E3, (print_message(error, E3), fail))), Sols),
        (   Sols == [] -> writeln(false)
        ;   forall(member(B, Sols), pg_bindings(B)),
            length(Sols, Len), ( Len >= 25 -> writeln('... (stopped after 25 answers)') ; true )
        )
    ),
    nl, flush_output.
pg_query(_) :- nl, flush_output.
pg_bindings(B) :-
    findall(Str, (member(N=V, B), \\\\+ sub_atom(N, 0, 1, _, '_'), \\\\+ var(V), format(string(Str), "~w = ~q", [N, V])), Ps),
    (   Ps == [] -> writeln(true) ; atomic_list_concat(Ps, ', ', Line), writeln(Line) ).
\`;
let pl;
async function init() {
  status('Loading SWI-Prolog (about 6 MB the first time)…');
  importScripts('${SWIPL}');
  pl = await SWIPL({ arguments: ['-q'], print: s => out(s + '\\n'), printErr: s => out(s + '\\n', 'stderr') });
  pl.FS.writeFile('/pg_helpers.pl', HELPERS);
  pl.prolog.call("consult('/pg_helpers.pl')");
}
async function run(code, m) {
  pl.FS.writeFile('/playground.pl', m.program);
  pl.prolog.call("consult('/playground.pl')");
  for (const q of m.queries) pl.prolog.call('pg_query(' + q + ')');
  if (!m.queries.length) out('Program loaded. Add queries as lines starting with ?- to run them.\\n');
  pl.prolog.call('flush_output');
}
`,
    scheme: `
let BiwaScheme;
async function init() {
  status('Loading BiwaScheme (about 0.3 MB)…');
  // The browser build looks for a DOM at load time; a worker has none, so give it an empty one.
  self.window = self;
  self.document = { querySelector: () => null, querySelectorAll: () => [], getElementById: () => null,
    getElementsByTagName: () => [], createElement: () => ({ style: {}, setAttribute() {}, appendChild() {} }),
    addEventListener() {}, readyState: 'complete' };
  BiwaScheme = (await import('${BIWA}')).default;
  BiwaScheme.Port.current_output = new BiwaScheme.Port.CustomOutput(s => out(s));
  BiwaScheme.Port.current_error = new BiwaScheme.Port.CustomOutput(s => out(s, 'stderr'));
}
async function run(code) {
  const value = await new Promise((resolve, reject) => {
    const intp = new BiwaScheme.Interpreter(e => reject(new Error(e.message || String(e))));
    try {
      const r = intp.evaluate(code, v => resolve(v));
      if (!(r instanceof BiwaScheme.Pause)) resolve(r);
    } catch (e) { reject(e); }
  });
  if (value !== undefined && value !== BiwaScheme.undef && value !== null) {
    const s = BiwaScheme.to_write(value);
    if (s && s !== '#<undef>') out('=> ' + s + '\\n');
  }
}
`,
    basic: `
async function init() { importScripts(BASIC_URL); }
async function run(code) {
  await GWBasic.run(code, {
    print: s => out(s),
    input: prompt => askInput(prompt),
    cls: () => send({ type: 'cls' }),
  });
}
`,
  };

  // ------------------------------------------------------------------ language table
  const LANGS = {
    javascript: { label: 'JavaScript', worker: 'javascript', note: 'Runs in a Web Worker' },
    python: { label: 'Python', worker: 'python', stdin: true, note: 'Pyodide · ~10 MB first run' },
    c: { label: 'C', worker: 'clang', module: true, stdin: true, note: 'Clang → WebAssembly · ~50 MB first run' },
    cpp: { label: 'C++', worker: 'clang', module: true, stdin: true, note: 'Clang → WebAssembly · ~50 MB first run' },
    lua: { label: 'Lua', worker: 'lua', note: 'Lua 5.4 · wasmoon' },
    ruby: { label: 'Ruby', worker: 'ruby', module: true, note: 'ruby.wasm · ~8 MB first run' },
    sql: { label: 'SQLite', worker: 'sql', note: 'sql.js · one database per page' },
    duckdb: { label: 'DuckDB', main: 'duckdb', csv: true, note: 'duckdb-wasm · ~35 MB first run' },
    prolog: { label: 'Prolog', worker: 'prolog', note: 'SWI-Prolog · ~6 MB first run' },
    scheme: { label: 'Scheme', worker: 'scheme', module: true, note: 'BiwaScheme' },
    basic: { label: 'BASIC', worker: 'basic', note: 'Classic line-numbered BASIC' },
    rust: { label: 'Rust', main: 'rust', network: true, note: 'Compiled on play.rust-lang.org' },
    go: { label: 'Go', main: 'go', network: true, note: 'Compiled on go.dev' },
    html: { label: 'HTML', html: true, note: 'Rendered in a sandboxed frame' },
  };

  // ------------------------------------------------------------------ runners

  class WorkerRunner {
    constructor(name, module) { this.name = name; this.module = module; this.w = null; this.pending = null; this.queue = Promise.resolve(); }
    ensure() {
      if (this.w) return;
      const basicUrl = new URL('/js/playground-basic.js', location.href).href;
      const src = `const BASIC_URL = ${JSON.stringify(basicUrl)};\n` + COMMON + W[this.name];
      const url = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }));
      this.w = new Worker(url, this.module ? { type: 'module' } : undefined);
      setTimeout(() => URL.revokeObjectURL(url), 60000);
      this.w.onmessage = e => this.onMessage(e.data);
      this.w.onerror = e => { e.preventDefault(); this.finish({ ok: false, error: e.message || 'The language runtime failed to start.' }); };
    }
    run(msg, h) {
      const job = this.queue.then(() => new Promise(resolve => {
        this.ensure();
        this.pending = { h, resolve };
        this.w.postMessage(Object.assign({ type: 'run' }, msg));
      }));
      this.queue = job.catch(() => {});
      return job;
    }
    onMessage(m) {
      const p = this.pending;
      if (!p) return;
      const h = p.h;
      switch (m.type) {
        case 'out': h.out(m.text, m.stream); break;
        case 'status': h.status(m.text); break;
        case 'progress': h.progress(m.done, m.total, m.text); break;
        case 'phase': h.phase(m.text); break;
        case 'table': h.table(m.columns, m.rows); break;
        case 'cls': h.cls(); break;
        case 'input': Promise.resolve(h.input(m.prompt)).then(v => this.w && this.w.postMessage({ type: 'input', value: v })); break;
        case 'done': this.finish(m); break;
      }
    }
    finish(m) { const p = this.pending; this.pending = null; if (p) p.resolve(m); }
    stop() {
      if (this.w) { this.w.terminate(); this.w = null; }
      this.finish({ ok: false, stopped: true });
    }
  }

  /** DuckDB manages its own worker; we drive it from the page. */
  class DuckRunner {
    constructor() { this.db = null; this.conn = null; this.loading = null; this.pending = null; }
    async load(h) {
      h.status('Loading DuckDB (duckdb-wasm, about 35 MB the first time)…');
      const duckdb = await import(DUCKDB);
      const bundle = await duckdb.selectBundle(duckdb.getJsDelivrBundles());
      const url = URL.createObjectURL(new Blob([`importScripts(${JSON.stringify(bundle.mainWorker)});`], { type: 'text/javascript' }));
      this.worker = new Worker(url);
      this.db = new duckdb.AsyncDuckDB(new duckdb.VoidLogger(), this.worker);
      await this.db.instantiate(bundle.mainModule, bundle.pthreadWorker);
      await this.db.open({ query: { castDecimalToDouble: true } });
      URL.revokeObjectURL(url);
      this.conn = await this.db.connect();
    }
    ensure(h) {
      if (!this.loading) this.loading = this.load(h).catch(e => { this.loading = null; throw e; });
      return this.loading;
    }
    async run(msg, h) {
      const token = (this.pending = {});
      try {
        await this.ensure(h);
        h.status('');
        const t0 = performance.now();
        for (const stmt of splitSql(msg.code)) {
          if (this.pending !== token) return { ok: false, stopped: true };
          const t = await this.conn.query(stmt);
          const cols = t.schema.fields;
          const isQuery = /^\s*(select|with|from|values|show|describe|summarize|pivot|unpivot|explain|table|pragma|call)\b/i.test(stripSqlComments(stmt));
          if (cols.length && (isQuery || !(cols.length === 1 && cols[0].name === 'Count'))) {
            const rows = [];
            const n = Math.min(t.numRows, 500);
            for (let i = 0; i < n; i++) {
              const r = t.get(i);
              rows.push(cols.map(f => cell(r[f.name], f)));
            }
            h.table(cols.map(f => f.name), rows, t.numRows);
          } else {
            const cnt = cols.length === 1 && t.numRows ? Number(t.get(0)[cols[0].name]) : null;
            h.out(`OK${cnt !== null && !isNaN(cnt) ? ` — ${cnt} row(s)` : ''}\n`);
          }
        }
        return { ok: true, ms: performance.now() - t0 };
      } catch (e) {
        if (this.pending !== token) return { ok: false, stopped: true };
        return { ok: false, error: e.message || String(e) };
      } finally {
        if (this.pending === token) this.pending = null;
      }
    }
    async loadFile(file, h) {
      await this.ensure(h);
      h.status('');
      const buf = new Uint8Array(await file.arrayBuffer());
      await this.db.registerFileBuffer(file.name, buf);
      const ext = (file.name.split('.').pop() || '').toLowerCase();
      let table = file.name.replace(/\.[^.]+$/, '').replace(/[^A-Za-z0-9_]+/g, '_').replace(/^(\d)/, '_$1').toLowerCase() || 'data';
      const reader = ext === 'parquet' ? 'read_parquet' : ext === 'json' || ext === 'ndjson' || ext === 'jsonl' ? 'read_json_auto' : 'read_csv_auto';
      await this.conn.query(`CREATE OR REPLACE TABLE "${table}" AS SELECT * FROM ${reader}('${file.name.replace(/'/g, "''")}')`);
      const n = await this.conn.query(`SELECT count(*) AS n FROM "${table}"`);
      const d = await this.conn.query(`DESCRIBE "${table}"`);
      const cols = [];
      for (let i = 0; i < d.numRows; i++) cols.push(`${d.get(i).column_name} ${d.get(i).column_type}`);
      return { table, rows: Number(n.get(0).n), cols };
    }
    stop() {
      this.pending = null;
      if (this.worker) { this.worker.terminate(); this.worker = null; }
      this.db = null; this.conn = null; this.loading = null;
    }
  }
  function cell(v, f) {
    if (v === null || v === undefined) return null;
    const type = String(f.type);
    if (typeof v === 'bigint') return v.toString();
    if (/^(Date|Timestamp)/.test(type) && typeof v === 'number') {
      const iso = new Date(v).toISOString();
      return /^Date/.test(type) ? iso.slice(0, 10) : iso.replace('T', ' ').replace(/\.000Z$|Z$/, '');
    }
    if (v && typeof v === 'object' && typeof v.toJSON === 'function') return JSON.stringify(v.toJSON());
    if (v instanceof Uint8Array) return `[${v.length} bytes]`;
    if (typeof v === 'object') { try { return JSON.stringify(v, (k, x) => (typeof x === 'bigint' ? x.toString() : x)); } catch { return String(v); } }
    return v;
  }

  /** Rust and Go: POST to the official playgrounds. */
  class FetchRunner {
    constructor(kind) { this.kind = kind; this.ac = null; }
    async run(msg, h) {
      this.ac = new AbortController();
      const t0 = performance.now();
      try {
        h.status(this.kind === 'rust' ? 'Compiling on play.rust-lang.org…' : 'Compiling on go.dev…');
        if (this.kind === 'rust') {
          const r = await fetch('https://play.rust-lang.org/execute', {
            method: 'POST', signal: this.ac.signal, headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ channel: 'stable', mode: 'debug', edition: '2024', crateType: 'bin', tests: false, code: msg.code, backtrace: false }),
          }).then(r => { if (!r.ok) throw new Error('play.rust-lang.org answered ' + r.status); return r.json(); });
          h.status('');
          const diag = (r.stderr || '').split('\n').filter(l => !/^\s*(Compiling playground|Finished `|Running `)/.test(l)).join('\n').trim();
          if (diag) h.out(diag + '\n', 'stderr');
          if (r.stdout) h.out(r.stdout);
          if (!r.success) return { ok: false, error: r.exitDetail || 'Build failed' };
        } else {
          const body = new URLSearchParams({ version: '2', body: msg.code, withVet: 'true' });
          const r = await fetch('https://play.golang.org/compile', { method: 'POST', signal: this.ac.signal, body })
            .then(r => { if (!r.ok) throw new Error('go.dev answered ' + r.status); return r.json(); });
          h.status('');
          if (r.Errors) { h.out(r.Errors + '\n', 'stderr'); return { ok: false, error: 'Build failed' }; }
          if (r.VetErrors) h.out('go vet: ' + r.VetErrors + '\n', 'stderr');
          for (const ev of r.Events || []) h.out(ev.Message, ev.Kind === 'stderr' ? 'stderr' : 'stdout');
          if (r.Status) return { ok: false, error: 'exit status ' + r.Status };
        }
        return { ok: true, ms: performance.now() - t0, remote: true };
      } catch (e) {
        if (e.name === 'AbortError') return { ok: false, stopped: true };
        return { ok: false, error: (e.message || String(e)) + ' — the online playground may be unreachable; use the "Open in" link instead.' };
      }
    }
    stop() { if (this.ac) this.ac.abort(); }
  }

  const runners = {};
  function runnerFor(lang) {
    const def = LANGS[lang];
    const key = def.worker || def.main;
    if (!runners[key]) {
      runners[key] = def.worker ? new WorkerRunner(def.worker, def.module) : def.main === 'duckdb' ? new DuckRunner() : new FetchRunner(def.main);
    }
    return runners[key];
  }

  async function openRemote(lang, code) {
    if (lang === 'rust') {
      window.open('https://play.rust-lang.org/?version=stable&mode=debug&edition=2024&code=' + encodeURIComponent(code), '_blank', 'noopener');
      return;
    }
    const win = window.open('about:blank', '_blank');
    try {
      const id = await fetch('https://play.golang.org/share', { method: 'POST', body: code }).then(r => r.text());
      if (win) win.location = 'https://go.dev/play/p/' + id.trim();
    } catch { if (win) win.location = 'https://go.dev/play/'; }
  }

  // ------------------------------------------------------------------ UI
  const STYLE = `
.playground-ui .pg-bar { flex-wrap: wrap; }
.playground-ui textarea { white-space: pre; overflow-wrap: normal; overflow-x: auto; }
.playground-ui .pg-note { font-size: .75rem; color: var(--faint); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.playground-ui button.stop { color: #c0392b; }
.playground-ui button[aria-pressed="true"] { background: var(--wash); border-color: color-mix(in srgb, var(--accent) 50%, var(--line)); }
.playground-ui a.pg-open { font: 600 .8rem var(--sans); color: var(--muted); text-decoration: none; border: 1px solid var(--line); border-radius: 6px; padding: .26rem .6rem; background: var(--surface); cursor: pointer; }
.playground-ui a.pg-open:hover { color: var(--ink); }
.playground-ui textarea.pg-stdin { min-height: 4rem; border-top: 1px dashed var(--line); font-size: .85rem; }
.playground-ui .pg-out:empty { display: none; }
.playground-ui .pg-out .pg-err { color: #c0392b; }
.playground-ui .pg-status { color: var(--muted); font-family: var(--sans); font-size: .82rem; display: flex; align-items: center; gap: .6rem; flex-wrap: wrap; }
.playground-ui .pg-status progress { width: 12rem; max-width: 100%; height: .5rem; accent-color: var(--accent); }
.playground-ui .pg-meta { display: block; margin-top: .4rem; font: .75rem var(--sans); color: var(--faint); }
.playground-ui .pg-meta.bad { color: #c0392b; }
.playground-ui .pg-tw { overflow-x: auto; margin: .35rem 0 .6rem; border: 1px solid var(--line); border-radius: 6px; white-space: normal; }
.playground-ui table.pg-table { border-collapse: collapse; font: .8rem/1.4 var(--mono); width: max-content; min-width: 100%; }
.playground-ui table.pg-table th, .playground-ui table.pg-table td { padding: .25rem .6rem; border-bottom: 1px solid var(--line); text-align: left; white-space: pre; }
.playground-ui table.pg-table th { background: var(--wash); color: var(--muted); font-weight: 700; position: sticky; top: 0; }
.playground-ui table.pg-table td.num { text-align: right; }
.playground-ui table.pg-table td.null { color: var(--faint); font-style: italic; }
.playground-ui .pg-caption { font: .75rem var(--sans); color: var(--faint); }
.playground-ui form.pg-input { display: flex; align-items: center; gap: .4rem; margin: .2rem 0; flex-wrap: wrap; }
.playground-ui form.pg-input input { font: .86rem var(--mono); padding: .2rem .45rem; border: 1px solid var(--accent); border-radius: 5px; background: var(--bg); color: var(--ink); min-width: 8rem; }
`;
  let styled = false;
  function addStyle() {
    if (styled) return;
    styled = true;
    const s = document.createElement('style');
    s.textContent = STYLE;
    document.head.appendChild(s);
  }

  function h(tag, attrs, ...kids) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === undefined || v === null || v === false) continue;
      if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
      else if (k === 'class') el.className = v;
      else el.setAttribute(k, v === true ? '' : v);
    }
    for (const c of kids.flat()) if (c !== null && c !== undefined && c !== false) el.append(c instanceof Node ? c : document.createTextNode(String(c)));
    return el;
  }

  const MAX_OUT = 200000;

  /** Build an editor for one snippet. Returns {el, getCode, setCode, run}. */
  function mount(container, opts) {
    addStyle();
    const lang = langOf(opts.lang) || 'javascript';
    const def = LANGS[lang];
    const original = opts.code || '';
    const runBtn = h('button', { type: 'button', class: 'run', title: 'Run (Cmd/Ctrl + Enter)' }, 'Run ▶');
    const stopBtn = h('button', { type: 'button', class: 'stop', hidden: true }, 'Stop ■');
    const resetBtn = h('button', { type: 'button', class: 'reset', title: 'Restore the original code' }, 'Reset');
    const stdinBtn = def.stdin ? h('button', { type: 'button', 'aria-pressed': 'false', title: 'Text the program reads from standard input' }, 'stdin') : null;
    const csvBtn = def.csv ? h('button', { type: 'button', title: 'Load a CSV, Parquet, or JSON file from your computer as a table' }, 'Load file…') : null;
    const openLink = def.network ? h('a', { class: 'pg-open', href: '#', title: 'Open this code in the official playground' }, lang === 'rust' ? 'Rust Playground ↗' : 'Go Playground ↗') : null;
    const bar = h('div', { class: 'pg-bar' },
      h('span', { class: 'lang' }, def.label), h('span', { class: 'pg-note' }, def.note || ''), h('span', { class: 'spacer' }),
      stdinBtn, csvBtn, openLink, resetBtn, stopBtn, runBtn);
    const ta = h('textarea', { spellcheck: 'false', autocapitalize: 'off', autocomplete: 'off', 'aria-label': def.label + ' code' });
    ta.value = original;
    const fit = () => { ta.rows = Math.min(28, Math.max(5, ta.value.split('\n').length + 1)); };
    fit();
    const stdin = def.stdin ? h('textarea', { class: 'pg-stdin', spellcheck: 'false', hidden: true, placeholder: 'Standard input — one value per line', 'aria-label': 'Standard input' }) : null;
    const out = h('div', { class: 'pg-out', 'aria-live': 'polite' });
    const ui = h('div', { class: 'playground-ui', 'data-lang': lang }, bar, ta, stdin, out);
    container.appendChild(ui);

    // ---- output helpers
    let statusEl = null, chars = 0, truncated = false, lastText = null;
    const clearOut = () => { out.textContent = ''; out.classList.remove('err'); statusEl = null; chars = 0; truncated = false; lastText = null; };
    const setStatus = (text, done, total) => {
      if (!text) { if (statusEl) { statusEl.remove(); statusEl = null; } return; }
      if (!statusEl) { statusEl = h('div', { class: 'pg-status' }); out.prepend(statusEl); }
      statusEl.textContent = text;
      if (total) {
        const p = h('progress', { max: String(total), value: String(done) });
        statusEl.append(p, h('span', {}, `${(done / 1048576).toFixed(1)} / ${(total / 1048576).toFixed(1)} MB`));
      }
    };
    const write = (text, stream) => {
      if (truncated || !text) return;
      if (chars + text.length > MAX_OUT) { text = text.slice(0, MAX_OUT - chars); truncated = true; }
      chars += text.length;
      if (stream === 'stderr') { out.append(h('span', { class: 'pg-err' }, text)); lastText = null; }
      else if (lastText && lastText.parentNode === out && out.lastChild === lastText) lastText.appendData(text);
      else { lastText = document.createTextNode(text); out.append(lastText); }
      if (truncated) out.append(h('span', { class: 'pg-err' }, '\n… output truncated'));
      out.scrollTop = out.scrollHeight;
    };
    const table = (cols, rows, total) => {
      const t = h('table', { class: 'pg-table' },
        h('thead', {}, h('tr', {}, cols.map(c => h('th', {}, c)))),
        h('tbody', {}, rows.slice(0, 500).map(r => h('tr', {}, r.map(v =>
          v === null || v === undefined ? h('td', { class: 'null' }, 'NULL')
            : v instanceof Uint8Array ? h('td', { class: 'null' }, `[${v.length} bytes]`)
              : h('td', { class: typeof v === 'number' ? 'num' : '' }, String(v)))))));
      const n = total !== undefined ? total : rows.length;
      out.append(h('div', { class: 'pg-tw' }, t), h('div', { class: 'pg-caption' }, `${n} row${n === 1 ? '' : 's'}${n > 500 ? ' (first 500 shown)' : ''}`));
      lastText = null;
    };
    const askInput = prompt => new Promise(resolve => {
      const input = h('input', { type: 'text', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false', 'aria-label': prompt || 'Input' });
      const form = h('form', { class: 'pg-input' }, h('span', {}, prompt || '?'), input, h('button', { type: 'submit' }, 'Enter'));
      form.addEventListener('submit', e => { e.preventDefault(); form.remove(); resolve(input.value); });
      out.append(form);
      lastText = null;
      input.focus();
    });

    let running = null;
    async function run() {
      if (running) return;
      clearOut();
      if (def.html) {
        const f = h('iframe', { class: 'pg-html', sandbox: 'allow-scripts', title: 'HTML output' });
        f.srcdoc = ta.value;
        out.append(f);
        return;
      }
      runBtn.disabled = true;
      stopBtn.hidden = false;
      let phase = '';
      const handlers = {
        out: write, status: t => setStatus(t), progress: (d, tot, t) => setStatus(t, d, tot), table,
        phase: t => { phase = t; }, input: askInput, cls: () => { const s = statusEl; clearOut(); if (s) { statusEl = s; out.append(s); } },
      };
      const msg = { code: ta.value, lang, stdin: stdin ? stdin.value : '' };
      if (lang === 'prolog') {
        const p = parseProlog(ta.value);
        Object.assign(msg, { program: p.program, queries: p.queries.map(prologString) });
      }
      const runner = runnerFor(lang);
      running = runner;
      setStatus('Starting…');
      let res;
      try { res = await runner.run(msg, handlers); }
      catch (e) { res = { ok: false, error: e.message || String(e) }; }
      running = null;
      setStatus('');
      out.querySelectorAll('form.pg-input').forEach(f => f.remove());
      runBtn.disabled = false;
      stopBtn.hidden = true;
      if (res.stopped) out.append(h('span', { class: 'pg-meta bad' }, '■ Stopped' + (def.worker ? ' — the runtime will restart on the next run' : '')));
      else if (!res.ok) { write((chars ? '\n' : '') + res.error + '\n', 'stderr'); out.append(h('span', { class: 'pg-meta bad' }, '✗ Failed')); }
      else out.append(h('span', { class: 'pg-meta' }, `✓ ${phase ? phase + ' · ' : ''}${res.remote ? 'round trip' : 'ran in'} ${fmtMs(res.ms)}`));
    }
    function stop() { if (running) running.stop(); }

    runBtn.addEventListener('click', run);
    stopBtn.addEventListener('click', stop);
    resetBtn.addEventListener('click', () => { ta.value = original; fit(); clearOut(); opts.onChange && opts.onChange(ta.value); });
    if (stdinBtn) stdinBtn.addEventListener('click', () => {
      stdin.hidden = !stdin.hidden;
      stdinBtn.setAttribute('aria-pressed', String(!stdin.hidden));
      if (!stdin.hidden) stdin.focus();
    });
    if (csvBtn) csvBtn.addEventListener('click', () => {
      const fi = h('input', { type: 'file', accept: '.csv,.tsv,.txt,.parquet,.json,.ndjson,.jsonl', hidden: true });
      fi.addEventListener('change', async () => {
        const file = fi.files[0];
        fi.remove();
        if (!file) return;
        clearOut();
        try {
          const r = await runnerFor(lang).loadFile(file, { status: t => setStatus(t) });
          setStatus('');
          write(`Loaded ${file.name} as table "${r.table}" — ${r.rows} rows.\nColumns: ${r.cols.join(', ')}\nTry: SELECT * FROM "${r.table}" LIMIT 10;\n`);
        } catch (e) { setStatus(''); write(e.message + '\n', 'stderr'); }
      });
      document.body.appendChild(fi);
      fi.click();
    });
    if (openLink) openLink.addEventListener('click', e => { e.preventDefault(); openRemote(lang, ta.value); });
    ta.addEventListener('keydown', e => {
      if (e.key === 'Tab' && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
        e.preventDefault();
        ta.setRangeText(lang === 'go' ? '\t' : '  ', ta.selectionStart, ta.selectionEnd, 'end');
        ta.dispatchEvent(new Event('input'));
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); run(); }
      if (e.key === 'Escape' && running) stop();
    });
    ta.addEventListener('input', () => { fit(); opts.onChange && opts.onChange(ta.value); });

    return { el: ui, run, stop, getCode: () => ta.value, setCode: c => { ta.value = c; fit(); }, output: out };
  }
  const fmtMs = ms => (ms >= 1000 ? (ms / 1000).toFixed(2) + ' s' : Math.max(0, Math.round(ms)) + ' ms');

  function enhanceAll(root) {
    (root || document).querySelectorAll('.listingblock.playground').forEach(block => {
      const codeEl = block.querySelector('pre code') || block.querySelector('pre');
      const raw = (codeEl.className.match(/language-([\w+#-]+)/) || [])[1] || codeEl.dataset.lang || block.dataset.lang || 'javascript';
      const lang = langOf(raw);
      const code = codeEl.textContent.replace(/\n$/, '');
      const holder = document.createElement('div');
      const title = block.querySelector('.title');
      block.replaceWith(holder);
      if (title) holder.before(title);
      if (!lang) {
        holder.append(h('p', { class: 'pg-meta bad' }, `Playground language "${raw}" is not supported.`), h('pre', {}, h('code', {}, code)));
        return;
      }
      const m = mount(holder, { lang, code });
      holder.replaceWith(m.el);
    });
  }

  const api = {
    mount, enhanceAll, langOf,
    languages: Object.entries(LANGS).map(([id, d]) => ({ id, label: d.label, note: d.note, network: !!d.network })),
    _internal: { splitSql, parseProlog, prologString },
  };
  if (typeof window !== 'undefined') window.GWPlay = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = { splitSql, parseProlog, prologString, langOf };
})();
