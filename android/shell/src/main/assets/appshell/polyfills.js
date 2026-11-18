// Newer JavaScript built-ins that PDF.js 6 uses, for Android WebViews that
// haven't been updated in a while. Each is only defined where it's missing;
// an up-to-date WebView never runs any of this. The Android shell puts this
// in front of the page and of PDF.js's worker.
(function (g) {
  'use strict';
  function def(o, k, v) {
    if (o && typeof o[k] !== 'function') Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  }
  def(Promise, 'withResolvers', function () {
    var res, rej;
    var p = new this(function (a, b) { res = a; rej = b; });
    return { promise: p, resolve: res, reject: rej };
  });
  def(Promise, 'try', function (f) {
    var args = Array.prototype.slice.call(arguments, 1);
    return new this(function (r) { r(f.apply(undefined, args)); });
  });
  def(Math, 'sumPrecise', function (items) {
    var s = 0, c = 0;
    for (var x of items) { var y = x - c, t = s + y; c = (t - s) - y; s = t; }
    return s;
  });
  [g.Map, g.WeakMap].forEach(function (M) {
    if (!M) return;
    def(M.prototype, 'getOrInsert', function (k, v) { if (!this.has(k)) this.set(k, v); return this.get(k); });
    def(M.prototype, 'getOrInsertComputed', function (k, f) { if (!this.has(k)) this.set(k, f(k)); return this.get(k); });
  });
  def(Set.prototype, 'intersection', function (other) {
    var out = new Set();
    for (var v of this) if (other.has(v)) out.add(v);
    return out;
  });
  def(Array.prototype, 'toReversed', function () { return this.slice().reverse(); });
  def(Array.prototype, 'findLast', function (f, t) {
    for (var i = this.length - 1; i >= 0; i--) if (f.call(t, this[i], i, this)) return this[i];
    return undefined;
  });
  def(RegExp, 'escape', function (s) { return String(s).replace(/[\\^$.*+?()[\]{}|\/\-]/g, '\\$&'); });
  if (g.ArrayBuffer) def(ArrayBuffer.prototype, 'transferToFixedLength', function (n) {
    var len = n === undefined ? this.byteLength : n;
    var out = new ArrayBuffer(len);
    new Uint8Array(out).set(new Uint8Array(this, 0, Math.min(len, this.byteLength)));
    return out;
  });
  [g.Response, g.Blob].forEach(function (C) {
    if (C) def(C.prototype, 'bytes', function () { return this.arrayBuffer().then(function (b) { return new Uint8Array(b); }); });
  });
  def(Uint8Array, 'fromBase64', function (s) {
    var bin = atob(String(s).replace(/\s+/g, ''));
    var u = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
    return u;
  });
  def(Uint8Array.prototype, 'toBase64', function () {
    var s = '';
    for (var i = 0; i < this.length; i += 0x8000) s += String.fromCharCode.apply(null, this.subarray(i, i + 0x8000));
    return btoa(s);
  });
  def(Uint8Array.prototype, 'toHex', function () {
    var s = '';
    for (var i = 0; i < this.length; i++) s += (this[i] < 16 ? '0' : '') + this[i].toString(16);
    return s;
  });
  // Iterator: the global itself, and the helpers.
  var P = Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()));
  if (typeof g.Iterator !== 'function') {
    var It = function Iterator() {};
    It.prototype = P;
    Object.defineProperty(g, 'Iterator', { value: It, writable: true, configurable: true });
  }
  def(P, 'toArray', function () { var out = []; for (var v of this) out.push(v); return out; });
  def(P, 'forEach', function (f) { var i = 0; for (var v of this) f(v, i++); });
  def(P, 'some', function (f) { var i = 0; for (var v of this) if (f(v, i++)) return true; return false; });
  def(P, 'every', function (f) { var i = 0; for (var v of this) if (!f(v, i++)) return false; return true; });
  def(P, 'find', function (f) { var i = 0; for (var v of this) if (f(v, i++)) return v; return undefined; });
  def(P, 'reduce', function (f, acc) {
    var i = 0, first = arguments.length < 2;
    for (var v of this) { if (first) { acc = v; first = false; } else acc = f(acc, v, i); i++; }
    return acc;
  });
  def(P, 'map', function* (f) { var i = 0; for (var v of this) yield f(v, i++); });
  def(P, 'filter', function* (f) { var i = 0; for (var v of this) if (f(v, i++)) yield v; });
  def(P, 'flatMap', function* (f) { var i = 0; for (var v of this) yield* f(v, i++); });
  def(P, 'take', function* (n) { if (n <= 0) return; for (var v of this) { yield v; if (--n <= 0) return; } });
  def(P, 'drop', function* (n) { for (var v of this) { if (n > 0) { n--; continue; } yield v; } });
})(typeof globalThis !== 'undefined' ? globalThis : self);
