// Shared by the self-test scripts: the shell defines window.__appshellReport.
window.__t = {
  sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
  async until(what, f, ms = 60000) {
    const t0 = Date.now();
    for (;;) {
      let v;
      try { v = await f(); } catch (e) { v = null; }
      if (v) return v;
      if (Date.now() - t0 > ms) throw new Error('timed out waiting for ' + what);
      await new Promise((r) => setTimeout(r, 150));
    }
  },
  run(name, body) {
    window.addEventListener('load', () => {
      body().then(
        (r) => window.__appshellReport(Object.assign({ ok: true, test: name }, r || {})),
        (e) => window.__appshellReport({ ok: false, test: name, error: String(e && e.stack || e) }),
      );
    });
  },
};
