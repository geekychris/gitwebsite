#!/usr/bin/env node
// Headless check that every playground language really runs in a browser.
//
// Builds a throwaway site (`gw init`) with one page holding a [source.playground,<lang>] block per
// case below, using this checkout's crates/gw-core/theme/static/js/playground.js; serves the build
// on 127.0.0.1; opens it in headless Chromium; presses Run on each block and checks the output.
// Prints a pass/fail table and exits 1 when anything failed.
//
//   node scripts/test-playground-langs.mjs                  every language
//   node scripts/test-playground-langs.mjs --only r,php     some of them (ids or case names)
//   node scripts/test-playground-langs.mjs --headed         watch it
//
// Needs: a gw binary (GW=…, default target/release/gw, else gw on PATH), Asciidoctor, and
// playwright-core with its Chromium (PLAYWRIGHT_CORE=/path/to/node_modules/playwright-core, or
// resolvable from here; CHROMIUM=/path/to/chrome to use another browser). Runtimes load from
// jsDelivr and Kotlin compiles on api.kotlinlang.org, so it needs the network; the first run
// downloads ~150 MB of runtimes. CONCURRENCY=4 pages run cases in parallel.

import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '..');
const args = process.argv.slice(2);
const only = (args.includes('--only') ? args[args.indexOf('--only') + 1] : '').split(',').filter(Boolean);
const headed = args.includes('--headed');

