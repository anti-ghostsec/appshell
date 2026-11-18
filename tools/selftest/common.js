// Shared by the self-test scripts: the shell defines window.__appshellReport.
(function () {
  var errors = [];
  window.addEventListener('error', function (e) { errors.push(String(e.message) + ' @' + e.filename + ':' + e.lineno); });
  window.addEventListener('unhandledrejection', function (e) { errors.push('rejection: ' + String(e.reason && e.reason.stack || e.reason)); });
  var origError = console.error;
  console.error = function () { errors.push('console: ' + Array.prototype.join.call(arguments, ' ')); return origError.apply(console, arguments); };
  window.__t = {
    sleep: function (ms) { return new Promise(function (r) { setTimeout(r, ms); }); },
    state: function () { return null; },
    until: async function (what, f, ms) {
      ms = ms || 60000;
      var t0 = Date.now();
      for (;;) {
        var v;
        try { v = await f(); } catch (e) { v = null; }
        if (v) return v;
        if (Date.now() - t0 > ms) throw new Error('timed out waiting for ' + what);
        await new Promise(function (r) { setTimeout(r, 150); });
      }
    },
    run: function (name, body) {
      window.addEventListener('load', function () {
        body().then(
          async function (r) {
            // let the window settle (fonts, the last render) so a screenshot shows it as a person would see it
            try { await document.fonts.ready; } catch (e) {}
            await new Promise(function (res) { setTimeout(res, 800); });
            window.__appshellReport(Object.assign({ ok: true, test: name, errors: errors }, r || {}));
          },
          function (e) {
            var state = null;
            try { state = window.__t.state(); } catch (x) { state = 'state: ' + x; }
            window.__appshellReport({ ok: false, test: name, error: String(e && e.stack || e), errors: errors, state: state,
              body: document.body ? document.body.innerText.slice(0, 600) : null });
          });
      });
    },
  };
})();
