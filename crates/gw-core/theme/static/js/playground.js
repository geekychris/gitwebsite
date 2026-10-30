// Runnable code playgrounds. AsciiDoc `[source.playground,<lang>]` blocks become editors with Run.
//
// site.js loads this file only on pages that have a playground (the /tools/sandbox/ page loads it
// too). Every language except HTML, p5.js, GLSL, DuckDB, Rust, Go, and Kotlin runs in its own Web
// Worker, so Stop can terminate a runaway loop; runtimes are fetched from jsDelivr the first time a
// language is used and are cached by the browser afterwards. HTML and p5.js sketches run in a
// sandboxed frame and GLSL shaders on a canvas in the page (Stop ends the animation). Rust, Go, and
// Kotlin are compiled by their official online playgrounds (the only parts that send code
// anywhere). Forth, Brainfuck, and Befunge are small interpreters in this file.
// scripts/test-playground-langs.mjs runs every language in headless Chromium.
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
  const WEBR = CDN + 'webr@0.6.0/dist/';
  const PHP = CDN + 'php-wasm@0.2.0/PhpWeb.mjs';
  const ZEROPERL = CDN + '@6over3/zeroperl-ts@1.0.10/dist/esm/';
  const OCAML = CDN + '@live-codes/ocaml@0.2.0/assets/interpreter/';
  const LFORTRAN = CDN + '@live-codes/lfortran-wasm@0.3.0/';
  const SCITTLE = CDN + 'scittle@0.8.33/dist/scittle.js';
  const BQN = 'https://cdn.jsdelivr.net/gh/mlochbaum/BQN@5abbab967fefc68ae9b3c2620d5d38473a90cb66/docs/bqn.js';
  const P5 = CDN + 'p5@1.11.13/lib/p5.min.js';
  const KOTLIN_API = 'https://api.kotlinlang.org';
  const KOTLIN_FALLBACK = '2.4.21';

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

  /**
   * A small Forth (most of the ANS core: stack words, arithmetic, `: ;`, IF ELSE THEN, DO LOOP
   * +LOOP ?DO LEAVE I J, BEGIN UNTIL AGAIN WHILE REPEAT, VARIABLE CONSTANT VALUE TO CREATE , ALLOT,
   * ." S" TYPE EMIT, BASE HEX DECIMAL, ' EXECUTE, RECURSE). Numbers are JavaScript integers; a
   * cell is one number. `write(text)` receives output. Returns `{ run(source), stack }`.
   */
  function makeForth(write) {
    const S = [], R = [], mem = [0], dict = new Map();
    const BASE = 0; // mem[0] holds BASE
    mem[BASE] = 10;
    let src = '', pos = 0, compiling = null, cs = [], depth = 0;
    class ForthError extends Error {}
    const fail = m => { throw new ForthError(m); };
    const pop = () => (S.length ? S.pop() : fail('Stack underflow'));
    const flag = b => (b ? -1 : 0);
    const int = n => Math.trunc(n);
    const alloc = n => { const a = mem.length; for (let i = 0; i < n; i++) mem.push(0); return a; };
    const fmtNum = n => { const b = mem[BASE]; return b === 10 ? String(n) : (n < 0 ? '-' : '') + Math.abs(n).toString(b).toUpperCase(); };
    const def = (name, fn, immediate) => dict.set(name.toLowerCase(), { name, prim: fn, immediate: !!immediate });
    const find = name => dict.get(name.toLowerCase());

    // Parsing the input stream.
    const skipWs = () => { while (pos < src.length && /\s/.test(src[pos])) pos++; };
    function word() { skipWs(); const s = pos; while (pos < src.length && !/\s/.test(src[pos])) pos++; return src.slice(s, pos); }
    function parseTo(ch) {
      if (/\s/.test(src[pos] || '')) pos++; // the one space after the word
      const e = src.indexOf(ch, pos);
      const s = src.slice(pos, e < 0 ? src.length : e);
      pos = e < 0 ? src.length : e + 1;
      return s;
    }
    function number(t) {
      let b = mem[BASE], s = t;
      if (/^'.'$/.test(s)) return s.charCodeAt(1);
      if (s[0] === '$') { b = 16; s = s.slice(1); } else if (s[0] === '#') { b = 10; s = s.slice(1); } else if (s[0] === '%') { b = 2; s = s.slice(1); }
      const neg = s[0] === '-';
      if (neg) s = s.slice(1);
      if (!s) return null;
      const digits = '0123456789abcdefghijklmnopqrstuvwxyz'.slice(0, b);
      let n = 0;
      for (const c of s.toLowerCase()) { const d = digits.indexOf(c); if (d < 0) return null; n = n * b + d; }
      return neg ? -n : n;
    }
    const strAt = (a, n) => { let s = ''; for (let i = 0; i < n; i++) s += String.fromCharCode(mem[a + i] || 0); return s; };
    const putStr = s => { const a = alloc(s.length); for (let i = 0; i < s.length; i++) mem[a + i] = s.charCodeAt(i); return a; };

    // Compiled code: arrays of [op, arg]; control flow is resolved to instruction indexes.
    function exec(w) {
      if (w.prim) return w.prim();
      if (++depth > 10000) { depth = 0; fail('Return stack overflow (runaway recursion?)'); }
      try { execBody(w.body); } finally { depth--; }
    }
    function execBody(body) {
      for (let ip = 0; ip < body.length;) {
        const [op, a] = body[ip++];
        switch (op) {
          case 'w': exec(a); break;
          case 'lit': S.push(a); break;
          case 'str': write(a); break;
          case 'slit': S.push(a[0], a[1]); break;
          case 'br': ip = a; break;
          case '0br': if (pop() === 0) ip = a; break;
          case 'do': { const i = pop(), lim = pop(); R.push(lim, i); break; }
          case '?do': { const i = pop(), lim = pop(); if (i === lim) ip = a; else R.push(lim, i); break; }
          case 'loop': { const i = R[R.length - 1] + 1; if (i === R[R.length - 2]) R.length -= 2; else { R[R.length - 1] = i; ip = a; } break; }
          case '+loop': {
            const n = pop(), lim = R[R.length - 2], old = R[R.length - 1], i = old + n;
            if (n >= 0 ? old < lim && i >= lim : old >= lim && i < lim) R.length -= 2; else { R[R.length - 1] = i; ip = a; }
            break;
          }
          case 'leave': R.length -= 2; ip = a; break;
          case 'exit': return;
        }
      }
    }
    const emit = (op, a) => compiling.body.push([op, a]);
    const here = () => compiling.body.length;
    const compileOnly = name => { if (!compiling) fail(name + ' is compile-only'); };
    const control = (kind, name) => { const c = cs.pop(); if (!c || c.kind !== kind) fail('Unbalanced ' + name); return c; };

    // Stack and arithmetic.
    const words = {
      '+': () => S.push(pop() + pop()), '-': () => { const b = pop(); S.push(pop() - b); },
      '*': () => S.push(pop() * pop()),
      '/': () => { const b = pop(); if (!b) fail('Division by zero'); S.push(int(pop() / b)); },
      'mod': () => { const b = pop(); if (!b) fail('Division by zero'); S.push(pop() % b); },
      '/mod': () => { const b = pop(), a = pop(); if (!b) fail('Division by zero'); S.push(a % b, int(a / b)); },
      '*/': () => { const c = pop(), b = pop(), a = pop(); if (!c) fail('Division by zero'); S.push(int(a * b / c)); },
      'negate': () => S.push(-pop()), 'abs': () => S.push(Math.abs(pop())),
      'min': () => S.push(Math.min(pop(), pop())), 'max': () => S.push(Math.max(pop(), pop())),
      '1+': () => S.push(pop() + 1), '1-': () => S.push(pop() - 1), '2*': () => S.push(pop() * 2), '2/': () => S.push(Math.floor(pop() / 2)),
      '=': () => S.push(flag(pop() === pop())), '<>': () => S.push(flag(pop() !== pop())),
      '<': () => { const b = pop(); S.push(flag(pop() < b)); }, '>': () => { const b = pop(); S.push(flag(pop() > b)); },
      '<=': () => { const b = pop(); S.push(flag(pop() <= b)); }, '>=': () => { const b = pop(); S.push(flag(pop() >= b)); },
      '0=': () => S.push(flag(pop() === 0)), '0<': () => S.push(flag(pop() < 0)), '0>': () => S.push(flag(pop() > 0)), '0<>': () => S.push(flag(pop() !== 0)),
      'and': () => S.push(pop() & pop()), 'or': () => S.push(pop() | pop()), 'xor': () => S.push(pop() ^ pop()),
      'invert': () => S.push(~pop()), 'lshift': () => { const n = pop(); S.push(pop() << n); }, 'rshift': () => { const n = pop(); S.push(pop() >>> n); },
      'true': () => S.push(-1), 'false': () => S.push(0),
      'dup': () => { const a = pop(); S.push(a, a); }, 'drop': () => { pop(); }, 'swap': () => { const b = pop(), a = pop(); S.push(b, a); },
      'over': () => { const b = pop(), a = pop(); S.push(a, b, a); }, 'rot': () => { const c = pop(), b = pop(), a = pop(); S.push(b, c, a); },
      '-rot': () => { const c = pop(), b = pop(), a = pop(); S.push(c, a, b); }, 'nip': () => { const b = pop(); pop(); S.push(b); },
      'tuck': () => { const b = pop(), a = pop(); S.push(b, a, b); }, '?dup': () => { const a = pop(); S.push(a); if (a) S.push(a); },
      'pick': () => { const n = pop(); if (n >= S.length) fail('Stack underflow'); S.push(S[S.length - 1 - n]); },
      'roll': () => { const n = pop(); if (n >= S.length) fail('Stack underflow'); S.push(S.splice(S.length - 1 - n, 1)[0]); },
      '2dup': () => { const b = pop(), a = pop(); S.push(a, b, a, b); }, '2drop': () => { pop(); pop(); },
      '2swap': () => { const d = pop(), c = pop(), b = pop(), a = pop(); S.push(c, d, a, b); },
      '2over': () => { const d = pop(), c = pop(), b = pop(), a = pop(); S.push(a, b, c, d, a, b); },
      'depth': () => S.push(S.length),
      '>r': () => R.push(pop()), 'r>': () => S.push(R.length ? R.pop() : fail('Return stack underflow')), 'r@': () => S.push(R[R.length - 1]),
      'i': () => S.push(R[R.length - 1]), 'j': () => S.push(R[R.length - 3]),
      // Output.
      '.': () => write(fmtNum(pop()) + ' '), 'u.': () => write(fmtNum(pop() >>> 0) + ' '),
      '.r': () => { const w = pop(); write(fmtNum(pop()).padStart(w)); },
      'emit': () => write(String.fromCharCode(pop())), 'cr': () => write('\n'), 'space': () => write(' '),
      'spaces': () => write(' '.repeat(Math.max(0, pop()))), 'type': () => { const n = pop(); write(strAt(pop(), n)); },
      '.s': () => write('<' + S.length + '> ' + S.map(fmtNum).join(' ') + (S.length ? ' ' : '')),
      'words': () => write([...dict.values()].map(w => w.name).join(' ') + '\n'),
      // Memory.
      '!': () => { const a = pop(); mem[a] = pop(); }, '@': () => S.push(mem[pop()] || 0), '+!': () => { const a = pop(); mem[a] = (mem[a] || 0) + pop(); },
      'c!': () => { const a = pop(); mem[a] = pop() & 255; }, 'c@': () => S.push(mem[pop()] || 0),
      ',': () => mem.push(pop()), 'c,': () => mem.push(pop() & 255), 'allot': () => alloc(Math.max(0, pop())), 'here': () => S.push(mem.length),
      'cells': () => {}, 'cell+': () => S.push(pop() + 1), 'chars': () => {}, 'char+': () => S.push(pop() + 1),
      'base': () => S.push(BASE), 'decimal': () => { mem[BASE] = 10; }, 'hex': () => { mem[BASE] = 16; },
      'execute': () => { const w = pop(); if (!w || typeof w !== 'object') fail('EXECUTE needs an execution token'); exec(w); },
      'bye': () => fail('bye'),
    };
    for (const [k, f] of Object.entries(words)) def(k, f);

    // Defining and parsing words.
    def(':', () => {
      const name = word();
      if (!name) fail('Missing name after :');
      compiling = { name, body: [] };
      cs = [];
    });
    def(';', () => { compileOnly(';'); if (cs.length) fail('Unbalanced control structure in ' + compiling.name); dict.set(compiling.name.toLowerCase(), compiling); compiling = null; }, true);
    def('variable', () => { const a = alloc(1); def(word(), () => S.push(a)); });
    def('constant', () => { const v = pop(); def(word(), () => S.push(v)); });
    def('value', () => { const a = alloc(1); mem[a] = pop(); const w = word(); def(w, () => S.push(mem[a])); find(w).addr = a; });
    def('to', () => {
      const w = find(word());
      if (!w || w.addr === undefined) fail('TO needs a VALUE');
      if (compiling) { emit('lit', w.addr); emit('w', find('!')); } else mem[w.addr] = pop();
    }, true);
    def('create', () => { const a = mem.length; def(word(), () => S.push(a)); });
    def("'", () => { const n = word(); const w = find(n); if (!w) fail('Undefined word: ' + n); S.push(w); });
    def("[']", () => { compileOnly("[']"); const n = word(); const w = find(n); if (!w) fail('Undefined word: ' + n); emit('lit', w); }, true);
    def('char', () => S.push(word().charCodeAt(0) || 0));
    def('[char]', () => { compileOnly('[CHAR]'); emit('lit', word().charCodeAt(0) || 0); }, true);
    def('(', () => { parseTo(')'); }, true);
    def('\\', () => { const e = src.indexOf('\n', pos); pos = e < 0 ? src.length : e + 1; }, true);
    def('."', () => { const s = parseTo('"'); if (compiling) emit('str', s); else write(s); }, true);
    def('.(', () => write(parseTo(')')), true);
    def('s"', () => { const s = parseTo('"'); const a = putStr(s); if (compiling) emit('slit', [a, s.length]); else S.push(a, s.length); }, true);
    def('recurse', () => { compileOnly('RECURSE'); emit('w', compiling); }, true);
    def('exit', () => { compileOnly('EXIT'); emit('exit'); }, true);
    def('literal', () => { compileOnly('LITERAL'); emit('lit', pop()); }, true);
    // Control flow (inside definitions).
    def('if', () => { compileOnly('IF'); cs.push({ kind: 'if', at: here() }); emit('0br', -1); }, true);
    def('else', () => { compileOnly('ELSE'); const c = control('if', 'ELSE'); const at = here(); emit('br', -1); compiling.body[c.at][1] = here(); cs.push({ kind: 'if', at }); }, true);
    def('then', () => { compileOnly('THEN'); const c = control('if', 'THEN'); compiling.body[c.at][1] = here(); }, true);
    def('begin', () => { compileOnly('BEGIN'); cs.push({ kind: 'begin', at: here(), whiles: [] }); }, true);
    def('until', () => { compileOnly('UNTIL'); emit('0br', control('begin', 'UNTIL').at); }, true);
    def('again', () => { compileOnly('AGAIN'); emit('br', control('begin', 'AGAIN').at); }, true);
    def('while', () => { compileOnly('WHILE'); const c = cs[cs.length - 1]; if (!c || c.kind !== 'begin') fail('WHILE without BEGIN'); c.whiles.push(here()); emit('0br', -1); }, true);
    def('repeat', () => { compileOnly('REPEAT'); const c = control('begin', 'REPEAT'); emit('br', c.at); for (const w of c.whiles) compiling.body[w][1] = here(); }, true);
    const doStart = op => () => { compileOnly(op.toUpperCase()); cs.push({ kind: 'do', at: here(), leaves: [] }); emit(op, -1); };
    def('do', doStart('do'), true);
    def('?do', doStart('?do'), true);
    const loopEnd = op => () => {
      compileOnly(op.toUpperCase());
      const c = control('do', op.toUpperCase());
      emit(op, c.at + 1);
      const end = here();
      if (compiling.body[c.at][0] === '?do') compiling.body[c.at][1] = end;
      for (const l of c.leaves) compiling.body[l][1] = end;
    };
    def('loop', loopEnd('loop'), true);
    def('+loop', loopEnd('+loop'), true);
    def('leave', () => { compileOnly('LEAVE'); const c = [...cs].reverse().find(x => x.kind === 'do'); if (!c) fail('LEAVE outside DO'); c.leaves.push(here()); emit('leave', -1); }, true);
    def('unloop', () => { R.length -= 2; });

    function run(source) {
      src = source; pos = 0;
      try {
        for (;;) {
          const t = word();
          if (!t) break;
          const w = find(t);
          if (w) {
            if (compiling && !w.immediate) emit('w', w);
            else exec(w);
            continue;
          }
          const n = number(t);
          if (n === null) fail('Undefined word: ' + t);
          if (compiling) emit('lit', n); else S.push(n);
        }
        if (compiling) { const n = compiling.name; compiling = null; fail('Unfinished definition: ' + n + ' (missing ;)'); }
      } catch (e) {
        compiling = null; R.length = 0; depth = 0;
        if (e instanceof ForthError) {
          if (e.message === 'bye') return;
          const line = src.slice(0, pos).split('\n').length;
          throw new Error(e.message + ' (line ' + line + ')');
        }
        throw e;
      }
    }
    return { run, stack: S };
  }

  /** Brainfuck on a 30 000-cell byte tape. `,` reads `input`, and 0 at its end. */
  function runBrainfuck(code, input, write) {
    const prog = code.replace(/[^+\-<>.,[\]]/g, '');
    const jump = new Int32Array(prog.length);
    const open = [];
    for (let i = 0; i < prog.length; i++) {
      if (prog[i] === '[') open.push(i);
      else if (prog[i] === ']') { if (!open.length) throw new Error('Unmatched ]'); const j = open.pop(); jump[i] = j; jump[j] = i; }
    }
    if (open.length) throw new Error('Unmatched [');
    const tape = new Uint8Array(30000);
    let p = 0, inp = 0, buf = '';
    for (let ip = 0; ip < prog.length; ip++) {
      switch (prog[ip]) {
        case '+': tape[p]++; break;
        case '-': tape[p]--; break;
        case '>': if (++p >= tape.length) throw new Error('Pointer moved past the end of the tape'); break;
        case '<': if (--p < 0) throw new Error('Pointer moved before the start of the tape'); break;
        case '.': buf += String.fromCharCode(tape[p]); if (buf.length > 4096) { write(buf); buf = ''; } break;
        case ',': tape[p] = inp < input.length ? input.charCodeAt(inp++) & 255 : 0; break;
        case '[': if (!tape[p]) ip = jump[ip]; break;
        case ']': if (tape[p]) ip = jump[ip]; break;
      }
    }
    if (buf) write(buf);
  }

  /** Befunge-93 on its 80×25 torus. `&` and `~` read `input` (−1 at its end). */
  function runBefunge(code, input, write) {
    const W = 80, H = 25;
    const grid = Array.from({ length: H }, () => new Array(W).fill(32));
    code.replace(/\r\n?/g, '\n').split('\n').slice(0, H).forEach((l, y) => { for (let x = 0; x < Math.min(W, l.length); x++) grid[y][x] = l.charCodeAt(x); });
    const S = [];
    const pop = () => (S.length ? S.pop() : 0);
    let x = 0, y = 0, dx = 1, dy = 0, str = false, inp = 0, buf = '';
    const out = s => { buf += s; if (buf.length > 4096) { write(buf); buf = ''; } };
    const readInt = () => {
      const m = /^\s*(-?\d+)/.exec(input.slice(inp));
      if (!m) { inp = input.length; return -1; }
      inp += m[0].length;
      return parseInt(m[1], 10);
    };
    for (;;) {
      const c = grid[y][x];
      if (str) { if (c === 34) str = false; else S.push(c); }
      else if (c >= 48 && c <= 57) S.push(c - 48);
      else {
        switch (String.fromCharCode(c)) {
          case '+': S.push(pop() + pop()); break;
          case '-': { const a = pop(); S.push(pop() - a); break; }
          case '*': S.push(pop() * pop()); break;
          case '/': { const a = pop(), b = pop(); S.push(a ? Math.trunc(b / a) : 0); break; }
          case '%': { const a = pop(), b = pop(); S.push(a ? b % a : 0); break; }
          case '!': S.push(pop() ? 0 : 1); break;
          case '`': { const a = pop(); S.push(pop() > a ? 1 : 0); break; }
          case '>': dx = 1; dy = 0; break;
          case '<': dx = -1; dy = 0; break;
          case '^': dx = 0; dy = -1; break;
          case 'v': dx = 0; dy = 1; break;
          case '?': [[dx, dy]] = [[[1, 0], [-1, 0], [0, 1], [0, -1]][Math.floor(Math.random() * 4)]]; break;
          case '_': dx = pop() ? -1 : 1; dy = 0; break;
          case '|': dy = pop() ? -1 : 1; dx = 0; break;
          case '"': str = true; break;
          case ':': { const a = pop(); S.push(a, a); break; }
          case '\\': { const a = pop(), b = pop(); S.push(a, b); break; }
          case '$': pop(); break;
          case '.': out(pop() + ' '); break;
          case ',': out(String.fromCharCode(pop())); break;
          case '#': x = (x + dx + W) % W; y = (y + dy + H) % H; break;
          case 'g': { const gy = pop(), gx = pop(); S.push(gx >= 0 && gx < W && gy >= 0 && gy < H ? grid[gy][gx] : 0); break; }
          case 'p': { const py = pop(), px = pop(), v = pop(); if (px >= 0 && px < W && py >= 0 && py < H) grid[py][px] = v; break; }
          case '&': S.push(readInt()); break;
          case '~': S.push(inp < input.length ? input.charCodeAt(inp++) : -1); break;
          case '@': if (buf) write(buf); return;
          case ' ': break;
          default: throw new Error(`Unknown instruction ${JSON.stringify(String.fromCharCode(c))} at column ${x + 1}, line ${y + 1}`);
        }
      }
      x = (x + dx + W) % W; y = (y + dy + H) % H;
    }
  }

  /**
   * A fragment shader made ready to compile: precision and the uniforms it uses but does not declare
   * (u_time, u_resolution, u_mouse; Shadertoy's iTime, iResolution, iMouse), and a main() that calls
   * Shadertoy's mainImage when there is one. `offset` is how many lines were added above the
   * user's first line (for error messages).
   */
  function glslSource(code) {
    const lines = code.replace(/\r\n?/g, '\n').split('\n');
    const version = /^\s*#version\s+300\s+es/.test(lines[0] || '') ? lines.shift() : null;
    const has = re => re.test(code);
    const pre = [];
    if (!has(/precision\s+\w+\s+float/)) pre.push('precision highp float;');
    const uni = { u_time: 'float', u_resolution: 'vec2', u_mouse: 'vec2', iTime: 'float', iResolution: 'vec3', iMouse: 'vec4' };
    for (const [n, t] of Object.entries(uni)) if (has(new RegExp('\\b' + n + '\\b')) && !has(new RegExp('uniform\\s+\\w+\\s+' + n + '\\b'))) pre.push(`uniform ${t} ${n};`);
    const post = [];
    if (!has(/\bvoid\s+main\s*\(/) && has(/\bvoid\s+mainImage\s*\(/)) {
      if (version) post.push('out vec4 pg_FragColor;', 'void main() { mainImage(pg_FragColor, gl_FragCoord.xy); }');
      else post.push('void main() { mainImage(gl_FragColor, gl_FragCoord.xy); }');
    }
    const head = (version ? [version] : []).concat(pre);
    return { source: head.concat(lines, post).join('\n'), offset: head.length - (version ? 1 : 0), version: !!version };
  }

  /** Compiler log lines ("ERROR: 0:7: …") renumbered to the user's lines. */
  function glslLog(log, offset) {
    return String(log || '').replace(/\0/g, '').trim().split('\n').filter(Boolean)
      .map(l => l.replace(/^(ERROR|WARNING):\s*\d+:(\d+):/, (m, k, n) => `${k}: line ${Math.max(1, n - offset)}:`)).join('\n');
  }

  const ALIASES = {
    js: 'javascript', javascript: 'javascript', py: 'python', python: 'python', python3: 'python',
    c: 'c', cpp: 'cpp', 'c++': 'cpp', cxx: 'cpp', cc: 'cpp',
    lua: 'lua', rb: 'ruby', ruby: 'ruby', sql: 'sql', sqlite: 'sql', duckdb: 'duckdb',
    prolog: 'prolog', pl: 'prolog', swipl: 'prolog', scheme: 'scheme', scm: 'scheme', lisp: 'scheme',
    basic: 'basic', bas: 'basic', rust: 'rust', rs: 'rust', go: 'go', golang: 'go', html: 'html',
    r: 'r', php: 'php', perl: 'perl', ocaml: 'ocaml', ml: 'ocaml', clojure: 'clojure', clj: 'clojure',
    cljs: 'clojure', forth: 'forth', fs: 'forth', fth: 'forth', fortran: 'fortran', f90: 'fortran', f95: 'fortran',
    bqn: 'bqn', brainfuck: 'brainfuck', bf: 'brainfuck', befunge: 'befunge', befunge93: 'befunge', kotlin: 'kotlin',
    kt: 'kotlin', p5: 'p5', p5js: 'p5', glsl: 'glsl', frag: 'glsl', shader: 'glsl',
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
    r: `
let webR;
async function init() {
  status('Loading R (webR, about 20 MB the first time)…');
  const { WebR } = await import('${WEBR}webr.js');
  webR = new WebR({ baseUrl: '${WEBR}' });
  await webR.init();
}
async function run(code) {
  const shelter = await new webR.Shelter();
  try {
    const canvas = typeof OffscreenCanvas !== 'undefined';
    // Messages, warnings and errors are printed on stderr as R prints them. An error ends the run
    // before the last line, which is how a failed run is told from one that printed to stderr.
    await webR.evalRVoid('.pg_done <- FALSE');
    const r = await shelter.captureR(code + '\\n.pg_done <- TRUE', { withAutoprint: true, captureStreams: true, captureConditions: false, captureGraphics: canvas ? { width: 640, height: 400 } : false });
    for (const o of r.output) if (o.type === 'stdout' || o.type === 'stderr') out(o.data + '\\n', o.type);
    for (const img of r.images || []) {
      const c = new OffscreenCanvas(img.width, img.height);
      c.getContext('2d').drawImage(img, 0, 0);
      const b = await c.convertToBlob({ type: 'image/png' });
      send({ type: 'image', src: await new Promise(res => { const fr = new FileReader(); fr.onload = () => res(fr.result); fr.readAsDataURL(b); }) });
    }
    if (!(await webR.evalRBoolean('.pg_done'))) throw new Error('Stopped at an R error (above).');
  } finally {
    await shelter.purge();
  }
}
`,
    php: `
let PhpWeb;
async function init() {
  status('Loading PHP 8.4 (php-wasm, about 12 MB the first time)…');
  // The browser build touches window and document while it loads; a worker has neither.
  self.window = self;
  self.document = { getElementById: () => null, querySelector: () => null, addEventListener() {}, removeEventListener() {}, body: null, currentScript: null };
  ({ PhpWeb } = await import('${PHP}'));
}
async function run(code) {
  // A fresh interpreter per run, as PHP serves a request: nothing leaks between runs.
  const php = new PhpWeb({ version: '8.4' });
  php.addEventListener('output', e => out(e.detail.join('')));
  php.addEventListener('error', e => out(e.detail.join(''), 'stderr'));
  await php.binary;
  status('');
  const rc = await php.run(/^\\s*<\\?php/.test(code) ? code : '<?php\\n' + code);
  await php.flush?.();
  if (rc) throw new Error('exit code ' + rc);
}
`,
    perl: `
let perl;
const dec = new TextDecoder();
const text = d => typeof d === 'string' ? d : dec.decode(d);
async function init() {
  status('Loading Perl 5 (zeroperl, about 25 MB the first time)…');
  // zeroperl takes the browser path only when it sees a window and a document.
  self.window = self;
  self.document = {};
  const { ZeroPerl } = await import('${ZEROPERL}index.js');
  perl = await ZeroPerl.create({ fetch: () => fetch('${ZEROPERL}zeroperl.wasm'), stdout: d => out(text(d)), stderr: d => out(text(d), 'stderr') });
}
async function run(code) {
  await perl.reset();
  const r = await perl.eval(code);
  perl.flush();
  if (!r.success) throw new Error(String(r.error || 'exit code ' + r.exitCode).trim());
}
`,
    ocaml: `
async function init() {
  status('Loading OCaml (the toplevel, compiled with js_of_ocaml — about 5 MB the first time)…');
  importScripts('${OCAML}worker.js');
  self.ocamlWasm.setBaseUrl('${OCAML}');
  self.ocamlWasm.init();
}
async function run(code, m) {
  const r = self.ocamlWasm.run({ code, input: m.stdin || '', args: [], files: [] });
  if (r.exitCode === null || r.exitCode === undefined || r.exitCode === 'null') {
    if (r.stdout) out(r.stdout);
    throw new Error((r.diagnostics || r.stderr || r.error || 'The program did not compile.').trim());
  }
  if (r.stdout) out(r.stdout);
  if (r.stderr) out(r.stderr, 'stderr');
  // Expression-only snippets print nothing; show what the toplevel says about them instead.
  if (!r.stdout && !r.stderr && r.outcome) out(r.outcome.trim() + '\\n');
  if (Number(r.exitCode)) out('\\n[exit code ' + r.exitCode + ']\\n', 'stderr');
}
`,
    fortran: `
let compiler;
async function init() {
  status('Loading LFortran (LLVM compiled to WebAssembly — about 18 MB the first time)…');
  const { createCompiler } = await import('${LFORTRAN}src/index.js');
  compiler = await createCompiler({ baseUrl: '${LFORTRAN}assets/' });
}
async function run(code, m) {
  const r = await compiler.run(code, m.stdin || '');
  if (r.stdout) out(r.stdout.endsWith('\\n') ? r.stdout : r.stdout + '\\n');
  if (r.exitCode === null) throw new Error((r.errors || 'The program did not compile.').trim());
  if (r.errors) out(r.errors, 'stderr');
  if (r.exitCode) out('\\n[exit code ' + r.exitCode + ']\\n', 'stderr');
}
`,
    clojure: `
async function init() {
  status('Loading Clojure (Scittle, the Small Clojure Interpreter — about 1 MB)…');
  // Scittle looks for a DOM at load time; a worker has none, so give it an empty one.
  self.window = self;
  self.document = { addEventListener() {}, querySelectorAll: () => [], getElementsByTagName: () => [], readyState: 'complete', currentScript: null };
  console.log = console.info = (...a) => out(a.join(' ') + '\\n');
  importScripts('${SCITTLE}');
}
async function run(code) {
  // SCI prints a long report on console.error before it throws; keep only the message and where.
  const err = [];
  console.error = console.warn = (...a) => err.push(a.join(' '));
  try {
    // One form, so the value of the last one can be shown; on the user's first line, so line
    // numbers in errors still match.
    const v = scittle.core.eval_string('(pr-str (do ' + code + '\\n))');
    if (err.length) out(err.join('\\n') + '\\n', 'stderr');
    if (v !== 'nil') out('=> ' + v + '\\n');
  } catch (e) {
    const at = err.join('\\n').match(/Location:\\s*(\\d+):(\\d+)/);
    const where = at ? ' (line ' + at[1] + ', column ' + (at[1] === '1' ? Math.max(1, at[2] - 12) : at[2]) + ')' : '';
    throw new Error((e.message || String(e)) + where);
  }
}
`,
    bqn: `
let B;
async function init() {
  status('Loading BQN (the reference JavaScript implementation, about 0.2 MB)…');
  const src = await fetch('${BQN}').then(r => { if (!r.ok) throw new Error('Could not load BQN (' + r.status + ')'); return r.text(); });
  // bqn.js declares globals (run, fmt…) that would clash with this worker's; give it a module of its own.
  const module = { exports: {} };
  new Function('module', src)(module);
  B = module.exports;
  B.sysvals.show = (x, w) => { out(B.fmt(x) + '\\n'); return x; };
  B.sysvals.out = (x, w) => { out(B.util.unstr(x) + '\\n'); return x; };
}
async function run(code) {
  let r;
  try { r = B(code); } catch (e) { throw new Error(B.fmtErr(e)); }
  if (r !== undefined) out(B.fmt(r) + '\\n');
}
`,
    forth: `
${makeForth.toString()}
let forth;
async function init() {}
async function run(code) {
  // A fresh Forth each run: definitions from a previous run do not linger.
  forth = makeForth(s => out(s));
  forth.run(code);
  out((forth.stack.length ? '\\n<' + forth.stack.length + '> ' + forth.stack.join(' ') + ' ok' : '\\nok') + '\\n');
}
`,
    brainfuck: `
${runBrainfuck.toString()}
async function init() {}
async function run(code, m) { runBrainfuck(code, m.stdin || '', s => out(s)); }
`,
    befunge: `
${runBefunge.toString()}
async function init() {}
async function run(code, m) { runBefunge(code, m.stdin || '', s => out(s)); }
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
    r: { label: 'R', worker: 'r', module: true, note: 'webR · ~20 MB first run' },
    php: { label: 'PHP', worker: 'php', module: true, note: 'PHP 8.4 · php-wasm · ~12 MB first run' },
    perl: { label: 'Perl', worker: 'perl', module: true, note: 'Perl 5 · zeroperl · ~25 MB first run' },
    ocaml: { label: 'OCaml', worker: 'ocaml', stdin: true, note: 'OCaml toplevel · js_of_ocaml · ~5 MB first run' },
    fortran: { label: 'Fortran', worker: 'fortran', module: true, stdin: true, note: 'LFortran → WebAssembly · ~18 MB first run' },
    clojure: { label: 'Clojure', worker: 'clojure', note: 'Scittle (SCI) · ~1 MB' },
    bqn: { label: 'BQN', worker: 'bqn', note: 'BQN reference implementation' },
    forth: { label: 'Forth', worker: 'forth', note: 'A small ANS-style Forth' },
    brainfuck: { label: 'Brainfuck', worker: 'brainfuck', stdin: true, note: '30 000-cell byte tape' },
    befunge: { label: 'Befunge-93', worker: 'befunge', stdin: true, note: '80 × 25 playfield' },
    kotlin: { label: 'Kotlin', main: 'kotlin', network: true, note: 'Compiled on api.kotlinlang.org (JetBrains)' },
    p5: { label: 'p5.js', sketch: 'p5', note: 'p5.js 1.11 in a sandboxed frame' },
    glsl: { label: 'GLSL', sketch: 'glsl', note: 'Fragment shader · WebGL' },
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
        case 'image': h.image(m.src); break;
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
        h.status(this.kind === 'rust' ? 'Compiling on play.rust-lang.org…' : this.kind === 'kotlin' ? 'Compiling on api.kotlinlang.org…' : 'Compiling on go.dev…');
        if (this.kind === 'kotlin') {
          if (!this.version) {
            try { this.version = kotlinVersion(await fetch(KOTLIN_API + '/versions', { signal: this.ac.signal }).then(r => r.json())); }
            catch (e) { if (e.name === 'AbortError') throw e; this.version = KOTLIN_FALLBACK; }
          }
          const r = await fetch(`${KOTLIN_API}/api/${this.version}/compiler/run`, {
            method: 'POST', signal: this.ac.signal, headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ args: '', files: [{ name: 'File.kt', text: msg.code, publicId: '' }], confType: 'java' }),
          }).then(r => { if (!r.ok) throw new Error('api.kotlinlang.org answered ' + r.status); return r.json(); });
          h.status('');
          h.phase('Kotlin ' + this.version);
          let failed = false;
          for (const [file, diags] of Object.entries(r.errors || {})) {
            for (const d of diags || []) {
              const st = (d.interval && d.interval.start) || { line: 0, ch: 0 };
              const sev = String(d.severity || 'ERROR').toLowerCase();
              if (sev === 'error') failed = true;
              h.out(`${file}:${st.line + 1}:${st.ch + 1}: ${sev}: ${d.message}\n`, sev === 'error' ? 'stderr' : 'stdout');
            }
          }
          if (failed) return { ok: false, error: 'Build failed' };
          // Output arrives as <outStream>…</outStream><errStream>…</errStream>.
          const text = r.text || '';
          const re = /<(outStream|errStream)>([\s\S]*?)<\/\1>/g;
          let m, last = 0;
          while ((m = re.exec(text))) {
            if (m.index > last) h.out(text.slice(last, m.index));
            h.out(m[2], m[1] === 'errStream' ? 'stderr' : 'stdout');
            last = re.lastIndex;
          }
          if (last < text.length) h.out(text.slice(last));
          if (r.exception) {
            const ex = r.exception;
            h.out(`Exception in thread "main" ${ex.fullName || 'java.lang.Throwable'}${ex.message ? ': ' + ex.message : ''}\n` +
              (ex.stackTrace || []).filter(f => !/^(jdk\.internal|java\.lang\.reflect|sun\.reflect)\./.test(f.className))
                .map(f => `\tat ${f.className}.${f.methodName}(${f.fileName || 'Unknown Source'}${f.lineNumber >= 0 ? ':' + f.lineNumber : ''})\n`).join(''), 'stderr');
            return { ok: false, error: 'Exited with an exception' };
          }
        } else if (this.kind === 'rust') {
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

  /** The latest stable compiler in api.kotlinlang.org's /versions listing. */
  function kotlinVersion(list) {
    const stable = Array.isArray(list) && list.find(v => v.latestStable);
    return stable ? stable.version : KOTLIN_FALLBACK;
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
.playground-ui img.pg-img { display: block; max-width: 100%; height: auto; margin: .4rem 0; background: #fff; border-radius: 4px; }
.playground-ui .pg-out:has(> iframe.pg-html, > canvas.pg-glsl) { max-height: none; }
.playground-ui canvas.pg-glsl { display: block; width: 100%; aspect-ratio: 16 / 9; max-height: min(70vh, 560px); background: #000; border-top: 1px solid var(--line); touch-action: none; }
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
    const openLink = lang === 'rust' || lang === 'go' ? h('a', { class: 'pg-open', href: '#', title: 'Open this code in the official playground' }, lang === 'rust' ? 'Rust Playground ↗' : 'Go Playground ↗') : null;
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
    const image = src => { out.append(h('img', { class: 'pg-img', src, alt: 'Plot' })); lastText = null; };
    const askInput = prompt => new Promise(resolve => {
      const input = h('input', { type: 'text', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false', 'aria-label': prompt || 'Input' });
      const form = h('form', { class: 'pg-input' }, h('span', {}, prompt || '?'), input, h('button', { type: 'submit' }, 'Enter'));
      form.addEventListener('submit', e => { e.preventDefault(); form.remove(); resolve(input.value); });
      out.append(form);
      lastText = null;
      input.focus();
    });

    let running = null, sketch = null;
    const stopSketch = () => { if (sketch) { sketch(); sketch = null; } stopBtn.hidden = true; };
    async function run() {
      if (running) return;
      stopSketch();
      clearOut();
      if (def.html) {
        const f = h('iframe', { class: 'pg-html', sandbox: 'allow-scripts', title: 'HTML output' });
        f.srcdoc = ta.value;
        out.append(f);
        return;
      }
      if (def.sketch === 'p5') { sketch = runP5(ta.value, out, write); return; }
      if (def.sketch === 'glsl') {
        const r = runGlsl(ta.value, out);
        if (!r.ok) { write(r.error + '\n', 'stderr'); out.append(h('span', { class: 'pg-meta bad' }, '✗ Failed')); return; }
        out.append(h('span', { class: 'pg-meta' }, `✓ compiled in ${fmtMs(r.ms)} · ${r.webgl}`));
        sketch = r.stop;
        stopBtn.hidden = false;
        return;
      }
      runBtn.disabled = true;
      stopBtn.hidden = false;
      let phase = '';
      const handlers = {
        out: write, status: t => setStatus(t), progress: (d, tot, t) => setStatus(t, d, tot), table, image,
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
    function stop() { if (running) running.stop(); else stopSketch(); }

    runBtn.addEventListener('click', run);
    stopBtn.addEventListener('click', stop);
    resetBtn.addEventListener('click', () => { ta.value = original; fit(); stopSketch(); clearOut(); opts.onChange && opts.onChange(ta.value); });
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
  /** p5.js: the sketch runs in a sandboxed frame with p5 loaded; its console and errors come back here. */
  function runP5(code, out, write) {
    const f = h('iframe', { class: 'pg-html', sandbox: 'allow-scripts', title: 'p5.js sketch' });
    const head = [
      '<!doctype html><html><head><meta charset="utf-8">',
      '<style>html,body{margin:0;background:#fff}main{display:flex;justify-content:center}canvas{display:block;max-width:100%;height:auto!important}</style>',
      '<script>(function(){var send=function(t,s){parent.postMessage({pgSketch:1,type:t,text:s},"*")};',
      'var fmt=function(a){return Array.prototype.map.call(a,function(x){if(typeof x==="string")return x;try{return JSON.stringify(x)}catch(e){return String(x)}}).join(" ")};',
      'console.log=console.info=function(){send("out",fmt(arguments)+"\\n")};console.warn=console.error=function(){send("err",fmt(arguments)+"\\n")};',
      'addEventListener("error",function(e){send("err",e.message+(e.lineno>=LINE?" (line "+(e.lineno-LINE+1)+")":"")+"\\n")});',
      // p5 calls setup() from a promise, so its errors arrive as rejections.
      'addEventListener("unhandledrejection",function(e){var r=e.reason;send("err",(r&&r.message?r.name+": "+r.message:String(r))+"\\n")});',
      'new ResizeObserver(function(){send("size",document.documentElement.scrollHeight)}).observe(document.documentElement);})();</script>',
      `<script src="${P5}" crossorigin="anonymous"></script></head><body><script>`,
    ];
    const line = head.length + 1;
    f.srcdoc = head.join('\n').replace(/LINE/g, String(line)) + '\n' + code.replace(/<\/script/gi, '<\\/script') + '\n</script></body></html>';
    const onMsg = e => {
      if (e.source !== f.contentWindow || !e.data || !e.data.pgSketch) return;
      if (e.data.type === 'size') f.style.height = Math.max(120, Math.min(900, Number(e.data.text) || 0)) + 'px';
      else write(String(e.data.text), e.data.type === 'err' ? 'stderr' : 'stdout');
    };
    addEventListener('message', onMsg);
    out.append(f);
    return () => removeEventListener('message', onMsg);
  }

  /**
   * GLSL: compile the fragment shader for a full-frame quad and animate it with u_time,
   * u_resolution and u_mouse (and Shadertoy's iTime, iResolution, iMouse). Returns
   * `{ok, ms, webgl, stop}` or `{ok: false, error}` with compiler messages on the user's lines.
   */
  function runGlsl(code, out) {
    const canvas = h('canvas', { class: 'pg-glsl', 'aria-label': 'Shader output' });
    out.append(canvas);
    const t0 = performance.now();
    const prep = glslSource(code);
    const opts = { preserveDrawingBuffer: true, antialias: false };
    const gl = canvas.getContext('webgl2', opts) || (prep.version ? null : canvas.getContext('webgl', opts));
    if (!gl) { canvas.remove(); return { ok: false, error: prep.version ? 'This browser has no WebGL 2, which #version 300 es needs.' : 'WebGL is not available in this browser.' }; }
    const vsrc = prep.version ? '#version 300 es\nin vec2 p;\nvoid main() { gl_Position = vec4(p, 0.0, 1.0); }' : 'attribute vec2 p;\nvoid main() { gl_Position = vec4(p, 0.0, 1.0); }';
    const shader = (type, src) => { const sh = gl.createShader(type); gl.shaderSource(sh, src); gl.compileShader(sh); return sh; };
    const vs = shader(gl.VERTEX_SHADER, vsrc), fs = shader(gl.FRAGMENT_SHADER, prep.source);
    if (!gl.getShaderParameter(fs, gl.COMPILE_STATUS)) { canvas.remove(); return { ok: false, error: glslLog(gl.getShaderInfoLog(fs), prep.offset) || 'The shader did not compile.' }; }
    const prog = gl.createProgram();
    gl.attachShader(prog, vs); gl.attachShader(prog, fs); gl.bindAttribLocation(prog, 0, 'p'); gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) { canvas.remove(); return { ok: false, error: glslLog(gl.getProgramInfoLog(prog), prep.offset) || 'The shader did not link.' }; }
    gl.useProgram(prog);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    const u = n => gl.getUniformLocation(prog, n);
    const U = { time: u('u_time'), res: u('u_resolution'), mouse: u('u_mouse'), iTime: u('iTime'), iRes: u('iResolution'), iMouse: u('iMouse') };
    const mouse = [0, 0, 0, 0];
    canvas.addEventListener('pointermove', e => {
      const r = canvas.getBoundingClientRect(), d = canvas.width / r.width;
      mouse[0] = (e.clientX - r.left) * d; mouse[1] = (r.bottom - e.clientY) * d;
      if (e.buttons) { mouse[2] = mouse[0]; mouse[3] = mouse[1]; }
    });
    canvas.addEventListener('pointerdown', () => { mouse[2] = mouse[0]; mouse[3] = mouse[1]; });
    let raf = 0, visible = true, stopped = false;
    const start = performance.now();
    const frame = now => {
      raf = 0;
      if (stopped || !visible) return;
      const dpr = Math.min(2, devicePixelRatio || 1);
      const w = Math.max(1, Math.round(canvas.clientWidth * dpr)), hgt = Math.max(1, Math.round(canvas.clientHeight * dpr));
      if (canvas.width !== w || canvas.height !== hgt) { canvas.width = w; canvas.height = hgt; }
      gl.viewport(0, 0, w, hgt);
      const t = (now - start) / 1000;
      if (U.time) gl.uniform1f(U.time, t);
      if (U.iTime) gl.uniform1f(U.iTime, t);
      if (U.res) gl.uniform2f(U.res, w, hgt);
      if (U.iRes) gl.uniform3f(U.iRes, w, hgt, 1);
      if (U.mouse) gl.uniform2f(U.mouse, mouse[0], mouse[1]);
      if (U.iMouse) gl.uniform4f(U.iMouse, mouse[0], mouse[1], mouse[2], mouse[3]);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      raf = requestAnimationFrame(frame);
    };
    // Off screen, the shader stops drawing; it carries on when scrolled back.
    const io = typeof IntersectionObserver !== 'undefined' ? new IntersectionObserver(es => {
      visible = es.some(e => e.isIntersecting);
      if (visible && !raf && !stopped) raf = requestAnimationFrame(frame);
    }) : null;
    if (io) io.observe(canvas);
    frame(performance.now());
    const ms = performance.now() - t0;
    return { ok: true, ms, webgl: typeof WebGL2RenderingContext !== 'undefined' && gl instanceof WebGL2RenderingContext ? 'WebGL 2' : 'WebGL', stop: () => { stopped = true; if (raf) cancelAnimationFrame(raf); if (io) io.disconnect(); } };
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
    _internal: { splitSql, parseProlog, prologString, makeForth, runBrainfuck, runBefunge, glslSource, glslLog },
  };
  if (typeof window !== 'undefined') window.GWPlay = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = { splitSql, parseProlog, prologString, langOf, makeForth, runBrainfuck, runBefunge, glslSource, glslLog };
})();