// name, language id, code, what the output must contain (string or RegExp), seconds allowed, and
// options: stdin (typed into the stdin box), stop (press Stop after this many seconds; the output
// must then say Stopped), img (the output must hold a plot image), fail (the run must end ✗ Failed).
const CASES = [
  ['javascript', 'javascript', 'console.log(6 * 7)', '42', 30],
  ['r', 'r', 'x <- c(1, 2, 3, 4)\ncat("mean", mean(x), "\\n")\nsummary(x)', /mean 2\.5[\s\S]*Median/, 240],
  ['php', 'php', '<?php\n$a = [3, 1, 2];\nsort($a);\necho implode(",", $a), " PHP ", PHP_MAJOR_VERSION, "\\n";', '1,2,3 PHP 8', 180],
  ['perl', 'perl', 'my @w = map { ucfirst } qw(hello perl world);\nprint join(" ", @w), "\\n";\nprintf("%.2f\\n", 22/7);', /Hello Perl World\s+3\.14/, 240],
  ['ocaml', 'ocaml', 'let rec fact n = if n <= 1 then 1 else n * fact (n - 1)\nlet () = Printf.printf "fact 10 = %d\\n" (fact 10)', 'fact 10 = 3628800', 180],
  ['fortran', 'fortran', 'program squares\n  integer :: i\n  do i = 1, 3\n    print *, i * i\n  end do\nend program', /1\s+4\s+9/, 300],
  ['clojure', 'clojure', '(defn fact [n] (reduce * (range 1 (inc n))))\n(println "fact 20 =" (fact 20))\n(map inc [1 2 3])', /fact 20 = 2432902008176640000[\s\S]*=> \(2 3 4\)/, 120],
  ['bqn', 'bqn', '•Show 2‿3⥊↕6\n+´ 1+↕10', /0 1 2[\s\S]*3 4 5[\s\S]*55/, 120],
  ['forth', 'forth', ': fact ( n -- n! ) dup 1 > if dup 1- recurse * else drop 1 then ;\n10 fact . cr\n: stars 0 ?do 42 emit loop ; 5 stars', /3628800\s+\*\*\*\*\*/, 30],
  ['brainfuck', 'brainfuck', '++++++++[>++++[>++>+++>+++>+<<<<-]>+>+>->>+[<]<-]>>.>---.+++++++..+++.>>.<-.<.+++.------.--------.>>+.>++.', 'Hello World!', 30],
  ['befunge', 'befunge', '"!egnufeB olleH">:#,_@', 'Hello Befunge!', 30],
  ['kotlin', 'kotlin', 'fun main() {\n    val squares = (1..5).map { it * it }\n    println("Kotlin says $squares")\n}', 'Kotlin says [1, 4, 9, 16, 25]', 120],
  ['p5', 'p5', 'function setup() {\n  createCanvas(200, 100);\n  background(220);\n  circle(100, 50, 40);\n  console.log("p5 canvas", width, height);\n}', 'p5 canvas 200 100', 120],
  ['glsl', 'glsl', 'uniform vec2 u_resolution;\nuniform float u_time;\nvoid main() {\n  vec2 st = gl_FragCoord.xy / u_resolution;\n  gl_FragColor = vec4(st, 0.5 + 0.5 * sin(u_time), 1.0);\n}', /compiled in .*WebGL/, 30],
  ['glsl-shadertoy', 'glsl', 'void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n  vec2 uv = fragCoord / iResolution.xy;\n  fragColor = vec4(uv, 0.5 + 0.5 * cos(iTime), 1.0);\n}', /compiled in/, 30],
  ['r-plot', 'r', 'plot(1:10, (1:10)^2, type = "b")\ncat("plotted\\n")', 'plotted', 240, { img: true }],
  ['r-error', 'r', 'message("note")\nwarning("careful")\nstop("boom")', /note[\s\S]*boom/, 240, { fail: true }],
  ['php-error', 'php', '<?php\nnope();', /undefined function nope/, 180, { fail: true }],
  ['perl-error', 'perl', 'print "before\\n";\ndie "oops\\n";', /before[\s\S]*oops/, 240, { fail: true }],
  ['ocaml-error', 'ocaml', 'let x : int = "text"', /Error/, 180, { fail: true }],
  ['ocaml-stdin', 'ocaml', 'let () = let n = int_of_string (read_line ()) in Printf.printf "%d squared is %d\\n" n (n * n)', '7 squared is 49', 180, { stdin: '7' }],
  ['fortran-stdin', 'fortran', 'program twice\n  integer :: n\n  read *, n\n  print *, "twice", 2 * n\nend program', /twice\s+42/, 300, { stdin: '21' }],
  ['fortran-error', 'fortran', 'program bad\n  x = undefined_thing(\nend program', /error/i, 300, { fail: true }],
  ['clojure-error', 'clojure', '(println "ok")\n(nope 1)', /Unable to resolve symbol: nope \(line 2/, 120, { fail: true }],
  ['bqn-error', 'bqn', '1+', /Second-level parts of a train/, 120, { fail: true }],
  ['forth-error', 'forth', '1 2 +\n3 frobnicate', /Undefined word: frobnicate \(line 2\)/, 30, { fail: true }],
  ['forth-stop', 'forth', ': forever begin again ; forever', /Stopped/, 30, { stop: 1.5 }],
  ['brainfuck-stdin', 'brainfuck', ',[.,]', 'echo me', 30, { stdin: 'echo me' }],
  ['befunge-stdin', 'befunge', '&&*.@', '42', 30, { stdin: '6 7' }],
  ['kotlin-error', 'kotlin', 'fun main() {\n    val x: Int = "no"\n}', /File\.kt:2:\d+: error/, 120, { fail: true }],
  ['p5-error', 'p5', 'function setup() {\n  createCanvas(100, 100);\n  nope();\n}', /nope is not defined/, 120],
  ['glsl-error', 'glsl', 'void main() {\n  float a = 1.0;\n  gl_FragColor = vec4(nope, a, 0.0, 1.0);\n}', /line 3:.*nope/, 30, { fail: true }],
];

const selected = CASES.filter(([name, lang]) => !only.length || only.includes(name) || only.includes(lang));
if (!selected.length) { console.error('no cases match --only ' + only.join(',')); process.exit(2); }

// ---------------------------------------------------------------- the site
const gw = process.env.GW || (fs.existsSync(path.join(repo, 'target/release/gw')) ? path.join(repo, 'target/release/gw') : 'gw');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'gw-pg-langs-'));
const site = path.join(tmp, 'site');
const sh = (cmd, a) => {
  const r = spawnSync(cmd, a, { encoding: 'utf8' });
  if (r.status !== 0) { console.error(`${cmd} ${a.join(' ')} failed:\n${r.stdout}${r.stderr}`); process.exit(2); }
  return r.stdout;
};
sh(gw, ['init', site, '--title', 'Playground languages', '--author', 'Test', '--base-url', 'https://example.com']);
const page = ['= Playground languages', ':type: page', ''];
for (const [name, lang, code] of selected) page.push(`.${name}`, `[source.playground,${lang}]`, '----', code, '----', '');
fs.writeFileSync(path.join(site, 'content/pages/langs.adoc'), page.join('\n'));
fs.mkdirSync(path.join(site, 'static/js'), { recursive: true });
fs.copyFileSync(path.join(repo, 'crates/gw-core/theme/static/js/playground.js'), path.join(site, 'static/js/playground.js'));
const report = JSON.parse(sh(gw, ['build', '--site', site, '--drafts', '--json']) || '{}');
const dist = report.out_dir || path.join(site, 'dist');

