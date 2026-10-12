// A small classic (Microsoft-flavoured) BASIC interpreter for the site's playgrounds.
//
//   await GWBasic.run(source, { print: s => ..., input: prompt => Promise<string>, cls: () => ... })
//
// Line numbers are optional (unnumbered lines continue from the previous number). Supports
// PRINT (with ; , TAB SPC), INPUT, LET, IF/THEN/ELSE, GOTO, GOSUB/RETURN, ON..GOTO/GOSUB,
// FOR/NEXT/STEP, WHILE/WEND, DIM, DATA/READ/RESTORE, DEF FN, RANDOMIZE, SWAP, CLS, END/STOP,
// REM/' and the usual numeric and string functions.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.GWBasic = factory();
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  const KEYWORDS = new Set(('PRINT INPUT LET IF THEN ELSE GOTO GOSUB RETURN ON FOR TO STEP NEXT WHILE WEND DIM ' +
    'DATA READ RESTORE DEF FN RANDOMIZE SWAP CLS END STOP REM AND OR NOT XOR MOD LINE').split(' '));
  const FUNCS = {
    ABS: Math.abs, ATN: Math.atan, COS: Math.cos, SIN: Math.sin, TAN: Math.tan, EXP: Math.exp, SQR: Math.sqrt,
    INT: Math.floor, FIX: Math.trunc, SGN: Math.sign,
    LOG: x => { if (x <= 0) throw new BasicError('ILLEGAL FUNCTION CALL'); return Math.log(x); },
    LEN: s => str(s).length, VAL: s => { const n = parseFloat(str(s).trim()); return isNaN(n) ? 0 : n; },
    ASC: s => { s = str(s); if (!s.length) throw new BasicError('ILLEGAL FUNCTION CALL'); return s.charCodeAt(0); },
    'CHR$': n => String.fromCharCode(num(n)), 'STR$': n => fmtNum(num(n)),
    'LEFT$': (s, n) => str(s).slice(0, Math.max(0, num(n))),
    'RIGHT$': (s, n) => { s = str(s); n = Math.max(0, num(n)); return n ? s.slice(-n) : ''; },
    'MID$': (s, a, n) => { s = str(s); a = Math.max(1, num(a)); return n === undefined ? s.slice(a - 1) : s.substr(a - 1, Math.max(0, num(n))); },
    INSTR: (...a) => { let st = 1; if (typeof a[0] === 'number') st = a.shift(); const i = str(a[0]).indexOf(str(a[1]), st - 1); return i + 1; },
    'UCASE$': s => str(s).toUpperCase(), 'UPPER$': s => str(s).toUpperCase(), 'LCASE$': s => str(s).toLowerCase(),
    'STRING$': (n, c) => (typeof c === 'number' ? String.fromCharCode(c) : str(c).charAt(0)).repeat(Math.max(0, num(n))),
    'SPACE$': n => ' '.repeat(Math.max(0, num(n))),
    MIN: (...a) => Math.min(...a.map(num)), MAX: (...a) => Math.max(...a.map(num)),
  };

  class BasicError extends Error {
    constructor(msg, line) { super(msg); this.name = 'BasicError'; this.line = line; }
  }
  function str(v) { if (typeof v !== 'string') throw new BasicError('TYPE MISMATCH'); return v; }
  function num(v) { if (typeof v !== 'number') throw new BasicError('TYPE MISMATCH'); return v; }

  /** Number formatting like MS BASIC: up to 9 significant digits, no trailing zeros. */
  function fmtNum(n) {
    if (Number.isInteger(n) && Math.abs(n) < 1e15) return String(n);
    if (!isFinite(n)) return n > 0 ? 'INF' : n < 0 ? '-INF' : 'NAN';
    let s = n.toPrecision(9);
    if (s.includes('e')) {
      let [m, e] = s.split('e');
      if (m.includes('.')) m = m.replace(/0+$/, '').replace(/\.$/, '');
      return m + 'E' + (e[0] === '-' ? '-' : '+') + e.replace(/^[+-]/, '').padStart(2, '0');
    }
    if (s.includes('.')) s = s.replace(/0+$/, '').replace(/\.$/, '');
    return s.replace(/^(-?)0\./, '$1.');
  }

  // ------------------------------------------------------------------ tokenizer
  function tokenize(text) {
    const t = [];
    let i = 0;
    while (i < text.length) {
      const c = text[i];
      if (c === ' ' || c === '\t') { i++; continue; }
      if (c === "'") { t.push({ k: 'kw', v: 'REM', rest: text.slice(i + 1) }); break; }
      if (c === '"') {
        let j = i + 1; while (j < text.length && text[j] !== '"') j++;
        t.push({ k: 'str', v: text.slice(i + 1, j) }); i = j + 1; continue;
      }
      if (/[0-9]/.test(c) || (c === '.' && /[0-9]/.test(text[i + 1] || ''))) {
        const m = /^(\d*\.?\d*(?:[eE][+-]?\d+)?)/.exec(text.slice(i));
        t.push({ k: 'num', v: parseFloat(m[1]) }); i += m[1].length; continue;
      }
      if (/[A-Za-z]/.test(c)) {
        const m = /^[A-Za-z][A-Za-z0-9_]*[$%]?/.exec(text.slice(i));
        const w = m[0].toUpperCase();
        i += m[0].length;
        if (w === 'REM') { t.push({ k: 'kw', v: 'REM', rest: text.slice(i) }); break; }
        if (w === 'FN') { t.push({ k: 'kw', v: 'FN' }); continue; }
        if (w.startsWith('FN') && w.length > 2 && !KEYWORDS.has(w)) { t.push({ k: 'kw', v: 'FN' }); t.push({ k: 'id', v: w.slice(2) }); continue; }
        t.push({ k: KEYWORDS.has(w) ? 'kw' : 'id', v: w }); continue;
      }
      const two = text.slice(i, i + 2);
      if (two === '<=' || two === '>=' || two === '<>' || two === '><' || two === '=<' || two === '=>') {
        t.push({ k: 'op', v: { '><': '<>', '=<': '<=', '=>': '>=' }[two] || two }); i += 2; continue;
      }
      if ('+-*/^\\=<>(),;:?'.includes(c)) {
        if (c === '?') t.push({ k: 'kw', v: 'PRINT' }); else t.push({ k: 'op', v: c });
        i++; continue;
      }
      throw new BasicError(`SYNTAX ERROR (unexpected "${c}")`);
    }
    return t;
  }

  // ------------------------------------------------------------------ parser
  class Parser {
    constructor(tokens, line) { this.t = tokens; this.i = 0; this.line = line; }
    peek(o = 0) { return this.t[this.i + o]; }
    next() { return this.t[this.i++]; }
    at(k, v) { const x = this.t[this.i]; return x && x.k === k && (v === undefined || x.v === v); }
    atOp(v) { return this.at('op', v); }
    atKw(v) { return this.at('kw', v); }
    eat(k, v) { if (this.at(k, v)) return this.next(); return null; }
    expect(k, v) {
      const x = this.next();
      if (!x || x.k !== k || (v !== undefined && x.v !== v)) throw new BasicError(`SYNTAX ERROR (expected ${v || k}${x ? `, found "${x.v}"` : ''})`);
      return x;
    }
    end() { return this.i >= this.t.length || this.atOp(':') || this.atKw('ELSE'); }

    statements(stopAtElse) {
      const list = [];
      while (this.i < this.t.length) {
        if (this.eat('op', ':')) continue;
        if (stopAtElse && this.atKw('ELSE')) break;
        list.push(this.statement());
        if (this.i < this.t.length && !this.atOp(':') && !(stopAtElse && this.atKw('ELSE'))) {
          throw new BasicError(`SYNTAX ERROR (unexpected "${this.peek().v}")`);
        }
      }
      return list;
    }

    statement() {
      const x = this.peek();
      if (x.k === 'id') return this.assign();
      if (x.k !== 'kw') throw new BasicError(`SYNTAX ERROR (unexpected "${x.v}")`);
      this.next();
      switch (x.v) {
        case 'REM': return { op: 'REM' };
        case 'LET': return this.assign();
        case 'PRINT': return this.print();
        case 'INPUT': return this.input();
        case 'LINE': this.expect('kw', 'INPUT'); return Object.assign(this.input(), { line: true });
        case 'IF': return this.ifStmt();
        case 'GOTO': return { op: 'GOTO', target: this.lineNum() };
        case 'GOSUB': return { op: 'GOSUB', target: this.lineNum() };
        case 'RETURN': return { op: 'RETURN' };
        case 'ON': {
          const e = this.expr();
          const kind = this.next();
          if (!kind || kind.k !== 'kw' || (kind.v !== 'GOTO' && kind.v !== 'GOSUB')) throw new BasicError('SYNTAX ERROR (ON needs GOTO or GOSUB)');
          const targets = [this.lineNum()];
          while (this.eat('op', ',')) targets.push(this.lineNum());
          return { op: 'ON', e, gosub: kind.v === 'GOSUB', targets };
        }
        case 'FOR': {
          const v = this.expect('id').v;
          this.expect('op', '=');
          const from = this.expr();
          this.expect('kw', 'TO');
          const to = this.expr();
          const step = this.eat('kw', 'STEP') ? this.expr() : null;
          return { op: 'FOR', v, from, to, step };
        }
        case 'NEXT': {
          const vars = [];
          if (this.at('id')) { vars.push(this.next().v); while (this.eat('op', ',')) vars.push(this.expect('id').v); }
          return { op: 'NEXT', vars };
        }
        case 'WHILE': return { op: 'WHILE', e: this.expr() };
        case 'WEND': return { op: 'WEND' };
        case 'DIM': {
          const arrs = [];
          do {
            const name = this.expect('id').v;
            this.expect('op', '(');
            const dims = [this.expr()];
            while (this.eat('op', ',')) dims.push(this.expr());
            this.expect('op', ')');
            arrs.push({ name, dims });
          } while (this.eat('op', ','));
          return { op: 'DIM', arrs };
        }
        case 'DATA': return { op: 'DATA' }; // values are collected when the program is loaded
        case 'READ': {
          const targets = [this.lvalue()];
          while (this.eat('op', ',')) targets.push(this.lvalue());
          return { op: 'READ', targets };
        }
        case 'RESTORE': return { op: 'RESTORE', target: this.at('num') ? this.next().v : null };
        case 'DEF': {
          this.expect('kw', 'FN');
          const name = this.expect('id').v;
          const params = [];
          if (this.eat('op', '(')) {
            if (!this.atOp(')')) { params.push(this.expect('id').v); while (this.eat('op', ',')) params.push(this.expect('id').v); }
            this.expect('op', ')');
          }
          this.expect('op', '=');
          return { op: 'DEF', name, params, body: this.expr() };
        }
        case 'RANDOMIZE': return { op: 'RANDOMIZE', e: this.end() ? null : this.expr() };
        case 'SWAP': { const a = this.lvalue(); this.expect('op', ','); return { op: 'SWAP', a, b: this.lvalue() }; }
        case 'CLS': return { op: 'CLS' };
        case 'END': case 'STOP': return { op: 'END' };
        default: throw new BasicError(`SYNTAX ERROR (unexpected ${x.v})`);
      }
    }

    lineNum() { const x = this.expect('num'); return x.v; }

    lvalue() {
      const name = this.expect('id').v;
      if (this.eat('op', '(')) {
        const idx = [this.expr()];
        while (this.eat('op', ',')) idx.push(this.expr());
        this.expect('op', ')');
        return { name, idx };
      }
      return { name };
    }

    assign() {
      const target = this.lvalue();
      this.expect('op', '=');
      return { op: 'LET', target, e: this.expr() };
    }

    print() {
      const items = [];
      while (!this.end()) {
        if (this.eat('op', ';')) { items.push({ sep: ';' }); continue; }
        if (this.eat('op', ',')) { items.push({ sep: ',' }); continue; }
        if (this.at('id', 'TAB') || this.at('id', 'SPC')) {
          const f = this.next().v; this.expect('op', '(');
          const e = this.expr(); this.expect('op', ')');
          items.push({ [f.toLowerCase()]: e }); continue;
        }
        items.push({ e: this.expr() });
      }
      return { op: 'PRINT', items };
    }

    input() {
      let prompt = null, q = true;
      if (this.at('str') && (this.peek(1) && this.peek(1).k === 'op' && (this.peek(1).v === ';' || this.peek(1).v === ','))) {
        prompt = this.next().v; q = this.next().v === ';';
      }
      const targets = [this.lvalue()];
      while (this.eat('op', ',')) targets.push(this.lvalue());
      return { op: 'INPUT', prompt, q, targets };
    }

    ifStmt() {
      const cond = this.expr();
      let then, els = null;
      if (this.eat('kw', 'GOTO')) then = [{ op: 'GOTO', target: this.lineNum() }];
      else {
        this.expect('kw', 'THEN');
        then = this.at('num') ? [{ op: 'GOTO', target: this.next().v }] : this.statements(true);
      }
      if (this.eat('kw', 'ELSE')) els = this.at('num') ? [{ op: 'GOTO', target: this.next().v }] : this.statements(false);
      return { op: 'IF', cond, then, els };
    }

    // Expressions, lowest precedence first.
    expr() { return this.or(); }
    or() { let l = this.and(); while (this.atKw('OR') || this.atKw('XOR')) { const o = this.next().v; l = { b: o, l, r: this.and() }; } return l; }
    and() { let l = this.not(); while (this.atKw('AND')) { this.next(); l = { b: 'AND', l, r: this.not() }; } return l; }
    not() { if (this.eat('kw', 'NOT')) return { u: 'NOT', e: this.not() }; return this.rel(); }
    rel() {
      let l = this.add();
      while (this.at('op') && ['=', '<>', '<', '>', '<=', '>='].includes(this.peek().v)) { const o = this.next().v; l = { b: o, l, r: this.add() }; }
      return l;
    }
    add() { let l = this.mod(); while (this.atOp('+') || this.atOp('-')) { const o = this.next().v; l = { b: o, l, r: this.mod() }; } return l; }
    mod() { let l = this.idiv(); while (this.atKw('MOD')) { this.next(); l = { b: 'MOD', l, r: this.idiv() }; } return l; }
    idiv() { let l = this.mul(); while (this.atOp('\\')) { this.next(); l = { b: '\\', l, r: this.mul() }; } return l; }
    mul() { let l = this.unary(); while (this.atOp('*') || this.atOp('/')) { const o = this.next().v; l = { b: o, l, r: this.unary() }; } return l; }
    unary() {
      if (this.eat('op', '-')) return { u: '-', e: this.unary() };
      if (this.eat('op', '+')) return this.unary();
      return this.pow();
    }
    pow() {
      const base = this.atom();
      if (this.eat('op', '^')) return { b: '^', l: base, r: this.unary() }; // right-assoc, binds tighter than unary minus on the left
      return base;
    }
    atom() {
      const x = this.next();
      if (!x) throw new BasicError('SYNTAX ERROR (missing value)');
      if (x.k === 'num') return { n: x.v };
      if (x.k === 'str') return { s: x.v };
      if (x.k === 'op' && x.v === '(') { const e = this.expr(); this.expect('op', ')'); return e; }
      if (x.k === 'kw' && x.v === 'FN') {
        const name = this.expect('id').v; const args = [];
        if (this.eat('op', '(')) { args.push(this.expr()); while (this.eat('op', ',')) args.push(this.expr()); this.expect('op', ')'); }
        return { fn: name, args };
      }
      if (x.k === 'id') {
        if (x.v === 'PI') return { n: Math.PI };
        if (x.v === 'RND' || x.v === 'TIMER') {
          let arg = null;
          if (this.eat('op', '(')) { if (!this.atOp(')')) arg = this.expr(); this.expect('op', ')'); }
          return { call: x.v, args: arg ? [arg] : [] };
        }
        if (FUNCS[x.v]) {
          this.expect('op', '(');
          const args = [this.expr()];
          while (this.eat('op', ',')) args.push(this.expr());
          this.expect('op', ')');
          return { call: x.v, args };
        }
        if (this.eat('op', '(')) {
          const idx = [this.expr()];
          while (this.eat('op', ',')) idx.push(this.expr());
          this.expect('op', ')');
          return { v: x.v, idx };
        }
        return { v: x.v };
      }
      throw new BasicError(`SYNTAX ERROR (unexpected "${x.v}")`);
    }
  }

  /** Parse source into sorted lines: [{num, stmts}] and the DATA list. */
  function parse(source) {
    const lines = [];
    const data = []; // {num, value}
    let last = 0;
    const seen = new Map();
    source.replace(/\r\n?/g, '\n').split('\n').forEach((raw, i) => {
      const text = raw.trim();
      if (!text) return;
      const m = /^(\d+)\s*(.*)$/.exec(text);
      const num = m ? parseInt(m[1], 10) : last + 1;
      const body = m ? m[2] : text;
      last = num;
      try {
        // DATA consumes the rest of its statement verbatim.
        const dm = /^DATA\s+(.*)$/i.exec(body);
        if (dm) {
          splitData(dm[1]).forEach(v => data.push({ num, value: v }));
          seen.set(num, lines.push({ num, stmts: [{ op: 'DATA' }], src: raw }) - 1);
          return;
        }
        const p = new Parser(tokenize(body), num);
        const stmts = p.statements(false);
        const entry = { num, stmts, src: raw };
        if (seen.has(num)) lines[seen.get(num)] = entry; else seen.set(num, lines.push(entry) - 1);
      } catch (e) {
        if (e instanceof BasicError) { e.line = num; e.message = `${e.message} IN ${num}`; }
        throw e;
      }
    });
    lines.sort((a, b) => a.num - b.num);
    data.sort((a, b) => a.num - b.num);
    return { lines, data };
  }

  function splitData(s) {
    const out = [];
    let i = 0;
    while (i <= s.length) {
      while (s[i] === ' ') i++;
      let v;
      if (s[i] === '"') { const j = s.indexOf('"', i + 1); v = s.slice(i + 1, j < 0 ? s.length : j); i = j < 0 ? s.length : j + 1; while (i < s.length && s[i] !== ',') i++; }
      else { let j = s.indexOf(',', i); if (j < 0) j = s.length; v = s.slice(i, j).trim(); i = j; }
      out.push(v);
      i++;
    }
    return out;
  }

  // ------------------------------------------------------------------ runtime
  function mulberry32(a) {
    return () => {
      a |= 0; a = (a + 0x6D2B79F5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  async function run(source, io = {}, opts = {}) {
    const { lines, data } = parse(source);
    const lineIndex = new Map(lines.map((l, i) => [l.num, i]));
    const vars = new Map();
    const arrays = new Map();
    const fns = new Map();
    const gosubs = [];
    const fors = [];
    const whiles = [];
    let dataPtr = 0;
    let rnd = opts.seed !== undefined ? mulberry32(opts.seed) : Math.random;
    let lastRnd = 0;
    let col = 0;
    let buf = '';
    let lastFlush = Date.now();
    const maxSteps = opts.maxSteps || Infinity;
    let steps = 0;
    const t0 = Date.now();

    const flush = () => { if (buf) { io.print && io.print(buf); buf = ''; } lastFlush = Date.now(); };
    const write = s => {
      buf += s;
      const nl = s.lastIndexOf('\n');
      col = nl >= 0 ? s.length - nl - 1 : col + s.length;
      if (buf.length > 4096 || Date.now() - lastFlush > 40) flush();
    };
    const isStr = name => name.endsWith('$');
    const getVar = name => vars.has(name) ? vars.get(name) : (isStr(name) ? '' : 0);
    const coerce = (name, v) => {
      if (isStr(name)) { if (typeof v !== 'string') throw new BasicError('TYPE MISMATCH'); return v; }
      if (typeof v !== 'number') throw new BasicError('TYPE MISMATCH');
      return name.endsWith('%') ? Math.trunc(v) : v;
    };
    const arrayFor = (name, n) => {
      if (!arrays.has(name)) arrays.set(name, makeArray(name, Array(n).fill(10)));
      return arrays.get(name);
    };
    function makeArray(name, dims) {
      const size = dims.reduce((a, d) => a * (d + 1), 1);
      if (size > 5e6) throw new BasicError('OUT OF MEMORY');
      return { dims, data: new Array(size).fill(isStr(name) ? '' : 0) };
    }
    const offset = (arr, idx) => {
      if (idx.length !== arr.dims.length) throw new BasicError('SUBSCRIPT OUT OF RANGE');
      let o = 0;
      idx.forEach((v, k) => {
        v = Math.floor(v);
        if (v < 0 || v > arr.dims[k]) throw new BasicError('SUBSCRIPT OUT OF RANGE');
        o = o * (arr.dims[k] + 1) + v;
      });
      return o;
    };
    const setL = (lv, v) => {
      v = coerce(lv.name, v);
      if (lv.idx) { const idx = lv.idx.map(e => num(ev(e))); const a = arrayFor(lv.name, idx.length); a.data[offset(a, idx)] = v; }
      else vars.set(lv.name, v);
    };
    const getL = lv => {
      if (lv.idx) { const idx = lv.idx.map(e => num(ev(e))); const a = arrayFor(lv.name, idx.length); return a.data[offset(a, idx)]; }
      return getVar(lv.name);
    };
    const truth = v => (typeof v === 'string' ? v.length > 0 : v !== 0);
    const B = b => (b ? -1 : 0);

    function ev(e) {
      if ('n' in e) return e.n;
      if ('s' in e) return e.s;
      if ('v' in e) {
        if (e.idx) { const idx = e.idx.map(x => num(ev(x))); const a = arrayFor(e.v, idx.length); return a.data[offset(a, idx)]; }
        return getVar(e.v);
      }
      if (e.u === '-') return -num(ev(e.e));
      if (e.u === 'NOT') return ~Math.trunc(num(ev(e.e)));
      if (e.call) {
        if (e.call === 'RND') {
          const a = e.args.length ? num(ev(e.args[0])) : 1;
          if (a < 0) { rnd = mulberry32(Math.floor(-a * 1000)); }
          if (a === 0) return lastRnd;
          lastRnd = rnd();
          return lastRnd;
        }
        if (e.call === 'TIMER') return (Date.now() - t0) / 1000;
        return FUNCS[e.call](...e.args.map(ev));
      }
      if (e.fn) {
        const f = fns.get(e.fn);
        if (!f) throw new BasicError('UNDEFINED USER FUNCTION');
        const saved = f.params.map(p => [p, vars.has(p), vars.get(p)]);
        const vals = e.args.map(ev);
        f.params.forEach((p, i) => vars.set(p, coerce(p, vals[i] === undefined ? (isStr(p) ? '' : 0) : vals[i])));
        try { return ev(f.body); }
        finally { saved.forEach(([p, had, v]) => (had ? vars.set(p, v) : vars.delete(p))); }
      }
      const l = ev(e.l), r = ev(e.r);
      switch (e.b) {
        case '+':
          if (typeof l === 'string' || typeof r === 'string') return str(l) + str(r);
          return l + r;
        case '-': return num(l) - num(r);
        case '*': return num(l) * num(r);
        case '/': if (num(r) === 0) throw new BasicError('DIVISION BY ZERO'); return num(l) / r;
        case '\\': if (Math.trunc(num(r)) === 0) throw new BasicError('DIVISION BY ZERO'); return Math.trunc(Math.trunc(num(l)) / Math.trunc(r));
        case 'MOD': { const d = Math.trunc(num(r)); if (d === 0) throw new BasicError('DIVISION BY ZERO'); return Math.trunc(num(l)) % d; }
        case '^': return Math.pow(num(l), num(r));
        case '=': return B(cmp(l, r) === 0);
        case '<>': return B(cmp(l, r) !== 0);
        case '<': return B(cmp(l, r) < 0);
        case '>': return B(cmp(l, r) > 0);
        case '<=': return B(cmp(l, r) <= 0);
        case '>=': return B(cmp(l, r) >= 0);
        case 'AND': return Math.trunc(num(l)) & Math.trunc(num(r));
        case 'OR': return Math.trunc(num(l)) | Math.trunc(num(r));
        case 'XOR': return Math.trunc(num(l)) ^ Math.trunc(num(r));
      }
      throw new BasicError('SYNTAX ERROR');
    }
    function cmp(l, r) {
      if (typeof l !== typeof r) throw new BasicError('TYPE MISMATCH');
      return l < r ? -1 : l > r ? 1 : 0;
    }

    const fmtPrint = v => (typeof v === 'number' ? (v >= 0 ? ' ' : '') + fmtNum(v) + ' ' : v);
    const jump = target => {
      if (!lineIndex.has(target)) throw new BasicError(`UNDEFINED LINE NUMBER ${target}`);
      return { li: lineIndex.get(target), si: 0 };
    };

    async function doInput(s) {
      for (;;) {
        flush();
        const prompt = (s.prompt !== null ? s.prompt : '') + (s.q ? '? ' : '');
        let answer = io.input ? await io.input(prompt) : '';
        if (answer === null || answer === undefined) answer = '';
        write(prompt + answer + '\n');
        if (s.line) { setL(s.targets[0], answer); return; }
        const parts = s.targets.length > 1 ? splitData(answer) : [answer.trim()];
        let ok = parts.length >= s.targets.length;
        const vals = [];
        if (ok) s.targets.forEach((t, i) => {
          if (isStr(t.name)) vals.push(parts[i].replace(/^"(.*)"$/, '$1'));
          else { const n = Number(parts[i].trim()); if (parts[i].trim() === '' || isNaN(n)) ok = false; vals.push(n); }
        });
        if (ok) { s.targets.forEach((t, i) => setL(t, vals[i])); return; }
        write('?REDO FROM START\n');
      }
    }

    // Execution state: line index and statement index; nested IF bodies run via a small stack.
    let li = 0;
    let si = 0;
    let block = null; // statements of the current IF branch, if we are inside one: {stmts, i, li, si}
    try {
      outer: while (li < lines.length) {
        const stmts = block ? block.stmts : lines[li].stmts;
        const idx = block ? block.i : si;
        if (idx >= stmts.length) {
          if (block) { li = block.li; si = block.si + 1; block = null; } else { li++; si = 0; }
          continue;
        }
        const s = stmts[idx];
        if (++steps > maxSteps) throw new BasicError('STEP LIMIT REACHED');
        const advance = () => { if (block) block.i++; else si++; };
        // Position after the current statement, used by FOR/GOSUB/WHILE return points.
        const here = () => (block ? { li: block.li, si: block.si, inner: block.i, stmts: block.stmts } : { li, si });
        let goto = null;
        switch (s.op) {
          case 'REM': case 'DATA': break;
          case 'LET': setL(s.target, ev(s.e)); break;
          case 'PRINT': {
            let nl = true;
            for (const it of s.items) {
              nl = true;
              if (it.sep === ';') { nl = false; continue; }
              if (it.sep === ',') { write(' '.repeat(14 - (col % 14))); nl = false; continue; }
              if (it.tab) { const n = Math.floor(num(ev(it.tab))); if (n > col) write(' '.repeat(n - col)); nl = false; continue; }
              if (it.spc) { write(' '.repeat(Math.max(0, Math.floor(num(ev(it.spc)))))); nl = false; continue; }
              write(fmtPrint(ev(it.e)));
            }
            if (nl) write('\n');
            break;
          }
          case 'INPUT': await doInput(s); break;
          case 'IF': {
            const branch = truth(ev(s.cond)) ? s.then : s.els;
            if (branch && branch.length) {
              const pos = here();
              advance();
              if (block) {
                // Nested IF inside an IF branch: run the inner branch inline, then continue the outer one.
                block = { stmts: branch.concat(block.stmts.slice(block.i)), i: 0, li: block.li, si: block.si };
              } else {
                block = { stmts: branch, i: 0, li: pos.li, si: pos.si };
              }
              continue outer;
            }
            break;
          }
          case 'GOTO': goto = jump(s.target); break;
          case 'GOSUB': gosubs.push(here()); goto = jump(s.target); break;
          case 'ON': {
            const n = Math.floor(num(ev(s.e)));
            if (n >= 1 && n <= s.targets.length) {
              if (s.gosub) gosubs.push(here());
              goto = jump(s.targets[n - 1]);
            }
            break;
          }
          case 'RETURN': {
            const r = gosubs.pop();
            if (!r) throw new BasicError('RETURN WITHOUT GOSUB');
            if (r.stmts) { li = r.li; si = r.si; block = { stmts: r.stmts, i: r.inner + 1, li: r.li, si: r.si }; }
            else { li = r.li; si = r.si + 1; block = null; }
            continue outer;
          }
          case 'FOR': {
            const start = num(ev(s.from));
            vars.set(s.v, coerce(s.v, start));
            const limit = num(ev(s.to));
            const step = s.step ? num(ev(s.step)) : 1;
            for (let k = fors.length - 1; k >= 0; k--) if (fors[k].v === s.v) { fors.splice(k); break; }
            const pos = here();
            fors.push({ v: s.v, limit, step, pos });
            // Skip the body entirely if the loop would not run (modern behaviour).
            if ((step >= 0 && start > limit) || (step < 0 && start < limit)) {
              fors.pop();
              const after = findNext(lines, pos, s.v);
              if (!after) throw new BasicError('FOR WITHOUT NEXT');
              li = after.li; si = after.si + 1; block = null;
              continue outer;
            }
            break;
          }
          case 'NEXT': {
            const names = s.vars.length ? s.vars : [null];
            let looped = false;
            for (const name of names) {
              let k = fors.length - 1;
              if (name) while (k >= 0 && fors[k].v !== name) k--;
              if (k < 0) throw new BasicError('NEXT WITHOUT FOR');
              const f = fors[k];
              fors.length = k + 1;
              const v = getVar(f.v) + f.step;
              vars.set(f.v, v);
              if ((f.step >= 0 && v <= f.limit) || (f.step < 0 && v >= f.limit)) {
                const p = f.pos;
                if (p.stmts) { li = p.li; si = p.si; block = { stmts: p.stmts, i: p.inner + 1, li: p.li, si: p.si }; }
                else { li = p.li; si = p.si + 1; block = null; }
                looped = true;
                break;
              }
              fors.pop();
            }
            if (looped) continue outer;
            break;
          }
          case 'WHILE': {
            if (truth(ev(s.e))) { whiles.push(here()); break; }
            const after = findWend(lines, here());
            if (!after) throw new BasicError('WHILE WITHOUT WEND');
            li = after.li; si = after.si + 1; block = null;
            continue outer;
          }
          case 'WEND': {
            const w = whiles.pop();
            if (!w) throw new BasicError('WEND WITHOUT WHILE');
            if (w.stmts) { li = w.li; si = w.si; block = { stmts: w.stmts, i: w.inner, li: w.li, si: w.si }; }
            else { li = w.li; si = w.si; block = null; }
            continue outer;
          }
          case 'DIM':
            for (const a of s.arrs) {
              if (arrays.has(a.name)) throw new BasicError('DUPLICATE DEFINITION');
              arrays.set(a.name, makeArray(a.name, a.dims.map(d => Math.floor(num(ev(d))))));
            }
            break;
          case 'READ':
            for (const t of s.targets) {
              if (dataPtr >= data.length) throw new BasicError('OUT OF DATA');
              const raw = data[dataPtr++].value;
              if (isStr(t.name)) setL(t, raw);
              else { const n = Number(raw); if (raw === '' || isNaN(n)) throw new BasicError('SYNTAX ERROR IN DATA'); setL(t, n); }
            }
            break;
          case 'RESTORE':
            dataPtr = s.target === null ? 0 : Math.max(0, data.findIndex(d => d.num >= s.target));
            if (dataPtr < 0) dataPtr = data.length;
            break;
          case 'DEF': fns.set(s.name, { params: s.params, body: s.body }); break;
          case 'RANDOMIZE': rnd = mulberry32(s.e ? Math.floor(num(ev(s.e))) : Date.now()); break;
          case 'SWAP': { const a = getL(s.a), b = getL(s.b); setL(s.a, b); setL(s.b, a); break; }
          case 'CLS': flush(); col = 0; io.cls && io.cls(); break;
          case 'END': break outer;
        }
        if (goto) { li = goto.li; si = 0; block = null; continue; }
        advance();
      }
    } catch (e) {
      flush();
      if (e instanceof BasicError && !/ IN \d+$/.test(e.message)) {
        const n = lines[li] ? lines[li].num : null;
        e.line = n;
        if (n !== null) e.message += ` IN ${n}`;
      }
      throw e;
    }
    flush();
    return { steps };
  }

  // Find the NEXT matching a FOR (for loops that run zero times) — scans forward by line.
  function findNext(lines, pos, v) {
    let depth = 0;
    for (let li = pos.li; li < lines.length; li++) {
      const st = lines[li].stmts;
      for (let si = li === pos.li ? pos.si + 1 : 0; si < st.length; si++) {
        if (st[si].op === 'FOR') depth++;
        if (st[si].op === 'NEXT') {
          if (depth === 0 && (!st[si].vars.length || st[si].vars.includes(v))) return { li, si };
          depth--;
        }
      }
    }
    return null;
  }
  function findWend(lines, pos) {
    let depth = 0;
    for (let li = pos.li; li < lines.length; li++) {
      const st = lines[li].stmts;
      for (let si = li === pos.li ? pos.si + 1 : 0; si < st.length; si++) {
        if (st[si].op === 'WHILE') depth++;
        if (st[si].op === 'WEND') { if (depth === 0) return { li, si }; depth--; }
      }
    }
    return null;
  }

  return { run, parse, tokenize, fmtNum, BasicError };
});
