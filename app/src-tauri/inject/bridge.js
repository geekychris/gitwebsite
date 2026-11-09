// Injected by the gitwebsite app at document start into every tool window (views.rs). It tells
// the page it runs in the app (window.__GW_APP__) and gives the site kit (k.native) and the
// Web API shims one way to reach the companion's /v1/native/… endpoints:
//   __gwNative.fetch(path, init)   fetch with the token and X-GW-Window
//   __gwNative.ws(path)            Promise<WebSocket> (the token in the subprotocol list)
//   __gwNative.caps()              Promise<string[]>: /v1/hello capabilities
//   __gwNative.on(event, fn)       events from the app (menu clicks, files to open …); returns off()
//   __gwNative.emit(event, data)   called by the app (WKWebView evaluateJavaScript)
// Pages from the app's own preview are paired without asking, so this never shows a prompt.
(function () {
  if (window.__GW_APP__) return;
  var WIN = __GW_WINDOW__, BASE = __GW_COMPANION__;
  var SCOPES = ['native.fs', 'native.device', 'native.exec', 'native.mac', 'native.app', 'native.store', 'relay.http', 'view'];
  var m = /\/tools\/([\w-]+)\//.exec(location.pathname);
  var tool = (m ? m[1] : 'app') + '_app';
  var listeners = {}, queued = {}, tokenP = null, helloP = null;

  function token() {
    if (!tokenP) tokenP = fetch(BASE + '/v1/pair', { method: 'POST', credentials: 'omit', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ tool: tool, scopes: SCOPES }) })
      .then(function (r) { return r.json().then(function (j) { if (!j.token) throw new Error(j.error || 'the app did not pair this window'); return j.token; }); })
      .catch(function (e) { tokenP = null; throw e; });
    return tokenP;
  }
  function nfetch(path, init) {
    init = init || {};
    return token().then(function (t) {
      var hd = new Headers(init.headers || {});
      hd.set('Authorization', 'Bearer ' + t);
      hd.set('X-GW-Window', WIN);
      return fetch(BASE + path, Object.assign({ credentials: 'omit', cache: 'no-store' }, init, { headers: hd }));
    });
  }
  function ws(path) {
    return token().then(function (t) {
      return new Promise(function (ok, bad) {
        var s = new WebSocket(BASE.replace(/^http/, 'ws') + path, ['gw-companion.v1', 'gw-companion.codes', 'bearer.' + t]);
        s.binaryType = 'arraybuffer';
        s.onopen = function () {
          if (s.protocol === 'gw-companion.codes') { s.onclose = function (e) { bad(new Error(e.reason || 'refused (' + e.code + ')')); }; return; }
          s.onopen = s.onerror = s.onclose = null; ok(s);
        };
        s.onerror = s.onclose = function (e) { bad(new Error((e && e.reason) || 'could not connect to the app')); };
      });
    });
  }
  function caps() {
    if (!helloP) helloP = fetch(BASE + '/v1/hello', { credentials: 'omit' }).then(function (r) { return r.json(); }).then(function (j) { return j.capabilities || []; }).catch(function () { helloP = null; return []; });
    return helloP;
  }
  function on(ev, fn) {
    (listeners[ev] = listeners[ev] || []).push(fn);
    // Events that arrived before anyone listened (a file to open at launch) are delivered now.
    var q = queued[ev]; delete queued[ev];
    if (q) setTimeout(function () { q.forEach(function (d) { fn(d); }); }, 0);
    return function () { var l = listeners[ev] || [], i = l.indexOf(fn); if (i >= 0) l.splice(i, 1); };
  }
  function emit(ev, data) {
    var l = (listeners[ev] || []).slice();
    if (!l.length) { (queued[ev] = queued[ev] || []).push(data); return; }
    l.forEach(function (fn) { try { fn(data); } catch (e) { console.error(e); } });
  }
  Object.defineProperty(window, '__GW_APP__', { value: Object.freeze({ window: WIN, companion: BASE, tool: tool, version: 1 }) });
  Object.defineProperty(window, '__gwNative', { value: Object.freeze({ fetch: nfetch, ws: ws, caps: caps, on: on, emit: emit, token: token }) });
})();