// ---------------------------------------------------------------- a static server
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml', '.wasm': 'application/wasm' };
const server = http.createServer((req, res) => {
  let p = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  if (p.endsWith('/')) p += 'index.html';
  const f = path.join(dist, p);
  if (!f.startsWith(dist) || !fs.existsSync(f)) { res.writeHead(404); res.end('not found'); return; }
  res.writeHead(200, { 'content-type': TYPES[path.extname(f)] || 'application/octet-stream' });
  fs.createReadStream(f).pipe(res);
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const url = `http://127.0.0.1:${server.address().port}/langs/`;

// ---------------------------------------------------------------- the browser
const require = createRequire(import.meta.url);
let pw;
for (const c of [process.env.PLAYWRIGHT_CORE, 'playwright-core', path.join(os.homedir(), 'code/claude_world/grid_maker/node_modules/playwright-core')].filter(Boolean)) {
  try { pw = require(c); break; } catch {}
}
if (!pw) { console.error('playwright-core not found: set PLAYWRIGHT_CORE=/path/to/node_modules/playwright-core'); process.exit(2); }
// playwright-core's own Chromium, else the newest one any Playwright installed.
function installedChromium() {
  const cache = process.env.PLAYWRIGHT_BROWSERS_PATH || (process.platform === 'darwin' ? path.join(os.homedir(), 'Library/Caches/ms-playwright') : path.join(os.homedir(), '.cache/ms-playwright'));
  const dirs = fs.existsSync(cache) ? fs.readdirSync(cache).filter(d => /^chromium_headless_shell-\d+$/.test(d)).sort((a, b) => b.split('-')[1] - a.split('-')[1]) : [];
  for (const d of dirs) {
    for (const sub of fs.readdirSync(path.join(cache, d))) {
      const exe = path.join(cache, d, sub, 'chrome-headless-shell');
      if (fs.existsSync(exe)) return exe;
    }
  }
  return undefined;
}
const launch = executablePath => pw.chromium.launch({
  headless: !headed, executablePath,
  args: ['--enable-unsafe-swiftshader', '--use-angle=swiftshader', '--ignore-gpu-blocklist'],
});
let browser;
try { browser = await launch(process.env.CHROMIUM || undefined); }
catch (e) {
  const exe = !process.env.CHROMIUM && installedChromium();
  if (!exe) throw e;
  browser = await launch(exe);
}

async function runCase(ctx, [name, lang, , expect, secs, opt = {}]) {
  const p = await ctx.newPage();
  const t0 = Date.now();
  try {
    await p.goto(url, { waitUntil: 'load' });
    const titles = await p.$$eval('.title', ts => ts.map(t => t.textContent.trim()));
    const i = titles.indexOf(name);
    const ui = p.locator('.playground-ui').nth(i);
    await ui.waitFor({ timeout: 20000 });
    await ui.scrollIntoViewIfNeeded();
    if (opt.stdin !== undefined) {
      await ui.locator('.pg-bar button', { hasText: 'stdin' }).click();
      await ui.locator('textarea.pg-stdin').fill(opt.stdin);
    }
    await ui.locator('button.run').click();
    if (opt.stop) { await p.waitForTimeout(opt.stop * 1000); await ui.locator('button.stop').click(); }
    const out = ui.locator('.pg-out');
    let text = '';
    const deadline = Date.now() + secs * 1000;
    const ok = () => (expect instanceof RegExp ? expect.test(text) : text.includes(expect));
    while (Date.now() < deadline) {
      text = (await out.innerText()).replace(/\r/g, '');
      // Plots and failures are only known once the run has finished, so those wait for the ✓/✗ line.
      if (ok() && !opt.img && !opt.fail) break;
      // A finished worker run (✓ or ✗) will not print anything more.
      if (lang !== 'p5' && (await out.locator('.pg-meta').count())) { text = (await out.innerText()).replace(/\r/g, ''); break; }
      await p.waitForTimeout(250);
    }
    const meta = await out.locator('.pg-meta').allInnerTexts();
    const failedRun = meta.some(m => m.includes('Failed'));
    let pass = ok() && (opt.fail ? failedRun : !failedRun || !!opt.stop);
    if (pass && opt.img) pass = (await out.locator('img.pg-img').count()) > 0;
    return { name, pass, secs: ((Date.now() - t0) / 1000).toFixed(1), text };
  } catch (e) {
    return { name, pass: false, secs: ((Date.now() - t0) / 1000).toFixed(1), text: String(e.message || e) };
  } finally {
    await p.close();
  }
}

const ctx = await browser.newContext({ viewport: { width: 1200, height: 900 } });
const queue = selected.slice();
const results = [];
await Promise.all(Array.from({ length: Number(process.env.CONCURRENCY) || 4 }, async () => {
  while (queue.length) {
    const c = queue.shift();
    const r = await runCase(ctx, c);
    results.push(r);
    console.error(`${r.pass ? 'pass' : 'FAIL'} ${r.name} (${r.secs} s)`);
  }
}));
await browser.close();
server.close();

results.sort((a, b) => selected.findIndex(c => c[0] === a.name) - selected.findIndex(c => c[0] === b.name));
const w = Math.max(...results.map(r => r.name.length));
console.log(`\n${'case'.padEnd(w)}  result  seconds  output`);
for (const r of results) {
  const snippet = r.text.replace(/\s+/g, ' ').trim().slice(0, 90);
  console.log(`${r.name.padEnd(w)}  ${r.pass ? 'pass  ' : 'FAIL  '}  ${r.secs.padStart(7)}  ${snippet}`);
}
const failed = results.filter(r => !r.pass).length;
console.log(`\n${results.length - failed}/${results.length} passed`);
if (!process.env.KEEP) fs.rmSync(tmp, { recursive: true, force: true });
else console.log('kept ' + tmp);
process.exit(failed ? 1 : 0);
