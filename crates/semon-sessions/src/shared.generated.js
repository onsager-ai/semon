if (!globalThis.__semonUIShared) {
/*! Preact 10.29.8
The MIT License (MIT)

Copyright (c) 2015-present Jason Miller

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/
"use strict";
var __semonUIShared = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // src/shared.ts
  var shared_exports = {};
  __export(shared_exports, {
    createSelect: () => createSelect,
    enhanceSelect: () => enhanceSelect,
    hooks: () => hooks_module_exports,
    installSelect: () => installSelect,
    jsxRuntime: () => jsxRuntime_module_exports,
    preact: () => preact_module_exports
  });

  // node_modules/preact/dist/preact.module.js
  var preact_module_exports = {};
  __export(preact_module_exports, {
    Component: () => C,
    Fragment: () => S,
    cloneElement: () => W,
    createContext: () => X,
    createElement: () => k,
    createRef: () => M,
    h: () => k,
    hydrate: () => U,
    isValidElement: () => t,
    options: () => l,
    render: () => R,
    toChildArray: () => F
  });
  var n;
  var l;
  var u;
  var t;
  var i;
  var r;
  var o;
  var e;
  var f;
  var c;
  var a;
  var s;
  var h;
  var p;
  var v;
  var y;
  var d = {};
  var w = [];
  var _ = /acit|ex(?:s|g|n|p|$)|rph|grid|ows|mnc|ntw|ine[ch]|zoo|^ord|itera/i;
  var g = Array.isArray;
  function m(n3, l4) {
    for (var u4 in l4) n3[u4] = l4[u4];
    return n3;
  }
  function b(n3) {
    n3 && n3.parentNode && n3.parentNode.removeChild(n3);
  }
  function k(l4, u4, t4) {
    var i4, r3, o4, e3 = {};
    for (o4 in u4) "key" == o4 ? i4 = u4[o4] : "ref" == o4 ? r3 = u4[o4] : e3[o4] = u4[o4];
    if (arguments.length > 2 && (e3.children = arguments.length > 3 ? n.call(arguments, 2) : t4), "function" == typeof l4 && null != l4.defaultProps) for (o4 in l4.defaultProps) void 0 === e3[o4] && (e3[o4] = l4.defaultProps[o4]);
    return x(l4, e3, i4, r3, null);
  }
  function x(n3, t4, i4, r3, o4) {
    var e3 = { type: n3, props: t4, key: i4, ref: r3, __k: null, __: null, __b: 0, __e: null, __c: null, constructor: void 0, __v: null == o4 ? ++u : o4, __i: -1, __u: 0 };
    return null == o4 && null != l.vnode && l.vnode(e3), e3;
  }
  function M() {
    return { current: null };
  }
  function S(n3) {
    return n3.children;
  }
  function C(n3, l4) {
    this.props = n3, this.context = l4;
  }
  function $(n3, l4) {
    if (null == l4) return n3.__ ? $(n3.__, n3.__i + 1) : null;
    for (var u4; l4 < n3.__k.length; l4++) if (null != (u4 = n3.__k[l4]) && null != u4.__e) return u4.__e;
    return "function" == typeof n3.type ? $(n3) : null;
  }
  function I(n3) {
    if (n3.__P && n3.__d) {
      var u4 = n3.__v, t4 = u4.__e, i4 = [], r3 = [], o4 = m({}, u4);
      o4.__v = u4.__v + 1, l.vnode && l.vnode(o4), q(n3.__P, o4, u4, n3.__n, n3.__P.namespaceURI, 32 & u4.__u ? [t4] : null, i4, null == t4 ? $(u4) : t4, !!(32 & u4.__u), r3), o4.__v = u4.__v, o4.__.__k[o4.__i] = o4, D(i4, o4, r3), u4.__e = u4.__ = null, o4.__e != t4 && P(o4);
    }
  }
  function P(n3) {
    if (null != (n3 = n3.__) && null != n3.__c) return n3.__e = n3.__c.base = null, n3.__k.some(function(l4) {
      if (null != l4 && null != l4.__e) return n3.__e = n3.__c.base = l4.__e;
    }), P(n3);
  }
  function A(n3) {
    (!n3.__d && (n3.__d = true) && i.push(n3) && !H.__r++ || r != l.debounceRendering) && ((r = l.debounceRendering) || o)(H);
  }
  function H() {
    try {
      for (var n3, l4 = 1; i.length; ) i.length > l4 && i.sort(e), n3 = i.shift(), l4 = i.length, I(n3);
    } finally {
      i.length = H.__r = 0;
    }
  }
  function L(n3, l4, u4, t4, i4, r3, o4, e3, f4, c4, a4) {
    var s4, h3, p4, v3, y3, _3, g3 = t4 && t4.__k || w, m3 = l4.length;
    for (f4 = T(u4, l4, g3, f4, m3), s4 = 0; s4 < m3; s4++) null != (p4 = u4.__k[s4]) && (h3 = -1 != p4.__i && g3[p4.__i] || d, p4.__i = s4, _3 = q(n3, p4, h3, i4, r3, o4, e3, f4, c4, a4), v3 = p4.__e, p4.ref && h3.ref != p4.ref && (h3.ref && J(h3.ref, null, p4), a4.push(p4.ref, p4.__c || v3, p4)), null == y3 && null != v3 && (y3 = v3), 4 & p4.__u ? (f4 = j(p4, f4, n3), h3.__e && (h3.__e = null)) : "function" == typeof p4.type && void 0 !== _3 ? f4 = _3 : v3 && (f4 = v3.nextSibling), p4.__u &= -7);
    return u4.__e = y3, f4;
  }
  function T(n3, l4, u4, t4, i4) {
    var r3, o4, e3, f4, c4, a4 = u4.length, s4 = a4, h3 = 0;
    for (n3.__k = new Array(i4), r3 = 0; r3 < i4; r3++) null != (o4 = l4[r3]) && "boolean" != typeof o4 && "function" != typeof o4 ? ("string" == typeof o4 || "number" == typeof o4 || "bigint" == typeof o4 || o4.constructor == String ? o4 = n3.__k[r3] = x(null, o4, null, null, null) : g(o4) ? o4 = n3.__k[r3] = x(S, { children: o4 }, null, null, null) : void 0 === o4.constructor && o4.__b > 0 ? o4 = n3.__k[r3] = x(o4.type, o4.props, o4.key, o4.ref ? o4.ref : null, o4.__v) : n3.__k[r3] = o4, f4 = r3 + h3, o4.__ = n3, o4.__b = n3.__b + 1, e3 = null, -1 != (c4 = o4.__i = O(o4, u4, f4, s4)) && (s4--, (e3 = u4[c4]) && (e3.__u |= 2)), null == e3 || null == e3.__v ? (-1 == c4 && (i4 > a4 ? h3-- : i4 < a4 && h3++), "function" != typeof o4.type && (o4.__u |= 4)) : c4 != f4 && (c4 == f4 - 1 ? h3-- : c4 == f4 + 1 ? h3++ : (c4 > f4 ? h3-- : h3++, o4.__u |= 4))) : n3.__k[r3] = null;
    if (s4) for (r3 = 0; r3 < a4; r3++) null != (e3 = u4[r3]) && 0 == (2 & e3.__u) && (e3.__e == t4 && (t4 = $(e3)), K(e3, e3));
    return t4;
  }
  function j(n3, l4, u4) {
    var t4, i4;
    if ("function" == typeof n3.type) {
      for (t4 = n3.__k, i4 = 0; t4 && i4 < t4.length; i4++) t4[i4] && (t4[i4].__ = n3, l4 = j(t4[i4], l4, u4));
      return l4;
    }
    n3.__e != l4 && (l4 && n3.type && !l4.parentNode && (l4 = $(n3)), l4 = u4.insertBefore(n3.__e, l4 || null));
    do {
      l4 = l4 && l4.nextSibling;
    } while (null != l4 && 8 == l4.nodeType);
    return l4;
  }
  function F(n3, l4) {
    return l4 = l4 || [], null == n3 || "boolean" == typeof n3 || (g(n3) ? n3.some(function(n4) {
      F(n4, l4);
    }) : l4.push(n3)), l4;
  }
  function O(n3, l4, u4, t4) {
    var i4, r3, o4, e3 = n3.key, f4 = n3.type, c4 = l4[u4], a4 = null != c4 && 0 == (2 & c4.__u);
    if (null === c4 && null == e3 || a4 && e3 == c4.key && f4 == c4.type) return u4;
    if (t4 > (a4 ? 1 : 0)) {
      for (i4 = u4 - 1, r3 = u4 + 1; i4 >= 0 || r3 < l4.length; ) if (null != (c4 = l4[o4 = i4 >= 0 ? i4-- : r3++]) && 0 == (2 & c4.__u) && e3 == c4.key && f4 == c4.type) return o4;
    }
    return -1;
  }
  function z(n3, l4, u4) {
    "-" == l4[0] ? n3.setProperty(l4, null == u4 ? "" : u4) : n3[l4] = null == u4 ? "" : "number" != typeof u4 || _.test(l4) ? u4 : u4 + "px";
  }
  function N(n3, l4, u4, t4, i4) {
    var r3, o4;
    n: if ("style" == l4) if ("string" == typeof u4) n3.style.cssText = u4;
    else {
      if ("string" == typeof t4 && (n3.style.cssText = t4 = ""), t4) for (l4 in t4) u4 && l4 in u4 || z(n3.style, l4, "");
      if (u4) for (l4 in u4) t4 && u4[l4] == t4[l4] || z(n3.style, l4, u4[l4]);
    }
    else if ("o" == l4[0] && "n" == l4[1]) r3 = l4 != (l4 = l4.replace(s, "$1")), o4 = l4.toLowerCase(), l4 = o4 in n3 || "onFocusOut" == l4 || "onFocusIn" == l4 ? o4.slice(2) : l4.slice(2), n3.l || (n3.l = {}), n3.l[l4 + r3] = u4, u4 ? t4 ? u4[a] = t4[a] : (u4[a] = h, n3.addEventListener(l4, r3 ? v : p, r3)) : n3.removeEventListener(l4, r3 ? v : p, r3);
    else {
      if ("http://www.w3.org/2000/svg" == i4) l4 = l4.replace(/xlink(H|:h)/, "h").replace(/sName$/, "s");
      else if ("width" != l4 && "height" != l4 && "href" != l4 && "list" != l4 && "form" != l4 && "tabIndex" != l4 && "download" != l4 && "rowSpan" != l4 && "colSpan" != l4 && "role" != l4 && "popover" != l4 && l4 in n3) try {
        n3[l4] = null == u4 ? "" : u4;
        break n;
      } catch (n4) {
      }
      "function" == typeof u4 || (null == u4 || false === u4 && "-" != l4[4] ? n3.removeAttribute(l4) : n3.setAttribute(l4, "popover" == l4 && 1 == u4 ? "" : u4));
    }
  }
  function V(n3) {
    return function(u4) {
      if (this.l) {
        var t4 = this.l[u4.type + n3];
        if (null == u4[c]) u4[c] = h++;
        else if (u4[c] < t4[a]) return;
        return t4(l.event ? l.event(u4) : u4);
      }
    };
  }
  function q(n3, u4, t4, i4, r3, o4, e3, f4, c4, a4) {
    var s4, h3, p4, v3, y3, d3, _3, k3, x3, M2, I2, P3, A3, H2, T3, j3, F3 = u4.type;
    if (void 0 !== u4.constructor) return null;
    128 & t4.__u && (c4 = !!(32 & t4.__u), o4 = [f4 = u4.__e = t4.__e]), (s4 = l.__b) && s4(u4);
    n: if ("function" == typeof F3) {
      h3 = e3.length;
      try {
        if (x3 = u4.props, M2 = F3.prototype && F3.prototype.render, I2 = (s4 = F3.contextType) && i4[s4.__c], P3 = s4 ? I2 ? I2.props.value : s4.__ : i4, t4.__c ? k3 = (p4 = u4.__c = t4.__c).__ = p4.__E : (M2 ? u4.__c = p4 = new F3(x3, P3) : (u4.__c = p4 = new C(x3, P3), p4.constructor = F3, p4.render = Q), I2 && I2.sub(p4), p4.state || (p4.state = {}), p4.__n = i4, v3 = p4.__d = true, p4.__h = [], p4._sb = []), M2 && null == p4.__s && (p4.__s = p4.state), M2 && null != F3.getDerivedStateFromProps && (p4.__s == p4.state && (p4.__s = m({}, p4.__s)), m(p4.__s, F3.getDerivedStateFromProps(x3, p4.__s))), y3 = p4.props, d3 = p4.state, p4.__v = u4, v3) M2 && null == F3.getDerivedStateFromProps && null != p4.componentWillMount && p4.componentWillMount(), M2 && null != p4.componentDidMount && p4.__h.push(p4.componentDidMount);
        else {
          if (M2 && null == F3.getDerivedStateFromProps && x3 !== y3 && null != p4.componentWillReceiveProps && p4.componentWillReceiveProps(x3, P3), u4.__v == t4.__v || !p4.__e && null != p4.shouldComponentUpdate && false === p4.shouldComponentUpdate(x3, p4.__s, P3)) {
            u4.__v != t4.__v && (p4.props = x3, p4.state = p4.__s, p4.__d = false), u4.__e = t4.__e, u4.__k = t4.__k, u4.__k.some(function(n4) {
              n4 && (n4.__ = u4);
            }), w.push.apply(p4.__h, p4._sb), p4._sb = [], p4.__h.length && e3.push(p4), f4 = $(t4);
            break n;
          }
          null != p4.componentWillUpdate && p4.componentWillUpdate(x3, p4.__s, P3), M2 && null != p4.componentDidUpdate && p4.__h.push(function() {
            p4.componentDidUpdate(y3, d3, _3);
          });
        }
        if (p4.context = P3, p4.props = x3, p4.__P = n3, p4.__e = false, A3 = l.__r, H2 = 0, M2) p4.state = p4.__s, p4.__d = false, A3 && A3(u4), s4 = p4.render(p4.props, p4.state, p4.context), w.push.apply(p4.__h, p4._sb), p4._sb = [];
        else do {
          p4.__d = false, A3 && A3(u4), s4 = p4.render(p4.props, p4.state, p4.context), p4.state = p4.__s;
        } while (p4.__d && ++H2 < 25);
        p4.state = p4.__s, null != p4.getChildContext && (i4 = m(m({}, i4), p4.getChildContext())), M2 && !v3 && null != p4.getSnapshotBeforeUpdate && (_3 = p4.getSnapshotBeforeUpdate(y3, d3)), T3 = null != s4 && s4.type === S && null == s4.key ? E(s4.props.children) : s4, f4 = L(n3, g(T3) ? T3 : [T3], u4, t4, i4, r3, o4, e3, f4, c4, a4), p4.base = u4.__e, u4.__u &= -161, p4.__h.length && e3.push(p4), k3 && (p4.__E = p4.__ = null);
      } catch (n4) {
        if (e3.length = h3, u4.__v = null, c4 || null != o4) {
          if (n4.then) {
            for (u4.__u |= c4 ? 160 : 128; f4 && 8 == f4.nodeType && f4.nextSibling; ) f4 = f4.nextSibling;
            null != o4 && (o4[o4.indexOf(f4)] = null), u4.__e = f4;
          } else if (null != o4) for (j3 = o4.length; j3--; ) b(o4[j3]);
        } else u4.__e = t4.__e;
        null == u4.__k && (u4.__k = t4.__k || []), n4.then || B(u4), l.__e(n4, u4, t4);
      }
    } else null == o4 && u4.__v == t4.__v ? (u4.__k = t4.__k, u4.__e = t4.__e) : f4 = u4.__e = G(t4.__e, u4, t4, i4, r3, o4, e3, c4, a4);
    return (s4 = l.diffed) && s4(u4), 128 & u4.__u ? void 0 : f4;
  }
  function B(n3) {
    n3 && (n3.__c && (n3.__c.__e = true), n3.__k && n3.__k.some(B));
  }
  function D(n3, u4, t4) {
    for (var i4 = 0; i4 < t4.length; i4++) J(t4[i4], t4[++i4], t4[++i4]);
    l.__c && l.__c(u4, n3), n3.some(function(u5) {
      try {
        n3 = u5.__h, u5.__h = [], n3.some(function(n4) {
          n4.call(u5);
        });
      } catch (n4) {
        l.__e(n4, u5.__v);
      }
    });
  }
  function E(n3) {
    return "object" != typeof n3 || null == n3 || n3.__b > 0 ? n3 : g(n3) ? n3.map(E) : void 0 !== n3.constructor ? null : m({}, n3);
  }
  function G(u4, t4, i4, r3, o4, e3, f4, c4, a4) {
    var s4, h3, p4, v3, y3, w3, _3, m3 = i4.props || d, k3 = t4.props, x3 = t4.type;
    if ("svg" == x3 ? o4 = "http://www.w3.org/2000/svg" : "math" == x3 ? o4 = "http://www.w3.org/1998/Math/MathML" : o4 || (o4 = "http://www.w3.org/1999/xhtml"), null != e3) {
      for (s4 = 0; s4 < e3.length; s4++) if ((y3 = e3[s4]) && "setAttribute" in y3 == !!x3 && (x3 ? y3.localName == x3 : 3 == y3.nodeType)) {
        u4 = y3, e3[s4] = null;
        break;
      }
    }
    if (null == u4) {
      if (null == x3) return document.createTextNode(k3);
      u4 = document.createElementNS(o4, x3, k3.is && k3), c4 && (l.__m && l.__m(t4, e3), c4 = false), e3 = null;
    }
    if (null == x3) m3 === k3 || c4 && u4.data == k3 || (u4.data = k3);
    else {
      if (e3 = "textarea" == x3 && null != k3.defaultValue ? null : e3 && n.call(u4.childNodes), !c4 && null != e3) for (m3 = {}, s4 = 0; s4 < u4.attributes.length; s4++) m3[(y3 = u4.attributes[s4]).name] = y3.value;
      for (s4 in m3) y3 = m3[s4], "dangerouslySetInnerHTML" == s4 ? p4 = y3 : "children" == s4 || s4 in k3 || "value" == s4 && "defaultValue" in k3 || "checked" == s4 && "defaultChecked" in k3 || N(u4, s4, null, y3, o4);
      for (s4 in k3) y3 = k3[s4], "children" == s4 ? v3 = y3 : "dangerouslySetInnerHTML" == s4 ? h3 = y3 : "value" == s4 ? w3 = y3 : "checked" == s4 ? _3 = y3 : c4 && "function" != typeof y3 || m3[s4] === y3 || N(u4, s4, y3, m3[s4], o4);
      if (h3) c4 || p4 && (h3.__html == p4.__html || h3.__html == u4.innerHTML) || (u4.innerHTML = h3.__html), t4.__k = [];
      else if (p4 && (u4.innerHTML = ""), L("template" == t4.type ? u4.content : u4, g(v3) ? v3 : [v3], t4, i4, r3, "foreignObject" == x3 ? "http://www.w3.org/1999/xhtml" : o4, e3, f4, e3 ? e3[0] : i4.__k && $(i4, 0), c4, a4), null != e3) for (s4 = e3.length; s4--; ) b(e3[s4]);
      c4 && "textarea" != x3 || (s4 = "value", "progress" == x3 && null == w3 ? u4.removeAttribute("value") : null != w3 && (w3 !== u4[s4] || "progress" == x3 && !w3 || "option" == x3 && w3 != m3[s4]) && N(u4, s4, w3, m3[s4], o4), s4 = "checked", null != _3 && _3 != u4[s4] && N(u4, s4, _3, m3[s4], o4));
    }
    return u4;
  }
  function J(n3, u4, t4) {
    try {
      if ("function" == typeof n3) {
        var i4 = "function" == typeof n3.__u;
        i4 && n3.__u(), i4 && null == u4 || (n3.__u = n3(u4));
      } else n3.current = u4;
    } catch (n4) {
      l.__e(n4, t4);
    }
  }
  function K(n3, u4, t4) {
    var i4, r3;
    if (l.unmount && l.unmount(n3), (i4 = n3.ref) && (i4.current && i4.current != n3.__e || J(i4, null, u4)), null != (i4 = n3.__c)) {
      if (i4.componentWillUnmount) try {
        i4.componentWillUnmount();
      } catch (n4) {
        l.__e(n4, u4);
      }
      i4.base = i4.__P = i4.__n = null;
    }
    if (i4 = n3.__k) for (r3 = 0; r3 < i4.length; r3++) i4[r3] && K(i4[r3], u4, t4 || "function" != typeof n3.type);
    t4 || b(n3.__e), n3.__c = n3.__ = n3.__e = void 0;
  }
  function Q(n3, l4, u4) {
    return this.constructor(n3, u4);
  }
  function R(u4, t4, i4) {
    var r3, o4, e3, f4;
    t4 == document && (t4 = document.documentElement), l.__ && l.__(u4, t4), o4 = (r3 = "function" == typeof i4) ? null : i4 && i4.__k || t4.__k, e3 = [], f4 = [], q(t4, u4 = (!r3 && i4 || t4).__k = k(S, null, [u4]), o4 || d, d, t4.namespaceURI, !r3 && i4 ? [i4] : o4 ? null : t4.firstChild ? n.call(t4.childNodes) : null, e3, !r3 && i4 ? i4 : o4 ? o4.__e : t4.firstChild, r3, f4), D(e3, u4, f4), u4.props.children = null;
  }
  function U(n3, l4) {
    R(n3, l4, U);
  }
  function W(l4, u4, t4) {
    var i4, r3, o4, e3, f4 = m({}, l4.props);
    for (o4 in l4.type && l4.type.defaultProps && (e3 = l4.type.defaultProps), u4) "key" == o4 ? i4 = u4[o4] : "ref" == o4 ? r3 = u4[o4] : f4[o4] = void 0 === u4[o4] && null != e3 ? e3[o4] : u4[o4];
    return arguments.length > 2 && (f4.children = arguments.length > 3 ? n.call(arguments, 2) : t4), x(l4.type, f4, i4 || l4.key, r3 || l4.ref, null);
  }
  function X(n3) {
    function l4(n4) {
      var u4, t4;
      return this.getChildContext || (u4 = /* @__PURE__ */ new Set(), (t4 = {})[l4.__c] = this, this.getChildContext = function() {
        return t4;
      }, this.componentWillUnmount = function() {
        u4 = null;
      }, this.shouldComponentUpdate = function(n5) {
        this.props.value != n5.value && u4.forEach(function(n6) {
          n6.__e = true, A(n6);
        });
      }, this.sub = function(n5) {
        u4.add(n5);
        var l5 = n5.componentWillUnmount;
        n5.componentWillUnmount = function() {
          u4 && u4.delete(n5), l5 && l5.call(n5);
        };
      }), n4.children;
    }
    return l4.__c = "__cC" + y++, l4.__ = n3, l4.Provider = l4.__l = (l4.Consumer = function(n4, l5) {
      return n4.children(l5);
    }).contextType = l4, l4;
  }
  n = w.slice, l = { __e: function(n3, l4, u4, t4) {
    for (var i4, r3, o4; l4 = l4.__; ) if ((i4 = l4.__c) && !i4.__) try {
      if ((r3 = i4.constructor) && null != r3.getDerivedStateFromError && (i4.setState(r3.getDerivedStateFromError(n3)), o4 = i4.__d), null != i4.componentDidCatch && (i4.componentDidCatch(n3, t4 || {}), o4 = i4.__d), o4) return i4.__E = i4;
    } catch (l5) {
      n3 = l5;
    }
    throw n3;
  } }, u = 0, t = function(n3) {
    return null != n3 && void 0 === n3.constructor;
  }, C.prototype.setState = function(n3, l4) {
    var u4;
    u4 = null != this.__s && this.__s != this.state ? this.__s : this.__s = m({}, this.state), "function" == typeof n3 && (n3 = n3(m({}, u4), this.props)), n3 && m(u4, n3), null != n3 && this.__v && (l4 && this._sb.push(l4), A(this));
  }, C.prototype.forceUpdate = function(n3) {
    this.__v && (this.__e = true, n3 && this.__h.push(n3), A(this));
  }, C.prototype.render = S, i = [], o = "function" == typeof Promise ? Promise.prototype.then.bind(Promise.resolve()) : setTimeout, e = function(n3, l4) {
    return n3.__v.__b - l4.__v.__b;
  }, H.__r = 0, f = Math.random().toString(8), c = "__d" + f, a = "__a" + f, s = /(PointerCapture)$|Capture$/i, h = 0, p = V(false), v = V(true), y = 0;

  // node_modules/preact/hooks/dist/hooks.module.js
  var hooks_module_exports = {};
  __export(hooks_module_exports, {
    useCallback: () => q2,
    useContext: () => x2,
    useDebugValue: () => P2,
    useEffect: () => h2,
    useErrorBoundary: () => b2,
    useId: () => g2,
    useImperativeHandle: () => F2,
    useLayoutEffect: () => _2,
    useMemo: () => T2,
    useReducer: () => y2,
    useRef: () => A2,
    useState: () => d2
  });
  var t2;
  var r2;
  var u2;
  var i2;
  var o2 = 0;
  var f2 = [];
  var c2 = l;
  var e2 = c2.__b;
  var a2 = c2.__r;
  var v2 = c2.diffed;
  var l2 = c2.__c;
  var m2 = c2.unmount;
  var p2 = c2.__;
  function s2(n3, t4) {
    c2.__h && c2.__h(r2, n3, o2 || t4), o2 = 0;
    var u4 = r2.__H || (r2.__H = { __: [], __h: [] });
    return n3 >= u4.__.length && u4.__.push({}), u4.__[n3];
  }
  function d2(n3) {
    return o2 = 1, y2(D2, n3);
  }
  function y2(n3, u4, i4) {
    var o4 = s2(t2++, 2);
    if (o4.t = n3, !o4.__c && (o4.__ = [i4 ? i4(u4) : D2(void 0, u4), function(n4) {
      var t4 = o4.__N ? o4.__N[0] : o4.__[0], r3 = o4.t(t4, n4);
      t4 !== r3 && (o4.__N = [r3, o4.__[1]], o4.__c.setState({}));
    }], o4.__c = r2, !r2.__f)) {
      var f4 = function(n4, t4, r3) {
        if (!o4.__c.__H) return true;
        var u5 = false, i5 = o4.__c.props !== n4;
        if (o4.__c.__H.__.some(function(n5) {
          if (n5.__N) {
            u5 = true;
            var t5 = n5.__[0];
            n5.__ = n5.__N, n5.__N = void 0, t5 !== n5.__[0] && (i5 = true);
          }
        }), c4) {
          var f5 = c4.call(this, n4, t4, r3);
          return u5 ? f5 || i5 : f5;
        }
        return !u5 || i5;
      };
      r2.__f = true;
      var c4 = r2.shouldComponentUpdate, e3 = r2.componentWillUpdate;
      r2.componentWillUpdate = function(n4, t4, r3) {
        if (this.__e) {
          var u5 = c4;
          c4 = void 0, f4(n4, t4, r3), c4 = u5;
        }
        e3 && e3.call(this, n4, t4, r3);
      }, r2.shouldComponentUpdate = f4;
    }
    return o4.__N || o4.__;
  }
  function h2(n3, u4) {
    var i4 = s2(t2++, 3);
    !c2.__s && C2(i4.__H, u4) && (i4.__ = n3, i4.u = u4, r2.__H.__h.push(i4));
  }
  function _2(n3, u4) {
    var i4 = s2(t2++, 4);
    !c2.__s && C2(i4.__H, u4) && (i4.__ = n3, i4.u = u4, r2.__h.push(i4));
  }
  function A2(n3) {
    return o2 = 5, T2(function() {
      return { current: n3 };
    }, []);
  }
  function F2(n3, t4, r3) {
    o2 = 6, _2(function() {
      if ("function" == typeof n3) {
        var r4 = n3(t4());
        return function() {
          n3(null), r4 && "function" == typeof r4 && r4();
        };
      }
      if (n3) return n3.current = t4(), function() {
        return n3.current = null;
      };
    }, null == r3 ? r3 : r3.concat(n3));
  }
  function T2(n3, r3) {
    var u4 = s2(t2++, 7);
    return C2(u4.__H, r3) && (u4.__ = n3(), u4.__H = r3, u4.__h = n3), u4.__;
  }
  function q2(n3, t4) {
    return o2 = 8, T2(function() {
      return n3;
    }, t4);
  }
  function x2(n3) {
    var u4 = r2.context[n3.__c], i4 = s2(t2++, 9);
    return i4.c = n3, u4 ? (null == i4.__ && (i4.__ = true, u4.sub(r2)), u4.props.value) : n3.__;
  }
  function P2(n3, t4) {
    c2.useDebugValue && c2.useDebugValue(t4 ? t4(n3) : n3);
  }
  function b2(n3) {
    var u4 = s2(t2++, 10), i4 = d2();
    return u4.__ = n3, r2.componentDidCatch || (r2.componentDidCatch = function(n4, t4) {
      u4.__ && u4.__(n4, t4), i4[1](n4);
    }), [i4[0], function() {
      i4[1](void 0);
    }];
  }
  function g2() {
    var n3 = s2(t2++, 11);
    if (!n3.__) {
      for (var u4 = r2.__v; null !== u4 && !u4.__m && null !== u4.__; ) u4 = u4.__;
      var i4 = u4.__m || (u4.__m = [0, 0]);
      n3.__ = "P" + i4[0] + "-" + i4[1]++;
    }
    return n3.__;
  }
  function j2() {
    for (var n3; n3 = f2.shift(); ) {
      var t4 = n3.__H;
      if (n3.__P && t4) try {
        t4.__h.some(z2), t4.__h.some(B2), t4.__h = [];
      } catch (r3) {
        t4.__h = [], c2.__e(r3, n3.__v);
      }
    }
  }
  c2.__b = function(n3) {
    r2 = null, e2 && e2(n3);
  }, c2.__ = function(n3, t4) {
    n3 && t4.__k && t4.__k.__m && (n3.__m = t4.__k.__m), p2 && p2(n3, t4);
  }, c2.__r = function(n3) {
    a2 && a2(n3), t2 = 0;
    var i4 = (r2 = n3.__c).__H;
    i4 && (u2 === r2 ? (i4.__h = [], r2.__h = [], i4.__.some(function(n4) {
      n4.__N && (n4.__ = n4.__N), n4.u = n4.__N = void 0;
    })) : (i4.__h.some(z2), i4.__h.some(B2), i4.__h = [], t2 = 0)), u2 = r2;
  }, c2.diffed = function(n3) {
    v2 && v2(n3);
    var t4 = n3.__c;
    t4 && t4.__H && (t4.__H.__h.length && (1 !== f2.push(t4) && i2 === c2.requestAnimationFrame || ((i2 = c2.requestAnimationFrame) || w2)(j2)), t4.__H.__.some(function(n4) {
      n4.u && (n4.__H = n4.u, n4.u = void 0);
    })), u2 = r2 = null;
  }, c2.__c = function(n3, t4) {
    t4.some(function(n4) {
      try {
        n4.__h.some(z2), n4.__h = n4.__h.filter(function(n5) {
          return !n5.__ || B2(n5);
        });
      } catch (r3) {
        t4.some(function(n5) {
          n5.__h && (n5.__h = []);
        }), t4 = [], c2.__e(r3, n4.__v);
      }
    }), l2 && l2(n3, t4);
  }, c2.unmount = function(n3) {
    m2 && m2(n3);
    var t4, r3 = n3.__c;
    r3 && r3.__H && (r3.__H.__.some(function(n4) {
      try {
        z2(n4);
      } catch (n5) {
        t4 = n5;
      }
    }), r3.__H = void 0, t4 && c2.__e(t4, r3.__v));
  };
  var k2 = "function" == typeof requestAnimationFrame;
  function w2(n3) {
    var t4, r3 = function() {
      clearTimeout(u4), k2 && cancelAnimationFrame(t4), setTimeout(n3);
    }, u4 = setTimeout(r3, 35);
    k2 && (t4 = requestAnimationFrame(r3));
  }
  function z2(n3) {
    var t4 = r2, u4 = n3.__c;
    "function" == typeof u4 && (n3.__c = void 0, u4()), r2 = t4;
  }
  function B2(n3) {
    var t4 = r2;
    n3.__c = n3.__(), r2 = t4;
  }
  function C2(n3, t4) {
    return !n3 || n3.length !== t4.length || t4.some(function(t5, r3) {
      return t5 !== n3[r3];
    });
  }
  function D2(n3, t4) {
    return "function" == typeof t4 ? t4(n3) : t4;
  }

  // node_modules/preact/jsx-runtime/dist/jsxRuntime.module.js
  var jsxRuntime_module_exports = {};
  __export(jsxRuntime_module_exports, {
    Fragment: () => S,
    jsx: () => u3,
    jsxAttr: () => l3,
    jsxDEV: () => u3,
    jsxEscape: () => s3,
    jsxTemplate: () => a3,
    jsxs: () => u3
  });
  var t3 = /["&<]/;
  function n2(r3) {
    if (0 === r3.length || false === t3.test(r3)) return r3;
    for (var e3 = 0, n3 = 0, o4 = "", f4 = ""; n3 < r3.length; n3++) {
      switch (r3.charCodeAt(n3)) {
        case 34:
          f4 = "&quot;";
          break;
        case 38:
          f4 = "&amp;";
          break;
        case 60:
          f4 = "&lt;";
          break;
        default:
          continue;
      }
      n3 !== e3 && (o4 += r3.slice(e3, n3)), o4 += f4, e3 = n3 + 1;
    }
    return n3 !== e3 && (o4 += r3.slice(e3, n3)), o4;
  }
  var o3 = /acit|ex(?:s|g|n|p|$)|rph|grid|ows|mnc|ntw|ine[ch]|zoo|^ord|itera/i;
  var f3 = 0;
  var i3 = Array.isArray;
  function u3(e3, t4, n3, o4, i4, u4) {
    t4 || (t4 = {});
    var a4, c4, p4 = t4;
    if ("ref" in p4) for (c4 in p4 = {}, t4) "ref" == c4 ? a4 = t4[c4] : p4[c4] = t4[c4];
    var l4 = { type: e3, props: p4, key: n3, ref: a4, __k: null, __: null, __b: 0, __e: null, __c: null, constructor: void 0, __v: --f3, __i: -1, __u: 0, __source: i4, __self: u4 };
    if ("function" == typeof e3 && (a4 = e3.defaultProps)) for (c4 in a4) void 0 === p4[c4] && (p4[c4] = a4[c4]);
    return l.vnode && l.vnode(l4), l4;
  }
  function a3(r3) {
    var t4 = u3(S, { tpl: r3, exprs: [].slice.call(arguments, 1) });
    return t4.key = t4.__v, t4;
  }
  var c3 = {};
  var p3 = /[A-Z]/g;
  function l3(e3, t4) {
    if (l.attr) {
      var f4 = l.attr(e3, t4);
      if ("string" == typeof f4) return f4;
    }
    if (t4 = (function(r3) {
      return null !== r3 && "object" == typeof r3 && "function" == typeof r3.valueOf ? r3.valueOf() : r3;
    })(t4), "ref" === e3 || "key" === e3) return "";
    if ("style" === e3 && "object" == typeof t4) {
      var i4 = "";
      for (var u4 in t4) {
        var a4 = t4[u4];
        if (null != a4 && "" !== a4) {
          var l4 = "-" == u4[0] ? u4 : c3[u4] || (c3[u4] = u4.replace(p3, "-$&").toLowerCase()), s4 = ";";
          "number" != typeof a4 || l4.startsWith("--") || o3.test(l4) || (s4 = "px;"), i4 = i4 + l4 + ":" + a4 + s4;
        }
      }
      return e3 + '="' + n2(i4) + '"';
    }
    return null == t4 || false === t4 || "function" == typeof t4 || "object" == typeof t4 ? "" : true === t4 ? e3 : e3 + '="' + n2("" + t4) + '"';
  }
  function s3(r3) {
    if (null == r3 || "boolean" == typeof r3 || "function" == typeof r3) return null;
    if ("object" == typeof r3) {
      if (void 0 === r3.constructor) return r3;
      if (i3(r3)) {
        for (var e3 = 0; e3 < r3.length; e3++) r3[e3] = s3(r3[e3]);
        return r3;
      }
    }
    return n2("" + r3);
  }

  // src/lib/account.ts
  var safePath = (href) => typeof href === "string" && href.startsWith("/") && !href.startsWith("//") && !href.includes("\\") && !/[\u0000-\u001f\u007f-\u009f]/.test(href) && href.length <= 512;

  // src/lib/richtext.tsx
  function externalUrl(value) {
    if (typeof value !== "string" || !/^https?:\/\//i.test(value)) return false;
    try {
      return /^https?:$/.test(new URL(value).protocol);
    } catch {
      return false;
    }
  }

  // src/lib/security.ts
  var installed = false;
  function installPropGuard() {
    if (installed) return;
    installed = true;
    const previous = l.vnode;
    l.vnode = (vnode) => {
      previous?.(vnode);
      if (typeof vnode.type === "string") {
        for (const key of Object.keys(vnode.props)) {
          if (["dangerouslySetInnerHTML", "style", "title"].includes(key))
            throw new Error(`Forbidden DOM prop: ${key}`);
          const value = vnode.props[key];
          const externalLink = key === "href" && vnode.type === "a" && externalUrl(value) && vnode.props.target === "_blank" && vnode.props.rel === "noopener noreferrer";
          if (["href", "src", "action"].includes(key) && value != null && !safePath(value) && !externalLink)
            throw new Error(`Unsafe DOM path: ${key}`);
          if (/^on/i.test(key) && typeof value === "string")
            throw new Error(`Inline DOM handler: ${key}`);
        }
      }
    };
  }

  // src/lib/layout.ts
  var properties = [
    "width",
    "height",
    "left",
    "top",
    "margin-left",
    "min-width",
    "max-height",
    "max-width"
  ];
  var serial = 0;
  function createMeasuredLayout() {
    const sheet2 = new CSSStyleSheet(), prefix = "semon-geometry-" + ++serial + "-";
    const rules = /* @__PURE__ */ new Map();
    let disposed = false;
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet2];
    return {
      reset() {
        if (disposed) return;
        rules.clear();
        sheet2.replaceSync("");
      },
      className(property, value, unit) {
        if (disposed) throw new Error("Measured layout is destroyed");
        if (!properties.includes(property) || !Number.isFinite(value) || Math.abs(value) > 1e8 || !["px", "%"].includes(unit))
          throw new Error("Invalid measured geometry");
        const key = property + ":" + value + unit, existing = rules.get(key);
        if (existing) return existing;
        const name = prefix + rules.size;
        sheet2.insertRule(
          "." + name + "{" + property + ":" + value + unit + "}",
          sheet2.cssRules.length
        );
        rules.set(key, name);
        return name;
      },
      destroy() {
        if (disposed) return;
        disposed = true;
        document.adoptedStyleSheets = document.adoptedStyleSheets.filter((value) => value !== sheet2);
        rules.clear();
        sheet2.replaceSync("");
      }
    };
  }

  // src/lib/select.tsx
  var ICON = {
    chevron: "M6 9l6 6 6-6",
    check: "M5 12.5l4.5 4.5L19 7",
    search: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4",
    close: "M6 6l12 12M18 6L6 18"
  };
  function Icon({ path, className }) {
    return /* @__PURE__ */ u3(
      "svg",
      {
        class: className,
        viewBox: "0 0 24 24",
        fill: "none",
        stroke: "currentColor",
        "stroke-width": "2",
        "stroke-linecap": "round",
        "stroke-linejoin": "round",
        "aria-hidden": "true",
        children: /* @__PURE__ */ u3("path", { d: path })
      }
    );
  }
  var serial2 = 0;
  var sheet = null;
  var pendingOpen = null;
  var swallow = 0;
  var sheetList = null;
  var touchY = 0;
  var installed2 = false;
  var phone = () => window.matchMedia("(max-width: 760px)").matches;
  function orphaned() {
    if (!sheetList || sheetList.isConnected) return false;
    if (sheet) sheet.orphaned();
    else lock(null);
    return true;
  }
  function refuse(event) {
    if (orphaned() || !sheetList) return;
    if (event instanceof TouchEvent && event.touches.length > 1) return;
    if (event instanceof TouchEvent && event.type === "touchstart") {
      touchY = event.touches[0]?.clientY ?? 0;
      return;
    }
    const nextTouchY = event instanceof TouchEvent ? event.touches[0]?.clientY ?? touchY : touchY;
    const touchDelta = touchY - nextTouchY;
    touchY = nextTouchY;
    if (!(event.target instanceof Node) || !sheetList.contains(event.target)) {
      event.preventDefault();
      return;
    }
    const room = sheetList.scrollHeight - sheetList.clientHeight, delta = event instanceof WheelEvent ? event.deltaY : event instanceof TouchEvent ? touchDelta : 0;
    if (room <= 1 || delta > 0 && sheetList.scrollTop >= room - 1 || delta < 0 && sheetList.scrollTop <= 0)
      event.preventDefault();
  }
  function lock(list) {
    sheetList = list;
    for (const type of ["wheel", "touchmove"]) {
      document.removeEventListener(type, refuse, true);
      if (list) document.addEventListener(type, refuse, { capture: true, passive: false });
    }
    document.removeEventListener("touchstart", refuse, true);
    if (list) document.addEventListener("touchstart", refuse, { capture: true, passive: true });
  }
  function createSelect(config = {}) {
    const n3 = ++serial2, listId = "sh-select-list-" + n3, labelId = "sh-select-label-" + n3, valueId = "sh-select-value-" + n3, label = config.label ?? "", searchAbove = config.searchAbove ?? 8;
    const root = document.createElement("div");
    root.className = "sh-select";
    if (label) root.dataset.label = label;
    const layout = createMeasuredLayout(), triggerRoot = document.createElement("div"), park = document.createElement("div"), pop = document.createElement("div"), parts = document.createElement("div");
    triggerRoot.className = "sh-select-trigger-slot";
    parts.className = "sh-select-parts";
    park.hidden = true;
    pop.className = "sh-select-pop";
    pop.hidden = true;
    root.append(triggerRoot, pop, park);
    park.append(parts);
    let options = [], value = config.value ?? "", opened = null, query = "", active = null, typed = "", typedAt = 0, disposed = false;
    let trigger = null, search = null, list = null, dialog = null, popClass = "", sizing = null;
    const shown = () => {
      const q3 = query.trim().toLowerCase();
      return q3 ? options.filter((o4) => o4.label.toLowerCase().includes(q3)) : options;
    };
    const owner = () => !opened ? null : opened.sheet ? list : opened.search ? search : trigger;
    const descendant = () => active === null ? void 0 : listId + "-" + options.findIndex((o4) => o4.value === active);
    function paintTrigger() {
      const text = options.find((o4) => o4.value === value)?.label ?? "";
      R(
        /* @__PURE__ */ u3(
          "button",
          {
            class: "sh-select-trigger",
            type: "button",
            role: "combobox",
            "aria-haspopup": "listbox",
            "aria-expanded": !!opened,
            "aria-controls": listId,
            "aria-labelledby": (label ? labelId + " " : "") + valueId,
            "aria-activedescendant": owner() === trigger ? descendant() : void 0,
            ref: (node) => {
              trigger = node;
            },
            onClick: (event) => {
              if (event.currentTarget.isConnected && !disposed) {
                if (opened) close();
                else openList();
              }
            },
            onKeyDown: onKey,
            onKeyUp: (event) => {
              if (event.key === " ") event.preventDefault();
            },
            children: [
              /* @__PURE__ */ u3(
                "span",
                {
                  class: "sh-select-text",
                  "data-tip-clipped": "",
                  "data-tip": (label ? label + ": " : "") + text,
                  children: [
                    /* @__PURE__ */ u3("span", { id: labelId, class: "sh-select-label", children: label ? label + ":" : "" }),
                    label ? " " : "",
                    /* @__PURE__ */ u3("span", { id: valueId, class: "sh-select-value", children: text })
                  ]
                }
              ),
              /* @__PURE__ */ u3(Icon, { path: ICON.chevron, className: "sh-select-chevron" })
            ]
          }
        ),
        triggerRoot
      );
    }
    function paintList() {
      const items = shown();
      R(
        /* @__PURE__ */ u3(S, { children: [
          /* @__PURE__ */ u3("div", { class: "sh-select-search", hidden: !opened?.search, children: [
            /* @__PURE__ */ u3(Icon, { path: ICON.search }),
            /* @__PURE__ */ u3(
              "input",
              {
                type: "search",
                autoComplete: "off",
                spellcheck: false,
                placeholder: "Search",
                "aria-label": "Search " + (label || "options"),
                role: "combobox",
                "aria-autocomplete": "list",
                "aria-expanded": "true",
                "aria-controls": listId,
                "aria-activedescendant": owner() === search ? descendant() : void 0,
                value: query,
                ref: (node) => {
                  search = node;
                },
                onKeyDown: onKey,
                onInput: (event) => {
                  if (!event.currentTarget.isConnected || disposed) return;
                  query = event.currentTarget.value;
                  paintList();
                  settle();
                  position();
                }
              }
            )
          ] }),
          /* @__PURE__ */ u3(
            "div",
            {
              class: "sh-select-list",
              id: listId,
              role: "listbox",
              "aria-label": label || void 0,
              tabIndex: -1,
              "aria-activedescendant": owner() === list ? descendant() : void 0,
              ref: (node) => {
                list = node;
              },
              onKeyDown: onKey,
              children: items.map((option) => /* @__PURE__ */ u3(
                "div",
                {
                  class: "sh-select-option" + (option.value === active ? " sh-active" : ""),
                  id: listId + "-" + options.indexOf(option),
                  "data-value": option.value,
                  role: "option",
                  "aria-selected": option.value === value,
                  onPointerDown: (event) => event.preventDefault(),
                  onPointerMove: (event) => {
                    if (event.currentTarget.isConnected && option.value !== active)
                      setActive(option.value, false);
                  },
                  onClick: (event) => {
                    if (event.currentTarget.isConnected && !disposed) choose(option.value);
                  },
                  children: [
                    /* @__PURE__ */ u3(Icon, { path: ICON.check, className: "sh-select-check" }),
                    /* @__PURE__ */ u3("span", { class: "sh-select-option-text", children: option.label })
                  ]
                },
                option.value
              ))
            }
          ),
          /* @__PURE__ */ u3("div", { class: "sh-select-empty", role: "status", hidden: items.length > 0, children: "No matches" })
        ] }),
        parts
      );
    }
    function setActive(next, scroll = true) {
      active = next;
      paintList();
      paintTrigger();
      if (scroll && active !== null)
        document.getElementById(descendant())?.scrollIntoView({ block: "nearest" });
    }
    function settle() {
      const items = shown();
      setActive(
        items.some((o4) => o4.value === active) ? active : items.some((o4) => o4.value === value) ? value : items[0]?.value ?? null
      );
    }
    function move(to) {
      const items = shown();
      if (!items.length) return;
      const at = items.findIndex((o4) => o4.value === active);
      setActive(items[Math.max(0, Math.min(items.length - 1, to(at, items.length)))].value);
    }
    function geometry(left, top, min, max) {
      layout.reset();
      popClass = [
        layout.className("left", left, "px"),
        layout.className("top", top, "px"),
        layout.className("min-width", min, "px"),
        ...max === void 0 ? [] : [layout.className("max-height", max, "px")]
      ].join(" ");
      pop.className = "sh-select-pop " + popClass;
    }
    function position() {
      if (!opened || opened.sheet || !trigger) return;
      const r3 = trigger.getBoundingClientRect(), vw = document.documentElement.clientWidth, vh = innerHeight, gap = 4, margin = 8, min = Math.max(r3.width, 200);
      geometry(0, 0, min);
      const bar = document.getElementById("topbar"), floor = Math.max(margin, bar ? bar.getBoundingClientRect().bottom + gap : 0), width = Math.min(pop.offsetWidth, vw - 2 * margin), height = pop.offsetHeight, below = vh - r3.bottom - gap - margin, above = r3.top - gap - floor;
      const up = height > below && above > below, room = Math.max(48, Math.min(up ? above : below, 360));
      geometry(
        Math.max(margin, Math.min(r3.left, vw - width - margin)),
        Math.max(floor, up ? r3.top - gap - Math.min(height, room) : r3.bottom + gap),
        min,
        room
      );
      pop.dataset.side = up ? "top" : "bottom";
    }
    const outside = (event) => {
      if (opened && event.target instanceof Node && !root.contains(event.target)) close(false);
    };
    const reposition = (event) => {
      if (!(event.target instanceof Node) || !pop.contains(event.target)) position();
    };
    function openList() {
      if (opened || disposed || !root.isConnected) return;
      const asSheet = phone() && typeof HTMLDialogElement.prototype.showModal === "function";
      if (asSheet && swallow) {
        pendingOpen = openList;
        return;
      }
      query = "";
      opened = { sheet: asSheet, search: options.length > searchAbove };
      paintTrigger();
      paintList();
      if (asSheet) {
        dialog = document.createElement("dialog");
        dialog.className = "sh-select-sheet";
        dialog.setAttribute("aria-label", label || "Options");
        const head = document.createElement("div");
        head.className = "sh-select-sheet-h";
        R(
          /* @__PURE__ */ u3(S, { children: [
            /* @__PURE__ */ u3("span", { class: "sh-select-sheet-title", children: label }),
            /* @__PURE__ */ u3("button", { class: "sh-select-close", type: "button", "aria-label": "Close", onClick: () => close(), children: /* @__PURE__ */ u3(Icon, { path: ICON.close }) })
          ] }),
          head
        );
        dialog.append(head, parts);
        root.append(dialog);
        dialog.addEventListener("click", (event) => {
          if (event.target === dialog) close();
        });
        dialog.addEventListener("close", () => {
          if (opened && !dialog?.open) close();
        });
        dialog.showModal();
        lock(list);
        try {
          history.pushState({ ...history.state, shSelect: n3 }, "");
          opened.entry = true;
          sheet = {
            orphaned() {
              close(false);
            },
            popped() {
              if (opened) opened.entry = false;
              close();
            }
          };
        } catch {
        }
        settle();
        list?.focus({ preventScroll: true });
      } else {
        pop.append(parts);
        pop.hidden = false;
        position();
        settle();
        document.addEventListener("pointerdown", outside, true);
        window.addEventListener("resize", position);
        document.addEventListener("scroll", reposition, true);
        if (opened.search) search?.focus({ preventScroll: true });
      }
    }
    function close(refocus = true) {
      if (!opened) return;
      const was = opened;
      opened = null;
      typed = "";
      active = null;
      if (was.sheet) {
        if (dialog?.open) dialog.close();
        if (dialog) {
          const head = dialog.firstElementChild;
          if (head) R(null, head);
          dialog.remove();
          dialog = null;
        }
        lock(null);
        sheet = null;
        if (was.entry && history.state?.shSelect === n3) {
          swallow++;
          history.back();
        }
      } else {
        if (refocus) trigger?.focus({ preventScroll: true });
        pop.hidden = true;
        document.removeEventListener("pointerdown", outside, true);
        window.removeEventListener("resize", position);
        document.removeEventListener("scroll", reposition, true);
      }
      park.append(parts);
      paintTrigger();
      paintList();
      if (refocus) trigger?.focus({ preventScroll: true });
    }
    function choose(next) {
      const changed = next !== value;
      close();
      if (changed) {
        value = next;
        paintTrigger();
        config.onChange?.(value, api);
      }
    }
    function typeahead(ch) {
      const now = Date.now();
      typed = now - typedAt > 500 ? ch : typed + ch;
      typedAt = now;
      const items = shown(), at = items.findIndex((o4) => o4.value === active), cycle = [...typed].every((c4) => c4 === typed[0]), key = cycle ? typed[0] : typed, from = cycle ? at + 1 : Math.max(0, at);
      const hit = [...items.slice(from), ...items.slice(0, from)].find(
        (o4) => o4.label.toLowerCase().startsWith(key)
      );
      if (hit) setActive(hit.value);
    }
    function toSearch(text) {
      search?.focus({ preventScroll: true });
      query += text;
      paintList();
      search?.setSelectionRange(query.length, query.length);
      settle();
      position();
    }
    function onKey(event) {
      if (disposed || !(event.currentTarget instanceof HTMLElement) || !event.currentTarget.isConnected)
        return;
      const key = event.key, inField = event.target === search, printable = key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
      if (!opened) {
        if (["ArrowDown", "ArrowUp", "Enter", " "].includes(key)) {
          event.preventDefault();
          openList();
        } else if (key === "Home" || key === "End") {
          event.preventDefault();
          openList();
          move((_3, len) => key === "Home" ? 0 : len - 1);
        } else if (printable) {
          event.preventDefault();
          openList();
          if (options.length > searchAbove) toSearch(key);
          else typeahead(key.toLowerCase());
        }
        return;
      }
      if (key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        close();
      } else if (key === "ArrowDown") {
        event.preventDefault();
        if (!event.altKey) move((at) => at + 1);
      } else if (key === "ArrowUp") {
        event.preventDefault();
        if (event.altKey) {
          if (active !== null) choose(active);
          else close();
        } else move((at, len) => at < 0 ? len - 1 : at - 1);
      } else if (key === "PageDown" || key === "PageUp") {
        event.preventDefault();
        move((at) => at + (key === "PageDown" ? 10 : -10));
      } else if ((key === "Home" || key === "End") && !inField) {
        event.preventDefault();
        move((_3, len) => key === "Home" ? 0 : len - 1);
      } else if (key === "Enter") {
        event.preventDefault();
        if (active !== null) choose(active);
        else close();
      } else if (key === " " && !inField) {
        event.preventDefault();
        if (Date.now() - typedAt < 500 && typed) typeahead(" ");
        else if (active !== null) choose(active);
      } else if (key === "Tab") {
        if (opened.sheet) return;
        const next = active;
        close();
        if (next !== null && next !== value) {
          value = next;
          paintTrigger();
          config.onChange?.(value, api);
        }
      } else if (printable && !inField) {
        event.preventDefault();
        if (opened.search) toSearch(key);
        else typeahead(key.toLowerCase());
      }
    }
    pop.addEventListener("mousedown", (event) => {
      if (event.target !== search) event.preventDefault();
    });
    root.addEventListener("focusout", (event) => {
      if (opened && !opened.sheet && (!(event.relatedTarget instanceof Node) || !root.contains(event.relatedTarget)))
        close(false);
    });
    const api = {
      el: root,
      get value() {
        return value;
      },
      get options() {
        return options.map((option) => ({ ...option }));
      },
      get isOpen() {
        return !!opened;
      },
      setOptions(next) {
        if (disposed) return;
        options = next.map((option) => ({
          value: String(option.value),
          label: String(option.label ?? option.value)
        }));
        paintTrigger();
        if (opened) {
          paintList();
          settle();
          position();
        }
      },
      setValue(next) {
        if (disposed) return;
        value = String(next ?? "");
        paintTrigger();
        if (opened) paintList();
      },
      open: openList,
      close,
      focus() {
        trigger?.focus();
      },
      destroy() {
        if (disposed) return;
        if (pendingOpen === openList) pendingOpen = null;
        close(false);
        disposed = true;
        if (sizing !== null) cancelAnimationFrame(sizing);
        R(null, triggerRoot);
        R(null, parts);
        layout.destroy();
        root.remove();
      }
    };
    root.semonSelect = api;
    api.setOptions(config.options ?? []);
    paintList();
    return api;
  }
  function enhanceSelect(native) {
    if (native.dataset.selectReady !== void 0) return null;
    const api = createSelect({
      label: native.dataset.label ?? native.getAttribute("aria-label") ?? "",
      value: native.value,
      options: [...native.options].map((option) => ({
        value: option.value,
        label: option.textContent?.trim() ?? ""
      })),
      onChange(value) {
        native.value = value;
        native.dispatchEvent(new Event("change", { bubbles: true }));
      }
    });
    native.dataset.selectReady = "";
    native.hidden = true;
    native.after(api.el);
    return api;
  }
  function installSelect() {
    if (installed2) return;
    installed2 = true;
    window.addEventListener("popstate", (event) => {
      if (swallow) {
        swallow--;
        if (!swallow && pendingOpen) {
          const open = pendingOpen;
          pendingOpen = null;
          queueMicrotask(open);
        }
        event.stopImmediatePropagation();
        return;
      }
      if (sheet && !orphaned()) {
        event.stopImmediatePropagation();
        sheet.popped();
      }
    });
    const run = () => document.querySelectorAll("select[data-select]").forEach(enhanceSelect);
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", run);
    else run();
  }

  // src/lib/tooltip.tsx
  var installed3 = false;
  function installTooltip() {
    if (installed3) return;
    installed3 = true;
    const layout = createMeasuredLayout();
    const SHOW_DELAY = 500, SKIP_WINDOW = 300, MARGIN = 8, GAP = 6, MAX_WIDTH = 280, ID = "sh-tooltip";
    const INTERACTIVE = 'a[href], button, input, select, textarea, summary, label, [role="button"], [role="link"], [role="menuitem"], [role="tab"], [contenteditable="true"]';
    const HAS_TIP = '[data-tip]:not([data-tip=""])';
    let tip = null;
    let target = null;
    let opened = false;
    let timer = 0;
    let closedAt = -Infinity;
    let dismissed = null;
    let pointerType = "mouse";
    let described = null;
    let watcher = null;
    let anchor = null;
    let pointer = null;
    let origin = "pointer";
    const tipOf = (node) => node instanceof Element ? node.closest(HAS_TIP) : null;
    const focusVisible = (node) => {
      try {
        return node instanceof Element && node.matches(":focus-visible");
      } catch {
        return true;
      }
    };
    const shown = (node) => node.isConnected && node.getClientRects().length > 0;
    const dismiss = () => {
      dismissed = target ? { node: target, text: target.getAttribute("data-tip"), x: pointer?.x, y: pointer?.y } : dismissed;
    };
    const isDismissed = (node) => !!dismissed && !!node && (node === dismissed.node || pointer != null && pointer.x === dismissed.x && pointer.y === dismissed.y && node.getAttribute("data-tip") === dismissed.text);
    const clipped = (node) => node.scrollWidth > node.clientWidth + 1 || node.scrollHeight > node.clientHeight + 1;
    function element() {
      if (tip) return tip;
      tip = document.createElement("div");
      tip.id = ID;
      tip.className = "tip";
      tip.hidden = true;
      tip.setAttribute("role", "tooltip");
      if (typeof tip.showPopover === "function") tip.setAttribute("popover", "manual");
      document.body.append(tip);
      return tip;
    }
    function place(node) {
      const box = node.getBoundingClientRect(), width = document.documentElement.clientWidth, height = document.documentElement.clientHeight;
      layout.reset();
      const maxWidth = layout.className("max-width", Math.min(MAX_WIDTH, width - 2 * MARGIN), "px");
      tip.className = "tip " + maxWidth + " " + layout.className("left", 0, "px") + " " + layout.className("top", 0, "px");
      const size = tip.getBoundingClientRect(), need = size.height + GAP + MARGIN, room = { top: box.top, bottom: height - box.bottom };
      const side = room.top >= need ? "top" : room.bottom >= need ? "bottom" : room.top >= room.bottom ? "top" : "bottom";
      const top = side === "top" ? box.top - size.height - GAP : box.bottom + GAP;
      const left = Math.min(
        Math.max(box.left + box.width / 2 - size.width / 2, MARGIN),
        Math.max(MARGIN, width - MARGIN - size.width)
      );
      tip.className = "tip " + maxWidth + " " + layout.className("left", Math.round(left), "px") + " " + layout.className(
        "top",
        Math.round(
          Math.min(Math.max(top, MARGIN), Math.max(MARGIN, height - MARGIN - size.height))
        ),
        "px"
      );
      tip.dataset.side = side;
    }
    function link(node, text) {
      const label = node.getAttribute("aria-label") ?? "";
      const before = node.getAttribute("aria-describedby");
      const said = (before ?? "").split(/\s+/).some((id) => id && document.getElementById(id)?.textContent?.includes(text));
      if (label && label.includes(text) || node.textContent?.trim() === text || said) return;
      node.setAttribute("aria-describedby", (before ? before + " " : "") + ID);
      described = { node, before };
    }
    function unlink() {
      if (!described) return;
      const { node, before } = described;
      if (before == null) node.removeAttribute("aria-describedby");
      else node.setAttribute("aria-describedby", before);
      described = null;
    }
    function successor() {
      const found = origin === "focus" ? tipOf(document.activeElement) : origin === "pointer" && pointer ? tipOf(document.elementFromPoint(pointer.x, pointer.y)) : null;
      return found && found !== target && shown(found) && !(found.hasAttribute("data-tip-clipped") && !clipped(found)) ? found : null;
    }
    function watch() {
      if (watcher || !window.MutationObserver) return;
      watcher = new MutationObserver(() => {
        if (!opened || !target) return;
        const text = target.getAttribute("data-tip");
        if (!shown(target)) {
          const next = successor();
          if (next) show(next, true);
          else hide();
          return;
        }
        if (!text) {
          hide();
          return;
        }
        if (text !== tip.textContent) {
          unlink();
          R(text, tip);
          link(target, text);
          place(target);
        }
      });
      watcher.observe(document.documentElement, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ["data-tip", "hidden"]
      });
    }
    function unwatch() {
      watcher?.disconnect();
      watcher = null;
    }
    function show(node, instant) {
      clearTimeout(timer);
      timer = 0;
      const text = node.getAttribute("data-tip");
      if (!text || !shown(node) || node.hasAttribute("data-tip-clipped") && !clipped(node)) {
        if (target === node && !opened) target = null;
        return;
      }
      const box = element();
      unlink();
      target = node;
      R(text, box);
      box.toggleAttribute("data-instant", instant);
      box.hidden = false;
      if (box.hasAttribute("popover") && !box.matches(":popover-open")) {
        try {
          box.showPopover();
        } catch {
        }
      }
      place(node);
      const at = node.getBoundingClientRect();
      anchor = { top: at.top, left: at.left };
      link(node, text);
      opened = true;
      watch();
    }
    function hide(moved = false) {
      clearTimeout(timer);
      timer = 0;
      if (opened) {
        unlink();
        unwatch();
        if (tip.hasAttribute("popover") && tip.matches(":popover-open")) {
          try {
            tip.hidePopover();
          } catch {
          }
        }
        tip.hidden = true;
        closedAt = moved ? performance.now() : -Infinity;
      }
      opened = false;
      target = null;
    }
    function schedule(node) {
      target = node;
      if (performance.now() - closedAt < SKIP_WINDOW) show(node, true);
      else
        timer = window.setTimeout(() => {
          const now = shown(node) ? node : successor();
          if (now) show(now, false);
        }, SHOW_DELAY);
    }
    document.addEventListener("pointerover", (event) => {
      pointerType = event.pointerType || "mouse";
      if (event.pointerType === "touch") return;
      pointer = { x: event.clientX, y: event.clientY };
      origin = "pointer";
      const node = tipOf(event.target);
      if (!isDismissed(node)) dismissed = null;
      if (node === target) return;
      if (node && isDismissed(node)) return;
      hide(true);
      if (node) schedule(node);
    });
    document.addEventListener(
      "pointermove",
      (event) => {
        if (event.pointerType === "touch") return;
        const over = dismissed ? tipOf(event.target) : null;
        if (over && dismissed && isDismissed(over)) {
          dismissed.node = over;
          dismissed.x = event.clientX;
          dismissed.y = event.clientY;
        }
        pointer = { x: event.clientX, y: event.clientY };
      },
      { capture: true, passive: true }
    );
    document.addEventListener("pointerout", (event) => {
      if (!event.relatedTarget && event.pointerType !== "touch") hide(true);
    });
    document.addEventListener(
      "pointerdown",
      (event) => {
        pointerType = event.pointerType || "mouse";
        if (event.pointerType === "touch") return;
        dismiss();
        hide();
      },
      true
    );
    document.addEventListener("focusin", (event) => {
      const focused = event.target, node = tipOf(focused);
      if (!node || !focusVisible(focused)) return;
      hide(true);
      target = node;
      origin = "focus";
      window.requestAnimationFrame(() => {
        if (target === node && document.activeElement === focused) show(node, true);
      });
    });
    document.addEventListener("focusout", (event) => {
      if (target && tipOf(event.target) === target) hide(true);
    });
    document.addEventListener(
      "keydown",
      (event) => {
        if (event.key !== "Escape" || !target) return;
        const inDialog = opened && !!target.closest?.("dialog[open]");
        dismiss();
        hide();
        if (inDialog) {
          event.preventDefault();
          event.stopPropagation();
        }
      },
      true
    );
    document.addEventListener(
      "click",
      (event) => {
        if (pointerType !== "touch") return;
        const node = tipOf(event.target), wasOpen = opened && target === node;
        hide();
        if (!node || wasOpen || event.target instanceof Element && event.target.closest(INTERACTIVE) || node.closest(INTERACTIVE))
          return;
        target = node;
        origin = "touch";
        show(node, true);
      },
      true
    );
    window.addEventListener(
      "scroll",
      () => {
        if (!opened || !target || !anchor) return;
        const box = target.getBoundingClientRect();
        if (Math.abs(box.top - anchor.top) > 0.5 || Math.abs(box.left - anchor.left) > 0.5) hide();
      },
      { capture: true, passive: true }
    );
    window.addEventListener("resize", () => hide());
    window.addEventListener("blur", () => hide());
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) hide();
    });
  }

  // src/shared.ts
  installPropGuard();
  var shell = window.SemonShell ?? {};
  shell.select = createSelect;
  shell.enhance = enhanceSelect;
  window.SemonShell = shell;
  installSelect();
  installTooltip();
  return __toCommonJS(shared_exports);
})();

globalThis.__semonUIShared = __semonUIShared;
}
