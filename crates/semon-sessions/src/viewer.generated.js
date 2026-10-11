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
// Generated by ui/build.mjs; edit sources and run npm --prefix ui run build.
"use strict";
(() => {
  // shared-preact:preact
  var Component = globalThis.__semonUIShared.preact.Component;
  var Fragment = globalThis.__semonUIShared.preact.Fragment;
  var h = globalThis.__semonUIShared.preact.h;
  var render = globalThis.__semonUIShared.preact.render;
  var options = globalThis.__semonUIShared.preact.options;
  var createElement = globalThis.__semonUIShared.preact.createElement;
  var cloneElement = globalThis.__semonUIShared.preact.cloneElement;
  var createContext = globalThis.__semonUIShared.preact.createContext;
  var createRef = globalThis.__semonUIShared.preact.createRef;
  var hydrate = globalThis.__semonUIShared.preact.hydrate;
  var isValidElement = globalThis.__semonUIShared.preact.isValidElement;
  var toChildArray = globalThis.__semonUIShared.preact.toChildArray;

  // src/lib/account.ts
  var safePath = (href) => typeof href === "string" && href.startsWith("/") && !href.startsWith("//") && !href.includes("\\") && !/[\u0000-\u001f\u007f-\u009f]/.test(href) && href.length <= 512;
  var textField = (value, min, max) => typeof value === "string" && [...value].length >= min && [...value].length <= max && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
  var record = (x) => !!x && typeof x === "object";
  function parseAccount(source) {
    let value;
    try {
      if (!record(source)) return null;
      const list2 = (xs, max, pick) => {
        if (!Array.isArray(xs)) return null;
        const n = xs.length;
        if (!(n <= max)) return null;
        const out = [];
        for (let i = 0; i < n; i++) {
          const x = xs[i];
          out.push(record(x) ? pick(x) : null);
        }
        return out;
      };
      value = {
        name: source.name,
        login: source.login,
        initials: source.initials,
        avatar_href: source.avatar_href ?? null,
        workspaces: list2(source.workspaces, 50, (w) => ({
          name: w.name,
          role: w.role,
          current: w.current,
          switch_href: w.switch_href
        })),
        links: list2(source.links, 12, (a) => ({
          label: a.label,
          href: a.href,
          method: a.method,
          danger: a.danger
        }))
      };
    } catch {
      return null;
    }
    if (!textField(value.name, 1, 80) || !value.name.trim() || !textField(value.login, 0, 80) || !textField(value.initials, 1, 3) || !value.initials.trim())
      return null;
    if (value.avatar_href !== null && !safePath(value.avatar_href)) return null;
    if (!value.workspaces || !value.links) return null;
    const workspaces = value.workspaces;
    const links = value.links;
    if (workspaces.some(
      (w) => !w || !textField(w.name, 1, 80) || !w.name.trim() || !textField(w.role, 0, 80) || typeof w.current !== "boolean" || !safePath(w.switch_href)
    ))
      return null;
    if (links.some(
      (a) => !a || !textField(a.label, 1, 80) || !a.label.trim() || !safePath(a.href) || a.method !== "get" && a.method !== "post" || typeof a.danger !== "boolean"
    ))
      return null;
    return {
      name: value.name,
      login: value.login,
      initials: value.initials,
      avatar_href: value.avatar_href,
      workspaces,
      links
    };
  }

  // shared-preact:preact/jsx-runtime
  var Fragment2 = globalThis.__semonUIShared.jsxRuntime.Fragment;
  var jsx = globalThis.__semonUIShared.jsxRuntime.jsx;
  var jsxs = globalThis.__semonUIShared.jsxRuntime.jsxs;
  var jsxDEV = globalThis.__semonUIShared.jsxRuntime.jsxDEV;
  var jsxAttr = globalThis.__semonUIShared.jsxRuntime.jsxAttr;
  var jsxEscape = globalThis.__semonUIShared.jsxRuntime.jsxEscape;
  var jsxTemplate = globalThis.__semonUIShared.jsxRuntime.jsxTemplate;

  // src/lib/richtext.tsx
  function externalUrl(value) {
    if (typeof value !== "string" || !/^https?:\/\//i.test(value)) return false;
    try {
      return /^https?:$/.test(new URL(value).protocol);
    } catch {
      return false;
    }
  }
  function Link({ text: text2, url }) {
    return externalUrl(url) ? /* @__PURE__ */ jsx("a", { href: url, target: "_blank", rel: "noopener noreferrer", children: text2 }) : /* @__PURE__ */ jsx(Fragment2, { children: text2 });
  }
  function Inline({ text: text2 }) {
    const nodes = [];
    const re = /(`[^`]+`|\*\*(?:`[^`\n]*`|[^*`\n])*?(?:\*\*|…\s*$)|~~[^~\n]+~~|\[[^\]\n]+\]\((?:[^()\s]|\([^()\s]*\))+\)|https?:\/\/[^\s<>`)\]]+|(?<![\w*])\*[^*\s](?:[^*\n]*[^*\s])?\*(?![\w*]))/g;
    let last = 0, match;
    while (match = re.exec(text2)) {
      if (match.index > last) nodes.push(text2.slice(last, match.index));
      let token = match[0];
      if (token[0] === "`") nodes.push(/* @__PURE__ */ jsx("code", { children: token.slice(1, -1) }));
      else if (token.startsWith("**"))
        nodes.push(
          /* @__PURE__ */ jsx("strong", { children: /* @__PURE__ */ jsx(Inline, { text: token.endsWith("**") ? token.slice(2, -2) : token.slice(2) }) })
        );
      else if (token.startsWith("~~"))
        nodes.push(
          /* @__PURE__ */ jsx("del", { children: /* @__PURE__ */ jsx(Inline, { text: token.slice(2, -2) }) })
        );
      else if (token[0] === "[") {
        const k = token.indexOf("](");
        nodes.push(/* @__PURE__ */ jsx(Link, { text: token.slice(1, k), url: token.slice(k + 2, -1) }));
      } else if (token[0] === "*")
        nodes.push(
          /* @__PURE__ */ jsx("em", { children: /* @__PURE__ */ jsx(Inline, { text: token.slice(1, -1) }) })
        );
      else {
        token = token.replace(/[.,;:!?'"…]+$/, "");
        nodes.push(/* @__PURE__ */ jsx(Link, { text: token, url: token }));
        re.lastIndex = match.index + token.length;
      }
      last = match.index + token.length;
    }
    if (last < text2.length) nodes.push(text2.slice(last));
    return nodes;
  }
  var cells = (line) => line.trim().replace(/^\|/, "").replace(/\|$/, "").split(/(?<!\\)\|/).map((c) => c.trim().replace(/\\\|/g, "|"));
  var separator = (line) => /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*…?\s*$/.test(line) && line.includes("-");
  function blocks(text2) {
    const output = [], lines = text2.split("\n");
    let lists = [];
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i], fence = /^\s*(```|~~~)\s*([\w+#.-]*)/.exec(line);
      if (fence) {
        lists = [];
        const code = [];
        for (i++; i < lines.length && !lines[i].trimStart().startsWith(fence[1]); i++)
          code.push(lines[i]);
        output.push({ kind: "code", text: code.join("\n"), language: fence[2] });
        continue;
      }
      if (/^\s*\|/.test(line) && separator(lines[i + 1] ?? "")) {
        lists = [];
        const rows = [];
        for (i += 2; i < lines.length && /^\s*\|/.test(lines[i]); i++) rows.push(cells(lines[i]));
        output.push({ kind: "table", head: cells(line), rows });
        i--;
        continue;
      }
      if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
        lists = [];
        output.push({ kind: "rule" });
        continue;
      }
      const item2 = /^(\s*)([-*]|\d{1,3}[.)])\s+(.*)$/.exec(line);
      if (item2) {
        const indent = item2[1].replace(/\t/g, "    ").length, ordered = /\d/.test(item2[2]);
        while (lists.length && (indent < lists.at(-1).indent || indent === lists.at(-1).indent && lists.at(-1).block.ordered !== ordered))
          lists.pop();
        let top = lists.at(-1);
        if (!top || indent > top.indent) {
          const block = {
            kind: "list",
            ordered,
            start: ordered ? parseInt(item2[2], 10) : 1,
            items: []
          };
          const parent = top?.block.items.at(-1);
          (parent ? parent.children : output).push(block);
          lists.push(top = { indent, block });
        }
        top.block.items.push({ lines: [item2[3]], children: [] });
        continue;
      }
      if (lists.length && /^\s+\S/.test(line)) {
        lists.at(-1).block.items.at(-1).lines.push(line.trim());
        continue;
      }
      lists = [];
      const heading = /^(#{1,6})\s+(.*)$/.exec(line);
      if (heading) {
        output.push({ kind: "heading", text: heading[2], level: heading[1].length });
        continue;
      }
      if (/^\s*>/.test(line)) {
        const quote = [];
        for (; i < lines.length && /^\s*>/.test(lines[i]); i++)
          quote.push(lines[i].replace(/^\s*>\s?/, ""));
        output.push({ kind: "quote", lines: quote });
        i--;
        continue;
      }
      output.push({ kind: "paragraph", text: line });
    }
    return output;
  }
  function Blocks({ items }) {
    return items.map((block, i) => {
      switch (block.kind) {
        case "paragraph":
          return /* @__PURE__ */ jsx("p", { children: /* @__PURE__ */ jsx(Inline, { text: block.text }) }, i);
        case "heading":
          return /* @__PURE__ */ jsx(
            "p",
            {
              class: "mh mh" + Math.min(block.level ?? 1, 3),
              role: "heading",
              "aria-level": Math.min(6, (block.level ?? 1) + 2),
              children: /* @__PURE__ */ jsx(Inline, { text: block.text })
            },
            i
          );
        case "rule":
          return /* @__PURE__ */ jsx("hr", {}, i);
        case "code":
          return /* @__PURE__ */ jsx(
            "div",
            {
              class: "codeblock",
              tabIndex: 0,
              role: "region",
              "aria-label": (block.language ? block.language + " " : "") + "code",
              children: /* @__PURE__ */ jsx("pre", { children: block.text })
            },
            i
          );
        case "table":
          return /* @__PURE__ */ jsx("div", { class: "tbl", tabIndex: 0, role: "region", "aria-label": "Table", children: /* @__PURE__ */ jsxs("table", { children: [
            /* @__PURE__ */ jsx("thead", { children: /* @__PURE__ */ jsx("tr", { children: block.head.map((cell, j) => /* @__PURE__ */ jsx("th", { children: /* @__PURE__ */ jsx(Inline, { text: cell }) }, j)) }) }),
            /* @__PURE__ */ jsx("tbody", { children: block.rows.map((row, j) => /* @__PURE__ */ jsx("tr", { children: row.map((cell, k) => /* @__PURE__ */ jsx("td", { children: /* @__PURE__ */ jsx(Inline, { text: cell }) }, k)) }, j)) })
          ] }) }, i);
        case "quote":
          return /* @__PURE__ */ jsx("blockquote", { children: block.lines.map((line, j) => /* @__PURE__ */ jsx("p", { children: /* @__PURE__ */ jsx(Inline, { text: line }) }, j)) }, i);
        case "list": {
          const children = block.items.map((item2, j) => /* @__PURE__ */ jsxs("li", { children: [
            item2.lines.map((line, k) => /* @__PURE__ */ jsxs(Fragment2, { children: [
              k > 0 && /* @__PURE__ */ jsx("br", {}),
              /* @__PURE__ */ jsx(Inline, { text: line })
            ] })),
            /* @__PURE__ */ jsx(Blocks, { items: item2.children })
          ] }, j));
          return block.ordered ? /* @__PURE__ */ jsx("ol", { start: block.start === 1 ? void 0 : block.start, children }, i) : /* @__PURE__ */ jsx("ul", { children }, i);
        }
      }
    });
  }
  function Markdown({ text: text2, className = "body" }) {
    return /* @__PURE__ */ jsx("div", { class: className + " md", children: /* @__PURE__ */ jsx(Blocks, { items: blocks(text2) }) });
  }
  function MarkdownContent({ text: text2 }) {
    return /* @__PURE__ */ jsx(Blocks, { items: blocks(text2) });
  }

  // src/lib/security.ts
  var installed = false;
  function installPropGuard() {
    if (installed) return;
    installed = true;
    const previous = options.vnode;
    options.vnode = (vnode) => {
      previous?.(vnode);
      if (typeof vnode.type === "string") {
        for (const key2 of Object.keys(vnode.props)) {
          if (["dangerouslySetInnerHTML", "style", "title"].includes(key2))
            throw new Error(`Forbidden DOM prop: ${key2}`);
          const value = vnode.props[key2];
          const externalLink = key2 === "href" && vnode.type === "a" && externalUrl(value) && vnode.props.target === "_blank" && vnode.props.rel === "noopener noreferrer";
          if (["href", "src", "action"].includes(key2) && value != null && !safePath(value) && !externalLink)
            throw new Error(`Unsafe DOM path: ${key2}`);
          if (/^on/i.test(key2) && typeof value === "string")
            throw new Error(`Inline DOM handler: ${key2}`);
        }
      }
    };
  }

  // src/lib/AccountMenu.tsx
  var AccountAvatar = class extends Component {
    state = { failed: false };
    render() {
      const { account } = this.props;
      return /* @__PURE__ */ jsxs("span", { class: "account-avatar", children: [
        account.initials,
        account.avatar_href && !this.state.failed && /* @__PURE__ */ jsx("img", { alt: "", src: account.avatar_href, onError: () => this.setState({ failed: true }) })
      ] });
    }
  };
  function Workspace({ workspace }) {
    return /* @__PURE__ */ jsx(
      "form",
      {
        class: "account-menu-form account-workspace-form",
        method: "post",
        action: workspace.switch_href,
        children: /* @__PURE__ */ jsxs(
          "button",
          {
            class: "account-menu-row",
            type: "submit",
            role: "menuitem",
            "aria-current": workspace.current ? "page" : void 0,
            children: [
              /* @__PURE__ */ jsxs("span", { class: "account-row-main", children: [
                /* @__PURE__ */ jsx("span", { class: "account-workspace-name", children: workspace.name }),
                /* @__PURE__ */ jsx("span", { class: "account-role", children: workspace.role })
              ] }),
              workspace.current && /* @__PURE__ */ jsx("span", { class: "account-check", children: "\u2713" })
            ]
          }
        )
      }
    );
  }
  function AccountLink({ link }) {
    const cls = "account-menu-row" + (link.danger ? " danger" : "");
    return link.method === "post" ? /* @__PURE__ */ jsx("form", { class: "account-menu-form account-link-form", method: "post", action: link.href, children: /* @__PURE__ */ jsx("button", { class: cls, type: "submit", role: "menuitem", children: link.label }) }) : /* @__PURE__ */ jsx("a", { class: cls, role: "menuitem", href: link.href, children: link.label });
  }
  function AccountMenu({ account, compact, wide, onWideChange }) {
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsxs("div", { class: "account-identity", children: [
        /* @__PURE__ */ jsx(AccountAvatar, { account }),
        /* @__PURE__ */ jsxs("span", { class: "account-identity-text", children: [
          /* @__PURE__ */ jsx("span", { class: "account-name", children: account.name }),
          /* @__PURE__ */ jsx("span", { class: "account-login-value", children: account.login })
        ] })
      ] }),
      /* @__PURE__ */ jsxs("section", { class: "account-section", children: [
        /* @__PURE__ */ jsx("div", { class: "account-heading", children: "Workspaces" }),
        account.workspaces.map((workspace, i) => /* @__PURE__ */ jsx(Workspace, { workspace }, `${workspace.switch_href}:${i}`))
      ] }),
      !compact && /* @__PURE__ */ jsxs("section", { class: "account-section account-display", children: [
        /* @__PURE__ */ jsx("div", { class: "account-heading", children: "Display" }),
        /* @__PURE__ */ jsxs(
          "button",
          {
            class: "account-menu-row account-switch-row",
            type: "button",
            role: "menuitemcheckbox",
            "aria-checked": wide,
            "data-pref": "wide",
            onClick: (event) => {
              event.stopPropagation();
              onWideChange();
            },
            children: [
              /* @__PURE__ */ jsx("span", { class: "account-row-main", children: "Wide reading mode" }),
              /* @__PURE__ */ jsx("span", { class: "switch", "aria-hidden": "true", children: /* @__PURE__ */ jsx("span", { class: "switch-knob" }) })
            ]
          }
        )
      ] }),
      [false, true].map((danger) => {
        const links = account.links.filter((link) => link.danger === danger);
        return links.length ? /* @__PURE__ */ jsx(
          "section",
          {
            class: "account-section account-links" + (danger ? " account-danger" : ""),
            children: links.map((link, i) => /* @__PURE__ */ jsx(AccountLink, { link }, `${link.method}:${link.href}:${i}`))
          },
          String(danger)
        ) : null;
      })
    ] });
  }

  // src/lib/account-chrome.tsx
  function createAccountChrome(host2) {
    const widgets = /* @__PURE__ */ new Map();
    let active = null;
    let destroyed = false;
    const initialFocus = { focusVisible: false };
    const returnFocus = {
      focusVisible: false,
      preventScroll: true
    };
    const triggerOf = (widget) => widget.root.querySelector(".account-trigger");
    function commit(widget) {
      const { root, props } = widget;
      const { account, compact, wide, onWideChange } = props;
      const current = account.workspaces.find((workspace) => workspace.current);
      const expanded = active === widget;
      render(
        /* @__PURE__ */ jsxs(Fragment2, { children: [
          expanded && compact && /* @__PURE__ */ jsx(
            "div",
            {
              class: "account-backdrop",
              onClick: (event) => {
                event.stopPropagation();
                close();
              }
            },
            "backdrop"
          ),
          /* @__PURE__ */ jsxs(
            "button",
            {
              class: "account-trigger" + (compact ? "" : " account-avatar-button"),
              type: "button",
              "aria-haspopup": "menu",
              "aria-expanded": expanded,
              "aria-label": compact ? account.name + ", " + (current?.name ?? account.login) : account.name + " account menu",
              onClick: (event) => {
                event.stopPropagation();
                toggle(widget);
              },
              children: [
                /* @__PURE__ */ jsx(AccountAvatar, { account }),
                compact && /* @__PURE__ */ jsxs("span", { class: "account-summary", children: [
                  /* @__PURE__ */ jsx("span", { class: "account-summary-name", children: account.name }),
                  /* @__PURE__ */ jsx("span", { class: "account-summary-workspace", children: current?.name ?? account.login })
                ] })
              ]
            },
            "trigger"
          ),
          expanded && /* @__PURE__ */ jsx(
            "div",
            {
              class: "menu account-popover",
              role: "menu",
              "aria-label": "Account",
              onClick: (event) => {
                const target = event.target;
                const link = target instanceof Element ? target.closest("a[href]") : null;
                if (compact && link && host2.navigate(link.href)) event.preventDefault();
              },
              onSubmit: (event) => {
                if (compact && event.target instanceof HTMLFormElement && host2.submit(event.target))
                  event.preventDefault();
              },
              children: /* @__PURE__ */ jsx(
                AccountMenu,
                {
                  account,
                  compact,
                  wide,
                  onWideChange
                }
              )
            },
            "menu"
          )
        ] }),
        root
      );
    }
    function close(options2 = {}) {
      const widget = active;
      if (!widget) return;
      const menu = widget?.root.querySelector(".account-popover");
      const focused = document.activeElement;
      const refocus = !!menu && (!focused || focused === document.body || menu.contains(focused));
      active = null;
      if (widget) commit(widget);
      if (refocus && widget?.root.isConnected) triggerOf(widget).focus(returnFocus);
      host2.closed(options2);
    }
    function toggle(widget) {
      if (destroyed || !widgets.has(widget.root) || !widget.root.isConnected) return;
      if (active) {
        close();
        return;
      }
      const trigger = triggerOf(widget);
      if (widget.props.compact) host2.place(widget.root, trigger);
      active = widget;
      commit(widget);
      host2.opened(widget.props.compact);
      widget.root.querySelector(".account-menu-row")?.focus(initialFocus);
    }
    const outside = (event) => {
      if (active && event.target instanceof Node && !active.root.contains(event.target)) close();
    };
    const pageshow = (event) => {
      if (event.persisted && active) close({ keepEntry: true });
    };
    let listening = false;
    function startListening() {
      if (listening) return;
      listening = true;
      document.addEventListener("click", outside);
      window.addEventListener("pageshow", pageshow);
    }
    function stopListening() {
      if (!listening) return;
      listening = false;
      document.removeEventListener("click", outside);
      window.removeEventListener("pageshow", pageshow);
    }
    function unmount(root) {
      const widget = widgets.get(root);
      if (!widget) return;
      if (active === widget) close();
      render(null, root);
      widgets.delete(root);
      if (!widgets.size) stopListening();
    }
    return {
      get open() {
        return active !== null;
      },
      mount(props) {
        if (destroyed) throw new Error("Account chrome is destroyed");
        const root = document.createElement("div");
        root.className = "account-widget " + (props.compact ? "account-widget-phone" : "account-widget-desktop");
        const widget = { root, props };
        widgets.set(root, widget);
        startListening();
        commit(widget);
        return root;
      },
      close,
      escape: () => close(),
      updateWide(wide) {
        for (const widget of widgets.values()) {
          widget.props = { ...widget.props, wide };
          commit(widget);
        }
      },
      unmount,
      destroy() {
        if (destroyed) return;
        destroyed = true;
        for (const root of widgets.keys()) unmount(root);
        stopListening();
      }
    };
  }

  // src/lib/geometry.ts
  var properties = {
    readingBottom: "--reading-bottom",
    readingClip: "--reading-clip",
    minHeight: "min-height",
    paddingBottom: "padding-bottom",
    scrollPaddingTop: "scroll-padding-top",
    barHeight: "--barh",
    accountLeft: "--account-left",
    accountWidth: "--account-width",
    accountBottom: "--account-bottom",
    intrinsicHeight: "contain-intrinsic-block-size"
  };
  var sheet = null;
  var observer = null;
  var serial = 0;
  var records = /* @__PURE__ */ new Map();
  function paint() {
    if (!sheet) {
      sheet = new CSSStyleSheet();
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    }
    sheet.replaceSync("");
    for (const [node, record4] of records) {
      if (!node.isConnected) {
        records.delete(node);
        continue;
      }
      const declarations = [...record4.values].map(
        ([slot, value]) => properties[slot] + ":" + (slot === "intrinsicHeight" ? "auto " : "") + value + "px !important"
      ).join(";");
      if (declarations)
        sheet.insertRule(
          '[data-semon-geometry="' + record4.id + '"]{' + declarations + "}",
          sheet.cssRules.length
        );
    }
  }
  function setGeometry(node, slot, value) {
    if (!Object.hasOwn(properties, slot) || value !== null && (!Number.isFinite(value) || Math.abs(value) > 1e8))
      throw new Error("Invalid host geometry");
    let record4 = records.get(node);
    if (!record4) {
      record4 = { id: ++serial, values: /* @__PURE__ */ new Map() };
      records.set(node, record4);
      node.dataset.semonGeometry = String(record4.id);
    }
    if (value === null) record4.values.delete(slot);
    else record4.values.set(slot, value);
    paint();
    if (!observer) {
      observer = new MutationObserver(() => {
        if ([...records.keys()].some((node2) => !node2.isConnected)) paint();
      });
      observer.observe(document.documentElement, { childList: true, subtree: true });
    }
  }
  function releaseGeometry(root, slots) {
    for (const [node, record4] of records) {
      if (slots ? node === root : node === root || root.contains(node) || !node.isConnected) {
        if (slots) for (const slot of slots) record4.values.delete(slot);
        if (!slots || !record4.values.size) {
          records.delete(node);
          delete node.dataset.semonGeometry;
        }
      }
    }
    if (records.size) paint();
    else {
      observer?.disconnect();
      observer = null;
      if (sheet)
        document.adoptedStyleSheets = document.adoptedStyleSheets.filter((value) => value !== sheet);
      sheet = null;
    }
  }

  // src/lib/shell.tsx
  var NS = "http://www.w3.org/2000/svg";
  function element(tag, className, id) {
    const node = document.createElement(tag);
    node.className = className;
    if (id) node.id = id;
    return node;
  }
  function svg(path, stroke = "1.8") {
    const node = document.createElementNS(NS, "svg");
    for (const [key2, value] of Object.entries({
      viewBox: "0 0 24 24",
      "aria-hidden": "true",
      fill: "none",
      stroke: "currentColor",
      "stroke-width": stroke,
      "stroke-linecap": "round",
      "stroke-linejoin": "round"
    }))
      node.setAttribute(key2, value);
    const p = document.createElementNS(NS, "path");
    p.setAttribute("d", path);
    node.append(p);
    return node;
  }
  function Icon({ path }) {
    return /* @__PURE__ */ jsx(
      "svg",
      {
        class: "icon",
        viewBox: "0 0 24 24",
        "aria-hidden": "true",
        fill: "none",
        stroke: "currentColor",
        "stroke-width": "1.8",
        "stroke-linecap": "round",
        "stroke-linejoin": "round",
        children: /* @__PURE__ */ jsx("path", { d: path })
      }
    );
  }
  function renderShellNavigation(nav, destinations, navigate) {
    const keys = /* @__PURE__ */ new Set();
    for (const destination of destinations) {
      if (!destination.key || keys.has(destination.key) || !safePath(destination.href))
        throw new Error("Invalid shell destination");
      keys.add(destination.key);
    }
    render(
      /* @__PURE__ */ jsx(Fragment2, { children: destinations.map((destination) => /* @__PURE__ */ jsxs(
        "a",
        {
          class: "nav-item",
          "data-go": destination.key,
          href: destination.href,
          "aria-current": destination.current ? "page" : void 0,
          onClick: (event) => {
            if (!event.currentTarget.isConnected || !nav.contains(event.currentTarget) || event.defaultPrevented || event.button !== 0 || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey)
              return;
            if (navigate(destination)) event.preventDefault();
          },
          children: [
            /* @__PURE__ */ jsx(Icon, { path: destination.icon }),
            /* @__PURE__ */ jsx("span", { children: destination.label }),
            !!destination.count && /* @__PURE__ */ jsx("span", { class: "cnt" + (destination.hot ? " hot" : ""), children: destination.count })
          ]
        },
        destination.key
      )) }),
      nav
    );
  }
  function createShellChrome(host2) {
    const account = createAccountChrome(host2.account);
    const phone = window.matchMedia("(max-width: 760px)");
    let app = null, sidebar, head, nav, bar, scrim;
    let recent, content2;
    let desktopAccount = null, phoneAccount = null;
    let lead = null, destroyed = false, sx = null;
    let barObserver = null;
    const measureBar = () => {
      if (app && !destroyed) setGeometry(app, "barHeight", bar.getBoundingClientRect().height);
    };
    let destinations = [], rail = false;
    const listeners = [];
    function listen(target, type, handler, options2) {
      target.addEventListener(type, handler, options2);
      listeners.push(() => target.removeEventListener(type, handler, options2));
    }
    function ready() {
      if (destroyed || !app) throw new Error("Shell is not mounted");
    }
    function closeDrawer(quiet = false) {
      if (!app || !document.body.classList.contains("drawer-open")) return;
      document.body.classList.remove("drawer-open");
      account.close();
      lead?.setAttribute("aria-expanded", "false");
      if (!quiet) lead?.focus();
      host2.drawerClosed();
    }
    function openDrawer() {
      if (!app?.isConnected || !phone.matches || destroyed) return;
      host2.drawerOpened();
      document.body.classList.add("drawer-open");
      lead?.setAttribute("aria-expanded", "true");
    }
    function paintHead() {
      const label = rail ? "Expand sidebar" : "Collapse sidebar";
      render(
        /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsxs("div", { class: "brandrow", children: [
            /* @__PURE__ */ jsx("span", { class: "mark", "aria-hidden": "true" }),
            /* @__PURE__ */ jsx("span", { class: "brandname", children: "Semon" }),
            /* @__PURE__ */ jsx(
              "button",
              {
                class: "ibtn close",
                id: "drawer-close",
                type: "button",
                "aria-label": "Close menu",
                onClick: (event) => {
                  if (event.currentTarget.isConnected && head.contains(event.currentTarget))
                    closeDrawer();
                },
                children: /* @__PURE__ */ jsx(
                  "svg",
                  {
                    viewBox: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    "stroke-width": "1.9",
                    "stroke-linecap": "round",
                    "aria-hidden": "true",
                    children: /* @__PURE__ */ jsx("path", { d: "M6 6l12 12M18 6L6 18" })
                  }
                )
              }
            )
          ] }),
          /* @__PURE__ */ jsx(
            "button",
            {
              class: "ibtn rail-toggle",
              id: "rail-toggle",
              type: "button",
              "aria-label": label,
              "aria-expanded": !rail,
              "data-tip": label,
              onClick: (event) => {
                if (app?.isConnected && !destroyed && event.currentTarget.isConnected && head.contains(event.currentTarget))
                  host2.railChanged();
              },
              children: /* @__PURE__ */ jsx(
                "svg",
                {
                  viewBox: "0 0 24 24",
                  fill: "none",
                  stroke: "currentColor",
                  "stroke-width": "1.8",
                  "stroke-linecap": "round",
                  "stroke-linejoin": "round",
                  "aria-hidden": "true",
                  children: /* @__PURE__ */ jsx("path", { d: "M4 5h16v14H4zM9 5v14" })
                }
              )
            }
          )
        ] }),
        head
      );
    }
    function dropDesktop() {
      if (desktopAccount) {
        account.unmount(desktopAccount);
        desktopAccount.remove();
        desktopAccount = null;
      }
    }
    function drawerAccount(props) {
      ready();
      if (phoneAccount) {
        account.unmount(phoneAccount);
        phoneAccount.remove();
        phoneAccount = null;
      }
      if (props) {
        phoneAccount = account.mount(props);
        phoneAccount.id = "account-drawer";
        sidebar.append(phoneAccount);
      }
    }
    function unmount() {
      if (!app) return;
      barObserver?.disconnect();
      barObserver = null;
      releaseGeometry(app, ["barHeight"]);
      closeDrawer(true);
      dropDesktop();
      drawerAccount(null);
      for (const remove of listeners.splice(0)) remove();
      phone.removeEventListener("change", resized);
      render(null, nav);
      render(null, head);
      for (const node of [...bar.childNodes]) node.remove();
      lead = null;
      app = null;
      sx = null;
    }
    const resized = () => {
      closeDrawer(true);
      app?.classList.toggle("rail", rail && !phone.matches);
    };
    return {
      account,
      get slots() {
        ready();
        return { recent, content: content2 };
      },
      get drawerOpen() {
        return !!app && document.body.classList.contains("drawer-open");
      },
      mount(container) {
        if (destroyed) throw new Error("Shell is destroyed");
        if (app) throw new Error("Shell is already mounted");
        if (!container.querySelector("#sidebar")) {
          const side = element("aside", "sidebar", "sidebar");
          side.setAttribute("aria-label", "Navigation");
          const heading = element("div", "sidebar-head");
          const navigation = element("div", "", "nav");
          const caption = element("div", "side-h");
          caption.textContent = "Recent";
          const list2 = element("div", "side-list", "side-list");
          const lanes = element("div", "", "lanes");
          lanes.setAttribute("role", "tree");
          lanes.setAttribute("aria-label", "Recent sessions");
          list2.append(lanes);
          side.append(heading, navigation, caption, list2);
          const main = element("div", "main", "main");
          main.append(element("header", "topbar", "topbar"), element("div", "page", "page"));
          container.append(side, element("div", "scrim", "scrim"), main);
        }
        const required = (selector) => {
          const node = container.querySelector(selector);
          if (!node) throw new Error("Missing shell slot " + selector);
          return node;
        };
        sidebar = required("#sidebar");
        head = required(".sidebar-head");
        nav = required("#nav");
        bar = required("#topbar");
        scrim = required("#scrim");
        recent = required("#side-list");
        content2 = required("#page");
        app = container;
        barObserver = new ResizeObserver(measureBar);
        barObserver.observe(bar);
        measureBar();
        paintHead();
        listen(scrim, "click", () => closeDrawer());
        listen(
          sidebar,
          "touchstart",
          (event) => {
            sx = event.touches[0]?.clientX ?? null;
          },
          { passive: true }
        );
        listen(
          sidebar,
          "touchmove",
          (event) => {
            if (sx !== null && event.touches[0] && event.touches[0].clientX - sx < -50) {
              sx = null;
              closeDrawer();
            }
          },
          { passive: true }
        );
        listen(
          sidebar,
          "touchend",
          () => {
            sx = null;
          },
          { passive: true }
        );
        phone.addEventListener("change", resized);
      },
      update(next, collapsed) {
        ready();
        const keys = /* @__PURE__ */ new Set();
        destinations = next.map((destination) => {
          if (!destination.key || keys.has(destination.key) || !safePath(destination.href))
            throw new Error("Invalid shell destination");
          keys.add(destination.key);
          return { ...destination };
        });
        rail = collapsed;
        app.classList.toggle("rail", rail && !phone.matches);
        paintHead();
        renderShellNavigation(nav, destinations, (destination) => host2.navigate(destination));
      },
      topbar(props) {
        ready();
        const leadFocused = lead !== null && document.activeElement === lead;
        account.close();
        dropDesktop();
        lead = null;
        const nodes = [];
        if (!props.mode && props.lead) {
          const button = document.createElement("button");
          button.type = "button";
          button.className = "ibtn lead" + (props.lead.back ? " trace-back" : "");
          button.id = "lead-btn";
          button.setAttribute("aria-label", props.lead.label);
          button.setAttribute("aria-controls", "sidebar");
          button.setAttribute(
            "aria-expanded",
            String(document.body.classList.contains("drawer-open"))
          );
          button.append(svg(props.lead.icon));
          const back = props.lead.back;
          button.addEventListener("click", () => {
            if (button !== lead || !button.isConnected || !app || destroyed) return;
            if (back) back();
            else openDrawer();
          });
          lead = button;
          nodes.push(button);
        }
        if (props.mode) nodes.push(...props.mode);
        else {
          if (props.titleSlot) nodes.push(props.titleSlot);
          if (props.actions) nodes.push(...props.actions);
        }
        if (props.account) {
          desktopAccount = account.mount(props.account);
          if (props.accountTarget) props.accountTarget.append(desktopAccount);
          else nodes.push(desktopAccount);
        }
        bar.classList.remove("scrolled");
        bar.classList.toggle("session-bar", !!props.session);
        const kept = new Set(nodes);
        for (const node of [...bar.children]) if (!kept.has(node)) node.remove();
        let cursor = bar.firstChild;
        for (const node of nodes) {
          if (node === cursor) cursor = cursor.nextSibling;
          else bar.insertBefore(node, cursor);
        }
        while (cursor) {
          const next = cursor.nextSibling;
          cursor.remove();
          cursor = next;
        }
        if (leadFocused) lead?.focus();
        measureBar();
      },
      drawerAccount,
      openDrawer,
      closeDrawer,
      restoreDrawer() {
        ready();
        document.body.classList.add("drawer-open");
        lead?.setAttribute("aria-expanded", "true");
      },
      unmount,
      destroy() {
        if (destroyed) return;
        unmount();
        account.destroy();
        destroyed = true;
      }
    };
  }

  // src/lib/recent.tsx
  var CHEVRON = "M9 6l6 6-6 6";
  function Icon2({ path }) {
    return /* @__PURE__ */ jsx(
      "svg",
      {
        class: "icon",
        viewBox: "0 0 24 24",
        fill: "none",
        stroke: "currentColor",
        "stroke-width": "1.8",
        "stroke-linecap": "round",
        "stroke-linejoin": "round",
        "aria-hidden": "true",
        children: /* @__PURE__ */ jsx("path", { d: path })
      }
    );
  }
  function createRecentRenderer(root, host2) {
    let destroyed = false, frame = 0;
    const active = (node) => !destroyed && node.isConnected && root.contains(node);
    function fit() {
      if (destroyed) return;
      for (const meta of root.querySelectorAll(".session-row-meta")) {
        if (!meta.isConnected || !meta.clientWidth) continue;
        const values = [...meta.querySelectorAll("[data-drop]")];
        for (const value of values)
          value.hidden = meta.clientWidth < 280 && value.classList.contains("row-duration");
        for (const value of values.sort((a, b) => Number(b.dataset.drop) - Number(a.dataset.drop))) {
          if (meta.scrollWidth <= meta.clientWidth + 1) break;
          value.hidden = true;
        }
      }
    }
    function Item({ item: item2 }) {
      const kids = item2.children, expandable = !!kids && !item2.rail;
      const harness = item2.harness;
      return /* @__PURE__ */ jsxs(
        "div",
        {
          class: "treeitem",
          role: "treeitem",
          "data-id": item2.id,
          "aria-label": item2.name,
          tabIndex: 0,
          "aria-expanded": expandable ? item2.open : void 0,
          onKeyDown: (event) => {
            if (!active(event.currentTarget)) return;
            const target = event.target;
            if (target !== event.currentTarget && target !== event.currentTarget.querySelector(":scope > .tree-row .srow") && target !== event.currentTarget.querySelector(":scope > .tree-row .tree-toggle"))
              return;
            if (expandable && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
              const open = event.key === "ArrowRight";
              if (open !== item2.open) {
                event.preventDefault();
                host2.toggle(item2.id, open);
              }
            } else if ((event.key === "Enter" || event.key === " ") && target === event.currentTarget) {
              event.preventDefault();
              host2.open(item2.id);
            }
          },
          children: [
            /* @__PURE__ */ jsxs("div", { class: "tree-row" + (expandable ? " has-toggle" : "") + (item2.stuck ? " stuck" : ""), children: [
              /* @__PURE__ */ jsxs(
                "button",
                {
                  class: "srow" + (item2.current === "true" ? " on-path" : ""),
                  type: "button",
                  "data-id": item2.id,
                  "data-session-row": "compact",
                  "data-tip": item2.rail ? item2.name : void 0,
                  "aria-label": item2.label,
                  "aria-current": item2.current,
                  onClick: (event) => {
                    if (active(event.currentTarget)) host2.open(item2.id);
                  },
                  children: [
                    /* @__PURE__ */ jsxs("span", { class: "session-row-main srow-main", children: [
                      /* @__PURE__ */ jsx(
                        "span",
                        {
                          class: "dot " + item2.state,
                          role: "img",
                          "aria-label": item2.stateLabel,
                          "data-tip": item2.rail ? void 0 : item2.stateLabel
                        }
                      ),
                      /* @__PURE__ */ jsx("span", { class: "nm", "data-tip": item2.name, "data-tip-clipped": "", children: item2.name }),
                      item2.flag && /* @__PURE__ */ jsx(
                        "span",
                        {
                          class: "kid-flag " + item2.flag.state,
                          "data-tip": item2.flag.tip,
                          "aria-hidden": "true"
                        }
                      ),
                      /* @__PURE__ */ jsx("span", { class: "ag", children: item2.age }),
                      item2.childState && /* @__PURE__ */ jsx(
                        "span",
                        {
                          class: "dot " + item2.childState + " child-dot",
                          role: "img",
                          "aria-label": item2.childState,
                          "aria-hidden": "true"
                        }
                      )
                    ] }),
                    /* @__PURE__ */ jsxs("span", { class: "session-row-meta srow-meta for", children: [
                      harness && /* @__PURE__ */ jsx(
                        "span",
                        {
                          class: "hicon hi-sidebar",
                          "data-harness": harness.id,
                          role: "img",
                          "aria-label": harness.name,
                          "data-tip": harness.name,
                          children: harness.light === harness.dark ? /* @__PURE__ */ jsx("img", { src: harness.light, alt: "", draggable: false, decoding: "async" }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
                            /* @__PURE__ */ jsx(
                              "img",
                              {
                                class: "hi-light",
                                loading: harness.darkTheme ? "lazy" : void 0,
                                src: harness.light,
                                alt: "",
                                draggable: false,
                                decoding: "async"
                              }
                            ),
                            /* @__PURE__ */ jsx(
                              "img",
                              {
                                class: "hi-dark",
                                loading: harness.darkTheme ? void 0 : "lazy",
                                src: harness.dark,
                                alt: "",
                                draggable: false,
                                decoding: "async"
                              }
                            )
                          ] })
                        }
                      ),
                      /* @__PURE__ */ jsx("span", { class: "row-model", "data-tip": item2.modelTip, children: item2.model }),
                      item2.fields.map((field) => /* @__PURE__ */ jsxs(
                        "span",
                        {
                          class: field.className + " row-field",
                          "data-drop": field.priority,
                          "data-tip": field.tip,
                          children: [
                            /* @__PURE__ */ jsx(Icon2, { path: field.icon }),
                            /* @__PURE__ */ jsx("span", { class: "field-value", children: field.text })
                          ]
                        },
                        field.priority
                      ))
                    ] })
                  ]
                }
              ),
              expandable && /* @__PURE__ */ jsx(
                "button",
                {
                  class: "tree-toggle",
                  type: "button",
                  "data-tree-toggle": item2.id,
                  "aria-label": (item2.open ? "Collapse " : "Expand ") + item2.name,
                  "aria-expanded": item2.open,
                  onClick: (event) => {
                    event.stopPropagation();
                    if (active(event.currentTarget)) host2.toggle(item2.id, !item2.open);
                  },
                  children: /* @__PURE__ */ jsx(Icon2, { path: CHEVRON })
                }
              ),
              item2.stuck && /* @__PURE__ */ jsxs(
                "button",
                {
                  class: "tree-fewer",
                  type: "button",
                  "aria-label": "Show fewer sessions under " + item2.name,
                  onClick: (event) => {
                    event.stopPropagation();
                    if (active(event.currentTarget)) {
                      host2.fewer(item2.id);
                      const parent = [...root.querySelectorAll(".treeitem")].find(
                        (node) => node.dataset.id === item2.id
                      );
                      const row = parent?.querySelector(":scope > .tree-row .srow");
                      row?.focus({ preventScroll: true });
                      row?.scrollIntoView({ block: "nearest" });
                    }
                  },
                  children: [
                    /* @__PURE__ */ jsx("span", { children: "Show fewer" }),
                    /* @__PURE__ */ jsx(Icon2, { path: CHEVRON })
                  ]
                }
              )
            ] }),
            expandable && /* @__PURE__ */ jsxs(
              "div",
              {
                class: "tree-group",
                "data-depth": Math.min(item2.depth + 1, 4),
                role: "group",
                "aria-label": "Sessions spawned by " + item2.name,
                children: [
                  kids.map((child) => /* @__PURE__ */ jsx(Item, { item: child }, child.id)),
                  item2.all !== void 0 && /* @__PURE__ */ jsxs(
                    "button",
                    {
                      class: "tree-all",
                      type: "button",
                      "data-id": item2.id,
                      role: "treeitem",
                      "aria-haspopup": window.matchMedia("(max-width: 760px)").matches ? "dialog" : void 0,
                      "aria-label": "All " + item2.all + " sessions under " + item2.name,
                      onClick: (event) => {
                        event.stopPropagation();
                        if (active(event.currentTarget)) host2.all(item2.id, event.currentTarget);
                      },
                      children: [
                        /* @__PURE__ */ jsx("span", { children: "All " + item2.all }),
                        /* @__PURE__ */ jsx(Icon2, { path: CHEVRON })
                      ]
                    }
                  )
                ]
              }
            )
          ]
        }
      );
    }
    window.addEventListener("resize", fit, { passive: true });
    return {
      update(snapshot) {
        if (destroyed) throw new Error("Recent renderer is destroyed");
        const ids = /* @__PURE__ */ new Set();
        function validate(items) {
          for (const item2 of items) {
            if (!item2.id || ids.has(item2.id)) throw new Error("Invalid Recent identity");
            ids.add(item2.id);
            if (item2.harness && (!safePath(item2.harness.light) || !safePath(item2.harness.dark)))
              throw new Error("Invalid Recent harness path");
            if (item2.children) validate(item2.children);
          }
        }
        validate(snapshot.items);
        render(
          /* @__PURE__ */ jsxs(Fragment2, { children: [
            snapshot.items.map((item2) => /* @__PURE__ */ jsx(Item, { item: item2 }, item2.id)),
            snapshot.empty && /* @__PURE__ */ jsx("p", { class: "ghead", role: "none", children: "No sessions match" })
          ] }),
          root
        );
        cancelAnimationFrame(frame);
        frame = requestAnimationFrame(fit);
      },
      fit,
      destroy() {
        if (destroyed) return;
        destroyed = true;
        cancelAnimationFrame(frame);
        window.removeEventListener("resize", fit);
        render(null, root);
      }
    };
  }

  // src/lib/panel.tsx
  function PanelHeader({ heading, sub, close }) {
    const spaced = (text2) => text2.replace(/ · /g, "\u2009 \xB7 \u2009").replace(/^· /, "\xB7\u2009 ");
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("div", { class: "panel-t", children: spaced(heading) }),
      sub && /* @__PURE__ */ jsx("div", { class: "panel-sub", children: spaced(sub) }),
      /* @__PURE__ */ jsx("button", { class: "ibtn", type: "button", "aria-label": "Close", onClick: close, children: /* @__PURE__ */ jsx(
        "svg",
        {
          viewBox: "0 0 24 24",
          "aria-hidden": "true",
          fill: "none",
          stroke: "currentColor",
          "stroke-width": "1.8",
          "stroke-linecap": "round",
          "stroke-linejoin": "round",
          children: /* @__PURE__ */ jsx("path", { d: "M6 6l12 12M18 6L6 18" })
        }
      ) })
    ] });
  }
  function createPanelChrome(options2, host2) {
    const dialog = document.createElement("dialog");
    dialog.className = "panel" + (options2.className ? " " + options2.className : "");
    dialog.setAttribute("aria-label", options2.label ?? options2.title);
    const header = document.createElement("div");
    header.className = "panel-h";
    const body = document.createElement("div");
    body.className = "panel-b";
    body.tabIndex = -1;
    let disposed = false, shown = false;
    const close = () => {
      if (!disposed && dialog.open) dialog.close();
    };
    render(/* @__PURE__ */ jsx(PanelHeader, { heading: options2.title, sub: options2.sub, close }), header);
    dialog.append(header, body);
    document.body.append(dialog);
    const hold = (event) => {
      if (!dialog.isConnected) {
        destroy();
        return;
      }
      if (!dialog.open) return;
      if (event.target instanceof Element && event.target.closest("dialog") !== dialog) return;
      if (!(event.target instanceof Node) || !body.contains(event.target) || body.scrollHeight <= body.clientHeight + 1)
        event.preventDefault();
    };
    const backdrop = (event) => {
      if (event.target === dialog) close();
    };
    function destroy() {
      if (disposed) return;
      disposed = true;
      dialog.removeEventListener("click", backdrop);
      dialog.removeEventListener("close", destroy);
      for (const type of ["wheel", "touchmove"])
        document.removeEventListener(type, hold, { capture: true });
      if (dialog.open) dialog.close();
      render(null, header);
      dialog.remove();
      if (shown) host2.closed();
    }
    dialog.addEventListener("click", backdrop);
    dialog.addEventListener("close", destroy);
    return {
      dialog,
      body,
      destroy,
      show() {
        if (disposed || shown || !dialog.isConnected) return;
        dialog.showModal();
        shown = true;
        for (const type of ["wheel", "touchmove"])
          document.addEventListener(type, hold, { capture: true, passive: false });
        body.focus({ preventScroll: true });
        host2.opened();
      }
    };
  }

  // src/lib/facets.tsx
  var X = "M6 6l12 12M18 6L6 18";
  function Icon3({ path }) {
    return /* @__PURE__ */ jsx(
      "svg",
      {
        viewBox: "0 0 24 24",
        "aria-hidden": "true",
        fill: "none",
        stroke: "currentColor",
        "stroke-width": "1.8",
        "stroke-linecap": "round",
        "stroke-linejoin": "round",
        children: /* @__PURE__ */ jsx("path", { d: path })
      }
    );
  }
  function createFacetChrome(host2) {
    const element2 = document.createElement("div");
    element2.className = "facet-filters";
    element2.setAttribute("role", "group");
    element2.setAttribute("aria-label", "Filter sessions");
    let dialog = null, body = null;
    const selects = /* @__PURE__ */ new Map();
    let trigger = null, disposed = false, opened = false;
    function release() {
      for (const type of ["wheel", "touchmove"])
        document.removeEventListener(type, hold, { capture: true });
    }
    function hold(event) {
      if (!dialog?.isConnected || !dialog.open) {
        release();
        if (opened && dialog) {
          opened = false;
          host2.closed(dialog, "destroyed");
        }
        return;
      }
      const target = event.target;
      if (target instanceof Element && (target.closest(".sh-select-list") || body && body.contains(target) && body.scrollHeight > body.clientHeight + 1))
        return;
      event.preventDefault();
    }
    function open() {
      if (disposed || opened || !element2.isConnected || !dialog || !host2.canOpen()) return;
      host2.opened(dialog);
      opened = true;
      dialog.showModal();
      for (const type of ["wheel", "touchmove"])
        document.addEventListener(type, hold, { capture: true, passive: false });
      selects.values().next().value?.focus();
    }
    function close() {
      if (!disposed && dialog?.open) dialog.close();
    }
    function closed() {
      release();
      if (!opened) return;
      opened = false;
      trigger?.focus({ preventScroll: true });
      if (dialog) host2.closed(dialog, "dismissed");
    }
    function backdrop(event) {
      if (event.target === dialog) close();
    }
    function tab(event) {
      if (event.key !== "Tab" || event.defaultPrevented) return;
      if (!dialog) return;
      const controls = [
        ...dialog.querySelectorAll(
          "button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex='-1'])"
        )
      ].filter((node) => node.getClientRects().length && !node.closest("[hidden]"));
      const first = controls[0], last = controls.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
    return {
      element: element2,
      update(fields) {
        if (disposed) return;
        for (const field of fields) {
          let select = selects.get(field.key);
          if (!select) {
            select = host2.select(field.label, field.value, (value) => {
              if (!disposed) host2.change(field.key, value);
            });
            selects.set(field.key, select);
          }
          select.setOptions(field.options);
          select.setValue(field.value);
        }
        const active = fields.filter((field) => field.value !== "").length;
        render(
          /* @__PURE__ */ jsxs(Fragment2, { children: [
            /* @__PURE__ */ jsxs(
              "button",
              {
                class: "facet-btn",
                type: "button",
                "aria-haspopup": "dialog",
                "aria-label": active ? "Filter, " + active + " active" : "Filter",
                ref: (node) => {
                  trigger = node;
                },
                onClick: open,
                children: [
                  /* @__PURE__ */ jsx(Icon3, { path: "M4 6h16M7 12h10M10 18h4" }),
                  /* @__PURE__ */ jsx("span", { children: "Filter" }),
                  /* @__PURE__ */ jsx("span", { class: "facet-n", hidden: !active, children: active || "" })
                ]
              }
            ),
            fields.map((field) => /* @__PURE__ */ jsxs(
              "button",
              {
                class: "facet-chip",
                type: "button",
                hidden: field.value === "",
                "data-facet": field.key,
                "aria-label": field.value ? "Remove " + field.label + ": " + field.display : void 0,
                onClick: () => {
                  if (disposed) return;
                  host2.cleared(field.key);
                  trigger?.focus({ preventScroll: true });
                },
                children: [
                  /* @__PURE__ */ jsx("span", { class: "txt", children: field.value ? field.label + ": " + field.display : "" }),
                  /* @__PURE__ */ jsx(Icon3, { path: X })
                ]
              },
              field.key
            )),
            /* @__PURE__ */ jsxs(
              "dialog",
              {
                class: "viewer filters-sheet",
                "aria-label": "Filter",
                ref: (node) => {
                  dialog = node;
                },
                onClose: closed,
                onClick: backdrop,
                onKeyDown: tab,
                children: [
                  /* @__PURE__ */ jsxs("div", { class: "vh", children: [
                    /* @__PURE__ */ jsx("div", { class: "vt", children: /* @__PURE__ */ jsx("span", { children: "Filter" }) }),
                    /* @__PURE__ */ jsx("button", { class: "vclose", type: "button", "aria-label": "Close filters", onClick: close, children: /* @__PURE__ */ jsx(Icon3, { path: X }) })
                  ] }),
                  /* @__PURE__ */ jsx(
                    "div",
                    {
                      class: "vb",
                      ref: (node) => {
                        body = node;
                      }
                    }
                  ),
                  /* @__PURE__ */ jsxs("div", { class: "vf", children: [
                    /* @__PURE__ */ jsx(
                      "button",
                      {
                        class: "fclear",
                        type: "button",
                        onClick: () => {
                          host2.clear();
                          close();
                        },
                        children: "Clear all"
                      }
                    ),
                    /* @__PURE__ */ jsx("button", { class: "fdone", type: "button", onClick: close, children: "Done" })
                  ] })
                ]
              }
            )
          ] }),
          element2
        );
        for (const select of selects.values())
          if (body && select.el.parentElement !== body) body.append(select.el);
      },
      destroy() {
        if (disposed) return;
        disposed = true;
        release();
        for (const select of selects.values()) {
          if (select.destroy) select.destroy();
          else select.close();
        }
        if (dialog?.open) dialog.close();
        if (opened && dialog) {
          opened = false;
          host2.closed(dialog, "destroyed");
        }
        render(null, element2);
        element2.remove();
      }
    };
  }

  // shared-preact:preact/hooks
  var useState = globalThis.__semonUIShared.hooks.useState;
  var useEffect = globalThis.__semonUIShared.hooks.useEffect;
  var useLayoutEffect = globalThis.__semonUIShared.hooks.useLayoutEffect;
  var useReducer = globalThis.__semonUIShared.hooks.useReducer;
  var useRef = globalThis.__semonUIShared.hooks.useRef;
  var useMemo = globalThis.__semonUIShared.hooks.useMemo;
  var useCallback = globalThis.__semonUIShared.hooks.useCallback;
  var useContext = globalThis.__semonUIShared.hooks.useContext;
  var useDebugValue = globalThis.__semonUIShared.hooks.useDebugValue;
  var useErrorBoundary = globalThis.__semonUIShared.hooks.useErrorBoundary;
  var useId = globalThis.__semonUIShared.hooks.useId;
  var useImperativeHandle = globalThis.__semonUIShared.hooks.useImperativeHandle;

  // src/lib/localControl.tsx
  function visible(value) {
    return String(value ?? "").replace(
      /[\u00ad\u202a-\u202e\u2066-\u2069]/g,
      (char) => "\\u" + char.charCodeAt(0).toString(16).padStart(4, "0")
    );
  }
  function Question({ request, view }) {
    const [answers, setAnswers] = useState({});
    const params = request.payload.params;
    const questions = params && typeof params === "object" && !Array.isArray(params) ? params.questions : null;
    if (!Array.isArray(questions)) return /* @__PURE__ */ jsx("p", { children: "Unsupported native question. Answer in Codex." });
    return /* @__PURE__ */ jsxs(
      "form",
      {
        onSubmit: (event) => {
          event.preventDefault();
          const result = {};
          for (const [id, value] of Object.entries(answers)) result[id] = [value];
          view.answer(request, { answers: result });
        },
        children: [
          questions.map((value) => {
            if (!value || typeof value !== "object" || Array.isArray(value) || typeof value.id !== "string" || typeof value.question !== "string")
              return null;
            const id = String(value.id), label = visible(value.question);
            return /* @__PURE__ */ jsxs("label", { children: [
              label,
              /* @__PURE__ */ jsx(
                "input",
                {
                  "aria-label": label,
                  value: answers[id] ?? "",
                  list: "options-" + request.id + "-" + id,
                  onInput: (event) => setAnswers({ ...answers, [id]: event.currentTarget.value })
                }
              ),
              /* @__PURE__ */ jsx("datalist", { id: "options-" + request.id + "-" + id, children: Array.isArray(value.options) && value.options.map(
                (option) => option && typeof option === "object" && !Array.isArray(option) && typeof option.label === "string" ? /* @__PURE__ */ jsx("option", { value: option.label }) : null
              ) })
            ] }, id);
          }),
          /* @__PURE__ */ jsx(
            "button",
            {
              type: "submit",
              disabled: view.busy || view.uncertain || !view.snapshot.capabilities.questions || !!request.reason || request.state.state !== "open" || request.remainingMs === 0,
              children: "Answer"
            }
          )
        ]
      }
    );
  }
  function Permission({ request }) {
    const p = request.payload.params;
    if (!p || typeof p !== "object" || Array.isArray(p))
      return /* @__PURE__ */ jsx("p", { children: "Exact request is no longer retained." });
    const item2 = request.payload.item;
    const changes = item2 && typeof item2 === "object" && !Array.isArray(item2) ? item2.changes : null;
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      typeof p.command === "string" && /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx("pre", { children: visible(p.command) }),
        /* @__PURE__ */ jsxs("p", { children: [
          "Working directory: ",
          visible(p.cwd)
        ] })
      ] }),
      Array.isArray(changes) && changes.map(
        (change) => change && typeof change === "object" && !Array.isArray(change) ? /* @__PURE__ */ jsxs("div", { children: [
          /* @__PURE__ */ jsx("p", { children: visible(change.path) }),
          /* @__PURE__ */ jsx("pre", { children: visible(change.diff) })
        ] }) : null
      ),
      typeof p.reason === "string" && /* @__PURE__ */ jsx("p", { children: visible(p.reason) }),
      typeof p.command !== "string" && !Array.isArray(changes) && /* @__PURE__ */ jsx("pre", { children: visible(JSON.stringify(request.payload, null, 2)) })
    ] });
  }
  function LocalControl({
    view,
    runtimeObserved = false
  }) {
    const input = useRef(null);
    const [localText, setLocalText] = useState("");
    const text2 = view.draft ?? localText;
    const setText = (next) => {
      const value = typeof next === "function" ? next(view.draft ?? localText) : next;
      view.setDraft?.(value);
      setLocalText(value);
    };
    const [disclosed, setDisclosed] = useState(false);
    const [focused, setFocused] = useState(false);
    const s = view.snapshot;
    const canWrite = !view.busy && !view.uncertain && s.connected && (s.activeTurn ? s.capabilities.steer : s.capabilities.input);
    const available = s.connected && (s.activeTurn ? s.capabilities.steer : s.capabilities.input) && !view.uncertain;
    const reason = s.runtime?.state === "ended" ? "This session has ended." : s.runtime?.state === "failed" ? "This environment needs attention." : view.uncertain ? "Delivery is unconfirmed. Inspect the session before sending again." : s.reason ?? (s.connected ? "Input is unavailable for this session." : "Native session controls are unavailable.");
    const collapsed = !available && !disclosed && !focused && input.current !== document.activeElement;
    const canSend = canWrite && !!text2.trim();
    const showStop = !!s.activeTurn && s.capabilities.interrupt && !text2.trim();
    const pending = s.requests.filter((r) => r.state.state === "open" || r.state.state === "claimed");
    const recent = s.requests.filter((r) => r.state.state !== "open" && r.state.state !== "claimed").slice(-10);
    return /* @__PURE__ */ jsxs("section", { class: "local-control", "aria-label": "Conversation controls", children: [
      !available && /* @__PURE__ */ jsxs(
        "button",
        {
          type: "button",
          class: "composer-unavailable",
          "aria-expanded": !collapsed,
          onClick: () => setDisclosed(!disclosed),
          children: [
            /* @__PURE__ */ jsxs("span", { role: "status", children: [
              visible(reason),
              text2 ? " \xB7 Draft saved" : ""
            ] }),
            /* @__PURE__ */ jsx("span", { "aria-hidden": "true", children: disclosed ? "\u2303" : "\u2304" })
          ]
        }
      ),
      !collapsed && !s.connected && /* @__PURE__ */ jsx("p", { role: "status", children: runtimeObserved ? "Native session controls are unavailable." : s.runtime?.state === "ended" ? "This session has ended." : s.runtime?.state === "failed" ? "This environment needs attention." : s.runtime?.state === "disconnected" ? "This environment is disconnected." : s.runtime?.state === "unavailable" ? "Runtime state is unavailable." : "Reconnecting to your session\u2026" }),
      !collapsed && s.reason && /* @__PURE__ */ jsx("p", { children: s.reason }),
      !collapsed && s.runtime && s.runtime.freshness !== "current" && /* @__PURE__ */ jsx("p", { role: "status", children: s.runtime.freshness === "updating" ? "Checking the environment\u2026" : s.runtime.freshness === "stale" ? "Last environment observation is stale." : "Current environment observation is unavailable." }),
      !collapsed && s.runtime?.observationError && /* @__PURE__ */ jsx("p", { children: s.runtime.observationError }),
      pending.map((request) => {
        const supported = request.kind === "question" ? s.capabilities.questions : request.payload.method === "item/fileChange/requestApproval" ? s.capabilities.fileApproval : s.capabilities.commandApproval;
        const disabled = view.busy || view.uncertain || !supported || !!request.reason || request.state.state !== "open" || request.remainingMs === 0;
        return /* @__PURE__ */ jsxs("article", { children: [
          /* @__PURE__ */ jsx("h3", { children: request.kind === "question" ? "Codex question" : "Codex permission" }),
          request.reason && /* @__PURE__ */ jsx("p", { children: request.reason }),
          request.state.state === "claimed" && /* @__PURE__ */ jsx("p", { children: "Response sent; waiting for Codex to answer or clear the request." }),
          request.kind === "question" ? /* @__PURE__ */ jsx(Question, { request, view }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
            /* @__PURE__ */ jsx(Permission, { request }),
            /* @__PURE__ */ jsxs("div", { children: [
              /* @__PURE__ */ jsx(
                "button",
                {
                  type: "button",
                  disabled,
                  onClick: () => view.answer(request, { decision: "allow" }),
                  children: "Allow once"
                }
              ),
              /* @__PURE__ */ jsx(
                "button",
                {
                  type: "button",
                  disabled,
                  onClick: () => view.answer(request, { decision: "deny" }),
                  children: "Deny"
                }
              )
            ] })
          ] })
        ] }, request.id);
      }),
      /* @__PURE__ */ jsxs(
        "form",
        {
          class: "sh-composer sh-composer-conversation" + (focused ? " sh-composer-focused" : ""),
          hidden: collapsed,
          "aria-label": "Send a follow-up",
          "aria-busy": view.busy || void 0,
          onSubmit: async (event) => {
            event.preventDefault();
            const submitted = event.currentTarget.querySelector("textarea")?.value ?? text2;
            if (!canWrite || !submitted.trim()) return;
            if (await view.send(submitted))
              setText((current) => current === submitted ? "" : current);
          },
          children: [
            /* @__PURE__ */ jsx(
              "textarea",
              {
                ref: input,
                class: "sh-composer-input",
                "aria-label": "Message to Codex",
                "aria-description": available ? "Enter to send. Shift+Enter for a new line." : "Sending is unavailable. Retained draft can be selected and copied.",
                rows: focused ? Math.min(6, Math.max(2, text2.split("\n").length)) : 1,
                readOnly: !available,
                "aria-disabled": !available || void 0,
                onFocus: () => setFocused(true),
                onBlur: () => setFocused(false),
                placeholder: s.activeTurn ? "Add instructions\u2026" : "Message Codex\u2026",
                value: text2,
                onInput: (event) => setText(event.currentTarget.value),
                onKeyDown: (event) => {
                  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
                    event.preventDefault();
                    if (canWrite && event.currentTarget.value.trim())
                      event.currentTarget.form?.requestSubmit();
                  }
                }
              }
            ),
            /* @__PURE__ */ jsx(
              "button",
              {
                type: showStop ? "button" : "submit",
                class: "ibtn sh-composer-send",
                "aria-label": showStop ? "Stop response" : "Send",
                "data-tip": showStop ? "Stop response" : "Send message",
                disabled: showStop ? view.busy || view.uncertain : !canSend,
                onClick: () => {
                  if (showStop) view.interrupt();
                },
                children: view.busy ? /* @__PURE__ */ jsx("span", { class: "spin", "aria-hidden": "true" }) : /* @__PURE__ */ jsx(
                  "svg",
                  {
                    viewBox: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    "stroke-width": "2",
                    "stroke-linecap": "round",
                    "stroke-linejoin": "round",
                    "aria-hidden": "true",
                    children: showStop ? /* @__PURE__ */ jsx("rect", { x: "6", y: "6", width: "12", height: "12", rx: "2", fill: "currentColor", stroke: "none" }) : /* @__PURE__ */ jsx("path", { d: "M12 19V5m-6 6 6-6 6 6" })
                  }
                )
              }
            )
          ]
        }
      ),
      !collapsed && (!s.connected || view.uncertain) && view.canReconnect !== false && s.runtime?.reconnectable !== false && /* @__PURE__ */ jsx("div", { class: "local-control-actions", children: /* @__PURE__ */ jsx(
        "button",
        {
          type: "button",
          class: "btn sh-quiet",
          disabled: view.busy,
          onClick: () => view.reconnect(),
          children: "Reconnect"
        }
      ) }),
      view.note && /* @__PURE__ */ jsx("p", { role: "status", children: view.note }),
      recent.length > 0 && /* @__PURE__ */ jsxs("details", { children: [
        /* @__PURE__ */ jsx("summary", { children: "Recent requests" }),
        /* @__PURE__ */ jsx("ul", { children: recent.map((r) => /* @__PURE__ */ jsxs("li", { children: [
          r.kind === "question" ? "Question" : "Permission",
          " \xB7",
          " ",
          r.state.reason === "answered_or_cleared" ? "Answered or cleared" : r.state.reason ?? r.state.state
        ] }, r.id)) })
      ] })
    ] });
  }

  // src/lib/application-view.tsx
  function ApplicationView({ children }) {
    return /* @__PURE__ */ jsx(Fragment2, { children });
  }
  function commitApplicationView(root, view) {
    render(/* @__PURE__ */ jsx(ApplicationView, { children: view }), root);
  }

  // src/lib/screens.tsx
  var owned = /* @__PURE__ */ new WeakSet();
  var kinds = /* @__PURE__ */ new WeakMap();
  var cleanups = /* @__PURE__ */ new WeakMap();
  function claimScreen(root, kind, cleanup) {
    owned.add(root);
    kinds.set(root, kind);
    cleanups.set(root, cleanup);
  }
  function releaseScreen(root) {
    cleanups.get(root)?.();
    cleanups.delete(root);
    kinds.delete(root);
    const state2 = sessionsState.get(root);
    if (state2) {
      cancelAnimationFrame(state2.frame);
      window.removeEventListener("resize", state2.resize);
      sessionsState.delete(root);
    }
    if (owned.delete(root)) render(null, root);
  }
  var screenText = (text2) => text2.replace(/ · /g, "\u2009 \xB7 \u2009").replace(/^· /, "\xB7\u2009 ");
  function MachinesView({
    snapshot,
    host: host2
  }) {
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsxs("div", { class: "ph", children: [
        /* @__PURE__ */ jsx("h1", { children: "Machines" }),
        /* @__PURE__ */ jsxs("div", { class: "sub", children: [
          /* @__PURE__ */ jsxs("span", { children: [
            /* @__PURE__ */ jsx("b", { children: snapshot.up + " of " + snapshot.rows.length }),
            "up"
          ] }),
          /* @__PURE__ */ jsxs("span", { children: [
            /* @__PURE__ */ jsx("b", { children: snapshot.working }),
            "sessions working"
          ] })
        ] })
      ] }),
      /* @__PURE__ */ jsxs("div", { class: "list", children: [
        snapshot.rows.map((row) => /* @__PURE__ */ jsxs(
          "button",
          {
            class: "nrow",
            type: "button",
            "data-m": row.id,
            onClick: (event) => {
              if (event.currentTarget.isConnected) host2.machine(row.id);
            },
            children: [
              /* @__PURE__ */ jsx(
                "span",
                {
                  class: "dot " + row.state,
                  role: "img",
                  "aria-label": row.stateLabel,
                  "data-tip": row.stateLabel
                }
              ),
              /* @__PURE__ */ jsx("span", { class: "nm", children: screenText(row.name) }),
              /* @__PURE__ */ jsx("span", { class: "ag", children: row.status }),
              /* @__PURE__ */ jsx("span", { class: "for", children: screenText(row.detail) })
            ]
          },
          row.id
        )),
        snapshot.admin && /* @__PURE__ */ jsx(
          "button",
          {
            class: "more",
            type: "button",
            onClick: (event) => {
              if (event.currentTarget.isConnected) host2.admin(snapshot.admin.href);
            },
            children: screenText(snapshot.admin.label)
          }
        )
      ] })
    ] });
  }
  function renderMachinesScreen(root, snapshot, host2) {
    commitApplicationView(root, /* @__PURE__ */ jsx(MachinesView, { snapshot, host: host2 }));
    owned.add(root);
    kinds.set(root, "machines");
    host2.committed();
  }
  function Glyph({ path, className = "icon" }) {
    return /* @__PURE__ */ jsx(
      "svg",
      {
        class: className,
        viewBox: "0 0 24 24",
        fill: "none",
        stroke: "currentColor",
        "stroke-width": "1.8",
        "stroke-linecap": "round",
        "stroke-linejoin": "round",
        "aria-hidden": "true",
        children: /* @__PURE__ */ jsx("path", { d: path })
      }
    );
  }
  function Harness({
    mark: mark2,
    size = 14,
    lead = true
  }) {
    return /* @__PURE__ */ jsx(
      "span",
      {
        class: "hicon " + (size === 16 ? "hi-screen-16" : "hi-screen") + (lead ? " hi-lead" : ""),
        "data-harness": mark2.id,
        "aria-hidden": "true",
        children: mark2.light === mark2.dark ? /* @__PURE__ */ jsx("img", { src: mark2.light, alt: "", draggable: false, decoding: "async" }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(
            "img",
            {
              class: "hi-light",
              loading: mark2.darkTheme ? "lazy" : void 0,
              src: mark2.light,
              alt: "",
              draggable: false,
              decoding: "async"
            }
          ),
          /* @__PURE__ */ jsx(
            "img",
            {
              class: "hi-dark",
              loading: mark2.darkTheme ? void 0 : "lazy",
              src: mark2.dark,
              alt: "",
              draggable: false,
              decoding: "async"
            }
          )
        ] })
      }
    );
  }
  function Section({ heading, count: count2 }) {
    return /* @__PURE__ */ jsxs("div", { class: "sec-h", children: [
      screenText(heading),
      /* @__PURE__ */ jsx("span", { class: "n", children: count2 })
    ] });
  }
  function Live({ row, host: host2 }) {
    return /* @__PURE__ */ jsxs(
      "button",
      {
        class: "nrow",
        type: "button",
        "data-id": row.id,
        onClick: (event) => {
          if (event.currentTarget.isConnected) host2.session(row.id);
        },
        children: [
          /* @__PURE__ */ jsx(
            "span",
            {
              class: "dot " + row.state,
              role: "img",
              "aria-label": row.stateLabel,
              "data-tip": row.stateLabel
            }
          ),
          /* @__PURE__ */ jsx("span", { class: "nm", children: screenText(row.name) }),
          /* @__PURE__ */ jsxs("span", { class: "ag", children: [
            row.harness && /* @__PURE__ */ jsx(Harness, { mark: row.harness }),
            screenText(row.status)
          ] }),
          /* @__PURE__ */ jsx("span", { class: "for", children: screenText(row.detail) }),
          row.activity && /* @__PURE__ */ jsxs("span", { class: "act", children: [
            /* @__PURE__ */ jsx("span", { class: "spin" }),
            /* @__PURE__ */ jsx("span", { children: screenText(row.activity[0]) }),
            /* @__PURE__ */ jsx("code", { children: row.activity[1] }),
            /* @__PURE__ */ jsx("span", { class: "el", children: Math.max(0, row.activity[2]) + "s" })
          ] })
        ]
      }
    );
  }
  function Inbox({ row, host: host2 }) {
    return /* @__PURE__ */ jsxs(
      "div",
      {
        class: "ib" + (row.quiet ? " quiet" : ""),
        tabIndex: 0,
        role: "link",
        "data-h": row.id,
        onClick: (event) => {
          if (event.currentTarget.isConnected && getSelection()?.isCollapsed) host2.inbox(row.id);
        },
        onKeyDown: (event) => {
          if (event.currentTarget.isConnected && event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            host2.inbox(row.id);
          }
        },
        children: [
          /* @__PURE__ */ jsx(Glyph, { path: row.icon }),
          /* @__PURE__ */ jsx("span", { class: "ln", children: row.parts.map((part, i) => /* @__PURE__ */ jsx("span", { class: part.className, "data-tip": part.tip, children: screenText(part.text) }, i)) }),
          /* @__PURE__ */ jsx("span", { class: "tm", children: row.age }),
          /* @__PURE__ */ jsx("span", { class: "q", children: /* @__PURE__ */ jsx(Inline, { text: row.preview }) }),
          row.answer !== void 0 && /* @__PURE__ */ jsx("span", { class: "ans", children: screenText(row.answer) }),
          row.origin && /* @__PURE__ */ jsx("span", { class: "org", children: row.origin.message ? /* @__PURE__ */ jsxs(Fragment2, { children: [
            "From your message:",
            " ",
            /* @__PURE__ */ jsx("span", { class: "oq", children: "\u201C" + screenText(row.origin.message) + "\u201D" })
          ] }) : screenText(row.origin.text ?? "") }),
          /* @__PURE__ */ jsxs("span", { class: "ctx", children: [
            /* @__PURE__ */ jsxs("span", { children: [
              row.harness && /* @__PURE__ */ jsx(Harness, { mark: row.harness }),
              screenText(row.context)
            ] }),
            row.trace && /* @__PURE__ */ jsxs(
              "button",
              {
                class: "tracebtn",
                type: "button",
                "aria-label": "Trace what this turn set off",
                onClick: (event) => {
                  event.stopPropagation();
                  if (event.currentTarget.isConnected) host2.trace(row.trace);
                },
                children: [
                  /* @__PURE__ */ jsx(Glyph, { path: "M6 4v10a4 4 0 0 0 4 4h8M6 10h12M15 7l3 3-3 3M15 15l3 3-3 3" }),
                  /* @__PURE__ */ jsx("span", { children: "Trace" })
                ]
              }
            )
          ] })
        ]
      }
    );
  }
  function HomeView({ snapshot, host: host2 }) {
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      snapshot.observation && /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: snapshot.observation }),
      /* @__PURE__ */ jsxs("div", { class: "ph", children: [
        /* @__PURE__ */ jsx("h1", { children: "Home" }),
        /* @__PURE__ */ jsxs("div", { class: "sub", children: [
          /* @__PURE__ */ jsxs("span", { children: [
            /* @__PURE__ */ jsx("b", { children: snapshot.waiting }),
            "waiting on you"
          ] }),
          /* @__PURE__ */ jsxs("span", { children: [
            /* @__PURE__ */ jsx("b", { children: snapshot.working }),
            "working"
          ] }),
          /* @__PURE__ */ jsxs("span", { children: [
            /* @__PURE__ */ jsx("b", { children: snapshot.up + " of " + snapshot.machines }),
            snapshot.machines === 1 ? "machine up" : "machines up"
          ] })
        ] })
      ] }),
      snapshot.control && /* @__PURE__ */ jsx(LocalControl, { view: snapshot.control }),
      /* @__PURE__ */ jsx(Section, { heading: "Needs you", count: snapshot.waiting }),
      /* @__PURE__ */ jsxs("div", { class: "list", children: [
        snapshot.inbox.map((row) => /* @__PURE__ */ jsx(Inbox, { row, host: host2 }, row.id)),
        snapshot.moreInbox && /* @__PURE__ */ jsx("button", { class: "more", type: "button", onClick: () => host2.more?.("inbox"), children: "Load more requests" }),
        !snapshot.waiting && /* @__PURE__ */ jsx("p", { class: "empty", children: "Nothing is waiting on you." })
      ] }),
      /* @__PURE__ */ jsx(Section, { heading: "Working now", count: snapshot.working }),
      /* @__PURE__ */ jsxs("div", { class: "list", children: [
        snapshot.live.map((row) => /* @__PURE__ */ jsx(Live, { row, host: host2 }, row.id)),
        snapshot.moreLive && /* @__PURE__ */ jsx("button", { class: "more", type: "button", onClick: () => host2.more?.("working"), children: "Load more working sessions" }),
        !snapshot.working && /* @__PURE__ */ jsx("p", { class: "empty", children: "Nothing is running." })
      ] }),
      snapshot.totalAnswered > 0 && /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(Section, { heading: "Answered", count: snapshot.totalAnswered }),
        /* @__PURE__ */ jsxs("div", { class: "list", children: [
          snapshot.answered.map((row) => /* @__PURE__ */ jsx(Inbox, { row, host: host2 }, row.id)),
          snapshot.allAnswered && snapshot.moreAnswered && /* @__PURE__ */ jsx("button", { class: "more", type: "button", onClick: () => host2.more?.("answered"), children: "Load more answered" }),
          !snapshot.allAnswered && snapshot.totalAnswered > 3 && /* @__PURE__ */ jsx(
            "button",
            {
              class: "more",
              type: "button",
              onClick: (event) => {
                if (event.currentTarget.isConnected) host2.answered();
              },
              children: "Show all " + snapshot.totalAnswered + " answered"
            }
          )
        ] })
      ] })
    ] });
  }
  function renderHomeScreen(root, snapshot, host2) {
    commitApplicationView(root, /* @__PURE__ */ jsx(HomeView, { snapshot, host: host2 }));
    owned.add(root);
    kinds.set(root, "home");
    host2.committed();
  }
  function fitSessionRows(root) {
    for (const meta of root.querySelectorAll(".session-row-meta")) {
      if (!meta.isConnected || !meta.clientWidth) continue;
      const values = [...meta.querySelectorAll("[data-drop]")];
      for (const value of values) value.hidden = false;
      for (const value of values.sort((a, b) => Number(b.dataset.drop) - Number(a.dataset.drop))) {
        if (meta.scrollWidth <= meta.clientWidth + 1) break;
        value.hidden = true;
      }
    }
  }
  function SessionRowView({
    row,
    host: host2
  }) {
    return /* @__PURE__ */ jsxs(
      "button",
      {
        class: "nrow",
        type: "button",
        "data-id": row.id,
        "data-session-row": "list",
        onClick: (event) => {
          if (event.currentTarget.isConnected) host2.session(row.id);
        },
        children: [
          /* @__PURE__ */ jsxs("span", { class: "session-row-main srow-main", children: [
            /* @__PURE__ */ jsx(
              "span",
              {
                class: "dot " + row.state,
                role: "img",
                "aria-label": row.stateLabel,
                "data-tip": row.stateLabel
              }
            ),
            /* @__PURE__ */ jsx("span", { class: "nm", "data-tip": row.name, "data-tip-clipped": "", children: screenText(row.name) }),
            /* @__PURE__ */ jsx("span", { class: "ag", children: row.age })
          ] }),
          /* @__PURE__ */ jsxs("span", { class: "session-row-meta srow-meta for", children: [
            row.harness && /* @__PURE__ */ jsx(
              "span",
              {
                class: "hicon hi-screen",
                "data-harness": row.harness.id,
                role: "img",
                "aria-label": row.harness.name,
                "data-tip": row.harness.name,
                children: row.harness.light === row.harness.dark ? /* @__PURE__ */ jsx("img", { src: row.harness.light, alt: "", draggable: false, decoding: "async" }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
                  /* @__PURE__ */ jsx(
                    "img",
                    {
                      class: "hi-light",
                      loading: row.harness.darkTheme ? "lazy" : void 0,
                      src: row.harness.light,
                      alt: "",
                      draggable: false,
                      decoding: "async"
                    }
                  ),
                  /* @__PURE__ */ jsx(
                    "img",
                    {
                      class: "hi-dark",
                      loading: row.harness.darkTheme ? void 0 : "lazy",
                      src: row.harness.dark,
                      alt: "",
                      draggable: false,
                      decoding: "async"
                    }
                  )
                ] })
              }
            ),
            /* @__PURE__ */ jsx("span", { class: "row-model", "data-tip": row.modelTip, children: screenText(row.model) }),
            row.delegation && /* @__PURE__ */ jsx(
              "svg",
              {
                class: "icon row-delegation",
                "data-tip": "Delegated session",
                viewBox: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                "stroke-width": "1.8",
                "stroke-linecap": "round",
                "stroke-linejoin": "round",
                "aria-hidden": "true",
                children: /* @__PURE__ */ jsx("path", { d: row.delegation })
              }
            ),
            row.fields.map((field) => /* @__PURE__ */ jsx(
              "span",
              {
                class: field.className + (field.icon ? " row-field" : ""),
                "data-drop": field.priority,
                "data-tip": field.tip,
                children: field.icon ? /* @__PURE__ */ jsxs(Fragment2, { children: [
                  /* @__PURE__ */ jsx(Glyph, { path: field.icon }),
                  /* @__PURE__ */ jsx("span", { class: "field-value", children: screenText(field.text) })
                ] }) : screenText(field.text)
              },
              field.priority
            ))
          ] })
        ]
      }
    );
  }
  var sessionsState = /* @__PURE__ */ new WeakMap();

  // src/lib/attachments.tsx
  function createImageViewer(url, label, host2) {
    const dialog = document.createElement("dialog");
    dialog.className = "viewer image-viewer";
    dialog.setAttribute("aria-label", label);
    let disposed = false, shown = false, failed = false, close = null;
    function finish() {
      if (disposed) return;
      disposed = true;
      dialog.removeEventListener("close", finish);
      dialog.removeEventListener("click", backdrop);
      if (dialog.open) dialog.close();
      render(null, dialog);
      dialog.remove();
      if (shown) host2.closed(dialog);
    }
    function backdrop(event) {
      if (event.target === dialog || event.target instanceof Element && event.target.classList.contains("vb"))
        dialog.close();
    }
    function paint2() {
      if (disposed) return;
      render(
        /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsxs("div", { class: "vh", children: [
            /* @__PURE__ */ jsx("div", { class: "vt", children: /* @__PURE__ */ jsx("span", { children: label }) }),
            /* @__PURE__ */ jsx(
              "button",
              {
                class: "vclose",
                type: "button",
                "aria-label": "Close",
                ref: (node) => {
                  close = node;
                },
                onClick: () => {
                  if (!disposed) dialog.close();
                },
                children: /* @__PURE__ */ jsx(
                  "svg",
                  {
                    class: "icon",
                    viewBox: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    "stroke-width": "1.8",
                    "stroke-linecap": "round",
                    "stroke-linejoin": "round",
                    "aria-hidden": "true",
                    children: /* @__PURE__ */ jsx("path", { d: "M6 6l12 12M18 6L6 18" })
                  }
                )
              }
            )
          ] }),
          /* @__PURE__ */ jsx("div", { class: "vb", children: failed ? /* @__PURE__ */ jsx("p", { class: "vnote", children: "Image not available." }) : /* @__PURE__ */ jsx(
            "img",
            {
              class: "attach-full",
              alt: label,
              decoding: "async",
              src: url,
              onError: () => {
                failed = true;
                paint2();
              }
            }
          ) })
        ] }),
        dialog
      );
    }
    paint2();
    dialog.addEventListener("close", finish);
    dialog.addEventListener("click", backdrop);
    return {
      dialog,
      show() {
        if (shown || disposed) return;
        shown = true;
        document.body.append(dialog);
        dialog.showModal();
        close?.focus();
        host2.opened(dialog);
      },
      destroy: finish
    };
  }

  // src/lib/tool-details.tsx
  var isCommand = (name) => /^(Bash|shell|exec_command|local_shell)$/.test(name);
  var gapText = (gap) => (gap.unit === "tokens" ? "About " : "") + (gap.unit === "lines" && gap.of != null ? gap.n.toLocaleString("en-US") + " of " + gap.of.toLocaleString("en-US") + " lines" : gap.n.toLocaleString("en-US") + " " + (gap.unit === "chars" ? "characters" : gap.unit)) + " cut here by Codex";
  var cutNoteText = (cut2) => "Codex cut this output before the model saw it" + (cut2.original_tokens ? " (about " + cut2.original_tokens.toLocaleString("en-US") + " tokens in all)" : "") + ".";
  function Icon4({ path }) {
    return /* @__PURE__ */ jsx(
      "svg",
      {
        class: "icon",
        viewBox: "0 0 24 24",
        fill: "none",
        stroke: "currentColor",
        "stroke-width": "1.8",
        "stroke-linecap": "round",
        "stroke-linejoin": "round",
        "aria-hidden": "true",
        children: /* @__PURE__ */ jsx("path", { d: path })
      }
    );
  }
  var COPY = "M9 9h11v11H9zM5 15H4V4h11v1";
  var EXPAND = "M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7";
  function Copy({ text: text2 }) {
    const [label, setLabel] = useState("Copy");
    return /* @__PURE__ */ jsxs(
      "button",
      {
        class: "link copy",
        type: "button",
        "aria-label": "Copy",
        onClick: (event) => {
          const button = event.currentTarget;
          if (!button.isConnected) return;
          navigator.clipboard?.writeText(text2).then(
            () => {
              if (button.isConnected) setLabel("Copied");
            },
            () => {
              if (button.isConnected) setLabel("Copy failed");
            }
          );
        },
        children: [
          /* @__PURE__ */ jsx(Icon4, { path: COPY }),
          /* @__PURE__ */ jsx("span", { children: label })
        ]
      }
    );
  }
  function Header({
    label,
    text: text2,
    className = "io"
  }) {
    return /* @__PURE__ */ jsxs("div", { class: className, children: [
      /* @__PURE__ */ jsx("span", { children: screenText(label) }),
      text2 && /* @__PURE__ */ jsx(Copy, { text: text2 })
    ] });
  }
  function Diff({ rows }) {
    return /* @__PURE__ */ jsx("div", { class: "diff", children: rows.map(([cls, text2], i) => /* @__PURE__ */ jsx("div", { class: cls, children: screenText(text2) }, i)) });
  }
  function Output({ data }) {
    return data.cut?.parts?.length ? /* @__PURE__ */ jsx("div", { class: "cutout", children: data.cut.parts.map(
      (part, i) => part.gap ? /* @__PURE__ */ jsx("div", { class: "cutgap", children: screenText(gapText(part.gap)) }, i) : /* @__PURE__ */ jsx("pre", { children: part.text }, i)
    ) }) : /* @__PURE__ */ jsx("pre", { children: data.out });
  }
  function ViewAll({ label, action }) {
    return /* @__PURE__ */ jsxs(
      "button",
      {
        class: "viewall",
        type: "button",
        onClick: (event) => {
          if (event.currentTarget.isConnected) action();
        },
        children: [
          /* @__PURE__ */ jsx(Icon4, { path: EXPAND }),
          /* @__PURE__ */ jsx("span", { children: label })
        ]
      }
    );
  }
  function ToolPreview({
    data,
    waiting,
    host: host2
  }) {
    const command = data.in ?? (isCommand(data.name) ? data.arg : null), inCut = typeof command === "string" && command.split("\n").length > 12;
    const inputLabel = command && isCommand(data.name) ? "Command" : "Input";
    const lines = (data.out ?? "").split("\n"), cut2 = lines.length > 12, tail = data.ok === false, parts = !!data.cut?.parts?.length, more = !!data.more?.length || data.expandable === true;
    const shown = !cut2 || parts ? lines : tail ? lines.slice(-12) : lines.slice(0, 12);
    const changed = !!data.changes || !!data.diff;
    const all = () => host2.all(inputLabel);
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      command ? /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(Header, { label: inputLabel, text: command }),
        /* @__PURE__ */ jsx("pre", { class: "in" + (inCut ? " clip clipped" : ""), children: command })
      ] }) : !changed && /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(
          Header,
          {
            label: /^(Read|Grep|Glob)$/.test(data.name) ? data.name === "Read" ? "File" : "Pattern" : "Input"
          }
        ),
        /* @__PURE__ */ jsx("pre", { class: "in", children: data.arg })
      ] }),
      data.cwd && data.cwd !== "." && /* @__PURE__ */ jsx(Header, { label: "Working directory \xB7 " + data.cwd }),
      data.changes ? /* @__PURE__ */ jsxs(Fragment2, { children: [
        data.changes.map((change, i) => /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(
            Header,
            {
              label: "Change \xB7 " + change.path + (change.move ? " \u2192 " + change.move : "")
            },
            "head" + i
          ),
          change.diff?.length ? /* @__PURE__ */ jsx(Diff, { rows: change.diff }, "diff" + i) : /* @__PURE__ */ jsx("div", { class: "noout", children: "No diff recorded" })
        ] })),
        !data.changes.length && /* @__PURE__ */ jsx("div", { class: "noout", children: "No changes recorded" })
      ] }) : data.diff ? /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(Header, { label: "Change \xB7 " + data.arg }),
        /* @__PURE__ */ jsx(Diff, { rows: data.diff })
      ] }) : !data.out ? /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(Header, { label: "Output" }),
        /* @__PURE__ */ jsx("div", { class: "noout", children: screenText(
          data.live ? waiting ? "Waiting for your input or permission \xB7 no output yet" : "Running \xB7 no output yet" : data.unfinished ? "No result recorded: the machine stopped responding while this ran." : "No output"
        ) })
      ] }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(
          Header,
          {
            label: parts || !cut2 ? "Output" : (tail ? "Output \xB7 last " : "Output \xB7 first ") + "12" + (more ? "" : " of " + lines.length) + " lines"
          }
        ),
        parts ? /* @__PURE__ */ jsx(Output, { data }) : /* @__PURE__ */ jsx("pre", { children: shown.join("\n") }),
        data.cut ? /* @__PURE__ */ jsx("div", { class: "cutnote", children: screenText(cutNoteText(data.cut)) }) : [data.in, data.out].some((text2) => /…(\(truncated\))?\s*$/.test(text2 ?? "")) && /* @__PURE__ */ jsx("div", { class: "cutnote", children: "Cut short in this copy of the logs" })
      ] }),
      (inCut || !changed && !!data.out && (cut2 && !parts || more)) && /* @__PURE__ */ jsx(
        ViewAll,
        {
          label: changed || more || parts || inCut ? "View all" : "View all " + lines.length + " lines",
          action: all
        }
      ),
      data.script != null && /* @__PURE__ */ jsxs(
        "button",
        {
          class: "viewall viewscript",
          type: "button",
          onClick: (event) => {
            if (event.currentTarget.isConnected) host2.script();
          },
          children: [
            /* @__PURE__ */ jsx(Icon4, { path: EXPAND }),
            /* @__PURE__ */ jsx("span", { children: "View script" })
          ]
        }
      ),
      data.bg?.summary && /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx("div", { class: "io", children: "Finished" }),
        /* @__PURE__ */ jsx("pre", { class: "finished", children: data.bg.summary })
      ] })
    ] });
  }
  function Note({ data, text: text2 }) {
    return data.cut ? /* @__PURE__ */ jsx("p", { class: "vnote", children: screenText(cutNoteText(data.cut)) }) : !data.fullFailed && !data.boundedPreview && /…(\(truncated\))?\s*$/.test(text2 ?? "") ? /* @__PURE__ */ jsx("p", { class: "vnote", children: "Cut short in this copy of the logs." }) : null;
  }
  function FullTool({
    data,
    inputLabel,
    waiting
  }) {
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      data.scriptText !== void 0 ? /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(Header, { className: "vs", label: "Script", text: data.scriptText }),
        /* @__PURE__ */ jsx("pre", { class: "script", children: data.scriptText }),
        data.scriptTruncated && /* @__PURE__ */ jsx("p", { class: "vnote", children: "The script preview is bounded; the remaining text is not shown." })
      ] }) : data.scriptFailed ? /* @__PURE__ */ jsx("p", { class: "vnote", children: "Couldn't load the script from these logs." }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
        data.in && /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Header, { className: "vs", label: inputLabel, text: data.in }),
          /* @__PURE__ */ jsx("pre", { class: "in", children: data.in }),
          /* @__PURE__ */ jsx(Note, { data, text: data.in })
        ] }),
        data.changes ? /* @__PURE__ */ jsxs(Fragment2, { children: [
          data.changes.map((change, i) => /* @__PURE__ */ jsxs(Fragment2, { children: [
            /* @__PURE__ */ jsx(
              Header,
              {
                className: "vs",
                label: "Change \xB7 " + change.path + (change.move ? " \u2192 " + change.move : "")
              },
              i
            ),
            /* @__PURE__ */ jsx(Diff, { rows: change.diff ?? [] })
          ] })),
          !data.changes.length && /* @__PURE__ */ jsx("p", { class: "vnote", children: "No changes recorded." })
        ] }) : data.diff ? /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Header, { className: "vs", label: "Change" }),
          /* @__PURE__ */ jsx(Diff, { rows: data.diff })
        ] }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Header, { className: "vs", label: "Output", text: data.out }),
          data.out ? /* @__PURE__ */ jsxs(Fragment2, { children: [
            /* @__PURE__ */ jsx(Output, { data }),
            /* @__PURE__ */ jsx(Note, { data, text: data.out })
          ] }) : /* @__PURE__ */ jsx("p", { class: "vnote", children: screenText(
            data.live ? waiting ? "Waiting for your input or permission \xB7 no output yet" : "Running \xB7 no output yet" : data.unfinished ? "No result recorded." : "No output."
          ) })
        ] })
      ] }),
      data.bg?.summary && /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(Header, { className: "vs", label: "Finished", text: data.bg.summary }),
        /* @__PURE__ */ jsx("pre", { children: data.bg.summary })
      ] }),
      data.fullFailed && /* @__PURE__ */ jsx("p", { class: "vnote", children: "Couldn't load the full text: this is the preview." }),
      !!data.fullCut?.length && /* @__PURE__ */ jsx("p", { class: "vnote", children: "The script preview is bounded; the remaining text is not shown." })
    ] });
  }

  // src/lib/tool-step.tsx
  function ToolStepContents({
    snapshot,
    expanded,
    mounted: mounted2,
    toggle,
    host: host2
  }) {
    function icon(path, cls) {
      return /* @__PURE__ */ jsx(
        "svg",
        {
          class: cls,
          viewBox: "0 0 24 24",
          fill: "none",
          stroke: "currentColor",
          "stroke-width": "1.8",
          "stroke-linecap": "round",
          "stroke-linejoin": "round",
          "aria-hidden": "true",
          children: /* @__PURE__ */ jsx("path", { d: path })
        }
      );
    }
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsxs(
        "button",
        {
          type: "button",
          "aria-expanded": expanded,
          onClick: (event) => {
            if (event.currentTarget.isConnected) toggle();
          },
          children: [
            snapshot.named && /* @__PURE__ */ jsx("span", { class: "sr-only", children: screenText(snapshot.prefix ?? "") }),
            snapshot.running ? /* @__PURE__ */ jsx("span", { class: "spin" }) : icon(snapshot.icon, "icon"),
            snapshot.background && /* @__PURE__ */ jsx("span", { class: "bgmark", role: "img", "aria-label": "background" }),
            !snapshot.named && /* @__PURE__ */ jsx("span", { class: "sv", children: screenText(snapshot.verb ?? "") }),
            snapshot.named ? /* @__PURE__ */ jsx("span", { class: "sa st", "data-tip": snapshot.tip, children: screenText(snapshot.label) }) : /* @__PURE__ */ jsx("code", { class: "sa", children: snapshot.label }),
            /* @__PURE__ */ jsxs("span", { class: "sd" + (snapshot.running ? " tick" : ""), children: [
              screenText(snapshot.status ?? ""),
              snapshot.background && /* @__PURE__ */ jsxs(Fragment2, { children: [
                /* @__PURE__ */ jsx("span", { class: "bgw", children: "background\u2009 \xB7 \u2009" }),
                /* @__PURE__ */ jsx("span", { class: "bgo", children: screenText(snapshot.backgroundStatus ?? "") })
              ] })
            ] }),
            icon(snapshot.chevron, "chev")
          ]
        }
      ),
      mounted2 && /* @__PURE__ */ jsx("div", { class: "out", hidden: !expanded, children: /* @__PURE__ */ jsx(ToolPreview, { data: snapshot.data, waiting: snapshot.waiting, host: host2 }) })
    ] });
  }

  // src/lib/session-menu.tsx
  function renderSessionMenu(root, snapshot, host2) {
    let tokens2 = false, allRuns = false, copyLabel = "Copy resume command", wide = host2.wide();
    const live = () => root.isConnected;
    function details(rows) {
      return /* @__PURE__ */ jsx("dl", { class: "kv", children: rows.map((row, i) => /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx("dt", { children: screenText(row.label) }, "dt" + i),
        /* @__PURE__ */ jsx("dd", { class: row.mono ? "mono" : void 0, children: row.harness ? /* @__PURE__ */ jsxs("span", { children: [
          /* @__PURE__ */ jsx(Harness, { mark: row.harness, size: 16 }),
          screenText(row.value)
        ] }) : screenText(row.value) }, "dd" + i)
      ] })) });
    }
    function paint2() {
      render(
        /* @__PURE__ */ jsxs(Fragment2, { children: [
          snapshot.path.length > 1 && /* @__PURE__ */ jsxs(Fragment2, { children: [
            /* @__PURE__ */ jsxs("div", { class: "menu-status", role: "presentation", children: [
              /* @__PURE__ */ jsx(
                "span",
                {
                  class: "dot " + snapshot.state,
                  role: "img",
                  "aria-label": snapshot.stateLabel,
                  "aria-hidden": "true"
                }
              ),
              /* @__PURE__ */ jsx("span", { children: screenText(snapshot.status) })
            ] }),
            /* @__PURE__ */ jsx("div", { class: "menu-list menu-path-menu", role: "menu", children: /* @__PURE__ */ jsxs("div", { class: "menu-path-group", role: "group", "aria-labelledby": "menu-path-heading", children: [
              /* @__PURE__ */ jsx("div", { class: "menu-section-heading", id: "menu-path-heading", children: "Session path" }),
              snapshot.path.slice(0, -1).map((ancestor, i) => /* @__PURE__ */ jsxs(
                "button",
                {
                  class: "menu-item menu-path-item",
                  type: "button",
                  role: "menuitem",
                  "aria-label": i === snapshot.path.length - 2 ? "Up to " + ancestor.name : void 0,
                  onClick: () => {
                    if (live()) host2.session(ancestor.id);
                  },
                  children: [
                    /* @__PURE__ */ jsx(
                      "span",
                      {
                        class: "menu-path-chevron" + (i === snapshot.path.length - 2 ? "" : " blank"),
                        "aria-hidden": i === snapshot.path.length - 2 ? void 0 : true
                      }
                    ),
                    /* @__PURE__ */ jsx("span", { class: "menu-path-name", children: screenText(ancestor.name) }),
                    /* @__PURE__ */ jsx("span", { class: "hname h-" + ancestor.harness, children: screenText(ancestor.harnessName) })
                  ]
                },
                ancestor.id
              ))
            ] }) }),
            /* @__PURE__ */ jsx("div", { class: "menu-separator", role: "separator" })
          ] }),
          /* @__PURE__ */ jsx("section", { class: "panel-sec", children: /* @__PURE__ */ jsx("div", { class: "menu-list", role: "menu", children: snapshot.actions.map((action) => /* @__PURE__ */ jsxs(
            "button",
            {
              class: "menu-item" + (action.className ? " " + action.className : ""),
              type: "button",
              role: action.checked === void 0 ? "menuitem" : "menuitemcheckbox",
              "aria-checked": action.checked === void 0 ? void 0 : wide,
              onClick: () => {
                if (!live()) return;
                if (action.key === "copy") {
                  navigator.clipboard?.writeText(snapshot.command).then(
                    () => {
                      if (live()) {
                        copyLabel = "Copied";
                        paint2();
                      }
                    },
                    () => {
                      if (live()) {
                        copyLabel = snapshot.command;
                        paint2();
                      }
                    }
                  );
                } else {
                  host2.action(action.key);
                  if (action.key === "wide") {
                    wide = host2.wide();
                    paint2();
                  }
                }
              },
              children: [
                action.icon && /* @__PURE__ */ jsx(Glyph, { path: action.icon }),
                action.dot && /* @__PURE__ */ jsx("span", { class: "dot " + action.dot, "aria-hidden": "true" }),
                /* @__PURE__ */ jsx("span", { children: screenText(action.key === "copy" ? copyLabel : action.text) }),
                action.checked !== void 0 && /* @__PURE__ */ jsx("span", { class: "switch" }),
                action.note && /* @__PURE__ */ jsx("span", { class: "menu-note", children: action.note })
              ]
            },
            action.key
          )) }) }),
          /* @__PURE__ */ jsxs("section", { class: "panel-sec", children: [
            /* @__PURE__ */ jsx("h3", { children: "Details" }),
            details(snapshot.details)
          ] }),
          /* @__PURE__ */ jsxs("section", { class: "panel-sec cost", children: [
            /* @__PURE__ */ jsx("h3", { children: "Cost" }),
            /* @__PURE__ */ jsxs("div", { class: "cost-fig", children: [
              /* @__PURE__ */ jsx("span", { class: "cost-big", children: snapshot.cost.figure }),
              /* @__PURE__ */ jsx("span", { class: "cost-cap", children: snapshot.cost.caption })
            ] }),
            /* @__PURE__ */ jsx("p", { class: "cost-note", children: screenText(snapshot.cost.note) }),
            snapshot.cost.details.length > 0 && details(snapshot.cost.details),
            snapshot.cost.mismatch && /* @__PURE__ */ jsx("p", { class: "cost-note", children: snapshot.cost.mismatch }),
            snapshot.cost.runs.length > 0 && /* @__PURE__ */ jsxs("div", { class: "runs", "aria-label": "Runs and their cost", children: [
              snapshot.cost.runs.map((run, i) => /* @__PURE__ */ jsxs(
                "button",
                {
                  class: "run-row depth" + Math.min(run.depth, 1),
                  type: "button",
                  hidden: !allRuns && i >= 5,
                  "aria-label": run.label,
                  onClick: () => {
                    if (live()) host2.session(run.id);
                  },
                  children: [
                    /* @__PURE__ */ jsx(
                      "span",
                      {
                        class: "dot " + run.state,
                        role: "img",
                        "aria-label": run.stateLabel,
                        "data-tip": run.stateLabel
                      }
                    ),
                    /* @__PURE__ */ jsxs("span", { class: "nm", children: [
                      screenText(run.name),
                      /* @__PURE__ */ jsx("span", { class: "kind", children: screenText("\xB7 " + run.kind) })
                    ] }),
                    /* @__PURE__ */ jsx("span", { class: "v", children: run.cost }),
                    /* @__PURE__ */ jsx(Glyph, { path: snapshot.icons.chevron, className: "chev" })
                  ]
                },
                run.id
              )),
              !allRuns && snapshot.cost.runs.length > 5 && /* @__PURE__ */ jsx(
                "button",
                {
                  class: "link",
                  type: "button",
                  onClick: () => {
                    if (live()) {
                      allRuns = true;
                      paint2();
                    }
                  },
                  children: "Show " + (snapshot.cost.runs.length - 5) + " more"
                }
              )
            ] }),
            /* @__PURE__ */ jsxs(
              "button",
              {
                class: "disclose",
                type: "button",
                "aria-expanded": tokens2,
                onClick: () => {
                  if (live()) {
                    tokens2 = !tokens2;
                    paint2();
                  }
                },
                children: [
                  /* @__PURE__ */ jsx("span", { children: screenText("Tokens by model" + (snapshot.cost.includesRuns ? " \xB7 incl. runs" : "")) }),
                  /* @__PURE__ */ jsx(Glyph, { path: snapshot.icons.chevron, className: "chev" })
                ]
              }
            ),
            /* @__PURE__ */ jsx("div", { class: "tokens", hidden: !tokens2, children: snapshot.cost.models.map((model2) => /* @__PURE__ */ jsxs(Fragment2, { children: [
              /* @__PURE__ */ jsx("div", { class: "tok-model", children: screenText(model2.id) }, model2.id),
              model2.rows.map((row) => /* @__PURE__ */ jsxs("div", { class: "tok-line", children: [
                /* @__PURE__ */ jsx("span", { children: row.label }),
                /* @__PURE__ */ jsxs("span", { "data-tip": row.exact ? row.exact + " tokens" : void 0, children: [
                  row.count,
                  row.exact && /* @__PURE__ */ jsx("span", { class: "sr-only", children: " (" + row.exact + ")" })
                ] }),
                /* @__PURE__ */ jsx("span", { children: row.cost })
              ] }, model2.id + row.label))
            ] })) })
          ] }),
          /* @__PURE__ */ jsx("p", { class: "third-party", children: snapshot.notice })
        ] }),
        root
      );
    }
    paint2();
  }

  // src/lib/sheets.tsx
  function createNativeSheet(options2, host2) {
    const dialog = document.createElement("dialog");
    dialog.className = "viewer " + options2.className;
    dialog.setAttribute("aria-label", options2.label);
    let body = null, close = null, shown = false, disposed = false;
    const cleanups2 = [];
    function finish() {
      if (disposed) return;
      disposed = true;
      dialog.removeEventListener("close", finish);
      dialog.removeEventListener("click", backdrop);
      if (dialog.open) dialog.close();
      for (const cleanup of cleanups2) cleanup();
      render(null, dialog);
      dialog.remove();
      if (shown) host2.closed(dialog);
    }
    function backdrop(event) {
      if (event.target === dialog) dialog.close();
    }
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsxs("div", { class: "vh", children: [
          /* @__PURE__ */ jsx("div", { class: "vt", children: /* @__PURE__ */ jsx("span", { children: screenText(options2.heading) }) }),
          options2.caption && /* @__PURE__ */ jsx("div", { class: "vm", children: screenText(options2.caption) }),
          /* @__PURE__ */ jsx(
            "button",
            {
              class: "vclose",
              type: "button",
              "aria-label": options2.closeLabel,
              ref: (node) => {
                close = node;
              },
              onClick: () => {
                if (!disposed) dialog.close();
              },
              children: /* @__PURE__ */ jsx(Glyph, { path: "M6 6l12 12M18 6L6 18", className: "" })
            }
          )
        ] }),
        options2.search && /* @__PURE__ */ jsxs("label", { class: "kids-search", children: [
          /* @__PURE__ */ jsx(Glyph, { path: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4", className: "" }),
          /* @__PURE__ */ jsx(
            "input",
            {
              type: "search",
              placeholder: options2.search.placeholder,
              "aria-label": options2.search.label,
              onInput: (event) => {
                if (!disposed) options2.search?.change(event.currentTarget.value);
              }
            }
          )
        ] }),
        /* @__PURE__ */ jsx(
          "div",
          {
            class: "vb",
            ref: (node) => {
              body = node;
            }
          }
        )
      ] }),
      dialog
    );
    const slot = body;
    if (!slot) throw new Error("Missing sheet content slot");
    dialog.addEventListener("close", finish);
    dialog.addEventListener("click", backdrop);
    return {
      dialog,
      body: slot,
      cleanup(callback) {
        cleanups2.push(callback);
      },
      show() {
        if (shown || disposed) return;
        shown = true;
        document.body.append(dialog);
        dialog.showModal();
        close?.focus({ focusVisible: false });
        host2.opened(dialog);
      },
      destroy: finish
    };
  }
  function createKidsSheet(name, rows, host2) {
    let query = "";
    const sheet2 = createNativeSheet(
      {
        className: "kids-sheet",
        label: "All sessions under " + name,
        heading: name,
        caption: rows.length + (rows.length === 1 ? " session" : " sessions"),
        closeLabel: "Close",
        search: {
          placeholder: "Search these sessions",
          label: "Search these sessions",
          change(value) {
            query = value.trim();
            paint2();
          }
        }
      },
      host2
    );
    function paint2() {
      const filtered = rows.filter((row) => host2.matches(row.id, query));
      render(
        /* @__PURE__ */ jsxs("div", { class: "kids-list", children: [
          !filtered.length && /* @__PURE__ */ jsx("p", { class: "empty", children: "No sessions match \u201C" + screenText(query) + "\u201D." }),
          ["Waiting for you", "Running", "Finished"].map((label, i) => {
            const items = filtered.filter((row) => row.bucket === i);
            return items.length > 0 && /* @__PURE__ */ jsxs("section", { class: "kids-sec", children: [
              /* @__PURE__ */ jsx("h3", { class: "kids-h", children: label + " (" + items.length + ")" }),
              items.map((row) => /* @__PURE__ */ jsxs(
                "button",
                {
                  class: "kids-row",
                  type: "button",
                  "data-id": row.id,
                  "aria-label": row.name + ", " + row.stateLabel,
                  onClick: (event) => {
                    if (event.currentTarget.isConnected) host2.select(row.id);
                  },
                  children: [
                    /* @__PURE__ */ jsx(
                      "span",
                      {
                        class: "dot " + row.state,
                        role: "img",
                        "aria-label": row.stateLabel,
                        "data-tip": row.stateLabel
                      }
                    ),
                    /* @__PURE__ */ jsx("span", { class: "nm", children: screenText(row.name) }),
                    row.under && /* @__PURE__ */ jsx("span", { class: "under", children: "under " + screenText(row.under) }),
                    /* @__PURE__ */ jsx("span", { class: "ag", children: row.age })
                  ]
                },
                row.id
              ))
            ] }, i);
          })
        ] }),
        sheet2.body
      );
    }
    paint2();
    sheet2.cleanup(() => render(null, sheet2.body));
    return sheet2;
  }

  // src/lib/layout.ts
  var properties2 = [
    "width",
    "height",
    "left",
    "top",
    "margin-left",
    "min-width",
    "max-height",
    "max-width"
  ];
  var serial2 = 0;
  function createMeasuredLayout() {
    const sheet2 = new CSSStyleSheet(), prefix = "semon-geometry-" + ++serial2 + "-";
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
        if (!properties2.includes(property) || !Number.isFinite(value) || Math.abs(value) > 1e8 || !["px", "%"].includes(unit))
          throw new Error("Invalid measured geometry");
        const key2 = property + ":" + value + unit, existing = rules.get(key2);
        if (existing) return existing;
        const name = prefix + rules.size;
        sheet2.insertRule(
          "." + name + "{" + property + ":" + value + unit + "}",
          sheet2.cssRules.length
        );
        rules.set(key2, name);
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

  // src/lib/analytics.tsx
  var states = /* @__PURE__ */ new WeakMap();
  var COST_INFO = "M9 9a3 3 0 1 1 4 2.8c-.7.3-1 .9-1 1.7V14M12 18h.01";
  function Info({ text: text2 }) {
    return /* @__PURE__ */ jsx("span", { class: "cost-info", tabIndex: 0, role: "img", "aria-label": text2, "data-tip": text2, children: /* @__PURE__ */ jsx(Glyph, { path: COST_INFO, className: "" }) });
  }
  function Row({ row, open, widthClass }) {
    const content2 = /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("span", { class: "session-name", children: screenText(row.name) }),
      /* @__PURE__ */ jsxs("span", { class: "hlabel", children: [
        row.mark && /* @__PURE__ */ jsx(Harness, { mark: row.mark, lead: false }),
        /* @__PURE__ */ jsx("span", { class: "hname h-" + row.harness, "data-tip": row.harnessTip, children: row.harnessName })
      ] }),
      /* @__PURE__ */ jsx("span", { class: "session-value", children: screenText(row.value) }),
      row.rank !== void 0 && /* @__PURE__ */ jsx("span", { class: "row-track", children: /* @__PURE__ */ jsx("i", { class: "row-bar " + widthClass }) }),
      row.missing && /* @__PURE__ */ jsx("span", { class: "no-price", children: screenText(row.missing) })
    ] });
    const cls = (row.className ?? "analytics-session") + (row.rank === void 0 ? "" : " ranked" + (/^[\w-]+$/.test(row.harness) ? " h-" + row.harness : ""));
    return row.href ? row.rank !== void 0 ? /* @__PURE__ */ jsx(
      "a",
      {
        class: cls,
        href: row.href,
        onClick: (event) => {
          if (event.defaultPrevented || event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || !event.currentTarget.isConnected)
            return;
          event.preventDefault();
          open();
        },
        children: content2
      }
    ) : /* @__PURE__ */ jsx("button", { class: cls, type: "button", onClick: open, children: content2 }) : /* @__PURE__ */ jsx("div", { class: cls, children: content2 });
  }
  function renderAnalyticsScreen(root, snapshot, host2) {
    let layout = states.get(root);
    if (!layout) {
      layout = createMeasuredLayout();
      states.set(root, layout);
      claimScreen(root, "analytics", () => {
        layout?.destroy();
        states.delete(root);
      });
    }
    layout.reset();
    const geometry = layout;
    const width = (value) => geometry.className("width", value, "%");
    const facets = host2.facets();
    const active = (action) => {
      if (root.isConnected) action();
    };
    commitApplicationView(
      root,
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsxs("div", { class: "ph", children: [
          /* @__PURE__ */ jsx("h1", { children: "Analytics" }),
          /* @__PURE__ */ jsx("div", { class: "sub", children: screenText(snapshot.heading) })
        ] }),
        /* @__PURE__ */ jsx(
          "div",
          {
            class: "session-facet-slot",
            ref: (node) => {
              if (node && facets.parentElement !== node) node.append(facets);
            }
          }
        ),
        snapshot.error && /* @__PURE__ */ jsx("p", { class: "empty", role: "status", children: screenText(snapshot.error) }),
        snapshot.ready && /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx("div", { class: "analytics-metrics", "data-analytics-ready": "", "data-query": snapshot.query, children: snapshot.metrics.map((metric) => /* @__PURE__ */ jsxs(
            "div",
            {
              class: "analytics-metric",
              "data-more": metric.explanation ? "" : void 0,
              "data-tip": metric.explanation,
              tabIndex: metric.explanation ? 0 : void 0,
              "aria-describedby": metric.explanation ? metric.id : void 0,
              children: [
                metric.explanation && /* @__PURE__ */ jsx("span", { hidden: true, id: metric.id, children: metric.explanation }),
                /* @__PURE__ */ jsxs("div", { class: "label", children: [
                  /* @__PURE__ */ jsx("span", { children: metric.label }),
                  metric.info && /* @__PURE__ */ jsx(Info, { text: metric.info })
                ] }),
                /* @__PURE__ */ jsx("div", { class: "value", children: screenText(metric.value) }),
                /* @__PURE__ */ jsxs("div", { class: "note", children: [
                  metric.note.lead ? /* @__PURE__ */ jsx("span", { class: metric.note.tone, children: screenText(metric.note.lead) }) : null,
                  screenText(metric.note.text),
                  metric.note.tail
                ] })
              ]
            },
            metric.id
          )) }),
          snapshot.charts.map((chart) => /* @__PURE__ */ jsxs("section", { class: "analytics-panel", children: [
            /* @__PURE__ */ jsxs("h2", { children: [
              chart.heading,
              chart.info && /* @__PURE__ */ jsx(Info, { text: chart.info })
            ] }),
            chart.empty ? /* @__PURE__ */ jsx("p", { class: "empty", children: chart.empty }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
              chart.sub && /* @__PURE__ */ jsx("div", { class: "panel-sub", children: screenText(chart.sub) }),
              /* @__PURE__ */ jsx("div", { class: "analytics-chart", children: /* @__PURE__ */ jsxs(
                "svg",
                {
                  viewBox: "0 0 " + chart.width + " 190",
                  role: chart.role,
                  "aria-label": chart.label,
                  children: [
                    chart.grid.map((line, i) => /* @__PURE__ */ jsxs(Fragment2, { children: [
                      /* @__PURE__ */ jsx(
                        "line",
                        {
                          x1: chart.left,
                          x2: chart.right,
                          y1: line.y,
                          y2: line.y,
                          class: "gridline"
                        },
                        "line" + i
                      ),
                      /* @__PURE__ */ jsx("text", { x: 0, y: line.y + 4, class: "axis-label", children: line.label }, "label" + i)
                    ] })),
                    chart.bins.map((bin) => /* @__PURE__ */ jsxs(Fragment2, { children: [
                      bin.claude > 0 && /* @__PURE__ */ jsx(
                        "rect",
                        {
                          x: bin.barX,
                          y: 151 - bin.claude,
                          width: bin.barWidth,
                          height: bin.claude,
                          class: "cost-claude"
                        },
                        "claude" + bin.key
                      ),
                      bin.codex > 0 && /* @__PURE__ */ jsx(
                        "rect",
                        {
                          x: bin.barX,
                          y: 151 - bin.claude - bin.codex,
                          width: bin.barWidth,
                          height: bin.codex,
                          class: "cost-codex"
                        },
                        "codex" + bin.key
                      ),
                      /* @__PURE__ */ jsx(
                        "rect",
                        {
                          x: bin.x,
                          y: 12,
                          width: bin.width,
                          height: 139,
                          class: "chart-hit",
                          "data-tip": bin.tip,
                          role: bin.active ? "button" : void 0,
                          tabIndex: bin.active ? 0 : void 0,
                          "aria-label": bin.active ? bin.label : void 0,
                          onClick: () => {
                            if (bin.active) active(() => host2.slice(bin.key));
                          },
                          onKeyDown: (event) => {
                            if (event.key === "Enter" || event.key === " ") {
                              event.preventDefault();
                              if (bin.active) active(() => host2.slice(bin.key));
                            }
                          }
                        },
                        "hit" + bin.key
                      )
                    ] })),
                    /* @__PURE__ */ jsx("text", { x: chart.left, y: 178, class: "axis-label", children: chart.ago }),
                    /* @__PURE__ */ jsx("text", { x: chart.right, y: 178, "text-anchor": "end", class: "axis-label", children: "Now" })
                  ]
                }
              ) }),
              /* @__PURE__ */ jsx("div", { class: "analytics-legend", children: chart.legend.map((item2) => /* @__PURE__ */ jsxs("span", { children: [
                /* @__PURE__ */ jsx("i", { class: "legend-" + item2.id }),
                item2.mark && /* @__PURE__ */ jsx(Harness, { mark: item2.mark, lead: false }),
                item2.label
              ] }, item2.id)) }),
              chart.bins.some((bin) => bin.active) && /* @__PURE__ */ jsxs("details", { class: "chart-slices", children: [
                /* @__PURE__ */ jsx("summary", { children: "Explore time slices" }),
                /* @__PURE__ */ jsx("div", { class: "chart-slice-list", children: chart.bins.filter((bin) => bin.active).map((bin) => /* @__PURE__ */ jsxs(
                  "button",
                  {
                    class: "chart-slice",
                    type: "button",
                    onClick: () => active(() => host2.slice(bin.key)),
                    children: [
                      /* @__PURE__ */ jsx("span", { children: bin.when }),
                      /* @__PURE__ */ jsx("span", { children: bin.value })
                    ]
                  },
                  bin.key
                )) })
              ] }),
              chart.missing && /* @__PURE__ */ jsx("div", { class: "no-price", children: screenText(chart.missing) })
            ] })
          ] }, chart.key)),
          /* @__PURE__ */ jsxs("div", { class: "analytics-breakdown", children: [
            /* @__PURE__ */ jsxs("div", { class: "analytics-bd-head", children: [
              /* @__PURE__ */ jsx("h2", { children: "Breakdown" }),
              /* @__PURE__ */ jsx("div", { class: "panel-sub", children: "Agent-hours and API-equivalent cost; bars follow the toggle" })
            ] }),
            /* @__PURE__ */ jsx("div", { class: "analytics-bd-bar", children: /* @__PURE__ */ jsx("div", { class: "analytics-measure", role: "group", "aria-label": "Breakdown bar measure", children: [
              ["hours", "Agent-hours"],
              ["cost", "API-equivalent cost"]
            ].map(([key2, label]) => /* @__PURE__ */ jsx(
              "button",
              {
                type: "button",
                "data-measure": key2,
                "aria-pressed": snapshot.measure === key2,
                onClick: () => active(() => host2.measure(key2)),
                children: label
              },
              key2
            )) }) }),
            /* @__PURE__ */ jsx("div", { class: "analytics-breakdowns", children: snapshot.breakdowns.map((group) => /* @__PURE__ */ jsxs("section", { class: "analytics-panel", children: [
              /* @__PURE__ */ jsx("h3", { children: group.heading }),
              /* @__PURE__ */ jsxs("div", { class: "analytics-list", children: [
                group.rows.map((row) => /* @__PURE__ */ jsxs(
                  "button",
                  {
                    class: "analytics-row",
                    type: "button",
                    "data-breakdown": group.key,
                    "data-key": row.key,
                    onClick: () => active(() => host2.breakdown(group.key, row.key)),
                    children: [
                      /* @__PURE__ */ jsx("span", { class: "row-title", children: screenText(row.name) }),
                      /* @__PURE__ */ jsx("span", { class: "row-count", children: row.count }),
                      /* @__PURE__ */ jsx("span", { class: "row-track", children: /* @__PURE__ */ jsx(
                        "i",
                        {
                          class: "row-bar " + width(row.width) + (row.color ? " bar-" + row.color : "")
                        }
                      ) }),
                      /* @__PURE__ */ jsx("span", { class: "row-hours" + (snapshot.measure === "hours" ? " on" : ""), children: row.hours }),
                      /* @__PURE__ */ jsx("span", { class: "row-cost" + (snapshot.measure === "cost" ? " on" : ""), children: row.cost }),
                      row.missing && /* @__PURE__ */ jsx("span", { class: "no-price", children: screenText(row.missing) })
                    ]
                  },
                  row.key
                )),
                !group.rows.length && /* @__PURE__ */ jsx("p", { class: "empty", children: "No activity in this range." })
              ] })
            ] }, group.key)) })
          ] }),
          /* @__PURE__ */ jsx("div", { class: "analytics-split", children: snapshot.lists.map((list2) => /* @__PURE__ */ jsxs("section", { class: "analytics-panel", children: [
            /* @__PURE__ */ jsx("h2", { children: screenText(list2.heading) }),
            /* @__PURE__ */ jsxs("div", { class: "analytics-list", children: [
              list2.rows.map((row) => /* @__PURE__ */ jsx(
                Row,
                {
                  row,
                  widthClass: width(row.rank ?? 0),
                  open: () => active(() => host2.session(row.id))
                },
                row.id
              )),
              !list2.rows.length && /* @__PURE__ */ jsx("p", { class: "empty", children: "No sessions in this range." })
            ] })
          ] }, list2.heading)) }),
          snapshot.models && /* @__PURE__ */ jsxs("section", { class: "analytics-panel models-panel", children: [
            /* @__PURE__ */ jsx("h2", { children: "Models" }),
            /* @__PURE__ */ jsx("p", { class: "panel-sub", children: "Your launched work, compared within each difficulty band. Every measure shows its own sample count; unknown values are not estimated." }),
            !snapshot.models.length && /* @__PURE__ */ jsx("p", { class: "empty", children: "No launched work with recorded per-message models in this range." }),
            snapshot.models.map((band) => /* @__PURE__ */ jsxs(Fragment2, { children: [
              /* @__PURE__ */ jsx("h3", { children: band.heading }, "heading" + band.key),
              /* @__PURE__ */ jsx(
                "div",
                {
                  class: "model-table-scroll",
                  tabIndex: 0,
                  role: "region",
                  "aria-label": band.key + " model comparison",
                  children: /* @__PURE__ */ jsxs("table", { class: "model-table", children: [
                    /* @__PURE__ */ jsx("thead", { children: /* @__PURE__ */ jsx("tr", { children: snapshot.modelHeaders.map((heading) => /* @__PURE__ */ jsx("th", { scope: "col", children: screenText(heading) }, heading)) }) }),
                    /* @__PURE__ */ jsx("tbody", { children: band.rows.map((row) => /* @__PURE__ */ jsxs("tr", { children: [
                      /* @__PURE__ */ jsx("td", { children: /* @__PURE__ */ jsx(
                        "button",
                        {
                          class: "model-comparison-open",
                          type: "button",
                          "data-tip": row.tip,
                          onClick: () => active(() => host2.model(row.key)),
                          children: screenText(row.name)
                        }
                      ) }),
                      /* @__PURE__ */ jsx("td", { children: screenText(row.count) }),
                      row.cells.map((cell, i) => /* @__PURE__ */ jsxs("td", { "data-tip": cell.tip, children: [
                        /* @__PURE__ */ jsx("span", { children: cell.text }),
                        /* @__PURE__ */ jsx("span", { class: "model-n", children: "n=" + cell.n })
                      ] }, i))
                    ] }, row.key)) })
                  ] })
                },
                "table" + band.key
              ),
              band.points.length ? /* @__PURE__ */ jsx("div", { class: "analytics-chart", children: /* @__PURE__ */ jsxs(
                "svg",
                {
                  viewBox: "0 0 " + band.width + " 190",
                  role: "img",
                  "aria-label": "API cost against first-pass acceptance for " + band.key + " work",
                  children: [
                    /* @__PURE__ */ jsx("line", { x1: 42, x2: band.width - 12, y1: 150, y2: 150, class: "gridline" }),
                    /* @__PURE__ */ jsx("line", { x1: 42, x2: 42, y1: 12, y2: 150, class: "gridline" }),
                    band.points.map((point, i) => /* @__PURE__ */ jsxs(Fragment2, { children: [
                      /* @__PURE__ */ jsx(
                        "circle",
                        {
                          cx: point.x,
                          cy: point.y,
                          r: 5,
                          class: "model-point",
                          "data-tip": point.tip,
                          "aria-label": point.tip
                        },
                        "point" + i
                      ),
                      /* @__PURE__ */ jsx(
                        "text",
                        {
                          x: Math.max(42, Math.min(band.width - 90, point.x + 8)),
                          y: Math.max(20, point.y - 8),
                          class: "axis-label",
                          children: point.label
                        },
                        "point-label" + i
                      )
                    ] })),
                    /* @__PURE__ */ jsx("text", { x: 42, y: 178, class: "axis-label", children: "Median API cost \u2192" }),
                    /* @__PURE__ */ jsx("text", { x: 2, y: 16, class: "axis-label", children: "100%" }),
                    /* @__PURE__ */ jsx("text", { x: 8, y: 151, class: "axis-label", children: "0%" })
                  ]
                }
              ) }, "plot" + band.key) : /* @__PURE__ */ jsx("p", { class: "panel-sub", children: "Cost / acceptance plot needs both recorded measures." }, "empty" + band.key)
            ] }))
          ] }),
          snapshot.allowance && /* @__PURE__ */ jsxs("section", { class: "analytics-panel", children: [
            /* @__PURE__ */ jsx("h2", { children: "Codex allowance" }),
            /* @__PURE__ */ jsx("div", { class: "panel-sub", children: screenText("Latest recorded rate limits \xB7 " + snapshot.allowance.when) }),
            /* @__PURE__ */ jsx("div", { class: "allowance-grid", children: snapshot.allowance.windows.map((window2) => /* @__PURE__ */ jsxs("div", { class: "allowance-window", children: [
              /* @__PURE__ */ jsx("div", { class: "window-name", children: window2.label }),
              /* @__PURE__ */ jsx("div", { class: "window-used", children: window2.used }),
              /* @__PURE__ */ jsx("div", { class: "window-reset", children: window2.reset })
            ] }, window2.label)) })
          ] })
        ] })
      ] })
    );
    host2.committed();
  }
  function renderSliceBody(root, rows, more, empty, open) {
    commitApplicationView(
      root,
      /* @__PURE__ */ jsx(Fragment2, { children: !rows.length ? /* @__PURE__ */ jsx("p", { class: "empty", children: empty }) : /* @__PURE__ */ jsxs("div", { class: "analytics-list", children: [
        rows.map((row) => /* @__PURE__ */ jsx(
          Row,
          {
            row,
            widthClass: "",
            open: () => {
              if (root.isConnected) open(row.id);
            }
          },
          row.id
        )),
        more > 0 && /* @__PURE__ */ jsx("p", { class: "empty", children: "and " + more + " more" })
      ] }) })
    );
  }
  function renderModelItems(root, count2, items, more, host2) {
    commitApplicationView(
      root,
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx("p", { class: "panel-sub", children: screenText(count2) }),
        items.map((item2, i) => /* @__PURE__ */ jsxs("div", { class: "model-item", children: [
          item2.name ? /* @__PURE__ */ jsx(
            "button",
            {
              class: "model-item-session",
              type: "button",
              onClick: (event) => {
                if (event.currentTarget.isConnected) host2.session(item2.id);
              },
              children: screenText(item2.name)
            }
          ) : /* @__PURE__ */ jsx("span", { children: screenText(item2.id + " \xB7 outside the session window") }),
          /* @__PURE__ */ jsx("span", { class: "panel-sub", children: screenText(item2.description) }),
          item2.trace && /* @__PURE__ */ jsx(
            "button",
            {
              class: "model-item-trace",
              type: "button",
              onClick: (event) => {
                if (event.currentTarget.isConnected) host2.trace(item2.trace);
              },
              children: "Trace"
            }
          ),
          item2.url && /* @__PURE__ */ jsx("a", { href: item2.url, target: "_blank", rel: "noopener noreferrer", children: "Open pull request" })
        ] }, i)),
        more && /* @__PURE__ */ jsx("p", { class: "panel-sub", children: more })
      ] })
    );
  }

  // src/lib/sentence.tsx
  function Sentence({ parts, host: host2 }) {
    return /* @__PURE__ */ jsx(Fragment2, { children: parts.map(
      (part, i) => part.action ? /* @__PURE__ */ jsx(
        "button",
        {
          class: part.className.split(/\s+/).includes("who") ? part.className + " who-link" : part.className,
          type: "button",
          "aria-label": part.label,
          "data-tip": part.tip,
          onClick: (event) => {
            event.stopPropagation();
            if (!event.currentTarget.isConnected || !part.action) return;
            if (part.action.kind === "session") host2.session(part.action.id, part.action.turn);
            else if (part.action.kind === "machine") host2.machine(part.action.id);
            else host2.sender(part.action.id);
          },
          children: screenText(part.text)
        },
        i
      ) : /* @__PURE__ */ jsx(
        "span",
        {
          class: part.className.split(/\s+/).filter((name) => name !== "who-link").join(" "),
          "data-tip": part.tip,
          children: screenText(part.text)
        },
        i
      )
    ) });
  }

  // src/lib/trace.tsx
  var states2 = /* @__PURE__ */ new WeakMap();
  function renderTraceScreen(root, snapshot, host2) {
    let state2 = states2.get(root);
    if (!state2) {
      state2 = {
        layout: createMeasuredLayout(),
        frame: 0,
        disposed: false,
        briefs: /* @__PURE__ */ new Map(),
        open: /* @__PURE__ */ new Set(),
        clipped: /* @__PURE__ */ new Set(),
        chart: null,
        axis: null,
        edges: null,
        paths: [],
        hot: null,
        paint() {
        },
        measure() {
        },
        observer: new ResizeObserver(() => state2?.measure())
      };
      states2.set(root, state2);
      const owner2 = state2;
      claimScreen(root, "trace", () => {
        owner2.disposed = true;
        owner2.observer.disconnect();
        cancelAnimationFrame(owner2.frame);
        owner2.layout.destroy();
        if (owner2.edges) render(null, owner2.edges);
        states2.delete(root);
      });
      document.fonts.ready.then(() => {
        if (!owner2.disposed) {
          cancelAnimationFrame(owner2.frame);
          owner2.frame = requestAnimationFrame(() => owner2.measure());
        }
      });
    }
    const owner = state2;
    const active = (action) => {
      if (!owner.disposed && root.isConnected) action();
    };
    function paintEdges() {
      if (!owner.edges || owner.disposed) return;
      render(
        /* @__PURE__ */ jsx(Fragment2, { children: owner.paths.map((edge, i) => /* @__PURE__ */ jsx(
          "path",
          {
            "data-parent": edge.parent,
            "data-child": edge.child,
            class: edge.className + (owner.hot === edge.parent || owner.hot === edge.child ? " hot" : ""),
            d: edge.path
          },
          i
        )) }),
        owner.edges
      );
    }
    function hot(id) {
      owner.hot = id;
      paintEdges();
    }
    owner.measure = () => {
      if (owner.disposed || !root.isConnected) return;
      let changed = false;
      for (const [key2, pair] of owner.briefs) {
        if (owner.open.has(key2) || !pair.node.clientHeight) continue;
        const clipped = pair.node.scrollHeight > pair.node.clientHeight + 1;
        if (owner.clipped.has(key2) !== clipped) {
          changed = true;
          if (clipped) owner.clipped.add(key2);
          else owner.clipped.delete(key2);
        }
      }
      if (changed) {
        owner.paint();
        return;
      }
      if (owner.axis) {
        const bounds = owner.axis.getBoundingClientRect();
        let previousRight = bounds.left - 8;
        for (const label of owner.axis.querySelectorAll(".agent-axis-tick > span")) {
          label.hidden = false;
          const rect2 = label.getBoundingClientRect();
          if (rect2.left < previousRight + 8 || rect2.right > bounds.right) label.hidden = true;
          else previousRight = rect2.right;
        }
      }
      if (!owner.chart || !owner.edges) return;
      const chart = owner.chart, rect = chart.getBoundingClientRect();
      owner.edges.setAttribute("viewBox", "0 0 " + rect.width + " " + rect.height);
      owner.paths = [];
      if (window.matchMedia("(min-width: 761px)").matches && rect.width && rect.height) {
        const rows = new Map(
          [...chart.querySelectorAll("[data-agent-id]")].map((row) => [
            row.dataset.agentId,
            row
          ])
        );
        for (const row of snapshot.agents?.rows ?? []) {
          if (!row.edge) continue;
          const parent = rows.get(row.edge.parent), child = rows.get(row.id);
          if (!parent || !child) continue;
          const parentTrack = parent.querySelector(".agent-track"), childTrack = child.querySelector(".agent-track");
          if (!parentTrack || !childTrack) continue;
          const pr = parent.getBoundingClientRect(), cr = child.getBoundingClientRect(), ptr = parentTrack.getBoundingClientRect(), ctr = childTrack.getBoundingClientRect();
          const yp = pr.top - rect.top + pr.height / 2, yc = cr.top - rect.top + cr.height / 2, at = ptr.left - rect.left + ptr.width * row.edge.started / 100, done = ctr.left - rect.left + ctr.width * row.edge.done / 100;
          const cls = "agent-edge" + (row.critical ? " critical" : "");
          const add = (path, className = cls) => owner.paths.push({ parent: row.edge.parent, child: row.id, path, className });
          add("M " + at + " " + yp + " V " + yc);
          add("M " + at + " " + (yp - 4) + " V " + (yp + 4));
          if (row.edge.returned) {
            add("M " + done + " " + yc + " V " + (yp + 4));
            add(
              "M " + (done - 3) + " " + (yp + 4) + " L " + done + " " + yp + " L " + (done + 3) + " " + (yp + 4),
              "agent-edge-arrow" + (row.critical ? " critical" : "")
            );
          }
        }
      }
      paintEdges();
    };
    owner.paint = () => {
      if (owner.disposed) return;
      owner.observer.disconnect();
      owner.briefs.clear();
      owner.layout.reset();
      const left = (value) => owner.layout.className("left", value, "%"), width = (value) => owner.layout.className("width", value, "%"), indent = (depth) => owner.layout.className("width", Math.min(3, depth) * 12, "px");
      const agents = snapshot.agents;
      commitApplicationView(
        root,
        /* @__PURE__ */ jsxs(Fragment2, { children: [
          snapshot.summary && /* @__PURE__ */ jsx("p", { class: "trace-summary", children: screenText(snapshot.summary) }),
          /* @__PURE__ */ jsx("div", { class: "ph sr", children: /* @__PURE__ */ jsx("h1", { children: "Trace" }) }),
          snapshot.empty ? /* @__PURE__ */ jsx("p", { class: "empty", children: "This turn isn't in the logs on this machine." }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
            agents && /* @__PURE__ */ jsxs("section", { class: "agents-panel", "aria-label": "Agents", children: [
              /* @__PURE__ */ jsx("h2", { class: "agents-title", children: "Agents" }),
              /* @__PURE__ */ jsx("div", { class: "agents-summary", children: agents.summary.map((item2, i) => /* @__PURE__ */ jsx("span", { children: screenText(item2) }, i)) }),
              /* @__PURE__ */ jsx("div", { class: "agents-axis", children: /* @__PURE__ */ jsxs(
                "div",
                {
                  class: "agent-axis-track",
                  ref: (node) => {
                    owner.axis = node;
                  },
                  children: [
                    /* @__PURE__ */ jsx("span", { class: "agent-axis-time start", children: agents.start }),
                    /* @__PURE__ */ jsx("span", { class: "agent-axis-time end", children: agents.end }),
                    agents.ticks.map((tick, i) => /* @__PURE__ */ jsx(
                      "span",
                      {
                        class: "agent-axis-tick" + (tick.nearEnd ? " near-end" : "") + " " + left(tick.left),
                        children: /* @__PURE__ */ jsx("span", { children: tick.label })
                      },
                      i
                    ))
                  ]
                }
              ) }),
              /* @__PURE__ */ jsxs("div", { class: "agents-legend", children: [
                /* @__PURE__ */ jsx("span", { class: "agents-legend-swatch" }),
                /* @__PURE__ */ jsx("span", { children: agents.legend }),
                agents.incomplete && /* @__PURE__ */ jsx("span", { children: screenText("\xB7 Wait history incomplete") })
              ] }),
              /* @__PURE__ */ jsxs(
                "div",
                {
                  class: "agents-chart",
                  ref: (node) => {
                    owner.chart = node;
                  },
                  children: [
                    agents.rows.map(
                      (row) => row.kind === "heading" ? /* @__PURE__ */ jsx("div", { class: "agent-relayed-heading", children: "Relayed to" }, row.id) : row.kind === "more" ? /* @__PURE__ */ jsxs(
                        "button",
                        {
                          class: "agent-row agent-more",
                          type: "button",
                          "data-depth": Math.min(3, row.depth),
                          "aria-label": row.label,
                          onClick: () => active(() => host2.fold(row.fold ?? row.id)),
                          children: [
                            /* @__PURE__ */ jsx("span", { class: "agent-label", children: /* @__PURE__ */ jsxs("span", { class: "agent-identity", children: [
                              /* @__PURE__ */ jsx("span", { class: "agent-indent " + indent(row.depth) }),
                              /* @__PURE__ */ jsx("span", { class: "agent-name", children: row.name })
                            ] }) }),
                            /* @__PURE__ */ jsx("span", { class: "agent-duration" }),
                            /* @__PURE__ */ jsx("span", { class: "agent-cost", "data-tip": row.costTip, children: row.cost }),
                            /* @__PURE__ */ jsx("span", { class: "agent-track" })
                          ]
                        },
                        row.id
                      ) : /* @__PURE__ */ jsxs(
                        "button",
                        {
                          class: "agent-row",
                          type: "button",
                          "data-agent-id": row.id,
                          "data-depth": Math.min(3, row.depth),
                          "data-critical": String(!!row.critical),
                          "aria-label": row.label,
                          onClick: () => active(() => host2.session(row.id, row.turn)),
                          onPointerEnter: () => hot(row.id),
                          onPointerLeave: () => hot(null),
                          onFocus: () => hot(row.id),
                          onBlur: () => hot(null),
                          children: [
                            /* @__PURE__ */ jsxs("span", { class: "agent-label", children: [
                              /* @__PURE__ */ jsxs("span", { class: "agent-identity", children: [
                                /* @__PURE__ */ jsx("span", { class: "agent-indent " + indent(row.depth) }),
                                /* @__PURE__ */ jsx(
                                  "span",
                                  {
                                    class: "agent-harness-dot " + row.harnessClass,
                                    "aria-hidden": "true"
                                  }
                                ),
                                row.relay && /* @__PURE__ */ jsx(Glyph, { path: "M4 7h13l-3-3M20 17H7l3 3", className: "agent-relay-icon" }),
                                /* @__PURE__ */ jsx("span", { class: "agent-name", children: screenText(row.name) })
                              ] }),
                              /* @__PURE__ */ jsxs("span", { class: "agent-meta", children: [
                                /* @__PURE__ */ jsx("span", { class: "agent-model", children: screenText(row.model ?? "") }),
                                /* @__PURE__ */ jsx(
                                  "span",
                                  {
                                    class: "agent-status stat " + row.state,
                                    "data-tip": row.stateLabel,
                                    children: /* @__PURE__ */ jsx(
                                      "span",
                                      {
                                        class: "dot " + row.state,
                                        role: "img",
                                        "aria-label": row.stateLabel,
                                        "data-tip": row.stateLabel
                                      }
                                    )
                                  }
                                ),
                                /* @__PURE__ */ jsx(
                                  "span",
                                  {
                                    class: "agent-tokens",
                                    "data-tip": "Session totals, including this row's spawned agents",
                                    children: row.tokens
                                  }
                                )
                              ] })
                            ] }),
                            /* @__PURE__ */ jsx("span", { class: "agent-duration", children: row.duration }),
                            /* @__PURE__ */ jsx("span", { class: "agent-cost", "data-tip": row.costTip, children: row.cost }),
                            /* @__PURE__ */ jsxs("span", { class: "agent-track", "aria-hidden": "true", children: [
                              agents.ticks.map((tick, i) => /* @__PURE__ */ jsx(
                                "span",
                                {
                                  class: "agent-gridline " + left(tick.left),
                                  "aria-hidden": "true"
                                },
                                "grid" + i
                              )),
                              row.segments?.map((segment, i) => /* @__PURE__ */ jsx(
                                "span",
                                {
                                  class: "agent-segment " + segment.kind + " " + left(segment.left) + " " + width(segment.width)
                                },
                                "segment" + i
                              )),
                              row.spawns?.map((position, i) => /* @__PURE__ */ jsx(
                                "span",
                                {
                                  class: "agent-spawn-tick " + left(position),
                                  "aria-hidden": "true"
                                },
                                "spawn" + i
                              )),
                              row.waits?.map((wait, i) => /* @__PURE__ */ jsx(
                                "span",
                                {
                                  class: "agent-wait " + left(wait.left) + " " + width(wait.width),
                                  "data-tip": wait.tip
                                },
                                "wait" + i
                              ))
                            ] })
                          ]
                        },
                        row.id
                      )
                    ),
                    /* @__PURE__ */ jsx(
                      "svg",
                      {
                        class: "agent-edges",
                        "aria-hidden": "true",
                        preserveAspectRatio: "none",
                        ref: (node) => {
                          owner.edges = node;
                        }
                      }
                    )
                  ]
                }
              )
            ] }),
            /* @__PURE__ */ jsx("div", { class: "flow", children: snapshot.hops.map((hop) => /* @__PURE__ */ jsxs(
              "div",
              {
                class: "hop " + hop.className,
                "data-h": hop.handoff,
                "data-turn": hop.turn,
                children: [
                  /* @__PURE__ */ jsx("div", { class: "node " + hop.nodeClass, children: /* @__PURE__ */ jsx(Glyph, { path: hop.icon, className: "" }) }),
                  /* @__PURE__ */ jsxs("div", { class: "body", children: [
                    /* @__PURE__ */ jsxs("div", { class: "sent", children: [
                      /* @__PURE__ */ jsx(Sentence, { parts: hop.parts, host: host2 }),
                      hop.time && /* @__PURE__ */ jsx("span", { class: "tm", children: hop.time })
                    ] }),
                    hop.brief && /* @__PURE__ */ jsxs(Fragment2, { children: [
                      /* @__PURE__ */ jsx(
                        "div",
                        {
                          class: "brief md" + (owner.open.has(hop.key) ? " open" : owner.clipped.has(hop.key) ? " clipped" : ""),
                          ref: (node) => {
                            if (node) {
                              const more = owner.briefs.get(hop.key)?.more;
                              if (more) owner.briefs.set(hop.key, { node, more });
                            }
                          },
                          children: /* @__PURE__ */ jsx(MarkdownContent, { text: hop.brief })
                        }
                      ),
                      /* @__PURE__ */ jsx(
                        "button",
                        {
                          class: "more",
                          type: "button",
                          hidden: !owner.clipped.has(hop.key) && !owner.open.has(hop.key),
                          "aria-expanded": owner.open.has(hop.key),
                          ref: (node) => {
                            const brief = node?.previousElementSibling;
                            if (node && brief instanceof HTMLElement)
                              owner.briefs.set(hop.key, { node: brief, more: node });
                          },
                          onClick: () => active(() => {
                            if (owner.open.has(hop.key)) owner.open.delete(hop.key);
                            else owner.open.add(hop.key);
                            owner.paint();
                          }),
                          children: owner.open.has(hop.key) ? "Show less" : "Show more"
                        }
                      )
                    ] }),
                    hop.answers && /* @__PURE__ */ jsx("div", { class: "result answer" + (hop.answers.length ? "" : " none"), children: !hop.answers.length ? /* @__PURE__ */ jsxs(Fragment2, { children: [
                      /* @__PURE__ */ jsx("b", { children: "Answered" }),
                      " \xB7 reply not in these logs"
                    ] }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
                      /* @__PURE__ */ jsx("b", { children: hop.answers.length === 1 ? "You answered: " : "You answered:" }),
                      hop.answers.length === 1 ? hop.answers[0] : /* @__PURE__ */ jsx("ol", { children: hop.answers.map((answer, i) => /* @__PURE__ */ jsx("li", { children: screenText(answer) }, i)) })
                    ] }) }),
                    hop.result && /* @__PURE__ */ jsxs("div", { class: "result", children: [
                      /* @__PURE__ */ jsx("span", { class: "rl", children: "Result:" }),
                      /* @__PURE__ */ jsx("span", { children: /* @__PURE__ */ jsx(Inline, { text: hop.result }) })
                    ] }),
                    hop.meta && /* @__PURE__ */ jsxs("div", { class: "meta", children: [
                      /* @__PURE__ */ jsxs("span", { class: "stat " + hop.meta.state, children: [
                        hop.meta.state === "work" ? /* @__PURE__ */ jsx("span", { class: "spin" }) : /* @__PURE__ */ jsx(
                          "span",
                          {
                            class: "dot " + hop.meta.state,
                            role: "img",
                            "aria-label": hop.meta.stateLabel
                          }
                        ),
                        hop.meta.text
                      ] }),
                      hop.meta.chip && /* @__PURE__ */ jsxs("span", { class: "chip-h " + hop.meta.chipClass, "data-tip": hop.meta.tip, children: [
                        hop.meta.harness && /* @__PURE__ */ jsx(Harness, { mark: hop.meta.harness, lead: false }),
                        screenText(hop.meta.chip)
                      ] }),
                      hop.meta.note && /* @__PURE__ */ jsx("span", { class: "gone", children: screenText(hop.meta.note) }),
                      hop.meta.session && /* @__PURE__ */ jsx(
                        "button",
                        {
                          class: "open",
                          type: "button",
                          onClick: () => active(() => host2.session(hop.meta.session, hop.meta.turn)),
                          children: screenText("Open in " + hop.meta.name + " \u203A")
                        }
                      )
                    ] })
                  ] })
                ]
              },
              hop.key
            )) })
          ] }),
          snapshot.observation && /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: screenText(snapshot.observation) }),
          !!snapshot.continuations?.length && /* @__PURE__ */ jsxs("p", { class: "catalog-note", children: [
            "Continue with another recorded branch:",
            " ",
            snapshot.continuations.map((branch) => /* @__PURE__ */ jsxs("a", { class: "link", href: safePath(branch.href) ? branch.href : void 0, children: [
              screenText(branch.label),
              " "
            ] }))
          ] })
        ] })
      );
      if (owner.chart) owner.observer.observe(owner.chart);
      for (const pair of owner.briefs.values()) owner.observer.observe(pair.node);
      cancelAnimationFrame(owner.frame);
      owner.frame = requestAnimationFrame(owner.measure);
    };
    owner.paint();
    host2.committed();
  }

  // src/lib/control.ts
  function record2(value) {
    return !!value && typeof value === "object" && !Array.isArray(value);
  }
  function parseControl(value) {
    if (value == null) return null;
    if (!record2(value) || typeof value.thread !== "string" || typeof value.generation !== "string" || !(value.activeTurn === null || typeof value.activeTurn === "string") || typeof value.connected !== "boolean" || !(value.reason === null || typeof value.reason === "string") || !record2(value.capabilities) || !["input", "steer", "interrupt", "commandApproval", "fileApproval", "questions"].every(
      (key2) => record2(value.capabilities) && typeof value.capabilities[key2] === "boolean"
    ) || !Array.isArray(value.requests) || value.requests.length > 1280 || !record2(value.actions))
      throw new Error("Invalid control snapshot");
    if (value.runtime !== void 0 && (!record2(value.runtime) || typeof value.runtime.state !== "string" || !["active", "disconnected", "failed", "ended", "unavailable"].includes(
      String(value.runtime.state)
    ) || typeof value.runtime.phase !== "string" || value.runtime.phase.length > 64 || typeof value.runtime.freshness !== "string" || !["current", "updating", "stale", "unavailable"].includes(value.runtime.freshness) || typeof value.runtime.reconnectable !== "boolean"))
      throw new Error("Invalid runtime status");
    if (record2(value.runtime) && (value.runtime.presence !== void 0 && (typeof value.runtime.presence !== "string" || !["active", "absent", "paused", "transitioning", "failed", "unknown"].includes(
      String(value.runtime.presence)
    )) || value.runtime.observedAt !== void 0 && value.runtime.observedAt !== null && (typeof value.runtime.observedAt !== "string" || value.runtime.observedAt.length > 64 || !Number.isFinite(Date.parse(value.runtime.observedAt))) || value.runtime.observationError !== void 0 && value.runtime.observationError !== null && (typeof value.runtime.observationError !== "string" || value.runtime.observationError.length > 512) || value.runtime.updating !== void 0 && typeof value.runtime.updating !== "boolean" || value.runtime.state !== "active" && (value.runtime.reconnectable || value.connected || ["input", "steer", "interrupt", "commandApproval", "fileApproval", "questions"].some(
      (key2) => record2(value.capabilities) && value.capabilities[key2] === true
    ))))
      throw new Error("Invalid runtime authority");
    for (const request of value.requests) {
      if (!record2(request) || typeof request.id !== "string" || !/^[0-9a-f]{32}$/.test(request.id) || !["permission", "question"].includes(String(request.kind)) || !record2(request.payload) || !(request.hash === null || typeof request.hash === "string" && /^[0-9a-f]{64}$/.test(request.hash)) || !record2(request.state) || typeof request.state.state !== "string" || !(request.reason === null || typeof request.reason === "string") || typeof request.remainingMs !== "number" || !Number.isFinite(request.remainingMs) || request.remainingMs < 0)
        throw new Error("Invalid control request");
    }
    return value;
  }
  function unavailableControl(thread, reason = "Native session controls are unavailable.") {
    return {
      snapshot: {
        thread,
        generation: "",
        activeTurn: null,
        connected: false,
        capabilities: {
          input: false,
          steer: false,
          interrupt: false,
          commandApproval: false,
          fileApproval: false,
          questions: false
        },
        reason,
        requests: [],
        actions: {}
      },
      busy: false,
      uncertain: false,
      note: "",
      canReconnect: false,
      send: async () => false,
      interrupt() {
      },
      answer() {
      },
      reconnect() {
      }
    };
  }

  // src/lib/transcript.tsx
  var owners = /* @__PURE__ */ new WeakMap();
  var SessionControls = class extends Component {
    componentDidMount() {
      this.props.owner.controls = () => this.forceUpdate();
    }
    componentWillUnmount() {
      this.props.owner.controls = null;
    }
    render() {
      const snapshot = this.props.owner.snapshot;
      let view = snapshot.control ?? unavailableControl(snapshot.id, "Native session controls are unavailable.");
      const runtime = snapshot.runtime?.observation;
      if (runtime?.freshness === "current" && ["ended", "failed"].includes(runtime.state)) {
        const blocked = unavailableControl(
          snapshot.id,
          runtime.state === "ended" ? "This session has ended." : runtime.state === "failed" ? "This environment needs attention." : "Current environment controls are unavailable."
        );
        const retained = view;
        view = {
          ...view,
          get draft() {
            return retained.draft;
          },
          canReconnect: false,
          snapshot: blocked.snapshot,
          send: blocked.send,
          interrupt: blocked.interrupt,
          answer: blocked.answer,
          reconnect: blocked.reconnect
        };
      }
      return /* @__PURE__ */ jsx(LocalControl, { view, runtimeObserved: !!snapshot.runtime });
    }
  };
  function paintComposer(owner) {
    if (owner.disposed) return;
    const pending = owner.snapshot.control?.snapshot.requests.filter(
      (request) => request.state.state === "open" || request.state.state === "claimed"
    ).length ?? 0;
    render(
      /* @__PURE__ */ jsxs(
        "button",
        {
          type: "button",
          class: "btn sh-quiet session-composer-trigger",
          "aria-label": pending ? "Composer; " + pending + " pending requests" : "Open Composer",
          "aria-haspopup": "dialog",
          "aria-expanded": !!owner.composerPanel,
          onClick: (event) => {
            if (owner.disposed || !event.currentTarget.isConnected || owner.composerPanel) return;
            const trigger = event.currentTarget;
            const panel = createPanelChrome(
              {
                title: "Composer",
                label: "Session Composer",
                className: "anchored session-composer-panel"
              },
              {
                opened() {
                  owner.host.composerOpened?.(panel);
                  paintComposer(owner);
                  owner.composer.querySelector("textarea")?.focus({ preventScroll: true });
                },
                closed() {
                  owner.host.composerClosed?.(panel);
                  owner.composer.remove();
                  owner.composerPanel = null;
                  if (!owner.disposed) {
                    paintComposer(owner);
                    if (trigger.isConnected) trigger.focus({ preventScroll: true });
                  }
                }
              }
            );
            owner.composerPanel = panel;
            panel.body.append(owner.composer);
            panel.show();
          },
          children: [
            "Composer",
            pending ? " \xB7 " + pending : ""
          ]
        }
      ),
      owner.composerTrigger
    );
  }
  var SessionRuntime = class extends Component {
    componentDidMount() {
      this.props.owner.runtime = () => this.forceUpdate();
    }
    componentWillUnmount() {
      this.props.owner.runtime = null;
    }
    render() {
      const value = this.props.owner.snapshot.runtime;
      if (!value) return null;
      const observation = value.observation;
      return /* @__PURE__ */ jsxs("section", { class: "local-control", "aria-label": "Environment status", children: [
        /* @__PURE__ */ jsx("p", { role: "status", children: observation ? {
          active: "Environment is active.",
          disconnected: "Environment is disconnected.",
          failed: "Environment needs attention.",
          ended: "This session has ended.",
          unavailable: "Runtime state is unavailable."
        }[observation.state] : "Runtime state is unavailable." }),
        observation?.phase && /* @__PURE__ */ jsxs("p", { children: [
          "Phase: ",
          screenText(observation.phase)
        ] }),
        observation?.presence && /* @__PURE__ */ jsxs("p", { children: [
          "Compute presence: ",
          screenText(observation.presence)
        ] }),
        (value.delivery !== "current" || observation?.freshness !== "current") && /* @__PURE__ */ jsx("p", { role: "status", children: value.delivery === "updating" || observation?.freshness === "updating" ? "Checking the environment\u2026" : value.delivery === "stale" || observation?.freshness === "stale" ? "Last environment observation is stale." : "Current environment observation is unavailable." }),
        value.reason && /* @__PURE__ */ jsx("p", { children: screenText(value.reason) }),
        observation?.observationError && /* @__PURE__ */ jsx("p", { children: screenText(observation.observationError) })
      ] });
    }
  };
  var SessionPager = class extends Component {
    componentDidMount() {
      this.props.owner.pagers[this.props.where] = () => this.forceUpdate();
    }
    componentWillUnmount() {
      delete this.props.owner.pagers[this.props.where];
    }
    render() {
      const { owner, where } = this.props, view = owner.snapshot[where];
      if (!view) {
        const started = where === "before" ? owner.snapshot.started : void 0;
        return started ? /* @__PURE__ */ jsx("div", { class: "divider started", children: /* @__PURE__ */ jsxs("span", { class: "dv-text", children: [
          /* @__PURE__ */ jsx("span", { class: "dv-lead", children: started.lead }),
          /* @__PURE__ */ jsx("span", { class: "dv-machine", "data-tip": started.machine, "data-tip-clipped": "", children: screenText(started.machine) })
        ] }) }) : null;
      }
      return /* @__PURE__ */ jsx("div", { class: "list", children: /* @__PURE__ */ jsxs(
        "button",
        {
          class: "more",
          type: "button",
          "data-load-earlier": where === "before" ? "" : void 0,
          "data-pager-sid": view.sid,
          "data-pager-where": where,
          disabled: view.disabled,
          "aria-busy": view.busy || void 0,
          onClick: (event) => {
            if (event.currentTarget.isConnected && !owner.disposed)
              owner.host.pager(event.currentTarget);
          },
          children: [
            view.busy && /* @__PURE__ */ jsx("span", { class: "spin", "aria-hidden": "true" }),
            /* @__PURE__ */ jsx("span", { class: "pager-label", "aria-live": "polite", children: screenText(view.text) })
          ]
        }
      ) });
    }
  };
  var identity = (entry2) => entry2.entryKey ?? entry2.key ?? "";
  function State({ state: state2, label, text: text2 }) {
    return /* @__PURE__ */ jsxs("span", { class: "state " + state2, children: [
      state2 === "work" ? /* @__PURE__ */ jsx("span", { class: "spin" }) : /* @__PURE__ */ jsx("span", { class: "dot " + state2, role: "img", "aria-label": label, "data-tip": label }),
      /* @__PURE__ */ jsx("span", { children: screenText(text2) })
    ] });
  }
  function Answer({ values }) {
    return /* @__PURE__ */ jsx(Fragment2, { children: !values.length ? /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("b", { children: "Answered" }),
      " \xB7 reply not in these logs"
    ] }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("b", { children: values.length === 1 ? "You answered: " : "You answered:" }),
      values.length === 1 ? values[0] : /* @__PURE__ */ jsx("ol", { children: values.map((value, i) => /* @__PURE__ */ jsx("li", { children: screenText(value) }, i)) })
    ] }) });
  }
  function Entry({ entry: entry2, owner }) {
    const key2 = identity(entry2), open = owner.open.has(key2), active = (action) => {
      if (!owner.disposed) action();
    };
    switch (entry2.kind) {
      case "message":
        return /* @__PURE__ */ jsxs(
          "div",
          {
            class: entry2.flavor === "incoming" ? "bubble in" : "msg " + entry2.flavor,
            "data-e": entry2.key,
            "data-entry-key": entry2.entryKey,
            "data-h": entry2.handoff,
            children: [
              entry2.images?.length ? /* @__PURE__ */ jsx("div", { class: "attach-row", children: entry2.images.map(
                (image2, i) => image2.unavailable || owner.failedImages.has(key2 + ":" + image2.url) ? /* @__PURE__ */ jsx("span", { class: "attach-na", children: "Image not available" }, i) : /* @__PURE__ */ jsx(
                  "button",
                  {
                    class: "attach",
                    type: "button",
                    "aria-haspopup": "dialog",
                    onClick: (event) => {
                      if (event.currentTarget.isConnected)
                        active(() => owner.host.image(image2.url, image2.label, event.currentTarget));
                    },
                    children: /* @__PURE__ */ jsx(
                      "img",
                      {
                        class: "attach-img" + (image2.width ? "" : " unsized"),
                        alt: image2.label,
                        loading: "lazy",
                        decoding: "async",
                        width: image2.width,
                        height: image2.height,
                        src: image2.url,
                        onError: () => active(() => {
                          owner.failedImages.add(key2 + ":" + image2.url);
                          owner.change(key2);
                        })
                      }
                    )
                  },
                  i
                )
              ) }) : null,
              (!entry2.images?.length || entry2.text) && /* @__PURE__ */ jsx(Markdown, { text: entry2.text })
            ]
          }
        );
      case "thought":
        return entry2.mode === "pending" ? /* @__PURE__ */ jsxs("div", { class: "think-pending", "data-e": entry2.key, "data-entry-key": entry2.entryKey, children: [
          /* @__PURE__ */ jsx("span", { class: "spin" }),
          /* @__PURE__ */ jsx("span", { children: "Thinking\u2026" })
        ] }) : /* @__PURE__ */ jsx(
          "div",
          {
            class: "thought" + (entry2.mode === "masked" ? " masked" : ""),
            "data-e": entry2.key,
            "data-entry-key": entry2.entryKey,
            children: entry2.mode === "readable" ? /* @__PURE__ */ jsxs(Fragment2, { children: [
              /* @__PURE__ */ jsxs(
                "button",
                {
                  type: "button",
                  class: "think-label think-disclosure",
                  "aria-expanded": open,
                  onClick: (event) => {
                    if (!event.currentTarget.isConnected || owner.disposed) return;
                    if (open) owner.open.delete(key2);
                    else owner.open.add(key2);
                    owner.change(key2);
                  },
                  children: [
                    /* @__PURE__ */ jsx(Glyph, { path: "m9 5 7 7-7 7", className: "chev" + (open ? " open" : "") }),
                    screenText(entry2.label ?? "Thinking")
                  ]
                }
              ),
              open && /* @__PURE__ */ jsx(Markdown, { text: entry2.text ?? "", className: "think-text" })
            ] }) : /* @__PURE__ */ jsx("div", { class: "think-label", children: screenText(entry2.label ?? "Thinking hidden by the harness") })
          }
        );
      case "label":
        return /* @__PURE__ */ jsxs("div", { class: entry2.className, "data-e": entry2.key, "data-entry-key": entry2.entryKey, children: [
          screenText(entry2.text),
          entry2.action && /* @__PURE__ */ jsx(
            "button",
            {
              type: "button",
              class: "link",
              disabled: entry2.action.busy,
              onClick: (event) => {
                if (event.currentTarget.isConnected) active(() => entry2.action?.run());
              },
              children: screenText(entry2.action.label)
            }
          )
        ] });
      case "tool":
        return /* @__PURE__ */ jsx(
          "div",
          {
            class: entry2.step.className,
            "data-e": entry2.key,
            "data-entry-key": entry2.entryKey,
            "data-tid": entry2.step.tid,
            "data-live": entry2.step.sid,
            "data-since": entry2.step.since,
            children: /* @__PURE__ */ jsx(
              ToolStepContents,
              {
                snapshot: entry2.step,
                expanded: open,
                mounted: owner.mounted.has(key2),
                toggle: () => active(() => {
                  owner.mounted.add(key2);
                  if (open) owner.open.delete(key2);
                  else owner.open.add(key2);
                  owner.change(key2);
                }),
                host: {
                  all(label) {
                    owner.host.toolAll(key2, label);
                  },
                  script() {
                    owner.host.script(key2);
                  }
                }
              }
            )
          }
        );
      case "background": {
        const content2 = /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Glyph, { path: "M4 17l5-5-5-5M12 19h8", className: "" }),
          /* @__PURE__ */ jsx("span", { class: "bg-label", children: screenText(entry2.label) })
        ] });
        return /* @__PURE__ */ jsx(
          "div",
          {
            class: "step bgend" + (entry2.failed ? " err" : ""),
            "data-e": entry2.key,
            "data-entry-key": entry2.entryKey,
            children: entry2.loaded ? /* @__PURE__ */ jsx(
              "button",
              {
                class: "bg-line",
                type: "button",
                onClick: (event) => {
                  if (event.currentTarget.isConnected)
                    owner.host.background(entry2.call, event.currentTarget);
                },
                children: content2
              }
            ) : /* @__PURE__ */ jsx("div", { class: "bg-line", children: content2 })
          }
        );
      }
      case "group": {
        const steps = /* @__PURE__ */ jsx(
          "div",
          {
            class: "steps" + (entry2.lone ? " lone" : ""),
            hidden: entry2.summary.length > 0 && !open,
            children: entry2.entries.map((item2, i) => /* @__PURE__ */ jsx(Entry, { entry: item2, owner }, identity(item2) || i))
          }
        );
        return !entry2.summary.length ? steps : /* @__PURE__ */ jsxs("div", { class: "tgroup", "data-e": entry2.key, "data-entry-key": entry2.entryKey, children: [
          /* @__PURE__ */ jsxs(
            "button",
            {
              class: "tsum",
              type: "button",
              "aria-expanded": open,
              onClick: (event) => {
                if (!event.currentTarget.isConnected) return;
                if (open) owner.open.delete(key2);
                else owner.open.add(key2);
                owner.change(key2);
              },
              children: [
                entry2.running ? /* @__PURE__ */ jsx("span", { class: "spin" }) : /* @__PURE__ */ jsx(Glyph, { path: entry2.stack, className: "" }),
                /* @__PURE__ */ jsx("span", { class: "tt" + (entry2.background ? " bgsum" : ""), children: entry2.summary.map(
                  (part, i) => part.className ? /* @__PURE__ */ jsx("span", { class: part.className, children: screenText(part.text) }, i) : part.text
                ) }),
                entry2.failed > 0 && /* @__PURE__ */ jsx("span", { class: "tf", children: screenText("\xB7 " + entry2.failed + " failed") }),
                entry2.running && /* @__PURE__ */ jsx("span", { class: "tl tick", children: entry2.clockTick ? "\xB7 running " + entry2.running : screenText("\xB7 running " + entry2.running) }),
                /* @__PURE__ */ jsx(Glyph, { path: entry2.chevron, className: "chev" })
              ]
            }
          ),
          steps
        ] });
      }
      case "child":
        return /* @__PURE__ */ jsxs(
          "div",
          {
            class: "child-card",
            "data-e": entry2.key,
            "data-entry-key": entry2.entryKey,
            "data-h": entry2.handoff,
            children: [
              /* @__PURE__ */ jsxs("div", { class: "cc-head", children: [
                /* @__PURE__ */ jsxs("span", { class: "cc-identity", children: [
                  /* @__PURE__ */ jsx(
                    "button",
                    {
                      class: "cc-name who-link",
                      type: "button",
                      "aria-label": "Open " + entry2.name,
                      onClick: (event) => {
                        if (event.currentTarget.isConnected) owner.host.session(entry2.id, entry2.turn);
                      },
                      children: screenText(entry2.name)
                    }
                  ),
                  /* @__PURE__ */ jsxs("span", { class: "cc-meta", children: [
                    entry2.mark && /* @__PURE__ */ jsx(Harness, { mark: entry2.mark }),
                    screenText(entry2.meta)
                  ] })
                ] }),
                /* @__PURE__ */ jsx(State, { state: entry2.state, label: entry2.stateLabel, text: entry2.stateLabel }),
                /* @__PURE__ */ jsx(
                  "button",
                  {
                    class: "ibtn cc-toggle",
                    type: "button",
                    "aria-label": (open ? "Collapse " : "Expand ") + entry2.name + " details",
                    "aria-expanded": open,
                    onClick: (event) => {
                      if (!event.currentTarget.isConnected || owner.disposed) return;
                      if (open) owner.open.delete(key2);
                      else owner.open.add(key2);
                      owner.change(key2);
                    },
                    children: /* @__PURE__ */ jsx(Glyph, { path: entry2.chevron, className: "chev" })
                  }
                )
              ] }),
              /* @__PURE__ */ jsxs("div", { class: "cc-details", hidden: !open, children: [
                /* @__PURE__ */ jsxs("span", { class: "cc-context", children: [
                  /* @__PURE__ */ jsx("span", { class: "cc-context-name", children: screenText(entry2.name) }),
                  /* @__PURE__ */ jsx("span", { class: "cc-context-meta", children: screenText(entry2.meta) })
                ] }),
                /* @__PURE__ */ jsx("span", { class: "cc-brief", children: /* @__PURE__ */ jsx(Inline, { text: entry2.brief }) }),
                entry2.result ? /* @__PURE__ */ jsxs("span", { class: "cc-result" + (entry2.failed ? " err" : ""), children: [
                  /* @__PURE__ */ jsx("span", { class: "rl", children: entry2.failed ? "Result: " : "Returned: " }),
                  /* @__PURE__ */ jsx(Inline, { text: entry2.result })
                ] }) : entry2.activity && /* @__PURE__ */ jsxs("span", { class: "cc-now", children: [
                  /* @__PURE__ */ jsx("span", { class: "spin" }),
                  /* @__PURE__ */ jsx("span", { children: screenText(entry2.activity[0]) }),
                  /* @__PURE__ */ jsx("code", { children: entry2.activity[1] })
                ] }),
                entry2.trace && /* @__PURE__ */ jsx(
                  "button",
                  {
                    class: "link cc-run",
                    type: "button",
                    "aria-label": entry2.traceLabel,
                    onClick: (event) => {
                      if (event.currentTarget.isConnected) owner.host.trace(entry2.trace);
                    },
                    children: "Run view"
                  }
                )
              ] })
            ]
          }
        );
      case "event":
        return /* @__PURE__ */ jsxs(
          "div",
          {
            class: entry2.className,
            "data-e": entry2.key,
            "data-entry-key": entry2.entryKey,
            "data-h": entry2.handoff,
            children: [
              /* @__PURE__ */ jsxs("div", { class: "ev-head", children: [
                /* @__PURE__ */ jsx(Glyph, { path: entry2.icon, className: "" }),
                /* @__PURE__ */ jsx("span", { class: "ln", children: /* @__PURE__ */ jsx(Sentence, { parts: entry2.parts, host: owner.host }) }),
                /* @__PURE__ */ jsx("span", { class: "tm", children: entry2.time })
              ] }),
              /* @__PURE__ */ jsx(
                "div",
                {
                  class: "ev-text" + (open ? " open" : owner.clipped.has(key2) ? " clipped" : ""),
                  ref: (node) => {
                    if (node) owner.clamps.set(key2, node);
                  },
                  children: /* @__PURE__ */ jsx(Inline, { text: entry2.brief })
                }
              ),
              /* @__PURE__ */ jsx(
                "button",
                {
                  class: "link ev-more",
                  type: "button",
                  hidden: !owner.clipped.has(key2) && !open,
                  "aria-expanded": open,
                  onClick: (event) => {
                    event.stopPropagation();
                    if (!event.currentTarget.isConnected) return;
                    if (open) owner.open.delete(key2);
                    else owner.open.add(key2);
                    owner.change(key2);
                  },
                  children: open ? "Show less" : "Show more"
                }
              ),
              entry2.result && /* @__PURE__ */ jsxs("div", { class: "ev-result", children: [
                /* @__PURE__ */ jsx("span", { class: "rl", children: entry2.resultLabel }),
                /* @__PURE__ */ jsx(Inline, { text: entry2.result })
              ] }),
              entry2.answers && /* @__PURE__ */ jsx("div", { class: "ev-result answer" + (entry2.answers.length ? "" : " none"), children: /* @__PURE__ */ jsx(Answer, { values: entry2.answers }) }),
              entry2.waiting && /* @__PURE__ */ jsx(State, { state: "wait", label: entry2.stateLabel, text: "Waiting on you" })
            ]
          }
        );
    }
  }
  var Turn = class extends Component {
    shouldComponentUpdate(next) {
      return next.view !== this.props.view || next.revision !== this.props.revision;
    }
    render() {
      const { view, owner } = this.props;
      return /* @__PURE__ */ jsxs("section", { class: "turn", "data-turn": view.id, "aria-label": view.label, children: [
        view.header && /* @__PURE__ */ jsxs("div", { class: "turn-h", children: [
          view.header.mark && /* @__PURE__ */ jsx(Harness, { mark: view.header.mark, size: 16, lead: false }),
          /* @__PURE__ */ jsx(Sentence, { parts: view.header.parts, host: owner.host }),
          /* @__PURE__ */ jsx("span", { class: "tm", children: view.header.time })
        ] }),
        !view.bare && /* @__PURE__ */ jsx("div", { class: "tx", children: view.entries.map((entry2, i) => /* @__PURE__ */ jsx(Entry, { entry: entry2, owner }, identity(entry2) || i)) }),
        (view.error || view.trace) && /* @__PURE__ */ jsxs("div", { class: "turn-end", children: [
          view.error && /* @__PURE__ */ jsx(State, { state: "err", label: view.stateLabel, text: view.error }),
          view.trace && /* @__PURE__ */ jsxs(
            "button",
            {
              class: "link",
              type: "button",
              "aria-label": "Trace what this turn set off",
              onClick: (event) => {
                if (event.currentTarget.isConnected) owner.host.trace(view.trace);
              },
              children: [
                /* @__PURE__ */ jsx(Glyph, { path: view.traceIcon, className: "" }),
                /* @__PURE__ */ jsx("span", { children: "Trace" })
              ]
            }
          )
        ] })
      ] });
    }
  };
  function mergeBlocks(previous, snapshot) {
    if (!snapshot.dirty) return snapshot.blocks;
    const blocks2 = [...previous], changed = new Map(
      snapshot.blocks.filter(
        (block) => block.kind === "turn"
      ).map((block) => [block.turn.id, block])
    );
    for (const id of snapshot.order) {
      if (!snapshot.dirty.has(id)) continue;
      const at = blocks2.findIndex((block) => block.kind === "turn" && block.turn.id === id), next = changed.get(id);
      if (at >= 0) {
        if (next) blocks2[at] = next;
        else blocks2.splice(at, 1);
      } else if (next) {
        const later = snapshot.order.slice(snapshot.order.indexOf(id) + 1), index = blocks2.findIndex((block) => block.kind === "turn" && later.includes(block.turn.id));
        if (index >= 0) blocks2.splice(index, 0, next);
        else blocks2.push(next);
      }
    }
    return blocks2.filter((block) => block.kind !== "turn" || snapshot.order.includes(block.turn.id));
  }
  function paintJump(owner) {
    if (!owner.jump || owner.disposed) return;
    owner.jump.hidden = !owner.jumpVisible;
    render(
      /* @__PURE__ */ jsxs(
        "button",
        {
          class: "jump",
          type: "button",
          id: "jump-bottom",
          "aria-label": owner.jumpCount ? "Jump to bottom; " + owner.jumpCount + " new entries" : "Jump to bottom of transcript",
          "data-tip": owner.jumpCount ? owner.jumpCount + " new entries \xB7 Jump to latest" : "Jump to latest",
          "data-count": owner.jumpCount > 0 ? owner.jumpCount > 99 ? "99+" : String(owner.jumpCount) : void 0,
          disabled: owner.jumpBusy,
          onClick: (event) => {
            if (event.currentTarget.isConnected) owner.host.jump();
          },
          children: [
            owner.jumpCount > 0 && /* @__PURE__ */ jsx("span", { class: "new-count sr-only", children: owner.jumpCount + " new" }),
            /* @__PURE__ */ jsx(Glyph, { path: "M12 4v15M5 12l7 7 7-7", className: "" })
          ]
        }
      ),
      owner.jump
    );
  }
  function updateSessionJump(root, visible2, count2, busy2 = false, target) {
    const owner = owners.get(root);
    if (!owner) return;
    if (target && owner.jump !== target) {
      if (owner.jump) render(null, owner.jump);
      owner.jump = target;
    }
    const slot = root.querySelector(".session-jump-slot");
    if (owner.jump && slot && owner.jump.parentNode !== slot) slot.append(owner.jump);
    owner.jumpVisible = visible2;
    owner.jumpCount = count2;
    owner.jumpBusy = busy2;
    paintJump(owner);
  }
  function renderSessionScreen(root, snapshot, host2) {
    let owner = owners.get(root);
    if (owner && owner.snapshot.id !== snapshot.id) {
      releaseScreen(root);
      owner = void 0;
    }
    if (!owner) {
      const state3 = {
        composer: document.createElement("div"),
        composerTrigger: document.createElement("span"),
        composerPanel: null,
        snapshot,
        host: host2,
        disposed: false,
        blocks: snapshot.blocks,
        open: /* @__PURE__ */ new Set(),
        mounted: /* @__PURE__ */ new Set(),
        failedImages: /* @__PURE__ */ new Set(),
        revisions: /* @__PURE__ */ new Map(),
        clipped: /* @__PURE__ */ new Set(),
        clamps: /* @__PURE__ */ new Map(),
        frame: 0,
        jump: null,
        jumpVisible: false,
        jumpCount: 0,
        jumpBusy: false,
        paint() {
        },
        controls: null,
        runtime: null,
        pagers: {},
        measure() {
        },
        change() {
        },
        observer: new ResizeObserver(() => {
        })
      };
      owner = state3;
      owners.set(root, state3);
      render(/* @__PURE__ */ jsx(SessionControls, { owner: state3 }), state3.composer);
      state3.measure = () => {
        if (state3.disposed) return;
        let changed = false;
        for (const [key2, node] of state3.clamps) {
          if (!node.isConnected || state3.open.has(key2) || !node.clientHeight) continue;
          const clipped = node.scrollHeight > node.clientHeight + 1;
          if (state3.clipped.has(key2) !== clipped) {
            if (clipped) state3.clipped.add(key2);
            else state3.clipped.delete(key2);
            changed = true;
            for (const block of state3.blocks)
              if (block.kind === "turn" && block.turn.entries.some((entry2) => identity(entry2) === key2))
                state3.revisions.set(block.turn.id, (state3.revisions.get(block.turn.id) ?? 0) + 1);
          }
        }
        if (changed) state3.paint();
      };
      state3.observer.disconnect();
      state3.observer = new ResizeObserver(state3.measure);
      const viewport = window.visualViewport;
      const fitJump = () => {
        const dock = root.querySelector(".session-dock");
        if (state3.disposed || !dock || !root.isConnected) return;
        const note = document.querySelector(
          "body > .livenote, body > .viewer-status-slot > .livenote"
        );
        setGeometry(
          dock,
          "readingBottom",
          Math.max(
            viewport ? Math.max(0, innerHeight - viewport.height - viewport.offsetTop) : 0,
            note ? Math.max(0, innerHeight - note.getBoundingClientRect().top) : 0
          )
        );
      };
      const notices = new MutationObserver(fitJump);
      notices.observe(document.body, { childList: true });
      viewport?.addEventListener("resize", fitJump);
      viewport?.addEventListener("scroll", fitJump);
      requestAnimationFrame(fitJump);
      claimScreen(root, "session", () => {
        notices.disconnect();
        viewport?.removeEventListener("resize", fitJump);
        viewport?.removeEventListener("scroll", fitJump);
        state3.disposed = true;
        state3.composerPanel?.destroy();
        render(null, state3.composer);
        render(null, state3.composerTrigger);
        state3.composerTrigger.remove();
        releaseGeometry(root);
        state3.disposed = true;
        state3.observer.disconnect();
        cancelAnimationFrame(state3.frame);
        if (state3.jump) {
          render(null, state3.jump);
          state3.jump.hidden = true;
        }
        owners.delete(root);
      });
    }
    const state2 = owner;
    state2.host = host2;
    state2.blocks = mergeBlocks(state2.blocks, snapshot);
    state2.snapshot = snapshot;
    if (snapshot.composerTarget && state2.composerTrigger.parentElement !== snapshot.composerTarget)
      snapshot.composerTarget.replaceChildren(state2.composerTrigger);
    state2.controls?.();
    paintComposer(state2);
    state2.change = (key2) => {
      for (const block of state2.blocks)
        if (block.kind === "turn" && block.turn.entries.some(
          (entry2) => identity(entry2) === key2 || entry2.kind === "group" && entry2.entries.some((item2) => identity(item2) === key2)
        ))
          state2.revisions.set(block.turn.id, (state2.revisions.get(block.turn.id) ?? 0) + 1);
      state2.paint();
    };
    state2.paint = () => {
      if (state2.disposed) return;
      const held = document.activeElement instanceof HTMLElement && document.activeElement.closest(".session-foot") ? document.activeElement : null, heldKind = held?.dataset.foot;
      const view = state2.snapshot;
      commitApplicationView(
        root,
        /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx("div", { class: "ph sr", children: /* @__PURE__ */ jsx("h1", { children: screenText(view.name) }) }),
          /* @__PURE__ */ jsxs("section", { class: "transcript", "aria-label": "Transcript", children: [
            view.observation && /* @__PURE__ */ jsx("p", { class: "empty", role: "status", "data-transcript-observation": "", children: screenText(view.observation) }),
            /* @__PURE__ */ jsxs("div", { class: "turns", children: [
              /* @__PURE__ */ jsx(SessionPager, { owner: state2, where: "before" }),
              state2.blocks.map(
                (block) => block.kind === "turn" ? /* @__PURE__ */ jsx(
                  Turn,
                  {
                    view: block.turn,
                    owner: state2,
                    revision: state2.revisions.get(block.turn.id) ?? 0
                  },
                  block.turn.id
                ) : /* @__PURE__ */ jsx(Fragment, { children: block.entries.map((entry2, i) => /* @__PURE__ */ jsx(Entry, { entry: entry2, owner: state2 }, identity(entry2) || block.key + i)) }, block.key)
              ),
              /* @__PURE__ */ jsx(SessionPager, { owner: state2, where: "after" }),
              view.empty && /* @__PURE__ */ jsxs("p", { class: "empty", children: [
                screenText(view.empty),
                " ",
                view.retry && /* @__PURE__ */ jsx(
                  "button",
                  {
                    type: "button",
                    class: "link",
                    onClick: () => {
                      if (!state2.disposed) state2.snapshot.retry?.run();
                    },
                    children: view.retry.label
                  }
                )
              ] })
            ] })
          ] }),
          /* @__PURE__ */ jsx(SessionRuntime, { owner: state2 }),
          /* @__PURE__ */ jsx("div", { class: "session-dock", children: /* @__PURE__ */ jsx(
            "div",
            {
              class: "session-jump-slot",
              ref: (node) => {
                if (node && state2.jump && state2.jump.parentNode !== node) node.append(state2.jump);
              }
            }
          ) }),
          view.footer && /* @__PURE__ */ jsxs("div", { class: "session-foot", children: [
            /* @__PURE__ */ jsxs("span", { class: "stat " + view.footer.state, children: [
              view.footer.state === "work" ? /* @__PURE__ */ jsx("span", { class: "spin" }) : /* @__PURE__ */ jsx(
                "span",
                {
                  class: "dot " + view.footer.state,
                  role: "img",
                  "aria-label": view.footer.stateLabel
                }
              ),
              /* @__PURE__ */ jsxs("span", { children: [
                screenText(view.footer.text),
                view.footer.items.map((item2) => /* @__PURE__ */ jsxs(Fragment, { children: [
                  screenText(" \xB7 "),
                  /* @__PURE__ */ jsx(
                    "span",
                    {
                      class: "cr-item",
                      "data-foot": item2.kind,
                      "data-tip": item2.tip || void 0,
                      tabIndex: item2.tip ? 0 : void 0,
                      children: screenText(item2.text)
                    },
                    item2.kind
                  )
                ] }, item2.kind))
              ] })
            ] }),
            view.footer.parent && /* @__PURE__ */ jsx(
              "button",
              {
                type: "button",
                "data-foot": "open",
                onClick: (event) => {
                  if (event.currentTarget.isConnected)
                    state2.host.session(view.footer.parent.id, view.footer.parent.turn);
                },
                children: screenText("Open in " + view.footer.parent.name)
              }
            )
          ] })
        ] })
      );
      if (heldKind && held && !held.isConnected)
        (root.querySelector('[data-foot="' + CSS.escape(heldKind) + '"]') ?? root.querySelector('[data-foot="time"]'))?.focus({ preventScroll: true });
      for (const [key2, node] of state2.clamps) {
        if (node.isConnected) state2.observer.observe(node);
        else {
          state2.observer.unobserve(node);
          state2.clamps.delete(key2);
        }
      }
    };
    state2.paint();
    host2.committed();
  }
  function updateSessionControl(root, control) {
    const owner = owners.get(root);
    if (!owner || owner.disposed) return;
    owner.snapshot = { ...owner.snapshot, control };
    if (owner.snapshot.composerTarget && owner.composerTrigger.parentElement !== owner.snapshot.composerTarget)
      owner.snapshot.composerTarget.replaceChildren(owner.composerTrigger);
    owner.controls?.();
    paintComposer(owner);
  }
  function updateSessionRuntime(root, runtime) {
    const owner = owners.get(root);
    if (!owner || owner.disposed) return;
    owner.snapshot = { ...owner.snapshot, runtime };
    owner.runtime?.();
    owner.controls?.();
  }
  function updateSessionPager(root, view) {
    const owner = owners.get(root);
    if (!owner || owner.snapshot.id !== view.sid) return;
    owner.snapshot = { ...owner.snapshot, [view.where]: view };
    owner.pagers[view.where]?.();
  }
  function updateSessionClock(root, now, starts) {
    const owner = owners.get(root);
    if (!owner || owner.disposed) return;
    const elapsed = (since) => {
      const n = Math.max(0, Math.floor((now - since) / 1e3));
      return n < 60 ? n + "s" : Math.floor(n / 60) + "m " + n % 60 + "s";
    };
    function update(entry2) {
      if (entry2.kind === "tool" && entry2.step.running && entry2.step.sid) {
        const since = entry2.step.since ?? starts[entry2.step.sid];
        if (!Number.isFinite(since)) return entry2;
        const text2 = elapsed(since), step = entry2.step;
        const status = step.background ? step.status : text2, backgroundStatus = step.background ? "running " + text2 : step.backgroundStatus;
        return status === step.status && backgroundStatus === step.backgroundStatus ? entry2 : { ...entry2, step: { ...step, status, backgroundStatus } };
      }
      if (entry2.kind === "group") {
        const entries = entry2.entries.map((item2) => update(item2)), since = entries.filter(
          (item2) => item2.kind === "tool" && item2.step.running && !!item2.step.sid
        ).map((item2) => item2.step.since ?? starts[item2.step.sid]).filter((value) => value !== void 0 && Number.isFinite(value));
        const running = since.length ? elapsed(Math.min(...since)) : entry2.running;
        return running === entry2.running && entries.every((item2, i) => item2 === entry2.entries[i]) ? entry2 : { ...entry2, entries, running, clockTick: true };
      }
      return entry2;
    }
    let changed = false;
    owner.blocks = owner.blocks.map((block) => {
      const previous = block.kind === "turn" ? block.turn.entries : block.entries, entries = previous.map(update);
      if (entries.every((entry2, i) => entry2 === previous[i])) return block;
      changed = true;
      return block.kind === "turn" ? { ...block, turn: { ...block.turn, entries } } : { ...block, entries };
    });
    if (changed) owner.paint();
  }
  function measureSessionScreen(root) {
    const turns = [...root.querySelectorAll(".turn")];
    for (const turn of turns) turn.classList.add("semon-measuring-turn");
    for (const turn of turns) {
      if (turn.isConnected) setGeometry(turn, "intrinsicHeight", turn.getBoundingClientRect().height);
      turn.classList.remove("semon-measuring-turn");
    }
    owners.get(root)?.measure();
  }
  function updateSessionName(root, name) {
    const owner = owners.get(root);
    if (!owner || owner.disposed) return;
    owner.snapshot = { ...owner.snapshot, name };
    const heading = root.querySelector(".ph > h1");
    if (heading) heading.textContent = screenText(name);
  }
  function closeSessionComposer(root) {
    owners.get(root)?.composerPanel?.destroy();
  }

  // src/lib/viewer-bar.tsx
  function createViewerBar() {
    const title = document.createElement("div"), actions = document.createElement("div"), mode = document.createElement("div"), jumpTarget = document.createElement("div");
    jumpTarget.className = "jump-wrap";
    jumpTarget.hidden = true;
    title.className = "ttl";
    actions.className = "viewer-bar-actions";
    mode.className = "viewer-bar-mode";
    let disposed = false, query = "", findRow = null;
    function update(view, host2) {
      if (disposed) throw new Error("Viewer bar is destroyed");
      const active = (node, action) => {
        if (!disposed && node.isConnected) action();
      };
      const icon = (name) => /* @__PURE__ */ jsx(Glyph, { path: view.icons[name], className: "" });
      const state2 = (value, label) => /* @__PURE__ */ jsx("span", { class: "dot " + value, role: "img", "aria-label": label });
      if (query.toLowerCase() !== view.query) query = view.query;
      render(
        view.mode === "normal" ? /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsxs("div", { class: "l1", children: [
            view.crumb && /* @__PURE__ */ jsxs(Fragment2, { children: [
              /* @__PURE__ */ jsx(
                "button",
                {
                  class: "crumb",
                  type: "button",
                  "aria-label": "Back to " + view.crumb,
                  onClick: (event) => active(event.currentTarget, host2.crumb),
                  children: screenText(view.crumb)
                }
              ),
              /* @__PURE__ */ jsx("span", { class: "crumb-sep", children: "\u203A" })
            ] }),
            view.ancestors.map((item2) => /* @__PURE__ */ jsxs("span", { class: "viewer-crumb", children: [
              /* @__PURE__ */ jsx(
                "button",
                {
                  class: "crumb",
                  type: "button",
                  "aria-label": "Open " + item2.name,
                  onClick: (event) => active(event.currentTarget, () => host2.ancestor(item2.id)),
                  children: screenText(item2.name)
                }
              ),
              /* @__PURE__ */ jsx("span", { class: "crumb-sep", children: "\u203A" })
            ] }, item2.id)),
            view.showState && view.state && /* @__PURE__ */ jsx("span", { class: "l1-state", "data-tip": view.stateTip, children: state2(view.state, view.stateLabel ?? view.state) }),
            /* @__PURE__ */ jsx("span", { class: "t", "data-tip": view.name, "data-tip-clipped": "", children: screenText(view.name) })
          ] }),
          view.labels.length > 0 && /* @__PURE__ */ jsx("div", { class: "meta-line", children: view.labels.map((label) => {
            const contents = /* @__PURE__ */ jsxs(Fragment2, { children: [
              label.state && state2(label.state, label.stateLabel ?? label.state),
              label.state ? /* @__PURE__ */ jsx("span", { children: screenText(label.text) }) : screenText(label.text),
              label.effort && /* @__PURE__ */ jsxs("span", { class: "meta-effort", children: [
                /* @__PURE__ */ jsx("span", { class: "meta-sep", children: "\xB7" }),
                label.effort
              ] })
            ] });
            const cls = "lab" + (label.action ? " lab-btn" : "") + (label.className ? " " + label.className : "");
            return label.action ? /* @__PURE__ */ jsx(
              "button",
              {
                class: cls,
                type: "button",
                "data-drop": label.drop,
                "data-tip": label.tip,
                "aria-label": label.tip,
                onClick: (event) => active(
                  event.currentTarget,
                  () => label.action === "errors" ? host2.errors() : host2.menu(
                    document.querySelector("#more-btn") ?? event.currentTarget,
                    true
                  )
                ),
                children: contents
              },
              label.key
            ) : /* @__PURE__ */ jsx("span", { class: cls, "data-drop": label.drop, "data-tip": label.tip, children: contents }, label.key);
          }) })
        ] }) : null,
        title
      );
      render(
        /* @__PURE__ */ jsx(Fragment2, { children: view.mode === "normal" ? view.analytics ? /* @__PURE__ */ jsx("div", { class: "analytics-range", role: "group", "aria-label": "Analytics range", children: [
          [1, "24 h"],
          [7, "7 d"],
          [30, "30 d"]
        ].map(([days, label]) => /* @__PURE__ */ jsx(
          "button",
          {
            type: "button",
            "data-e": "analytics-range:" + days,
            "aria-pressed": view.days === days,
            onClick: (event) => active(event.currentTarget, () => host2.range(Number(days))),
            children: label
          },
          days
        )) }) : view.session ? /* @__PURE__ */ jsxs(Fragment2, { children: [
          !view.trace && /* @__PURE__ */ jsx(
            "button",
            {
              class: "ibtn",
              type: "button",
              id: "find-btn",
              "aria-label": "Find and filter",
              onClick: (event) => active(event.currentTarget, host2.find),
              children: icon("search")
            },
            "find:" + view.session
          ),
          /* @__PURE__ */ jsx(
            "button",
            {
              class: "ibtn",
              type: "button",
              id: "more-btn",
              "aria-label": "Session menu: details, cost and actions",
              "aria-haspopup": "dialog",
              "aria-expanded": "false",
              onClick: (event) => active(event.currentTarget, () => host2.menu(event.currentTarget)),
              children: icon("more")
            },
            "more:" + view.session
          )
        ] }) : null : null }),
        actions
      );
      render(
        view.mode === "find" ? /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsxs(
            "div",
            {
              class: "find-row",
              ref: (node) => {
                findRow = node;
              },
              children: [
                /* @__PURE__ */ jsx(
                  "button",
                  {
                    class: "ibtn",
                    type: "button",
                    "aria-label": "Close find",
                    onClick: (event) => active(event.currentTarget, host2.closeFind),
                    children: icon("back")
                  }
                ),
                /* @__PURE__ */ jsxs("label", { class: "search", children: [
                  icon("search"),
                  /* @__PURE__ */ jsx(
                    "input",
                    {
                      id: "find",
                      type: "search",
                      placeholder: "Find in " + view.name,
                      "aria-label": "Find in transcript",
                      value: query,
                      onInput: (event) => {
                        query = event.currentTarget.value;
                        active(event.currentTarget, () => host2.query(query));
                      },
                      onKeyDown: (event) => {
                        if (event.key === "Escape") {
                          event.stopPropagation();
                          active(event.currentTarget, host2.closeFind);
                        }
                      }
                    }
                  )
                ] }),
                /* @__PURE__ */ jsx("span", { class: "fcount", "aria-live": "polite", children: view.count })
              ]
            }
          ),
          /* @__PURE__ */ jsx("div", { class: "find-chips", role: "group", "aria-label": "Show", children: [
            ["all", "All", void 0],
            ["messages", "Messages", void 0],
            ["steps", "Steps", void 0],
            ...view.failed ? [["failures", "Failed steps", view.failed]] : [],
            ...view.signals ? [["signals", "Signals", view.signals]] : []
          ].map(([key2, label, count2]) => /* @__PURE__ */ jsxs(
            "button",
            {
              class: "chip",
              type: "button",
              "data-filter": key2,
              "aria-pressed": view.filter === key2,
              onClick: (event) => active(event.currentTarget, () => host2.filter(String(key2))),
              children: [
                /* @__PURE__ */ jsx("span", { children: label }),
                count2 !== void 0 && /* @__PURE__ */ jsx("span", { class: "n", children: count2 })
              ]
            },
            key2
          )) })
        ] }) : view.mode === "errors" ? /* @__PURE__ */ jsxs(
          "div",
          {
            class: "errnav-bar",
            role: "group",
            "aria-label": view.errorMode === "signals" ? "Session signals" : "Failed steps",
            children: [
              /* @__PURE__ */ jsx(
                "button",
                {
                  class: "ibtn",
                  type: "button",
                  id: "err-close",
                  "aria-label": "Close " + view.errorMode,
                  onClick: (event) => active(event.currentTarget, host2.closeErrors),
                  children: icon("x")
                }
              ),
              /* @__PURE__ */ jsxs("div", { class: "find errnav", children: [
                /* @__PURE__ */ jsx("span", { class: "errs-dot", "aria-hidden": "true" }),
                /* @__PURE__ */ jsx("span", { class: "errnav-count", role: "status", "aria-live": "polite", children: view.errorText })
              ] }),
              [-1, 1].map((direction) => /* @__PURE__ */ jsx(
                "button",
                {
                  class: "ibtn errnav-btn",
                  type: "button",
                  id: direction < 0 ? "err-prev" : "err-next",
                  "aria-label": (direction < 0 ? "Previous " : "Next ") + (view.errorMode === "signals" ? "signal" : "error"),
                  disabled: view.errorDisabled,
                  onClick: (event) => active(event.currentTarget, () => host2.step(direction)),
                  children: icon(direction < 0 ? "up" : "dn")
                },
                direction
              ))
            ]
          }
        ) : null,
        mode
      );
      return {
        titleSlot: title,
        actions,
        mode,
        accountTarget: view.mode === "find" ? findRow ?? void 0 : void 0
      };
    }
    return {
      update,
      jumpTarget,
      destroy() {
        if (disposed) return;
        disposed = true;
        for (const root of [title, actions, mode]) {
          render(null, root);
          root.remove();
        }
      }
    };
  }
  function measureViewerBar(root) {
    const line = root.querySelector(".meta-line");
    if (!line) return;
    const labels = [...line.querySelectorAll(".lab")];
    for (const label of labels) label.hidden = false;
    const order = labels.filter((label) => !label.classList.contains("state")).sort((a, b) => Number(b.dataset.drop ?? 0) - Number(a.dataset.drop ?? 0));
    for (const label of order) {
      if (line.scrollWidth <= line.clientWidth + 1) break;
      label.hidden = true;
    }
  }

  // shared-select:select
  var createSelect = globalThis.__semonUIShared.createSelect;
  var enhanceSelect = globalThis.__semonUIShared.enhanceSelect;
  var installSelect = globalThis.__semonUIShared.installSelect;

  // src/lib/api.ts
  var ApiError = class extends Error {
    constructor(message, status) {
      super(message);
      this.status = status;
    }
    status;
  };
  async function requestJson(path, signal, unchanged = false, body) {
    let response;
    try {
      response = await fetch(path, {
        credentials: "same-origin",
        signal,
        ...body === void 0 ? {} : {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body)
        }
      });
    } catch (error) {
      if (error instanceof Error && error.name === "AbortError") throw error;
      throw new ApiError(error instanceof Error ? error.message : "No response", 0);
    }
    if (unchanged && response.status === 304) return null;
    if (!response.ok) {
      try {
        await response.body?.cancel();
      } catch {
      }
      throw new ApiError(response.status + " " + response.statusText, response.status);
    }
    return response.json();
  }

  // src/lib/ordering.ts
  function createOrdering() {
    const scopes = /* @__PURE__ */ new Map();
    function begin(name, sig, tie, state2) {
      const previous = scopes.get(name), keep = !!previous && previous.sig === sig && previous.tie === tie;
      const scope = {
        sig,
        tie,
        keep,
        calm: state2.inView && !state2.touched,
        prev: keep ? previous.lists : /* @__PURE__ */ new Map(),
        prevKids: keep ? previous.kids : /* @__PURE__ */ new Set(),
        kids: /* @__PURE__ */ new Set(),
        prevExp: keep ? previous.exp : null,
        exp: null,
        reseed: false,
        lists: /* @__PURE__ */ new Map(),
        n: 0
      };
      scopes.set(name, scope);
      return scope;
    }
    return { scopes, begin };
  }
  function moves(rows, compare) {
    const tails = [];
    for (const row of rows) {
      let low = 0, high = tails.length;
      while (low < high) {
        const mid = low + high >> 1;
        if (compare(tails[mid], row) > 0) high = mid;
        else low = mid + 1;
      }
      tails[low] = row;
    }
    return rows.length - tails.length;
  }
  function orderRows(scope, key2, items, compare, {
    must = null,
    limit = Infinity,
    quiet = false,
    seed = false
  } = {}) {
    const sorted = [...items].sort(compare), by = new Map(items.map((row) => [row.id, row]));
    let ids = sorted.map((row) => row.id);
    if (scope.keep && !quiet && !(seed && !scope.prev.has(key2))) {
      const existed = scope.prev.has(key2), old = (scope.prev.get(key2) ?? []).filter((id) => by.has(id)), have = new Set(old), fresh = sorted.filter((row) => !have.has(row.id)), up = fresh.filter((row) => must?.has(row.id) || scope.calm && existed);
      ids = [...up.map((row) => row.id), ...old];
      const shown = ids.slice(0, limit), inShown = new Set(shown), top = new Set(sorted.slice(0, limit).map((row) => row.id)), held = fresh.filter((row) => top.has(row.id) && !up.includes(row)).length;
      scope.n += held + sorted.slice(0, limit).filter((row) => have.has(row.id) && !inShown.has(row.id)).length + moves(
        shown.map((id) => by.get(id)),
        compare
      );
    }
    scope.lists.set(key2, ids);
    return ids.map((id) => by.get(id));
  }

  // src/lib/navigation.json
  var navigation_default = [
    { key: "home", label: "Home", path: "/", icon: "M4 11l8-7 8 7M6 9.5V20h12V9.5M10 20v-5h4v5" },
    { key: "sessions", label: "Sessions", path: "/sessions", icon: "M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01" },
    { key: "analytics", label: "Analytics", path: "/analytics", icon: "M4 19V5M4 19h17M8 15l3-4 3 2 5-7" },
    { key: "machines", label: "Machines", path: "/machines", icon: "M3 5h18v11H3zM8 20h8M12 16v4" }
  ];

  // src/lib/navigation.ts
  function readShellPreference(key2) {
    try {
      return localStorage.getItem("semon." + key2) === "1";
    } catch {
      return false;
    }
  }
  function writeShellPreference(key2, value) {
    try {
      localStorage.setItem("semon." + key2, value ? "1" : "0");
    } catch {
    }
  }
  function withReturnPath(href, path) {
    if (!safePath(href) || !safePath(path)) return href;
    const url = new URL(href, location.href);
    url.searchParams.set("return_to", path.split("#")[0]);
    return url.pathname + url.search;
  }
  function parseShellNavigation(value) {
    if (!value || typeof value !== "object") return void 0;
    const policy = {};
    if ("paths" in value) {
      if (!value.paths || typeof value.paths !== "object" || Array.isArray(value.paths))
        return void 0;
      const paths = {};
      for (const [key2, path] of Object.entries(value.paths)) {
        if (typeof path !== "string" || !safePath(path)) return void 0;
        paths[key2] = path;
      }
      policy.paths = paths;
    }
    for (const position of ["leading", "trailing"]) {
      const entries = position === "leading" ? "leading" in value ? value.leading : [] : "trailing" in value ? value.trailing : [];
      if (!Array.isArray(entries)) return void 0;
      const destinations = [];
      for (const entry2 of entries) {
        if (!entry2 || typeof entry2 !== "object" || typeof entry2.key !== "string" || typeof entry2.label !== "string" || typeof entry2.icon !== "string" || typeof entry2.href !== "string" || !safePath(entry2.href) || navigation_default.some((definition) => definition.key === entry2.key) || destinations.some((destination) => destination.key === entry2.key))
          return void 0;
        destinations.push({
          key: entry2.key,
          label: entry2.label,
          icon: entry2.icon,
          href: entry2.href,
          current: false
        });
      }
      policy[position] = destinations;
    }
    if (policy.leading?.some((entry2) => policy.trailing?.some((other) => other.key === entry2.key)))
      return void 0;
    return policy;
  }
  function projectShellNavigation(policy, route, paths = {}, counts = {}) {
    const current = ["session", "trace", "sources"].includes(route) ? "sessions" : route === "machine" ? "machines" : route;
    return [
      ...policy?.leading ?? [],
      ...navigation_default.map(({ path, ...definition }) => ({
        ...definition,
        href: paths[definition.key] ?? policy?.paths?.[definition.key] ?? path,
        current: false
      })),
      ...policy?.trailing ?? []
    ].map((destination) => ({
      ...destination,
      href: paths[destination.key] ?? destination.href,
      current: destination.key === current,
      ...counts[destination.key] === void 0 ? {} : { count: counts[destination.key], hot: true }
    }));
  }

  // src/lib/index.ts
  installPropGuard();

  // src/viewer-host.ts
  var host = null;
  function getViewerHost() {
    return host;
  }

  // src/domain/validate.ts
  function object(value) {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new Error("Invalid object");
    return Object.fromEntries(Object.entries(value));
  }
  function text(value) {
    if (typeof value !== "string") throw new Error("Invalid string");
    return value;
  }
  function number(value) {
    if (typeof value !== "number" || !Number.isFinite(value)) throw new Error("Invalid number");
    return value;
  }
  function boolean(value) {
    if (typeof value !== "boolean") throw new Error("Invalid boolean");
    return value;
  }
  function optional(value, parse) {
    return value == null ? void 0 : parse(value);
  }
  function array(value, parse) {
    if (!Array.isArray(value)) throw new Error("Invalid array");
    return value.map(parse);
  }
  function dictionary(value, parse) {
    return Object.fromEntries(Object.entries(object(value)).map(([id, row]) => [id, parse(row)]));
  }

  // src/state/catalog-source-candidates.ts
  var nullableText = (v) => v === null ? null : text(v);
  function key(v) {
    const value = text(v);
    if (!value || value.length > 8192) throw new Error("Invalid source candidate identity");
    return value;
  }
  function generation(v) {
    const value = text(v);
    if (!/^[a-f0-9]{64}$/.test(value)) throw new Error("Invalid source candidate generation");
    return value;
  }
  function parseCatalogSourceCandidates(v) {
    const row = object(v);
    const items = array(row.items, (value) => {
      const candidate = object(value), source = object(candidate.source), path = text(source.path), root = text(source.root);
      const observed = candidate.archive_observed_at === null ? null : number(candidate.archive_observed_at);
      if (!root || !path || path.length > 4096 || path.startsWith("/") || path.includes("\\") || path.includes("\0") || path.split("/").includes("..") || observed !== null && (!Number.isSafeInteger(observed) || observed < 0))
        throw new Error("Invalid archive source hint");
      return {
        candidate_key: key(candidate.candidate_key),
        source: { root, path },
        native_name_hint: nullableText(candidate.native_name_hint),
        generation: generation(candidate.generation),
        archive_observed_at: observed
      };
    });
    if (row.api !== 1 || items.length > 60 || new Set(items.map((item2) => item2.candidate_key)).size !== items.length)
      throw new Error("Invalid source candidate page");
    return {
      api: 1,
      source_key: key(row.source_key),
      items,
      next_cursor: row.next_cursor === null ? null : key(row.next_cursor)
    };
  }
  function parseCatalogSourceCandidateProgress(v) {
    const row = object(v), catalogKey = row.catalog_key === null ? null : key(row.catalog_key);
    if (row.api !== 1 || !["updating", "ready", "unavailable"].includes(text(row.state)) || row.state === "ready" !== (catalogKey !== null))
      throw new Error("Invalid source candidate progress");
    return {
      api: 1,
      source_key: key(row.source_key),
      candidate_key: key(row.candidate_key),
      generation: generation(row.generation),
      state: row.state,
      catalog_key: catalogKey,
      retryable: boolean(row.retryable),
      reason: nullableText(row.reason)
    };
  }

  // src/state/analytics-wire.ts
  function shape(fields) {
    return (value) => {
      const source = object(value), parsed = Object.fromEntries(
        Object.entries(fields).map(([key2, parse]) => [key2, parse(source[key2])])
      );
      return parsed;
    };
  }
  var list = (parse) => (value) => array(value, parse);
  var nullable = (parse) => (value) => value == null ? null : parse(value);
  var optional2 = (parse) => (value) => value == null ? void 0 : parse(value);
  var strings = list(text);
  var money = shape({ usd: nullable(number), unpriced_models: strings });
  var period = shape({
    agent_ms: number,
    started: number,
    turns: number,
    tools: number,
    errors: number,
    peak: number,
    wait_ms: number,
    median_wait_ms: number,
    longest_wait_ms: number,
    cost: money
  });
  var busy = shape({ sid: text, ms: number });
  var priced = shape({ sid: text, usd: nullable(number), unpriced_models: strings });
  var column = shape({
    from: number,
    to: number,
    claude_ms: number,
    codex_ms: number,
    sessions: list(busy),
    more: number
  });
  var day = shape({
    from: number,
    to: number,
    claude_usd: number,
    codex_usd: number,
    sessions: list(priced),
    more: number
  });
  var breakdown = shape({
    repo: optional2(nullable(text)),
    machine: optional2(text),
    harness: optional2(text),
    model: optional2(text),
    ms: number,
    usd: number,
    sessions: number,
    unpriced_models: strings,
    incomplete_usage: optional2(boolean)
  });
  var tokens = shape({
    input: optional2(number),
    output: optional2(number),
    cache_read: optional2(number),
    cache_write: optional2(number)
  });
  var item = shape({
    sid: text,
    models: optional2(strings),
    cost_usd: nullable(number),
    pr_url: nullable(text)
  });
  var model = shape({
    model: text,
    band: text,
    n: number,
    small_sample: boolean,
    first_pass_acceptance: nullable(number),
    acceptance_n: number,
    median_review_rounds: nullable(number),
    review_rounds_n: number,
    median_red_ci_heads: nullable(number),
    red_ci_n: number,
    median_model_ms: nullable(number),
    model_time_n: number,
    median_cost_usd: nullable(number),
    cost_n: number,
    allowance_per_million_input: nullable(number),
    allowance_n: number,
    tokens,
    tokens_n: number,
    items: list(item),
    items_more: number
  });
  var parseAnalytics = shape({
    days: number,
    version: text,
    current: period,
    previous: period,
    longest_current_wait: nullable(busy),
    calls_unknown: number,
    agents: shape({ unit: text, columns: list(column) }),
    cost: nullable(
      shape({ days: list(day), unpriced_models: strings, incomplete_usage: optional2(boolean) })
    ),
    breakdown: shape({ repo: list(breakdown), machine: list(breakdown), model: list(breakdown) }),
    top: shape({ busy: list(busy), waited: list(busy), cost: list(priced) }),
    sessions: (v) => dictionary(v, shape({ name: text, harness: text })),
    models: optional2(
      shape({ groups: list(model), unknown_reasons: (v) => dictionary(v, text) })
    ),
    allowance: nullable(
      shape({
        recorded_at: number,
        windows: list(shape({ minutes: number, used_percent: number, resets_at: number }))
      })
    ),
    facets: shape({ repo: list(nullable(text)), machine: strings, harness: strings, model: strings })
  });

  // src/domain/format.ts
  var clock = (t, NOW) => {
    const d = new Date(t), n = new Date(NOW);
    const hm = String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0");
    return d.toDateString() === n.toDateString() ? hm : d.toLocaleDateString(void 0, { weekday: "short" }) + " " + hm;
  };
  var ago = (t, NOW) => {
    const d = Math.floor((NOW - t) / 6e4);
    return d < 1 ? "now" : d < 60 ? d + "m" : d < 2880 ? Math.floor(d / 60) + "h" : Math.floor(d / 1440) + "d";
  };
  var dur = (a, b, NOW) => {
    const d = Math.max(0, Math.floor(((b ?? NOW) - a) / 6e4));
    return d >= 1440 ? Math.floor(d / 1440) + "d " + Math.floor(d % 1440 / 60) + "h" : d >= 60 ? Math.floor(d / 60) + "h " + d % 60 + "m" : d + "m";
  };
  var tok = (m) => m >= 1 ? m.toFixed(1) + "M" : Math.round(m * 1e3) + "k";
  var shortModel = (model2) => String(model2 ?? "Unknown model").replace(/^gpt-(\d+\.\d+)-(.+)$/i, "$2 $1").replace(/^gpt-\d+-/i, "").replace(/^claude-/i, "").replace(/^(opus|sonnet|haiku)-(\d+)-(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)\.(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)$/i, "$1 $2");
  var liveUrl = (u) => {
    try {
      return /^https?:$/.test(new URL(u).protocol) && /^https?:\/\//i.test(u) ? u : null;
    } catch {
      return null;
    }
  };
  var compactCount = (n) => {
    if (n < 1e3) return String(n);
    if (n < 1e4) return +(n / 1e3).toFixed(1) + "k";
    const k = Math.round(n / 1e3);
    return k < 1e3 ? k + "k" : +(n / 1e6).toFixed(1) + "M";
  };
  var niceStep = (max) => [0.25, 0.5, 1, 2, 5, 10, 20, 50, 100, 200, 500].find((x) => x * 3 >= max) ?? 1e3;
  function timeText(ms) {
    const mins = Math.max(0, Math.round(ms / 6e4)), days = Math.floor(mins / 1440), hours = Math.floor(mins % 1440 / 60), rem = mins % 60;
    return days ? days + "d " + hours + "h" : hours ? hours + "h " + rem + "m" : mins + "m";
  }
  var countText = (n) => Math.round(n).toLocaleString();
  var hLabel = (n) => n ? +n.toFixed(2) + " h" : "0";

  // src/app/registry.ts
  var HARNESSES = {
    claude: {
      name: "Claude Code",
      short: "Claude",
      icon: { light: "/harness/claude-code.svg", dark: "/harness/claude-code.svg" }
    },
    codex: {
      name: "Codex",
      short: "Codex",
      icon: { light: "/harness/codex-black.svg", dark: "/harness/codex.svg" }
    },
    copilot: {
      name: "GitHub Copilot CLI",
      short: "Copilot",
      icon: { light: "/harness/copilot.svg", dark: "/harness/copilot.svg" }
    },
    opencode: {
      name: "OpenCode",
      short: "OpenCode",
      icon: { light: "/harness/opencode-light.svg", dark: "/harness/opencode-dark.svg" }
    }
  };
  var HARNESS = Object.fromEntries(Object.entries(HARNESSES).map(([id, h2]) => [id, h2.name]));
  var HARNESS_SHORT = Object.fromEntries(
    Object.entries(HARNESSES).map(([id, h2]) => [id, h2.short])
  );
  var I = {
    menu: "M4 7h16M4 12h16M4 17h16",
    more: "M5 12h.01M12 12h.01M19 12h.01",
    back: "M15 6l-6 6 6 6",
    chev: "M9 6l6 6-6 6",
    search: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4",
    filter: "M4 6h16M7 12h10M10 18h4",
    home: "M4 11l8-7 8 7M6 9.5V20h12V9.5M10 20v-5h4v5",
    inbox: "M4 13l2.5-8h11L20 13v6H4zM4 13h5l1 2h4l1-2h5",
    now: "M3 12h4l2.5-6 5 12 2.5-6h4",
    trace: "M6 4v10a4 4 0 0 0 4 4h8M6 10h12M15 7l3 3-3 3M15 15l3 3-3 3",
    sessions: "M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01",
    run: "M4 17l5-5-5-5M12 19h8",
    stack: "M12 3l9 5-9 5-9-5zM3 13l9 5 9-5",
    read: "M6 3h8l4 4v14H6zM14 3v4h4",
    edit: "M4 20h4L19 9l-4-4L4 16zM13 7l4 4",
    find: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4",
    machine: "M3 5h18v11H3zM8 20h8M12 16v4",
    repo: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6",
    duration: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7v5l3 2",
    delegate: "M4 3h7v5H4zM7.5 8v9H13M13 14h7v6h-7z",
    out: "M7 17L17 7M9 7h8v8",
    in: "M17 7L7 17M15 17H7V9",
    move: "M4 8h13l-3-3M20 16H7l3 3",
    ask: "M5 18l-1 3 3-1 11-11-2-2zM14 6l4 4",
    you: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c1-4 4-6 8-6s7 2 8 6",
    q: "M9 9a3 3 0 1 1 4 2.8c-.7.3-1 .9-1 1.7V14M12 18h.01",
    check: "M5 12l4 4 10-10",
    qc: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9.6 9.4a2.5 2.5 0 1 1 3.4 2.4c-.6.3-1 .8-1 1.5v.4M12 17h.01",
    decide: "M12 21v-6M12 15L6 9M12 15l6-6M6 9V4M18 9V4M4 6l2-2 2 2M16 6l2-2 2 2",
    result: "M14 3H6v18h12V7zM14 3v4h4M9 12h6M9 16h6",
    done: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM8 12.5l2.7 2.7L16 9.8",
    x: "M6 6l12 12M18 6L6 18",
    expand: "M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7",
    copy: "M9 9h11v11H9zM5 15H4V4h11v1",
    ext: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
    down: "M12 4v15M5 12l7 7 7-7",
    up: "M6 15l6-6 6 6",
    dn: "M6 9l6 6 6-6",
    branch: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6",
    wrench: "M14.5 6.5a5 5 0 0 0-6.9 6.9l-4.8 4.8a2 2 0 0 0 2.8 2.8l4.8-4.8a5 5 0 0 0 6.9-6.9l-3 3-2.8-2.8z",
    wide: "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5",
    sidebar: "M4 5h16v14H4zM9 5v14",
    tokens: "M5 5h14M12 5v14M9 19h6",
    chart: "M4 19V5M4 19h17M8 15l3-4 3 2 5-7",
    coin: "M12 3v18M17 7.5C17 6.1 14.8 5 12 5S7 6.1 7 7.5 9.2 10 12 10s5 1.1 5 2.5-2.2 2.5-5 2.5-5-1.1-5-2.5",
    relay: "M4 7h13l-3-3M20 17H7l3 3"
  };
  var STATE = {
    work: "Working",
    wait: "Needs you",
    idle: "Idle",
    done: "Done",
    err: "Failed",
    new: "New result",
    read: "Read result"
  };

  // src/app/analyticsPresentation.ts
  var HOUR = 36e5;
  var hoursText = (ms) => (ms / HOUR).toFixed(1) + " h";
  var nameOfSid = (data, sid) => data.sessions[sid]?.name ?? sid;
  var harnessOfSid = (data, sid) => data.sessions[sid]?.harness ?? "";
  function renderAnalyticsPresentation(page, A, failure, host2, actions) {
    const rangeName = host2.range === 1 ? "24 h" : host2.range + " d";
    const metrics = [], charts = [], breakdowns = [], lists = [], modelGroups = /* @__PURE__ */ new Map(), slices = /* @__PURE__ */ new Map();
    const heading = "Measured activity \xB7 Last " + (host2.range === 1 ? "24 hours" : host2.range + " days");
    let error = failure ? A ? "Couldn't update Analytics: " + failure + ". Showing the last answer." : "Couldn't load Analytics: " + failure : A ? void 0 : "Loading\u2026";
    const note = (value, previous, format) => {
      const delta = value - previous;
      return Math.abs(delta) < 1e-9 ? { text: "No change vs previous " + rangeName } : {
        lead: (delta > 0 ? "+" : "\u2212") + format(Math.abs(delta)),
        tone: delta > 0 ? "up" : "down",
        text: " vs previous " + rangeName
      };
    };
    const metric = (label, value, delta, explanation, info) => metrics.push({
      id: "metric-more-analytics-" + metrics.length,
      label,
      value,
      note: delta,
      explanation,
      info
    });
    const widthOf = (measure, max) => Math.max(measure ? 2 : 0, measure / max * 100);
    const makeChart = (costMode) => {
      if (!A) throw new Error("Analytics model unavailable");
      const key2 = costMode ? "cost" : "agents", W = host2.width(), left = costMode ? 46 : 40, right = W - 4;
      if (costMode && !A.cost)
        return {
          key: key2,
          heading: "Cost over time",
          info: host2.costTip,
          empty: "Cost is recorded per UTC day, so there is no hourly series. Pick 7 d or 30 d for a daily chart.",
          width: W,
          left,
          right,
          grid: [],
          bins: [],
          legend: [],
          role: "img",
          label: "",
          ago: A.days === 1 ? "24 h ago" : A.days + " d ago"
        };
      const raw = costMode ? A.cost.days.map((d) => ({
        a: d.from,
        b: d.to,
        claude: d.claude_usd,
        codex: d.codex_usd,
        sessions: d.sessions,
        more: d.more
      })) : A.agents.columns.map((c) => ({
        a: c.from,
        b: c.to,
        claude: c.claude_ms / HOUR,
        codex: c.codex_ms / HOUR,
        sessions: c.sessions,
        more: c.more
      }));
      const most = Math.max(0, ...raw.map((b) => b.claude + b.codex)), stepY = niceStep(most || 1), max = costMode ? Math.max(0.01, most) : Math.max(stepY, Math.ceil(most / stepY) * stepY);
      const grid = [];
      if (costMode)
        for (let i = 0; i <= 2; i++)
          grid.push({ y: 151 - 139 * i / 2, label: "$" + (max * i / 2).toFixed(2) });
      else
        for (let n = 0; n <= max + 1e-9; n += stepY)
          grid.push({ y: 151 - 139 * n / max, label: hLabel(n) });
      const step = (right - left) / Math.max(1, raw.length), barWidth = Math.max(2, step * 0.64);
      const bins = raw.map((bin, i) => {
        const id = key2 + ":" + i, total = bin.claude + bin.codex, when = host2.clock(bin.a) + "\u2013" + host2.clock(bin.b), tip = when + ": " + hLabel(total);
        slices.set(id, { bin, costMode });
        return {
          key: id,
          x: left + i * step,
          width: step,
          barX: left + i * step + (step - barWidth) / 2,
          barWidth,
          claude: 139 * bin.claude / max,
          codex: 139 * bin.codex / max,
          active: costMode ? !!bin.sessions.length : total > 0,
          tip: costMode ? void 0 : tip,
          label: costMode ? host2.clock(bin.a) + " to " + host2.clock(bin.b) + ": " + host2.money(total) : tip + ". Open the sessions busy then",
          when,
          value: costMode ? host2.money(total) : hLabel(total) + " agent-hours"
        };
      });
      return {
        key: key2,
        heading: costMode ? "Cost over time" : "Agents at work",
        info: costMode ? host2.costTip : void 0,
        sub: costMode ? "API-equivalent cost per UTC day \xB7 today so far \xB7 stacked by harness" : "Agent-hours " + A.agents.unit + " \xB7 stacked by harness",
        width: W,
        left,
        right,
        grid,
        bins,
        label: costMode ? "API-equivalent cost per day, stacked by harness" : "Agent-hours " + A.agents.unit + " over the selected range, stacked by harness",
        role: costMode ? "img" : "group",
        ago: A.days === 1 ? "24 h ago" : A.days + " d ago",
        legend: [
          ["claude", "Claude"],
          ["codex", "Codex"]
        ].map(([id, label]) => ({ id, label, mark: host2.mark(id) })),
        missing: costMode && A.cost.incomplete_usage ? "Copied usage is omitted from bars. Its original owner and the full cost are unknown." : costMode && A.cost.unpriced_models.length ? "no price for " + A.cost.unpriced_models.join(", ") + "; unpriced usage is omitted from bars." : void 0
      };
    };
    let models, allowance;
    const measures = [
      [
        "Acceptance",
        "first_pass_acceptance",
        "acceptance_n",
        "acceptance",
        (n) => (n * 100).toFixed(0) + "%"
      ],
      ["Review rounds", "median_review_rounds", "review_rounds_n", "review_rounds", String],
      ["Red CI heads", "median_red_ci_heads", "red_ci_n", "ci", String],
      ["Model time", "median_model_ms", "model_time_n", "model_time", timeText],
      [
        "API cost",
        "median_cost_usd",
        "cost_n",
        "cost",
        (...args) => host2.money(...args)
      ],
      [
        "Allowance / M input",
        "allowance_per_million_input",
        "allowance_n",
        "allowance",
        (n) => n.toFixed(2) + "%"
      ]
    ];
    if (A) {
      const now = A.current, previous = A.previous, pct = (errors, tools) => tools ? Math.round(errors / tools * 100) + "%" : "0%";
      metric(
        "Agent-hours",
        hoursText(now.agent_ms),
        note(now.agent_ms, previous.agent_ms, hoursText),
        "Busy time summed across sessions; two sessions busy for an hour count two hours."
      );
      metric(
        A.days === 1 ? "Cost today (UTC)" : "Cost, last " + A.days + " UTC days",
        now.cost.usd == null ? "\u2014" : host2.money(now.cost.usd),
        now.cost.usd == null || previous.cost.usd == null ? {
          text: "no price for " + [.../* @__PURE__ */ new Set([...now.cost.unpriced_models, ...previous.cost.unpriced_models])].join(
            ", "
          )
        } : note(
          now.cost.usd,
          previous.cost.usd,
          (...args) => host2.money(...args)
        ),
        A.days === 1 ? "API-equivalent cost. Cost is recorded per UTC day: this is the whole current UTC day so far, compared with the whole day before." : "API-equivalent cost. Cost is recorded per UTC day: the last " + A.days + " UTC days count, today so far, compared with the " + A.days + " whole UTC days before.",
        host2.costTip
      );
      metric(
        "Sessions started",
        countText(now.started),
        note(now.started, previous.started, countText)
      );
      metric("Turns", countText(now.turns), note(now.turns, previous.turns, countText));
      const toolNote = note(now.tools, previous.tools, countText);
      if (A.calls_unknown)
        toolNote.tail = " \xB7 \u2014 for " + A.calls_unknown + (A.calls_unknown === 1 ? " session" : " sessions");
      metric(
        "Tool calls",
        countText(now.tools),
        toolNote,
        countText(now.errors) + " failed (" + pct(now.errors, now.tools) + ") \xB7 previous " + rangeName + ": " + countText(previous.errors) + " failed (" + pct(previous.errors, previous.tools) + ")"
      );
      metric(
        "Peak concurrency",
        countText(now.peak),
        note(now.peak, previous.peak, countText),
        "The most sessions busy at the same moment."
      );
      metric(
        "Waited on you",
        timeText(now.wait_ms),
        note(now.wait_ms, previous.wait_ms, timeText),
        "Median wait " + timeText(now.median_wait_ms) + " \xB7 previous " + rangeName + ": " + timeText(previous.median_wait_ms)
      );
      const wait = A.longest_current_wait;
      metric(
        "Longest current wait",
        wait ? timeText(wait.ms) : "\u2014",
        note(wait ? wait.ms : 0, previous.longest_wait_ms, timeText),
        wait ? nameOfSid(A, wait.sid) + " has waited on you for " + timeText(wait.ms) : "No session is waiting on you"
      );
      charts.push(makeChart(false), makeChart(true));
      for (const [key2, title, groups] of [
        ["repo", "By repo", A.breakdown.repo],
        ["machine", "By machine", A.breakdown.machine],
        ["harness", "By harness and model", A.breakdown.model]
      ]) {
        const keyFor = (g) => key2 === "repo" ? g.repo ?? "__none__" : key2 === "machine" ? g.machine ?? "" : g.harness + "\0" + g.model;
        const labelFor = (id) => key2 === "repo" ? id === "__none__" ? "No repo (roles)" : id : key2 === "machine" ? host2.machines[id] ?? id : (HARNESS[id.split("\0")[0]] ?? id.split("\0")[0]) + " \xB7 " + shortModel(id.split("\0")[1]);
        const selected = (g) => host2.measure === "cost" ? g.usd : g.ms, rows = [...groups].sort(
          (a, b) => selected(b) - selected(a) || labelFor(keyFor(a)).localeCompare(labelFor(keyFor(b)))
        ), max = Math.max(1, ...rows.map(selected));
        breakdowns.push({
          key: key2,
          heading: title,
          rows: rows.map((g) => ({
            key: keyFor(g),
            name: labelFor(keyFor(g)),
            count: g.sessions + (g.sessions === 1 ? " session" : " sessions"),
            width: widthOf(selected(g), max),
            color: key2 === "harness" ? keyFor(g).startsWith("claude") ? "claude" : "codex" : void 0,
            hours: hoursText(g.ms),
            cost: g.incomplete_usage || g.unpriced_models.length ? "\u2014" : host2.money(g.usd),
            missing: g.incomplete_usage ? "copied usage; original owner unknown" : g.unpriced_models.length ? "no price for " + g.unpriced_models.join(", ") : void 0
          }))
        });
      }
      const topLists = [
        [
          "Top sessions \xB7 busy time",
          A.top.busy,
          (x) => timeText("ms" in x ? x.ms : 0),
          (x) => "ms" in x ? x.ms : 0
        ],
        [
          "Top sessions \xB7 waited on",
          A.top.waited,
          (x) => timeText("ms" in x ? x.ms : 0),
          (x) => "ms" in x ? x.ms : 0
        ],
        [
          "Most expensive sessions \xB7 API-equivalent cost",
          A.top.cost,
          (x) => "usd" in x && x.usd != null ? host2.money(x.usd) : "\u2014",
          (x) => "usd" in x ? x.usd ?? 0 : 0
        ]
      ];
      for (const [title, items, value, measure] of topLists) {
        const max = Math.max(1, ...items.map(measure));
        lists.push({
          heading: title,
          rows: items.map((item2) => {
            const harness = harnessOfSid(A, item2.sid), missing = "unpriced_models" in item2 ? item2.unpriced_models : [];
            return {
              id: item2.sid,
              name: nameOfSid(A, item2.sid),
              harness,
              harnessName: HARNESS_SHORT[harness] ?? harness,
              harnessTip: HARNESS[harness] && HARNESS[harness] !== HARNESS_SHORT[harness] ? HARNESS[harness] : void 0,
              mark: host2.mark(harness),
              value: value(item2),
              href: host2.sessionHref(item2.sid),
              rank: widthOf(measure(item2), max),
              missing: missing.length ? "no price for " + missing.join(", ") : void 0
            };
          })
        });
      }
      if (A.models) {
        const reasons = A.models.unknown_reasons ?? {};
        models = [];
        for (const band of ["easy", "medium", "hard", "unknown"]) {
          const rows = (A.models.groups ?? []).filter((g) => g.band === band);
          if (!rows.length) continue;
          const points = rows.filter(
            (g) => g.first_pass_acceptance != null && g.median_cost_usd != null
          ), W = Math.max(280, host2.width()), maxCost = Math.max(0.01, ...points.map((g) => g.median_cost_usd ?? 0));
          models.push({
            key: band,
            heading: band === "unknown" ? "Unknown difficulty" : band[0].toUpperCase() + band.slice(1),
            width: W,
            rows: rows.map((g) => {
              const key2 = band + "\0" + g.model;
              modelGroups.set(key2, g);
              const tokens2 = g.tokens ?? {};
              return {
                key: key2,
                name: shortModel(g.model),
                tip: g.model,
                count: g.n + (g.small_sample ? " \xB7 small sample" : ""),
                cells: [
                  ...measures.map(([, valueKey, nKey, reason, format]) => ({
                    text: g[valueKey] == null ? "Unknown" : format(g[valueKey]),
                    n: g[nKey] ?? 0,
                    tip: g[valueKey] == null ? reasons[reason] : void 0
                  })),
                  {
                    text: g.tokens_n ? [
                      tokens2.input ?? 0,
                      tokens2.output ?? 0,
                      (tokens2.cache_read ?? 0) + (tokens2.cache_write ?? 0)
                    ].map(countText).join(" / ") : "Unknown",
                    n: g.tokens_n ?? 0
                  }
                ]
              };
            }),
            points: points.map((g) => ({
              x: 42 + g.median_cost_usd / maxCost * (W - 58),
              y: 150 - g.first_pass_acceptance * 130,
              label: shortModel(g.model),
              tip: g.model + ": " + host2.money(g.median_cost_usd ?? 0) + ", " + (g.first_pass_acceptance * 100).toFixed(0) + "% accepted; acceptance n=" + g.acceptance_n + ", cost n=" + g.cost_n
            }))
          });
        }
      }
      const limits = A.allowance;
      if (limits?.recorded_at != null && limits.windows?.length)
        allowance = {
          when: new Date(limits.recorded_at).toLocaleString([], {
            hour: "numeric",
            minute: "2-digit"
          }),
          windows: limits.windows.map((limit) => ({
            label: limit.minutes === 300 ? "5-hour window" : limit.minutes === 10080 ? "Weekly window" : limit.minutes + "-minute window",
            used: limit.used_percent + "% used",
            reset: "Resets " + new Date(limit.resets_at).toLocaleString([], {
              weekday: "short",
              hour: "numeric",
              minute: "2-digit"
            })
          }))
        };
    }
    renderAnalyticsScreen(
      page,
      {
        heading,
        error,
        ready: !!A,
        query: host2.query,
        metrics,
        charts,
        breakdowns,
        lists,
        models,
        measure: host2.measure,
        allowance,
        modelHeaders: [
          "Model",
          "Work items",
          ...measures.map((m) => m[0]),
          "Tokens \xB7 input / output / cache"
        ]
      },
      {
        committed: actions.committed,
        facets: actions.facets,
        session: actions.session,
        measure: actions.measure,
        breakdown: actions.breakdown,
        slice(key2) {
          const slice = slices.get(key2);
          if (slice && A) actions.slice(A, slice.bin, slice.costMode);
        },
        model(key2) {
          const group = modelGroups.get(key2);
          if (group) actions.model(group);
        }
      }
    );
  }

  // src/app/seenResults.ts
  function createSeenResults(host2) {
    const SEEN_KEY = "semon.seen", SEEN_LIMIT = 2e3;
    const SEEN_RESULTS = (() => {
      try {
        const ids = JSON.parse(window.localStorage.getItem(SEEN_KEY) ?? "[]");
        if (!Array.isArray(ids)) return /* @__PURE__ */ new Set();
        const clean = ids.filter((id) => typeof id === "string").slice(-SEEN_LIMIT), seen = new Set(clean);
        if (seen.size !== ids.length)
          try {
            window.localStorage.setItem(SEEN_KEY, JSON.stringify([...seen]));
          } catch {
          }
        return seen;
      } catch {
        return /* @__PURE__ */ new Set();
      }
    })();
    return { SEEN_RESULTS, SEEN_LIMIT, SEEN_KEY };
  }

  // src/app/overviewPages.tsx
  var nullable2 = (value, parse) => value == null ? null : parse(value);
  function parseSelection(value) {
    const row = object(value);
    return { source_key: text(row.source_key), catalog_key: text(row.catalog_key) };
  }
  function parseHomeRow(value) {
    const row = object(value), session = object(row.session), event = row.event == null ? null : object(row.event);
    return {
      ...parseSelection(row),
      id: text(row.id),
      contentGeneration: nullable2(session.content_generation, text),
      session: {
        name: text(session.name),
        kind: session.kind == null ? void 0 : text(session.kind),
        harness: text(session.harness),
        model: text(session.model),
        repo: nullable2(session.repo, text),
        state: text(session.state),
        last: number(session.last),
        start: number(session.start),
        waiting_for: nullable2(session.waiting_for, text),
        waiting_since: nullable2(session.waiting_since, number),
        cost: nullable2(session.cost, number),
        parent: nullable2(session.parent, text) ?? void 0,
        branch: nullable2(session.branch, text) ?? void 0,
        detail: nullable2(session.detail, text) ?? void 0,
        activity: session.activity == null ? void 0 : [
          text(session.activity[0]),
          text(session.activity[1]),
          number(session.activity[2])
        ]
      },
      event: event ? {
        id: text(event.id),
        at: number(event.at),
        done: nullable2(event.done, number),
        ask: nullable2(event.ask, text),
        status: text(event.status),
        brief: text(event.brief),
        answer: nullable2(event.answer, (value2) => array(value2, text)),
        declined: boolean(event.declined),
        origin: nullable2(event.origin, text) ?? void 0,
        turn: nullable2(event.turn, text) ?? void 0
      } : void 0
    };
  }
  function parseHome(value) {
    const data = object(value), totals = object(data.totals);
    if (data.api !== 1) throw Error("Unsupported Home response");
    for (const key2 of ["inbox", "working", "answered", "recent"]) {
      if (!Array.isArray(data[key2]) || data[key2].length > (key2 === "recent" ? 60 : 100))
        throw Error("Home response exceeds its bound");
    }
    return {
      version: text(data.version),
      now: number(data.now),
      next: {
        inbox: nullable2(object(data.next).inbox, number),
        working: nullable2(object(data.next).working, number),
        answered: nullable2(object(data.next).answered, number)
      },
      totals: {
        waiting: number(totals.waiting),
        working: number(totals.working),
        answered: number(totals.answered)
      },
      machines: array(data.machines, (value2) => {
        const row = object(value2);
        return {
          key: text(row.key),
          label: text(row.label),
          up: boolean(row.up),
          working: number(row.working),
          sessions: number(row.sessions),
          last: nullable2(row.last, number)
        };
      }),
      recent: array(data.recent, parseHomeRow),
      inbox: array(data.inbox, parseHomeRow),
      working: array(data.working, parseHomeRow),
      answered: array(data.answered, parseHomeRow)
    };
  }
  function createOverviewPages(host2) {
    let disposed = false, epoch = 0, loading = false, queued = false, queuedForce = false, error = null, home = null, analytics = null, machineLabels = {}, selections = {}, range = 7, measure = "hours", allAnswered = false;
    const filters = { repo: "", machine: "", harness: "", model: "" };
    const analyticsCache = /* @__PURE__ */ new Map();
    let analyticsRetryAt = 0;
    let observationAfter = null;
    function acceptAnalytics(value) {
      analytics = value?.data ?? null;
      if (!value) return;
      selections = value.selections;
      machineLabels = value.labels;
      recentRows = value.recent;
      recentNow = value.now;
    }
    let recentRows = [], recentNow = 0;
    const recentOpen = /* @__PURE__ */ new Map();
    const recentAll = /* @__PURE__ */ new Set();
    try {
      const preferences = object(JSON.parse(localStorage.getItem("semon.tree") ?? "{}"));
      for (const [id, value] of Object.entries(preferences).slice(-200)) {
        if (value && typeof value === "object" && "open" in value && typeof value.open === "boolean")
          recentOpen.set(id, value.open);
      }
    } catch {
    }
    function recentSnapshot(source, catalog = [], current, rail = false, selectedMetadata) {
      const borrowed = source ? recentRows.filter((row) => row.source_key === source) : recentRows;
      const rows = borrowed.length ? borrowed.map((row) => {
        const observed = catalog.find((item3) => item3.key === row.catalog_key);
        return observed ? {
          ...row,
          session: {
            ...row.session,
            name: observed.name,
            kind: observed.presentation_kind ?? row.session.kind,
            last: observed.last ?? row.session.last,
            parent: observed.parent ?? void 0,
            model: observed.model,
            repo: observed.repo
          }
        } : row;
      }).concat(
        catalog.filter((item3) => !borrowed.some((row) => row.catalog_key === item3.key)).slice(0, 60).map((row) => ({
          id: JSON.stringify([source, row.key]),
          source_key: source,
          catalog_key: row.key,
          contentGeneration: null,
          session: {
            name: row.name,
            kind: row.presentation_kind ?? void 0,
            harness: row.harness,
            model: row.model,
            repo: row.repo,
            state: "off",
            start: row.start ?? 0,
            last: row.last ?? 0,
            parent: row.parent ?? void 0,
            cost: row.cost.usd,
            waiting_for: null,
            waiting_since: null
          }
        }))
      ).sort((a, b) => b.session.last - a.session.last).slice(0, 60) : catalog.slice(0, 60).map((row) => ({
        id: JSON.stringify([source, row.key]),
        source_key: source,
        catalog_key: row.key,
        contentGeneration: null,
        session: {
          name: row.name,
          kind: row.presentation_kind ?? void 0,
          harness: row.harness,
          model: row.model,
          repo: row.repo,
          state: row.details?.state ?? "off",
          start: row.start ?? 0,
          last: row.last ?? 0,
          parent: row.parent ?? void 0,
          branch: row.branch ?? void 0,
          cost: row.cost.usd,
          waiting_for: null,
          waiting_since: null
        }
      }));
      if (source && selectedMetadata && selectedMetadata.key === current) {
        for (const relative of [
          ...selectedMetadata.details?.family?.ancestors ?? [],
          ...selectedMetadata.details?.family?.runs ?? [],
          {
            key: selectedMetadata.key,
            name: selectedMetadata.name,
            harness: selectedMetadata.harness,
            kind: selectedMetadata.presentation_kind ?? selectedMetadata.kind,
            parent: selectedMetadata.parent,
            cost: selectedMetadata.cost
          }
        ]) {
          if (rows.some((row) => row.catalog_key === relative.key)) continue;
          rows.push({
            id: JSON.stringify([source, relative.key]),
            source_key: source,
            catalog_key: relative.key,
            contentGeneration: null,
            session: {
              name: relative.name,
              kind: relative.kind,
              harness: relative.harness,
              parent: relative.parent ?? void 0,
              model: "",
              repo: null,
              state: "off",
              start: 0,
              last: 0,
              cost: relative.cost.usd,
              waiting_for: null,
              waiting_since: null
            }
          });
        }
      }
      const byId = new Map(rows.map((row) => [row.id, row]));
      const canonical = /* @__PURE__ */ new Map();
      const idOf = (row) => JSON.stringify([row.source_key, row.session.parent]);
      for (const row of rows)
        if (row.session.parent && byId.has(idOf(row))) {
          const parent = idOf(row);
          canonical.set(parent, [...canonical.get(parent) ?? [], row]);
        }
      const path = /* @__PURE__ */ new Set();
      let selected = rows.find((row) => row.source_key === source && row.catalog_key === current);
      while (selected && !path.has(selected.id)) {
        path.add(selected.id);
        selected = byId.get(idOf(selected));
      }
      const children = /* @__PURE__ */ new Map(), roots = [], reachable = /* @__PURE__ */ new Set();
      function visit(row, parent) {
        if (reachable.has(row.id)) return;
        reachable.add(row.id);
        let nearest = parent;
        if (host2.reviews() || row.session.kind !== "Approval review" || path.has(row.id)) {
          if (parent) children.set(parent, [...children.get(parent) ?? [], row]);
          else roots.push(row);
          nearest = row.id;
        }
        for (const child of canonical.get(row.id) ?? []) visit(child, nearest);
      }
      for (const row of rows.filter((row2) => !row2.session.parent || !byId.has(idOf(row2)))) visit(row);
      for (const row of rows) if (!reachable.has(row.id)) visit(row);
      const order = host2.orderScope("recent", JSON.stringify([source, host2.reviews(), rail]));
      const visibleParents = new Set(
        [...document.querySelectorAll("#lanes .treeitem")].filter((node) => node.querySelector(":scope > .tree-row .srow")?.getClientRects().length).map((node) => node.dataset.id)
      );
      const keys = new Set(children.keys());
      if (order.keep) {
        for (const [parent, kids] of children)
          if (!order.prevKids.has(parent) && visibleParents.has(parent) && !path.has(parent)) {
            order.n += kids.length;
            children.delete(parent);
            keys.delete(parent);
          }
      }
      order.kids = keys;
      function item2(row, depth, visited) {
        const next = new Set(visited).add(row.id);
        const rank = (candidate, seen2 = /* @__PURE__ */ new Set()) => {
          if (seen2.has(candidate.id)) return 2;
          seen2.add(candidate.id);
          return Math.min(
            candidate.session.state === "wait" ? 0 : candidate.session.state === "work" ? 1 : 2,
            ...(children.get(candidate.id) ?? []).map((child) => rank(child, seen2))
          );
        };
        const candidates = (children.get(row.id) ?? []).filter((child) => !next.has(child.id));
        const kids = [...candidates].sort(
          (a, b) => rank(a) - rank(b) || b.session.last - a.session.last
        );
        const visible2 = recentAll.has(row.id) ? kids : kids.filter((child, index) => index < 3 || index < 8 && rank(child) < 2);
        const open2 = recentOpen.get(row.id) ?? (home?.machines.length === 1 ? recentOpen.get(row.catalog_key) : void 0) ?? (path.has(row.id) || kids.some((child) => rank(child) < 2));
        const shown = orderRows(
          order,
          (recentAll.has(row.id) ? "all:" : "kids:") + row.id,
          visible2,
          (a, b) => rank(a) - rank(b) || b.session.last - a.session.last,
          { must: path, quiet: !open2, seed: recentAll.has(row.id) || !visibleParents.has(row.id) }
        );
        return {
          id: row.id,
          name: row.session.name,
          label: row.session.name + ", " + (HARNESSES[row.session.harness]?.name ?? row.session.harness),
          current: current === row.catalog_key ? "page" : void 0,
          state: row.session.state,
          stateLabel: row.session.state === "work" ? "Working" : row.session.state === "wait" ? "Waiting on you" : "Recorded history",
          age: timeText(Math.max(0, recentNow - row.session.last)),
          model: shortModel(row.session.model),
          modelTip: row.session.model,
          harness: mark2(row.session.harness),
          fields: row.session.repo ? [
            {
              className: "repo",
              text: row.session.repo.split("/").pop() ?? row.session.repo,
              priority: 1,
              tip: row.session.repo,
              icon: I.repo
            }
          ] : [],
          rail,
          open: open2,
          depth,
          stuck: recentAll.has(row.id),
          flag: (() => {
            const below = (id, seen2 = /* @__PURE__ */ new Set()) => {
              if (seen2.has(id)) return [];
              seen2.add(id);
              return (children.get(id) ?? []).flatMap((child) => [child, ...below(child.id, seen2)]);
            };
            const descendants = below(row.id), waiting = descendants.filter((child) => child.session.state === "wait").length, working = descendants.filter((child) => child.session.state === "work").length;
            return waiting ? { state: "wait", tip: waiting + " needs you" } : working ? { state: "work", tip: working + " working" } : void 0;
          })(),
          childState: kids.some((child) => rank(child) === 0) ? "wait" : kids.some((child) => rank(child) === 1) ? "work" : void 0,
          children: kids.length ? shown.map((child) => item2(child, depth + 1, next)) : void 0,
          all: shown.length < kids.length ? kids.length : void 0
        };
      }
      const orderedRoots = orderRows(
        order,
        "roots",
        roots,
        (a, b) => b.session.last - a.session.last || a.id.localeCompare(b.id),
        { limit: 8, must: path }
      );
      const visibleRoots = orderedRoots.slice(0, 8);
      for (const row of orderedRoots.filter((row2) => path.has(row2.id))) {
        if (visibleRoots.includes(row)) continue;
        let last = visibleRoots.length - 1;
        while (last >= 0 && path.has(visibleRoots[last].id)) last--;
        if (last >= 0) visibleRoots.splice(last, 1, row);
      }
      return {
        empty: !rows.length,
        items: visibleRoots.map((row) => item2(row, 0, /* @__PURE__ */ new Set()))
      };
    }
    let beforeFilters = "";
    let facetPanel = null;
    const facetOwner = createFacetChrome({
      select(label, value, onChange) {
        return createSelect({ label, value, options: [], onChange });
      },
      change(key2, value) {
        filters[key2] = value;
      },
      cleared(key2) {
        filters[key2] = "";
        analyticsChanged();
      },
      clear() {
        for (const key2 of Object.keys(filters)) filters[key2] = "";
        draw();
      },
      canOpen() {
        return host2.route() === "analytics" && !disposed;
      },
      opened(dialog) {
        beforeFilters = JSON.stringify(filters);
        facetPanel = { destroy: () => dialog.close() };
        host2.panelOpened(facetPanel);
      },
      closed(_dialog, reason) {
        if (facetPanel) host2.panelClosed(facetPanel);
        facetPanel = null;
        if (reason === "dismissed" && beforeFilters !== JSON.stringify(filters)) analyticsChanged();
      }
    });
    const facets = facetOwner.element;
    const actions = document.createElement("div");
    actions.className = "analytics-range";
    actions.setAttribute("role", "group");
    actions.setAttribute("aria-label", "Analytics range");
    const dialogs = /* @__PURE__ */ new Set();
    const seen = createSeenResults({});
    const money3 = (n) => "$" + n.toFixed(2);
    const mark2 = (id) => {
      const harness = HARNESSES[id];
      return harness ? {
        id,
        name: harness.name,
        light: harness.icon.light,
        dark: harness.icon.dark,
        darkTheme: matchMedia("(prefers-color-scheme: dark)").matches
      } : void 0;
    };
    const query = () => {
      const value = new URLSearchParams({ range: range === 1 ? "24h" : range + "d" });
      for (const key2 of ["repo", "machine", "harness", "model"])
        if (filters[key2]) value.set(key2, filters[key2] === "__none__" ? "" : filters[key2]);
      return value.toString();
    };
    function analyticsChanged() {
      acceptAnalytics(analyticsCache.get(query()));
      error = null;
      draw();
      void refresh(true);
    }
    function open(selection, turn) {
      if (!disposed && selection) host2.session(selection, turn);
    }
    function panel(title, className) {
      const value = createPanelChrome(
        { title, className },
        {
          opened() {
            host2.panelOpened(value);
          },
          closed() {
            host2.panelClosed(value);
            dialogs.delete(value);
            render(null, value.body);
          }
        }
      );
      dialogs.add(value);
      return value;
    }
    function drawHome() {
      if (!home) return;
      if (!home.machines.length && host2.homeIntro) {
        if (host2.homeIntro.parentElement !== host2.root) {
          render(null, host2.root);
          host2.root.replaceChildren(host2.homeIntro);
        }
        return;
      }
      const inbox = home.inbox.filter(
        (row) => row.event?.ask !== "result" || !(home.machines.length === 1 && seen.SEEN_RESULTS.has(row.event.id)) && !seen.SEEN_RESULTS.has(JSON.stringify([row.source_key, row.event.id]))
      );
      const byId = /* @__PURE__ */ new Map();
      const rowId = (row) => JSON.stringify([row.source_key, row.event?.id ?? row.catalog_key]);
      const inboxRow = (row, quiet) => {
        const event = row.event;
        const id = rowId(row);
        byId.set(id, row);
        return {
          id,
          quiet,
          icon: quiet ? I.done : event.ask === "question" ? I.qc : event.ask === "decision" ? I.decide : I.result,
          parts: [
            { className: "who", text: row.session.name },
            {
              className: "verb",
              text: event.ask === "question" ? " asked you" : event.ask === "decision" ? " needs your decision" : " sent you a result"
            }
          ],
          age: timeText(Math.max(0, home.now - event.at)) + " ago",
          preview: event.brief || (loading ? "Loading recorded text\u2026" : "Recorded preview unavailable"),
          answer: quiet && event.ask !== "result" ? event.answer?.length ? "You answered: " + event.answer.join(" \xB7 ") : event.declined ? "Declined" : "Answered \xB7 reply not in these logs" : void 0,
          origin: event.origin ? { message: event.origin } : void 0,
          context: [
            HARNESSES[row.session.harness]?.name ?? row.session.harness,
            home.machines.find((machine) => machine.key === row.source_key)?.label ?? row.source_key
          ].join(" \xB7 "),
          harness: mark2(row.session.harness),
          trace: event.turn ? id : void 0
        };
      };
      const sessions = new Map(home.working.map((row) => [row.id, row]));
      renderHomeScreen(
        host2.root,
        {
          observation: error ?? void 0,
          waiting: home.totals.waiting - (home.inbox.length - inbox.length),
          working: home.totals.working,
          up: home.machines.filter((machine) => machine.up).length,
          machines: home.machines.length,
          inbox: inbox.map((row) => inboxRow(row, false)),
          live: orderRows(
            host2.orderScope("working"),
            "working",
            home.working,
            (a, b) => b.session.last - a.session.last
          ).map((row) => ({
            id: row.id,
            name: row.session.name,
            state: "work",
            stateLabel: "Working",
            status: HARNESSES[row.session.harness]?.name ?? row.session.harness,
            detail: [
              home.machines.length > 1 ? home.machines.find((machine) => machine.key === row.source_key)?.label : null,
              row.session.detail ? "for you" : null,
              row.session.detail
            ].filter(Boolean).join(" \xB7 "),
            activity: row.session.activity,
            harness: mark2(row.session.harness)
          })),
          answered: (allAnswered ? home.answered : home.answered.slice(0, 3)).map(
            (row) => inboxRow(row, true)
          ),
          totalAnswered: home.totals.answered,
          allAnswered,
          moreInbox: home.next.inbox !== null,
          moreLive: home.next.working !== null,
          moreAnswered: home.next.answered !== null
        },
        {
          committed() {
          },
          trace(id) {
            const row = byId.get(id);
            if (row?.event?.turn) host2.trace(row, row.event.turn);
          },
          session(id) {
            open(sessions.get(id));
          },
          inbox(id) {
            const row = byId.get(id);
            if (!row) return;
            if (row.event?.ask === "result") {
              seen.SEEN_RESULTS.add(JSON.stringify([row.source_key, row.event.id]));
              while (seen.SEEN_RESULTS.size > seen.SEEN_LIMIT)
                seen.SEEN_RESULTS.delete(seen.SEEN_RESULTS.values().next().value);
              try {
                localStorage.setItem(seen.SEEN_KEY, JSON.stringify([...seen.SEEN_RESULTS]));
              } catch {
              }
            }
            open(row, row.event?.turn);
          },
          answered() {
            allAnswered = true;
            draw();
          },
          more(section) {
            void moreHome(section);
          }
        }
      );
    }
    function drawAnalytics() {
      render(
        /* @__PURE__ */ jsx(Fragment2, { children: [
          [1, "24 h"],
          [7, "7 d"],
          [30, "30 d"]
        ].map(([days, label]) => /* @__PURE__ */ jsx(
          "button",
          {
            type: "button",
            class: range === days ? "on" : "",
            "aria-pressed": range === days,
            onClick: () => {
              range = Number(days);
              analyticsChanged();
            },
            children: label
          }
        )) }),
        actions
      );
      const choices = analytics?.facets;
      facetOwner.update(
        ["repo", "machine", "harness", "model"].map((key2) => ({
          key: key2,
          label: key2 === "repo" ? "Repo" : key2[0].toUpperCase() + key2.slice(1),
          value: filters[key2],
          display: key2 === "machine" ? machineLabels[filters[key2]] ?? filters[key2] : key2 === "harness" ? HARNESSES[filters[key2]]?.name ?? filters[key2] : key2 === "model" ? shortModel(filters[key2]) : filters[key2] === "__none__" ? "No repo" : filters[key2],
          options: [
            {
              value: "",
              label: "All " + (key2 === "repo" ? "repos" : key2 === "harness" ? "harnesses" : key2 + "s")
            },
            ...(choices?.[key2] ?? []).map((value) => ({
              value: value ?? "__none__",
              label: value == null ? "No repo" : key2 === "machine" ? machineLabels[value] ?? value : key2 === "harness" ? HARNESSES[value]?.name ?? value : key2 === "model" ? shortModel(value) : value
            }))
          ]
        }))
      );
      const machines = { ...machineLabels };
      for (const machine of home?.machines ?? []) machines[machine.key] = machine.label;
      renderAnalyticsPresentation(
        host2.root,
        analytics,
        error,
        {
          range,
          measure,
          query: query(),
          machines,
          mark: mark2,
          money: money3,
          clock: (time) => new Date(time).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }),
          costTip: "API-equivalent cost; actual plan charges may differ.",
          width: () => {
            const style = getComputedStyle(host2.root);
            return Math.max(
              280,
              host2.root.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)
            );
          },
          sessionHref: (id) => selections[id] ? "/s/native/" + encodeURIComponent(selections[id].catalog_key) + "?machine=" + encodeURIComponent(selections[id].source_key) : void 0
        },
        {
          committed() {
          },
          facets: () => facets,
          session: (id) => open(selections[id]),
          measure: (value) => {
            measure = value;
            draw();
          },
          breakdown(group, key2) {
            if (group === "repo") filters.repo = key2;
            else if (group === "machine") filters.machine = key2;
            else [filters.harness, filters.model] = key2.split("\0");
            const machines2 = Object.keys(machineLabels);
            host2.sessions({
              ...filters,
              machine: filters.machine || (machines2.length === 1 ? machines2[0] : "")
            });
          },
          slice(data, bin, costMode) {
            const value = panel(costMode ? "Sessions with cost" : "Sessions busy", "analytics-slice");
            renderSliceBody(
              value.body,
              bin.sessions.map((item2) => ({
                id: item2.sid,
                name: data.sessions[item2.sid]?.name ?? item2.sid,
                harness: data.sessions[item2.sid]?.harness ?? "",
                harnessName: data.sessions[item2.sid]?.harness ?? "",
                mark: mark2(data.sessions[item2.sid]?.harness ?? ""),
                value: costMode && "usd" in item2 ? item2.usd == null ? "\u2014" : money3(item2.usd) : timeText("ms" in item2 ? item2.ms : 0)
              })),
              bin.more,
              "No sessions in this range.",
              (id) => {
                open(selections[id]);
              }
            );
            value.show();
          },
          model(group) {
            const value = panel(shortModel(group.model) + " \xB7 " + group.band);
            renderModelItems(
              value.body,
              group.n + " work items",
              (group.items ?? []).map((item2) => ({
                id: item2.sid,
                name: analytics?.sessions[item2.sid]?.name,
                description: (item2.models ?? []).map(shortModel).join(" \u2192 ") + " \xB7 " + (item2.cost_usd == null ? "cost unknown" : money3(item2.cost_usd)),
                url: liveUrl(item2.pr_url ?? "") ?? void 0
              })),
              group.items_more ? group.items_more + " more items" : void 0,
              {
                session(id) {
                  open(selections[id]);
                },
                trace() {
                }
              }
            );
            value.show();
          }
        }
      );
    }
    function draw() {
      if (disposed) return;
      if (host2.nativeContent) {
        host2.chrome();
        return;
      }
      if (loading) host2.root.setAttribute("aria-busy", "true");
      else host2.root.removeAttribute("aria-busy");
      if (host2.route() === "home") {
        if (home) drawHome();
        else if (host2.homeIntro) host2.root.replaceChildren(host2.homeIntro);
        else
          render(
            /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: error ?? "Loading Home\u2026" }),
            host2.root
          );
      } else if (host2.route() === "analytics") drawAnalytics();
      else if (host2.route() === "machines") {
        if (home)
          renderMachinesScreen(
            host2.root,
            {
              up: home.machines.filter((machine) => machine.up).length,
              working: home.machines.reduce((sum, machine) => sum + machine.working, 0),
              rows: home.machines.map((machine) => ({
                id: machine.key,
                name: machine.label,
                state: machine.up ? "work" : "off",
                stateLabel: machine.up ? "Online" : "Offline",
                status: machine.working ? machine.working + " working" : machine.up ? "Online" : "Offline",
                detail: machine.sessions + " sessions" + (machine.last == null ? "" : " \xB7 " + timeText(Math.max(0, home.now - machine.last)) + " ago")
              })),
              admin: null
            },
            {
              machine(machine) {
                host2.sessions({ machine, harness: "", repo: "", model: "" });
              },
              admin() {
              },
              committed() {
              }
            }
          );
        else
          render(
            /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: error ?? "Loading Machines\u2026" }),
            host2.root
          );
      }
      host2.chrome();
    }
    async function moreHome(section) {
      const offset = home?.next[section];
      if (disposed || loading || offset == null || host2.route() !== "home") return;
      const ticket = epoch;
      loading = true;
      host2.root.setAttribute("aria-busy", "true");
      try {
        const next = parseHome(
          await host2.api(
            "/api/viewer/home?" + new URLSearchParams({ [section + "_offset"]: String(offset) }),
            { seen: [...seen.SEEN_RESULTS] }
          )
        );
        if (disposed || ticket !== epoch || host2.route() !== "home" || !home) return;
        if (next.version !== home.version) {
          queued = true;
          return;
        }
        const unique = new Map(
          home[section].map((row) => [
            JSON.stringify([row.source_key, row.event?.id ?? row.catalog_key]),
            row
          ])
        );
        for (const row of next[section])
          unique.set(JSON.stringify([row.source_key, row.event?.id ?? row.catalog_key]), row);
        error = null;
        const previewsIncomplete = await hydrateHome(next, ticket);
        if (disposed || ticket !== epoch || host2.route() !== "home") return;
        if (previewsIncomplete) error = "Some recorded previews are unavailable. Retrying\u2026";
        home = {
          ...home,
          [section]: [...unique.values()],
          next: { ...home.next, [section]: next.next[section] },
          totals: next.totals
        };
      } catch (failure) {
        if (!disposed && ticket === epoch)
          error = failure instanceof Error ? failure.message : "Activity unavailable";
      } finally {
        if (!disposed && ticket === epoch) {
          loading = false;
          draw();
          if (queued) {
            queued = false;
            void refresh();
          }
        }
      }
    }
    async function hydrateHome(next, ticket) {
      const selected = /* @__PURE__ */ new Map();
      for (const row of [...next.inbox, ...next.working, ...next.answered]) {
        if (row.previewReady) continue;
        if (!row.contentGeneration) continue;
        const key2 = JSON.stringify([row.source_key, row.catalog_key]);
        selected.set(key2, [...selected.get(key2) ?? [], row]);
      }
      let incomplete = false;
      const work = [...selected.values()].flatMap(
        (rows) => Array.from(
          { length: Math.ceil(rows.length / 8) },
          (_, page) => rows.slice(page * 8, (page + 1) * 8)
        )
      );
      let cursor = 0;
      await Promise.all(
        Array.from({ length: Math.min(4, work.length) }, async () => {
          while (cursor < work.length && !disposed && ticket === epoch && host2.route() === "home") {
            const rows = work[cursor++], first = rows[0];
            const ids = [...new Set(rows.flatMap((row) => row.event ? [row.event.id] : []))];
            const args = new URLSearchParams({
              machine: first.source_key,
              sid: first.catalog_key,
              scope: "retained_history",
              generation: first.contentGeneration,
              events: JSON.stringify(ids)
            });
            try {
              const value = object(await host2.api("/api/session-home?" + args));
              if (disposed || ticket !== epoch || host2.route() !== "home") return;
              const identity2 = object(value.identity), session = object(value.session);
              if (value.api !== 1 || identity2.source_key !== first.source_key || identity2.catalog_key !== first.catalog_key || value.generation !== first.contentGeneration)
                throw Error("Home preview identity changed");
              const events = dictionary(value.events, (value2) => {
                const row = object(value2);
                return {
                  brief: text(row.brief),
                  answer: nullable2(row.answer, (value3) => array(value3, text)),
                  origin: nullable2(row.origin, text)
                };
              });
              const detail = nullable2(session.detail, text);
              const activity = session.activity == null ? void 0 : [
                text(session.activity[0]),
                text(session.activity[1]),
                number(session.activity[2])
              ];
              const failed = boolean(value.incomplete);
              if (failed && rows.length > 1) {
                const half = Math.ceil(rows.length / 2);
                work.push(rows.slice(0, half), rows.slice(half));
              } else incomplete ||= failed;
              for (const row of rows) {
                row.previewReady = !failed;
                row.session.detail = detail ?? void 0;
                row.session.activity = activity;
                if (row.event && events[row.event.id])
                  Object.assign(row.event, events[row.event.id], {
                    origin: events[row.event.id].origin ?? void 0
                  });
              }
              if (home === next && !disposed && ticket === epoch && host2.route() === "home") draw();
            } catch {
              if (!disposed && ticket === epoch) incomplete = true;
            }
          }
        })
      );
      return incomplete;
    }
    async function refresh(force = false) {
      if (disposed || !["home", "analytics", "machines"].includes(host2.route())) return;
      if (!force && host2.route() === "analytics" && Date.now() < analyticsRetryAt) return;
      if (loading) {
        queued = true;
        queuedForce ||= force;
        return;
      }
      const route = host2.route(), ticket = ++epoch, requestedQuery = route === "analytics" ? query() : "";
      const current = () => !disposed && ticket === epoch && host2.route() === route && (route !== "analytics" || query() === requestedQuery);
      loading = true;
      host2.root.setAttribute("aria-busy", "true");
      try {
        const args = new URLSearchParams(requestedQuery);
        if (observationAfter)
          args.set(
            "observation_after",
            JSON.stringify([observationAfter.source_key, observationAfter.catalog_key])
          );
        const value = await host2.api(
          "/api/viewer/" + (route === "analytics" ? "analytics" : "home") + (args.size ? "?" + args : ""),
          route === "analytics" ? void 0 : { seen: [...seen.SEEN_RESULTS] }
        );
        if (!current()) return;
        if (route === "analytics" && value === null) {
          const kept = analyticsCache.get(requestedQuery);
          if (!kept) throw Error("Unchanged Analytics has no accepted result");
          acceptAnalytics(kept);
          error = null;
          analyticsRetryAt = 0;
          return;
        }
        const observations2 = array(object(value).observations, (value2) => object(value2));
        const pending = array(object(value).pending_observations ?? [], parseSelection);
        if (pending.length > 8) throw Error("Pending observations exceed their bound");
        if (pending.length) observationAfter = pending.at(-1);
        const observe = async () => {
          let cursor = 0;
          await Promise.all(
            Array.from({ length: Math.min(2, pending.length) }, async () => {
              while (cursor < pending.length && current()) {
                const selected = pending[cursor++];
                try {
                  await host2.api(
                    "/api/session-observe?" + new URLSearchParams({
                      machine: selected.source_key,
                      sid: selected.catalog_key,
                      scope: "retained_history"
                    })
                  );
                } catch {
                }
              }
            })
          );
        };
        if (route !== "analytics") {
          const next = parseHome(value);
          const pages = Math.max(
            1,
            ...["inbox", "working", "answered"].map(
              (section) => Math.ceil((home?.[section].length ?? 0) / 100)
            )
          );
          for (let page = 1; page < pages; page++) {
            const args2 = new URLSearchParams();
            for (const section of ["inbox", "working", "answered"])
              if ((home?.[section].length ?? 0) > page * 100)
                args2.set(section + "_offset", String(page * 100));
            const additional = parseHome(
              await host2.api("/api/viewer/home?" + args2, { seen: [...seen.SEEN_RESULTS] })
            );
            if (disposed || ticket !== epoch || host2.route() !== route) return;
            if (additional.version !== next.version)
              throw Error("Activity changed while loading. Retrying\u2026");
            for (const section of ["inbox", "working", "answered"])
              if (args2.has(section + "_offset")) {
                next[section].push(...additional[section]);
                next.next[section] = additional.next[section];
              }
          }
          if (home?.version === next.version) {
            const previous = new Map(
              [...home.inbox, ...home.working, ...home.answered].map((row) => [
                JSON.stringify([row.source_key, row.catalog_key, row.event?.id]),
                row
              ])
            );
            for (const row of [...next.inbox, ...next.working, ...next.answered]) {
              const retained = previous.get(
                JSON.stringify([row.source_key, row.catalog_key, row.event?.id])
              );
              if (!retained || retained.contentGeneration !== row.contentGeneration) continue;
              row.previewReady = retained.previewReady;
              row.session.detail = retained.session.detail;
              if (retained.session.activity) row.session.activity = retained.session.activity;
              if (row.event && retained.event) {
                row.event.brief = retained.event.brief;
                row.event.answer = retained.event.answer;
                row.event.origin = retained.event.origin;
              }
            }
          }
          home = next;
          recentRows = next.recent;
          recentNow = next.now;
          error = observations2.length ? "Some machine history is still being observed. Retrying\u2026" : null;
          draw();
          if (route !== "home") {
            await observe();
            return;
          }
          const [previewsIncomplete] = await Promise.all([
            host2.nativeContent ? Promise.resolve(false) : hydrateHome(next, ticket),
            observe()
          ]);
          if (disposed || ticket !== epoch || host2.route() !== "home") return;
          if (previewsIncomplete) observations2.push({ state: "previews_unavailable" });
          home = next;
          recentRows = next.recent;
          recentNow = next.now;
        } else {
          const data = object(value);
          const nextAnalytics = parseAnalytics(data), nextSelections = dictionary(data.selections, parseSelection), nextLabels = dictionary(data.machine_labels, text), nextRecent = array(data.recent, parseHomeRow), nextNow = number(data.now);
          if (nextRecent.length > 60) throw Error("Recent response exceeds its bound");
          const accepted = {
            data: nextAnalytics,
            selections: nextSelections,
            labels: nextLabels,
            recent: nextRecent,
            now: nextNow
          };
          analyticsCache.delete(requestedQuery);
          analyticsCache.set(requestedQuery, accepted);
          while (analyticsCache.size > 3) analyticsCache.delete(analyticsCache.keys().next().value);
          acceptAnalytics(accepted);
          analyticsRetryAt = 0;
          draw();
          await observe();
        }
        if (!current()) return;
        error = observations2.length ? "Some machine history is still being observed. Retrying\u2026" : null;
      } catch (failure) {
        if (current()) {
          error = failure instanceof Error ? failure.message : "Overview unavailable. Retrying\u2026";
          if (route === "analytics") analyticsRetryAt = Date.now() + 1e4;
        }
      } finally {
        if (!disposed && ticket === epoch) {
          loading = false;
          draw();
          if (queued) {
            const force2 = queuedForce;
            queued = false;
            queuedForce = false;
            void refresh(force2);
          }
        }
      }
    }
    return {
      actions,
      waitingCount: () => home ? Math.max(
        0,
        home.totals.waiting - home.inbox.filter(
          (row) => row.event?.ask === "result" && (home.machines.length === 1 && seen.SEEN_RESULTS.has(row.event.id) || seen.SEEN_RESULTS.has(JSON.stringify([row.source_key, row.event.id])))
        ).length
      ) : void 0,
      recentSnapshot,
      markResults(source, ids) {
        let changed = false;
        for (const id of ids) {
          const key2 = JSON.stringify([source, id]);
          if (seen.SEEN_RESULTS.has(key2)) continue;
          seen.SEEN_RESULTS.add(key2);
          changed = true;
        }
        if (!changed) return;
        while (seen.SEEN_RESULTS.size > seen.SEEN_LIMIT)
          seen.SEEN_RESULTS.delete(seen.SEEN_RESULTS.values().next().value);
        try {
          localStorage.setItem(seen.SEEN_KEY, JSON.stringify([...seen.SEEN_RESULTS]));
        } catch {
        }
        host2.chrome();
      },
      openRecent(id) {
        const row = recentRows.find((row2) => row2.id === id);
        if (!row) return false;
        open(row);
        return true;
      },
      toggleRecent(id, value) {
        recentOpen.set(id, value);
        while (recentOpen.size > 200) recentOpen.delete(recentOpen.keys().next().value);
        try {
          localStorage.setItem(
            "semon.tree",
            JSON.stringify(Object.fromEntries([...recentOpen].map(([key2, open2]) => [key2, { open: open2 }])))
          );
        } catch {
        }
        host2.chrome();
      },
      allRecent(id, value, trigger) {
        if (value && trigger && matchMedia("(max-width: 760px)").matches) {
          let collect2 = function(parent) {
            for (const row of recentRows) {
              if (visited.has(row.id) || row.source_key !== parent.source_key || row.session.parent !== parent.catalog_key)
                continue;
              visited.add(row.id);
              rows.push(row);
              collect2(row);
            }
          };
          var collect = collect2;
          const root = recentRows.find((row) => row.id === id);
          if (!root) return;
          const rows = [], visited = /* @__PURE__ */ new Set([root.id]);
          collect2(root);
          const sheet2 = createKidsSheet(
            root.session.name,
            rows.map((row) => ({
              id: row.id,
              name: row.session.name,
              state: row.session.state,
              stateLabel: row.session.state === "wait" ? "Waiting on you" : row.session.state === "work" ? "Working" : "Recorded history",
              age: timeText(Math.max(0, recentNow - row.session.last)),
              under: row.session.parent === root.catalog_key ? void 0 : recentRows.find(
                (parent) => parent.source_key === row.source_key && parent.catalog_key === row.session.parent
              )?.session.name,
              bucket: row.session.state === "wait" ? 0 : row.session.state === "work" ? 1 : 2
            })),
            {
              opened() {
                host2.panelOpened(sheet2);
              },
              closed() {
                host2.panelClosed(sheet2);
                dialogs.delete(sheet2);
                if (trigger.isConnected) trigger.focus({ preventScroll: true });
              },
              matches(key2, query2) {
                const row = rows.find((row2) => row2.id === key2);
                return !!row && (!query2 || [row.session.name, row.session.repo, row.session.model, row.session.harness].some(
                  (field) => field?.toLowerCase().includes(query2.toLowerCase())
                ));
              },
              select(key2) {
                open(rows.find((row) => row.id === key2));
              }
            }
          );
          dialogs.add(sheet2);
          sheet2.show();
          return;
        }
        if (value) recentAll.add(id);
        else recentAll.delete(id);
        host2.chrome();
      },
      draw,
      refresh,
      leave() {
        ++epoch;
        queued = false;
        queuedForce = false;
        loading = false;
        host2.root.removeAttribute("aria-busy");
        facets.querySelector("dialog[open]")?.close();
        error = null;
        for (const value of [...dialogs]) value.destroy();
      },
      destroy() {
        disposed = true;
        ++epoch;
        for (const value of [...dialogs]) value.destroy();
        facetOwner.destroy();
        analyticsCache.clear();
        render(null, actions);
      }
    };
  }

  // src/domain/trace.ts
  var finite = (value) => typeof value === "number" && Number.isFinite(value);
  function createTraceCalculations(state2, domain, now, RUN_EXPANDED, HARNESS2, STATE2) {
    const { sessions: SESS, turns: TURNS, starts: STARTS } = state2;
    const workingEnd = (session) => session.state === "work" && session.liveObservation !== false ? now() : session.last;
    const { costForSessions, costForSession, costText, shortMoney, hcls, usageTotal, nameOf } = domain;
    const clock2 = (at) => clock(at, now()), dur2 = (from, to) => dur(from, to, now()), kindText = (s) => s.kind ?? HARNESS2[s.harness];
    const allRunNodes = (model2) => {
      const rows = [], walk = (n) => {
        rows.push(n);
        n.children.forEach(walk);
      };
      walk(model2.rootNode);
      model2.peers.forEach(walk);
      return rows;
    };
    function traceAgentTree(root) {
      const rootNode = {
        sid: root.sid,
        turn: root,
        handoff: null,
        kind: "root",
        depth: 0,
        children: []
      };
      const peers = [], handoffs = [], spawns = [], seenTurns = /* @__PURE__ */ new Set(), seenSessions = /* @__PURE__ */ new Set([root.sid]);
      const walk = (node) => {
        const turn = node.turn;
        if (!turn || seenTurns.has(turn.id)) return;
        seenTurns.add(turn.id);
        const sent = [...turn.sent ?? []].sort((a, b) => a.at - b.at);
        handoffs.push(...sent);
        const children = [];
        for (const h2 of sent) {
          if (h2.kind === "spawn") {
            spawns.push(h2);
            const childTurn = STARTS.get(h2.id);
            if (!SESS[h2.to] || seenSessions.has(h2.to)) continue;
            seenSessions.add(h2.to);
            const child = {
              sid: h2.to,
              turn: childTurn ?? null,
              handoff: h2,
              kind: "spawn",
              depth: node.depth + 1,
              children: []
            };
            node.children.push(child);
            children.push(child);
          } else if (h2.kind === "relay" && SESS[h2.to] && !seenSessions.has(h2.to)) {
            const peerTurn = STARTS.get(h2.id);
            seenSessions.add(h2.to);
            const peer = {
              sid: h2.to,
              turn: peerTurn ?? null,
              handoff: h2,
              kind: "relay",
              depth: 0,
              children: []
            };
            peers.push(peer);
            children.push(peer);
          }
        }
        for (const child of children) walk(child);
      };
      walk(rootNode);
      return { rootNode, peers, handoffs, spawns };
    }
    function agentSnapshot(root) {
      const model2 = traceAgentTree(root), session = SESS[root.sid];
      const start = root.at ?? root.start?.at ?? session.start;
      const nextTurn = (TURNS[root.sid] ?? []).find((t) => (t.at ?? 0) > start);
      const upper = root.end?.at ?? nextTurn?.at ?? workingEnd(session) ?? start;
      const end = Math.max(
        start,
        upper,
        ...model2.handoffs.map((h2) => h2.done).filter(finite),
        ...allRunNodes(model2).filter((n) => n.kind !== "root").map(
          (n) => SESS[n.sid]?.state === "work" && SESS[n.sid]?.liveObservation !== false ? now() : n.handoff?.done ?? SESS[n.sid]?.last
        ).filter(finite)
      );
      const span = Math.max(1, end - start);
      const waits = allRunNodes(model2).flatMap(
        (node) => (SESS[node.sid]?.wait_edges ?? []).filter((w) => (!w.turn || w.turn === node.turn?.id) && w.start >= start && w.start <= end).map((w) => ({ ...w, sid: node.sid }))
      );
      const critical = new Set(
        model2.spawns.filter((h2) => waits.some((w) => w.sid === h2.from && w.targets?.includes(h2.to))).map((h2) => h2.id)
      );
      const pct = (m) => Math.max(0, Math.min(100, (m - start) / span * 100));
      const steps = [1, 2, 5, 10, 15, 30, 60, 120, 240].map((n) => n * 6e4).map((step2) => ({ step: step2, count: Math.floor((span - 1) / step2) })).filter((x) => x.count >= 3 && x.count <= 6).sort((a, b) => Math.abs(a.count - 5) - Math.abs(b.count - 5) || a.step - b.step);
      const step = steps[0]?.step ?? [1, 2, 5, 10, 15, 30, 60, 120, 240].map((n) => n * 6e4).sort(
        (a, b) => Math.abs(Math.floor((span - 1) / a) - 5) - Math.abs(Math.floor((span - 1) / b) - 5)
      )[0];
      const ticks = [];
      for (let offset = step; offset < span; offset += step) ticks.push(offset);
      const rowCount = (node) => 1 + node.children.reduce((n, child) => n + rowCount(child), 0);
      const allNodes = (node, into = []) => {
        into.push(node);
        for (const child of node.children) allNodes(child, into);
        return into;
      };
      const totalRows = rowCount(model2.rootNode) + model2.peers.reduce((n, peer) => n + rowCount(peer), 0), foldRows = totalRows > 12;
      const spawned = model2.spawns.filter((h2) => Number.isFinite(h2.at) && Number.isFinite(h2.done) && h2.done > h2.at).map((h2) => ({ start: h2.at, end: h2.done }));
      const missingSpawnEnds = model2.spawns.filter((h2) => !Number.isFinite(h2.done)).length;
      const points = spawned.flatMap((x) => [
        { at: x.start, change: 1 },
        { at: x.end, change: -1 }
      ]).sort((a, b) => a.at - b.at || a.change - b.change);
      let active = 0, peak = 0;
      for (const p of points) {
        active += p.change;
        peak = Math.max(peak, active);
      }
      const totalCost = costForSessions(
        allRunNodes(model2).map((n) => SESS[n.sid]).filter(Boolean)
      );
      const participantCount = allRunNodes(model2).length;
      const summary = [
        participantCount + (participantCount === 1 ? " agent" : " agents"),
        dur2(start, end),
        costText(totalCost) + " session totals",
        ...spawned.length ? ["Recorded spawn overlap: " + peak] : [],
        ...missingSpawnEnds ? [missingSpawnEnds + " spawn end" + (missingSpawnEnds === 1 ? "" : "s") + " not recorded"] : []
      ];
      while (RUN_EXPANDED.size > 16) RUN_EXPANDED.delete(RUN_EXPANDED.keys().next().value);
      const expanded = RUN_EXPANDED.get(root.id) ?? /* @__PURE__ */ new Set();
      RUN_EXPANDED.set(root.id, expanded);
      const rows = [];
      const add = (node, depth, parent) => {
        const s = SESS[node.sid];
        if (!s) return;
        const handoff = node.handoff, from = node.kind === "root" ? start : handoff.at, to = node.kind === "root" ? end : s.state === "work" && s.liveObservation !== false ? now() : handoff.done ?? s.last ?? from;
        const duration = dur2(from, to), ownCost = costForSession(node.sid), rollup = costForSessions(
          allNodes(node).map((x) => SESS[x.sid]).filter(Boolean)
        ), status = (s.liveObservation === false && ["work", "wait"].includes(s.state) ? "Recorded: " : "") + (STATE2[s.state] ?? s.state);
        const segments = [], segment = (a, b, kind) => {
          if (b > a) segments.push({ left: pct(a), width: Math.max(0.2, pct(b) - pct(a)), kind });
        };
        const intervals = (s.busy ?? []).map(([a, b]) => [Math.max(from, a), Math.min(to, b)]).filter(([a, b]) => b > a).sort((a, b) => a[0] - b[0]);
        let cursor = from;
        for (const [a, b] of intervals) {
          if (a > cursor) segment(cursor, a, "idle");
          segment(a, b, "busy");
          cursor = Math.max(cursor, b);
        }
        if (to > cursor) segment(cursor, to, "idle");
        rows.push({
          kind: "agent",
          id: node.sid,
          depth: Math.min(3, depth),
          label: [s.name, s.kind ?? HARNESS2[s.harness], status, duration, costText(rollup)].join(
            ", "
          ),
          name: s.name,
          relay: node.kind === "relay",
          critical: !!handoff && critical.has(handoff.id),
          harnessClass: hcls(node.sid),
          model: [kindText(s), shortModel(s.model)].filter(Boolean).join(" \xB7 "),
          state: s.state,
          stateLabel: status,
          tokens: tok(allNodes(node).reduce((sum, n) => sum + usageTotal(SESS[n.sid]), 0) / 1e6) + " tokens",
          duration,
          cost: rollup.usd == null ? "\u2014" : shortMoney(rollup.usd),
          costTip: "Own cost: " + costText(ownCost),
          segments,
          spawns: node.children.map((child) => pct(child.handoff.at)),
          waits: waits.filter((w) => w.sid === node.sid).map((w) => ({
            left: pct(w.start),
            width: Math.max(0.2, pct(w.end ?? end) - pct(w.start)),
            tip: w.targets?.length ? "Waiting on " + w.targets.map(nameOf).join(", ") : "Wait without a recorded target"
          })),
          turn: node.turn?.id,
          edge: parent ? {
            parent,
            started: pct(handoff.at),
            done: pct(handoff.done ?? workingEnd(s)),
            returned: Number.isFinite(handoff.done)
          } : void 0
        });
        const folded = foldRows && node.children.length > 8 && !expanded.has(node.sid);
        for (const child of folded ? node.children.slice(0, 8) : node.children)
          add(child, Math.min(3, depth + 1), node.sid);
        if (folded) {
          const hidden = node.children.slice(8), ids = new Set(hidden.flatMap((n) => allNodes(n).map((x) => x.sid))), cost = costForSessions([...ids].map((id) => SESS[id]).filter(Boolean));
          rows.push({
            kind: "more",
            id: "more:" + node.sid,
            fold: node.sid,
            depth: Math.min(3, depth + 1),
            name: "+" + hidden.length + " more",
            label: "Show " + hidden.length + " more agents under " + s.name,
            cost: cost.usd == null ? "\u2014" : shortMoney(cost.usd),
            costTip: "Hidden agents' rollup cost: " + costText(cost)
          });
        }
      };
      add(model2.rootNode, 0);
      if (model2.peers.length) {
        rows.push({ kind: "heading", id: "relayed-heading", depth: 0, label: "", name: "" });
        for (const peer of model2.peers) add(peer, 0);
      }
      return {
        summary,
        start: clock2(start),
        end: clock2(end),
        ticks: ticks.map((offset) => ({
          left: pct(start + offset),
          label: "+" + Math.round(offset / 6e4) + "m",
          nearEnd: offset / span > 0.86
        })),
        legend: waits.some((w) => w.targets?.length) ? "Recorded waits on agents" : waits.length ? "Recorded waits without known targets" : "Wait dependencies not recorded",
        incomplete: allRunNodes(model2).some((node) => SESS[node.sid]?.wait_edges_truncated),
        rows
      };
    }
    return { traceAgentTree, agentSnapshot };
  }

  // src/domain/relationships.ts
  function parseRecordedTurn(value) {
    const row = object(value), end = object(row.end), sent = array(row.sent, text), st = text(end.st);
    if (sent.length > 1e3 || !["work", "wait", "idle", "done", "err", "off"].includes(st))
      throw Error("Invalid recorded turn");
    return {
      id: text(row.id),
      start: row.start == null ? void 0 : text(row.start),
      u: boolean(row.u),
      at: row.at == null ? void 0 : number(row.at),
      end: { st, why: text(end.why), at: end.at == null ? void 0 : number(end.at) },
      sent,
      last: boolean(row.last)
    };
  }
  function parseRecordedRelationship(value) {
    const row = object(value), kind = text(row.kind), status = text(row.status);
    if (!["work", "wait", "done", "err", "new"].includes(status))
      throw Error("Invalid recorded handoff state");
    const base = {
      id: text(row.id),
      from: text(row.from),
      to: row.to == null ? "" : text(row.to),
      at: number(row.at),
      done: row.done == null ? void 0 : number(row.done),
      status,
      brief: "",
      declined: boolean(row.declined)
    };
    let edge;
    if (kind === "toyou") {
      const ask = text(row.ask);
      if (!["question", "decision", "result"].includes(ask)) throw Error("Invalid recorded question");
      edge = { ...base, kind, ask };
    } else if (kind === "move") edge = { ...base, kind, fromMachine: "", toMachine: "" };
    else if (kind === "ask" || kind === "spawn" || kind === "relay") edge = { ...base, kind };
    else throw Error("Invalid recorded handoff");
    return {
      edge,
      briefOwner: row.brief_session == null ? void 0 : text(row.brief_session),
      resultOwner: row.result_session == null ? void 0 : text(row.result_session),
      targetTurn: row.target_turn == null ? void 0 : text(row.target_turn)
    };
  }
  function parseNativeRelationships(value, ids) {
    const data = object(value), allowed = new Set(ids);
    const rows = Object.entries(object(data.events)).map(([id, value2]) => {
      const row = object(value2);
      if (!allowed.has(id)) throw Error("Native relationship exceeds its issued selection");
      return {
        id,
        brief: row.brief == null ? void 0 : text(row.brief),
        result: row.result == null ? void 0 : text(row.result),
        answer: row.answer == null ? void 0 : Array.isArray(row.answer) ? array(row.answer, text) : text(row.answer)
      };
    });
    if (rows.length > ids.length || new Set(rows.map((row) => row.id)).size !== rows.length)
      throw Error("Repeated native relationship");
    return rows;
  }

  // src/app/tracePresentation.ts
  var optionalText = (value) => value == null ? void 0 : text(value);
  var optionalNumber = (value) => value == null ? void 0 : number(value);
  function state(value) {
    if (value === "off") return "idle";
    if (value !== "work" && value !== "wait" && value !== "idle" && value !== "done" && value !== "err")
      throw Error("Invalid recorded state");
    return value;
  }
  function parseCost(value) {
    const cost = object(value);
    return {
      usd: cost.usd == null ? null : number(cost.usd),
      unpriced_models: cost.unpriced_models == null ? [] : array(cost.unpriced_models, text)
    };
  }
  function parseCausalTrace(value, source, key2, root) {
    const data = object(value);
    if (data.api !== 1 || data.source_key !== source || data.catalog_key !== key2 || data.root !== root)
      throw Error("Causal selection changed");
    const previews = dictionary(data.previews, text);
    if (Object.keys(previews).length > 128 || Object.values(previews).some((generation2) => !/^[a-f0-9]{64}$/.test(generation2)))
      throw Error("Invalid causal preview scope");
    const sessions = dictionary(data.sessions, (value2) => {
      const row = object(value2);
      const session = {
        id: "",
        name: text(row.name),
        harness: text(row.harness),
        state: state(row.state),
        liveObservation: row.live_observation == null ? false : boolean(row.live_observation),
        machine: source,
        model: text(row.model),
        kind: optionalText(row.kind),
        start: number(row.start),
        last: number(row.last),
        cost: parseCost(row.cost),
        tokens: array(row.tokens, number),
        busy: array(row.busy, (value3) => {
          const span = array(value3, number);
          if (span.length !== 2) throw Error("Invalid busy interval");
          return [span[0], span[1]];
        }),
        wait_edges: array(row.wait_edges, (value3) => {
          const edge = object(value3);
          return {
            call: text(edge.call),
            tool: text(edge.tool),
            targets: array(edge.targets, text),
            start: number(edge.start),
            end: optionalNumber(edge.end),
            turn: optionalText(edge.turn)
          };
        }),
        wait_edges_truncated: boolean(row.wait_edges_truncated)
      };
      return session;
    });
    for (const [id, session] of Object.entries(sessions)) {
      session.id = id;
    }
    const handoffs = /* @__PURE__ */ new Map();
    const previewOwners = /* @__PURE__ */ new Map();
    for (const value2 of array(data.handoffs, object)) {
      const relationship = parseRecordedRelationship(value2), edge = relationship.edge;
      if (edge.kind === "move") {
        edge.fromMachine = source;
        edge.toMachine = source;
      }
      if (handoffs.has(edge.id)) throw Error("Duplicate causal handoff");
      handoffs.set(edge.id, edge);
      previewOwners.set(edge.id, {
        brief: relationship.briefOwner,
        result: relationship.resultOwner
      });
    }
    const turns = {}, starts = /* @__PURE__ */ new Map(), byId = /* @__PURE__ */ new Map();
    for (const value2 of array(data.turns, object)) {
      const end = object(value2.end), sid = text(value2.sid), id = text(value2.id), start = optionalText(value2.start);
      if (!sessions[sid] || byId.has(id)) throw Error("Invalid causal turn identity");
      const sent = array(value2.sent, text).map((id2) => {
        const edge = handoffs.get(id2);
        if (!edge) throw Error("Missing causal handoff");
        return edge;
      });
      const turn = {
        id,
        sid,
        start: start ? handoffs.get(start) ?? null : null,
        at: optionalNumber(value2.at),
        u: boolean(value2.u) ? { k: "u", text: "" } : null,
        entries: [],
        out: sent,
        sent,
        end: { st: state(end.st), why: text(end.why), at: optionalNumber(end.at) },
        last: boolean(value2.last)
      };
      byId.set(id, turn);
      (turns[sid] ??= []).push(turn);
      if (start) starts.set(start, turn);
    }
    const selected = byId.get(root);
    if (!selected || selected.sid !== key2 || byId.size > 100 || handoffs.size > 1e3 || Object.keys(sessions).length > 100)
      throw Error("Causal response exceeds its selection or bound");
    const frontier = array(data.frontier, (value2) => {
      const row = object(value2);
      return { catalog_key: text(row.catalog_key), turn: text(row.turn) };
    });
    if (frontier.length > 1e3) throw Error("Causal frontier exceeds its bound");
    return {
      source,
      key: key2,
      root: selected,
      sessions,
      turns,
      starts,
      handoffs,
      generation: text(data.generation),
      frontier,
      incomplete: boolean(data.incomplete),
      previewsIncomplete: boolean(data.previews_incomplete),
      previews,
      previewOwners
    };
  }
  function renderCausalTrace(root, data, now, expanded, host2) {
    const costForSessions = (sessions) => ({
      usd: sessions.some((session) => session.cost?.usd == null) ? null : sessions.reduce((sum, session) => sum + session.cost.usd, 0),
      unpriced_models: [
        ...new Set(sessions.flatMap((session) => session.cost?.unpriced_models ?? []))
      ]
    });
    const costText = (cost) => cost.usd == null ? "Cost unavailable" : "$" + cost.usd.toFixed(2);
    const { agentSnapshot } = createTraceCalculations(
      data,
      {
        costForSessions,
        costForSession: (id) => data.sessions[id]?.cost ?? { usd: null },
        costText,
        shortMoney: (value) => "$" + value.toFixed(2),
        hcls: (id) => data.sessions[id]?.harness ?? "you",
        usageTotal: (session) => (session.tokens ?? []).reduce((sum, value) => sum + value, 0) * 1e6,
        nameOf: (id) => id === "you" ? "You" : data.sessions[id]?.name ?? "Unrecorded session"
      },
      () => now,
      expanded,
      HARNESS,
      STATE
    );
    const hops = [];
    const seen = /* @__PURE__ */ new Set();
    const sessionPart = (id, turn) => ({
      className: "who",
      text: id === "you" ? "You" : data.sessions[id]?.name ?? "Unrecorded session",
      action: data.sessions[id] ? { kind: "session", id, turn } : void 0
    });
    const harness = (sid) => {
      const known = HARNESSES[data.sessions[sid]?.harness];
      return known ? {
        name: known.name,
        light: known.icon.light,
        dark: known.icon.dark,
        id: data.sessions[sid].harness,
        darkTheme: matchMedia("(prefers-color-scheme: dark)").matches
      } : void 0;
    };
    const meta = (turn) => ({
      state: turn.end?.st ?? "done",
      stateLabel: STATE[turn.end?.st ?? "done"] ?? "Recorded",
      text: turn.end?.why ?? "Recorded turn",
      session: turn.sid,
      turn: turn.id,
      name: data.sessions[turn.sid]?.name,
      harness: harness(turn.sid),
      chip: shortModel(data.sessions[turn.sid]?.model)
    });
    hops.push({
      key: "root:" + data.root.id,
      className: "k-root",
      nodeClass: "you",
      icon: I.ask,
      turn: data.root.id,
      parts: [
        sessionPart(data.root.start?.from ?? (data.root.u ? "you" : data.key)),
        { className: "verb", text: " started a turn in " },
        sessionPart(data.key, data.root.id)
      ],
      time: clock(data.root.at ?? data.sessions[data.key].start, now),
      brief: data.root.u?.text || data.root.start?.brief || void 0,
      meta: meta(data.root)
    });
    const walk = (turn) => {
      if (seen.has(turn.id)) return;
      seen.add(turn.id);
      for (const edge of turn.sent) {
        const child = data.starts.get(edge.id);
        hops.push({
          key: edge.id,
          className: "child k-" + edge.kind + " s-" + edge.status,
          nodeClass: data.sessions[edge.to]?.harness ?? "you",
          icon: I[edge.kind] ?? I.result,
          handoff: edge.id,
          turn: child?.id,
          parts: [
            sessionPart(edge.from, turn.id),
            {
              className: "verb",
              text: edge.kind === "spawn" ? " spawned " : edge.kind === "relay" ? " relayed to " : edge.kind === "toyou" ? edge.ask === "result" ? " sent a result to " : " asked " : " sent to "
            },
            sessionPart(edge.to, child?.id)
          ],
          time: clock(edge.at, now),
          brief: edge.kind === "toyou" && edge.ask === "result" ? void 0 : edge.brief || void 0,
          answers: edge.kind === "toyou" && Array.isArray(edge.answer) ? edge.answer : void 0,
          result: edge.kind === "toyou" && edge.ask === "result" ? void 0 : edge.result,
          meta: child ? meta(child) : {
            state: edge.status,
            stateLabel: STATE[edge.status] ?? edge.status,
            text: edge.declined ? "Declined" : edge.status === "done" ? "Completed" : edge.status === "wait" ? "Waiting on you" : "Recorded",
            note: edge.to !== "you" && !child ? "Its turn is not in this source publication" : void 0
          }
        });
        if (child) walk(child);
      }
    };
    walk(data.root);
    renderTraceScreen(
      root,
      {
        empty: false,
        summary: `${Object.keys(data.sessions).length} sessions \xB7 ${data.handoffs.size} handoffs` + (data.incomplete ? " \xB7 some relationship context is unavailable" : ""),
        hops,
        agents: agentSnapshot(data.root),
        observation: data.observation,
        continuations: data.frontier.map((branch) => ({
          label: data.sessions[branch.catalog_key]?.name ?? "Recorded branch",
          href: "/trace/native/" + encodeURIComponent(branch.catalog_key) + "/" + encodeURIComponent(branch.turn) + "?" + new URLSearchParams({ machine: data.source })
        }))
      },
      {
        ...host2,
        machine() {
        },
        sender() {
        },
        committed() {
        },
        fold(id) {
          const values = expanded.get(data.root.id) ?? /* @__PURE__ */ new Set();
          values.add(id);
          expanded.set(data.root.id, values);
          host2.fold();
        }
      }
    );
  }

  // src/state/catalog-capabilities.ts
  function parseCatalogCapabilities(value) {
    const row = object(value), filters = array(row.filters, text), sourceKey = text(row.source_key), metadataSearch = row.metadata_search === void 0 ? false : boolean(row.metadata_search);
    if (row.api !== 1 || row.read_contract !== "catalog-v1" || !sourceKey || row.pagination !== true || row.order !== "last_desc_key_asc" || row.full_text_search !== false || row.global_union !== false || metadataSearch !== filters.includes("q") || filters.some(
      (filter) => !["harness", "repo", "no_repo", "model"].includes(filter) && !(metadataSearch && filter === "q")
    ) || new Set(filters).size !== filters.length)
      throw new Error("Unsupported catalog read contract");
    return {
      api: 1,
      read_contract: "catalog-v1",
      source_key: sourceKey,
      selected_transcript: boolean(row.selected_transcript),
      selected_identity: boolean(row.selected_identity),
      selected_entry: boolean(row.selected_entry),
      source_candidates: row.source_candidates === void 0 ? false : boolean(row.source_candidates),
      metadata_search: metadataSearch,
      attachment: boolean(row.attachment),
      relationship_context: boolean(row.relationship_context),
      large_native_records: boolean(row.large_native_records),
      pagination: true,
      filters,
      order: "last_desc_key_asc",
      full_text_search: false,
      global_union: false
    };
  }

  // src/domain/sessionDetails.ts
  var optionalNumber2 = (value) => value == null ? void 0 : number(value);
  var nullableText2 = (value) => value == null ? null : text(value);
  function count(value) {
    const n = number(value);
    if (!Number.isSafeInteger(n) || n < 0) throw Error("Invalid recorded session count");
    return n;
  }
  function usage(value) {
    const row = object(value), result = {};
    for (const key2 of [
      "input",
      "output",
      "cache_write",
      "cache_write_5m",
      "cache_write_1h",
      "cache_read",
      "web_search"
    ]) {
      if (row[key2] == null) continue;
      const n = number(row[key2]);
      if (n < 0) throw Error("Invalid recorded usage");
      result[key2] = n;
    }
    return result;
  }
  function parseCost2(value) {
    const cost = object(value);
    const models = dictionary(cost.by_model ?? {}, (value2) => {
      const model2 = object(value2);
      return {
        usd: model2.usd == null ? null : number(model2.usd),
        tokens: usage(model2.tokens ?? {}),
        usd_by_kind: usage(model2.usd_by_kind ?? {})
      };
    });
    if (Object.keys(models).length > 256) throw Error("Recorded model breakdown exceeds its bound");
    return {
      usd: cost.usd == null ? null : number(cost.usd),
      unpriced_models: array(cost.unpriced_models ?? [], text),
      by_model: models
    };
  }
  function parseRelative(value) {
    const row = object(value);
    return {
      key: text(row.key),
      name: text(row.name),
      harness: text(row.harness),
      kind: text(row.kind),
      parent: nullableText2(row.parent),
      cost: parseCost2(row.cost)
    };
  }
  function parseSessionDetails(value) {
    const row = object(value), state2 = text(row.state);
    if (!["work", "wait", "idle", "done", "err", "off"].includes(state2))
      throw Error("Invalid recorded session state");
    const family = row.family == null ? null : object(row.family);
    if (family && (family.state !== "complete" && family.state !== "incomplete" || !Array.isArray(family.ancestors) || family.ancestors.length > 32 || !Array.isArray(family.runs) || family.runs.length > 99))
      throw Error("Recorded family exceeds its bound");
    const copilot = row.copilot == null ? null : object(row.copilot);
    const activity = row.activity == null ? null : object(row.activity);
    if (activity && text(activity.tool).length > 256) throw Error("Invalid recorded activity tool");
    const ancestors = family ? array(family.ancestors, parseRelative) : [], runs = family ? array(family.runs, parseRelative) : [], keys = [...ancestors, ...runs].map((row2) => row2.key);
    if (new Set(keys).size !== keys.length) throw Error("Invalid recorded family identity");
    return {
      state: state2,
      turns: count(row.turns),
      calls: row.calls == null ? null : count(row.calls),
      errors: row.errors == null ? null : count(row.errors),
      tool_calls: dictionary(row.tool_calls, count),
      signals: dictionary(row.signals, count),
      cost: parseCost2(row.cost),
      cwd: nullableText2(row.cwd),
      family: family ? { state: family.state, ancestors, runs } : void 0,
      copied_usage: boolean(row.copied_usage),
      live_observation: row.live_observation == null ? void 0 : boolean(row.live_observation),
      activity: activity ? { tool: text(activity.tool), since: count(activity.since) } : void 0,
      copilot: copilot ? {
        version: text(copilot.version),
        schema: count(copilot.schema),
        lifecycle: nullableText2(copilot.lifecycle),
        logical_parent: nullableText2(copilot.logical_parent),
        approvals: nullableText2(copilot.approvals)
      } : null,
      reported_runs: array(row.reported_runs, (value2) => {
        const item2 = object(value2);
        return { start: number(item2.start), cost_usd: optionalNumber2(item2.cost_usd) };
      }),
      cost_check: array(row.cost_check, (value2) => {
        const item2 = object(value2);
        return {
          start: number(item2.start),
          computed_usd: optionalNumber2(item2.computed_usd),
          reported_usd: optionalNumber2(item2.reported_usd),
          ok: item2.ok == null ? void 0 : boolean(item2.ok)
        };
      })
    };
  }

  // src/state/catalog-wire.ts
  var nullable3 = (value, parse) => value === null ? null : parse(value);
  var strings2 = (value) => array(value, text);
  function integer(value) {
    const parsed = number(value);
    if (!Number.isSafeInteger(parsed)) throw new Error("Invalid catalog integer");
    return parsed;
  }
  function freshness(value) {
    if (value === "cached" || value === "stale" || value === "incomplete" || value === "unavailable")
      return value;
    throw new Error("Invalid catalog freshness");
  }
  function readScope(value) {
    if (value === void 0 || value === "current") return "current";
    if (value === "retained_history") return value;
    throw new Error("Invalid catalog read scope");
  }
  function nativeSelection(value) {
    return value === "retired" ? value : freshness(value);
  }
  function observations(row) {
    return {
      facts_observation: row.facts_observation === void 0 ? void 0 : { state: freshness(object(row.facts_observation).state) },
      native_selection: row.native_selection === void 0 ? void 0 : row.native_selection === null ? null : { state: nativeSelection(object(row.native_selection).state) }
    };
  }
  function digest(value) {
    const bytes = array(value, integer);
    if (bytes.length !== 32 || bytes.some((byte) => byte < 0 || byte > 255))
      throw new Error("Invalid catalog source digest");
    return bytes;
  }
  function parseCatalogSource(value) {
    const source = object(value), offset = integer(source.offset);
    if (offset < 0) throw new Error("Invalid catalog source offset");
    return {
      root: text(source.root),
      path: text(source.path),
      native_id: text(source.native_id),
      offset,
      prefix_sha256: digest(source.prefix_sha256),
      tail_sha256: digest(source.tail_sha256)
    };
  }
  function sourceReference(value) {
    const row = object(value);
    return { source: parseCatalogSource(row.source), state: freshness(row.state) };
  }
  function parseCatalogSession(value) {
    const row = object(value), cost = object(row.cost), state2 = object(row.freshness), tokens2 = array(row.tokens, number);
    if (tokens2.length !== 3 || tokens2.some((token) => token < 0))
      throw new Error("Invalid catalog tokens");
    return {
      details: row.details == null ? void 0 : parseSessionDetails(row.details),
      transcript: row.transcript == null ? void 0 : (() => {
        const projection = object(row.transcript), generation2 = text(projection.generation), total = integer(projection.total);
        if (!/^[a-f0-9]{64}$/.test(generation2) || total < 0)
          throw Error("Invalid selected transcript observation");
        return { generation: generation2, total };
      })(),
      key: text(row.key),
      name: text(row.name),
      harness: text(row.harness),
      kind: text(row.kind),
      presentation_kind: row.presentation_kind == null ? null : text(row.presentation_kind),
      native_ids: strings2(row.native_ids),
      parent: nullable3(row.parent, text),
      parent_source: nullable3(row.parent_source, text),
      repo: nullable3(row.repo, text),
      branch: nullable3(row.branch, text),
      model: text(row.model),
      effort: nullable3(row.effort, text),
      start: nullable3(row.start, integer),
      last: nullable3(row.last, integer),
      tokens: tokens2,
      cost: { usd: nullable3(cost.usd, number), unpriced_models: strings2(cost.unpriced_models) },
      source_refs: array(row.source_refs, sourceReference),
      freshness: { state: freshness(state2.state) },
      ...observations(row)
    };
  }
  function searchObservation(value) {
    if (value === void 0 || value === null) return null;
    const row = object(value), fields = strings2(row.fields), candidates = integer(row.candidates);
    if (row.semantics !== "unicode_lowercase_substring" || fields.length !== 6 || new Set(fields).size !== 6 || fields.some(
      (field) => !["name", "key", "repo", "branch", "model", "harness"].includes(field)
    ) || row.candidate_budget !== 512 || row.byte_budget !== 2097152 || candidates < 0 || candidates > 512)
      throw new Error("Unsupported metadata search observation");
    return {
      semantics: "unicode_lowercase_substring",
      fields,
      partial: boolean(row.partial),
      candidates,
      index_complete: boolean(row.index_complete),
      candidate_budget: 512,
      byte_budget: 2097152
    };
  }
  function parseCatalogPage(value) {
    const page = object(value), machine = text(page.machine), info = object(page.machine_info), capabilities = object(page.capabilities), generation2 = text(page.generation), items = array(page.items, parseCatalogSession), next = nullable3(page.next_cursor, text), completeness = page.completeness === void 0 ? "complete" : object(page.completeness).state, collectionFreshness = page.freshness === "updating" ? "updating" : freshness(page.freshness);
    if (page.api !== 1 || completeness !== "partial" && completeness !== "complete" || collectionFreshness === "updating" && completeness !== "partial" || info.key !== machine || info.freshness !== "cached" || !/^[0-9a-f]{64}$/i.test(generation2) || items.length > 100 || new Set(items.map((item2) => item2.key)).size !== items.length || next !== null && (!next || next.length > 8192))
      throw new Error("Invalid catalog page");
    return {
      api: 1,
      machine,
      read_scope: readScope(page.read_scope),
      machine_info: { key: machine, label: text(info.label), freshness: "cached" },
      generation: generation2,
      observed_at: nullable3(page.observed_at, integer),
      freshness: collectionFreshness,
      completeness: { state: completeness },
      capabilities: {
        pagination: boolean(capabilities.pagination),
        filters: strings2(capabilities.filters),
        order: text(capabilities.order),
        full_text_search: boolean(capabilities.full_text_search),
        selected_session_lookup: boolean(capabilities.selected_session_lookup),
        runtime_status: boolean(capabilities.runtime_status),
        global_union: boolean(capabilities.global_union)
      },
      items,
      next_cursor: next,
      search: searchObservation(page.search)
    };
  }
  function parseCatalogIdentity(value) {
    const row = object(value), refs = array(row.source_refs, sourceReference), nativeIds = strings2(row.native_ids), nativeId = nullable3(row.native_id, text), ids = new Set(refs.map((ref) => ref.source.native_id)), generation2 = text(row.generation), state2 = object(row.freshness), sourceKey = text(row.source_key), catalogKey = text(row.catalog_key), harness = text(row.harness), qualification = row.owner_qualification;
    if (qualification !== void 0 && qualification !== "qualified" && qualification !== "provisional" || qualification === "provisional" && nativeId !== null || !sourceKey || !catalogKey || !harness || !/^[0-9a-f]{64}$/i.test(generation2) || nativeIds.some((id) => !id) || ids.has("") || new Set(nativeIds).size !== nativeIds.length || ids.size !== nativeIds.length || nativeIds.some((id) => !ids.has(id)) || nativeId !== null && (ids.size !== 1 || !ids.has(nativeId)))
      throw new Error("Invalid catalog identity");
    return {
      source_key: sourceKey,
      read_scope: readScope(row.read_scope),
      catalog_key: catalogKey,
      harness,
      native_id: nativeId,
      owner_qualification: qualification,
      native_ids: nativeIds,
      source_refs: refs,
      machine_label: nullable3(row.machine_label, text),
      generation: generation2,
      observed_at: nullable3(row.observed_at, integer),
      freshness: { state: freshness(state2.state) },
      ...observations(row)
    };
  }

  // src/state/catalog-selection.ts
  var CatalogSelectionStore = class {
    epoch = 0;
    disposed = false;
    pending = null;
    scope = null;
    identity = null;
    listeners = /* @__PURE__ */ new Set();
    begin(scope) {
      if (this.disposed) throw new Error("Catalog selection is destroyed");
      if (typeof scope.source_key !== "string" || !scope.source_key || typeof scope.catalog_key !== "string" || !scope.catalog_key)
        throw new Error("Invalid catalog selection");
      const request = Object.freeze({
        source_key: scope.source_key,
        catalog_key: scope.catalog_key,
        epoch: ++this.epoch
      });
      this.pending = request;
      if (this.scope?.source_key !== scope.source_key || this.scope?.catalog_key !== scope.catalog_key)
        this.scope = Object.freeze({ source_key: scope.source_key, catalog_key: scope.catalog_key });
      this.identity = null;
      this.changed();
      return request;
    }
    accept(request, value) {
      if (request !== this.pending || request.epoch !== this.epoch) return false;
      const identity2 = parseCatalogIdentity(value);
      if (identity2.read_scope !== "current" || identity2.native_selection?.state === "retired")
        throw new Error("Historical identity cannot supply current catalog authority");
      if (identity2.source_key !== request.source_key || identity2.catalog_key !== request.catalog_key)
        throw new Error("Catalog selection scope mismatch");
      for (const ref of identity2.source_refs) {
        Object.freeze(ref.source.prefix_sha256);
        Object.freeze(ref.source.tail_sha256);
        Object.freeze(ref.source);
        Object.freeze(ref);
      }
      Object.freeze(identity2.source_refs);
      Object.freeze(identity2.native_ids);
      Object.freeze(identity2.freshness);
      if (identity2.facts_observation) Object.freeze(identity2.facts_observation);
      if (identity2.native_selection) Object.freeze(identity2.native_selection);
      this.identity = Object.freeze(identity2);
      this.changed();
      return true;
    }
    selectedIdentity() {
      return this.identity;
    }
    /** Read intent survives a failed current identity check; it grants no native authority. */
    selectedScope() {
      return this.scope;
    }
    subscribe(listener) {
      if (this.disposed) throw new Error("Catalog selection is destroyed");
      this.listeners.add(listener);
      return () => this.listeners.delete(listener);
    }
    clear() {
      ++this.epoch;
      this.pending = null;
      this.scope = null;
      this.identity = null;
      this.changed();
    }
    destroy() {
      this.disposed = true;
      this.listeners.clear();
      this.clear();
    }
    changed() {
      for (const listener of [...this.listeners]) listener();
    }
  };

  // src/state/catalog-annotations.ts
  var integer2 = (value) => {
    const result = number(value);
    if (!Number.isSafeInteger(result) || result < 0) throw Error("Invalid annotation address");
    return result;
  };
  function parseCatalogAnnotations(value) {
    const row = object(value);
    if (row.api !== 1 || row.scope !== "retained_history" || !["errors", "signals"].includes(String(row.kind)))
      throw Error("Invalid annotation response");
    const entries = array(row.entries, (value2) => {
      const entry2 = object(value2);
      return { slot: integer2(entry2.slot) };
    });
    const first = integer2(row.first), total = integer2(row.total), generation2 = text(row.generation);
    if (entries.length > 100 || first + entries.length > total || !/^[a-f0-9]{64}$/.test(generation2) || entries.some((entry2, index) => index > 0 && entry2.slot <= entries[index - 1].slot))
      throw Error("Annotation response exceeds its bound");
    return {
      source: text(row.source_key),
      key: text(row.catalog_key),
      generation: generation2,
      kind: row.kind,
      first,
      total,
      entries
    };
  }

  // src/state/transcript-wire.ts
  var strings3 = (value) => array(value, text);
  function background(value) {
    const v = object(value), state2 = v.state;
    if (state2 !== "running" && state2 !== "unknown" && state2 !== "failed" && state2 !== "killed" && state2 !== "done")
      throw new Error("Invalid background state");
    return {
      state: state2,
      secs: optional(
        v.secs,
        (value2) => typeof value2 === "number" ? String(number(value2)) : text(value2)
      ),
      since: optional(v.since, number),
      exit: optional(v.exit, number),
      summary: optional(v.summary, text)
    };
  }
  function image(value) {
    const v = object(value), base = {
      o: number(v.o),
      b: number(v.b),
      w: optional(v.w, number),
      h: optional(v.h, number),
      type: optional(v.type, text),
      size: optional(v.size, number)
    };
    return v.na === true ? { ...base, na: true, v: optional(v.v, text) } : {
      ...base,
      na: optional(v.na, (value2) => {
        if (value2 !== false) throw new Error("Invalid image availability");
        return false;
      }),
      v: text(v.v)
    };
  }
  var diff = (value) => array(value, (value2) => {
    if (!Array.isArray(value2) || value2.length !== 2) throw new Error("Invalid diff");
    return [text(value2[0]), text(value2[1])];
  });
  function cut(value) {
    const v = object(value);
    return {
      original_tokens: optional(v.original_tokens, number),
      parts: optional(
        v.parts,
        (v2) => array(v2, (v3) => {
          const p = object(v3);
          return {
            text: optional(p.text, text),
            gap: optional(p.gap, (v4) => {
              const g = object(v4);
              return { unit: text(g.unit), n: number(g.n), of: optional(g.of, number) };
            })
          };
        })
      )
    };
  }
  function parseEntry(value) {
    const v = object(value), base = {
      img: optional(v.img, (v2) => array(v2, image)),
      turn: optional(v.turn, text),
      slot: optional(v.slot, number),
      live: optional(v.live, boolean),
      unfinished: optional(v.unfinished, boolean),
      tid: optional(v.tid, text),
      bg: optional(v.bg, background)
    };
    switch (v.k) {
      case "u":
      case "a":
        return { ...base, k: v.k, text: text(v.text), img: optional(v.img, (v2) => array(v2, image)) };
      case "think":
        return {
          ...base,
          k: "think",
          text: optional(v.text, text),
          pending: optional(v.pending, boolean),
          status: optional(v.status, text),
          secs: optional(v.secs, (v2) => typeof v2 === "string" ? v2 : number(v2))
        };
      case "h":
        return { ...base, k: "h", id: text(v.id) };
      case "end":
        return {
          ...base,
          k: "end",
          text: optional(v.text, text),
          ret: optional(v.ret, (v2) => {
            const r = object(v2);
            return {
              to: text(r.to),
              failed: optional(r.failed, boolean),
              at: optional(r.at, number)
            };
          })
        };
      case "harness":
        return { ...base, k: "harness", label: text(v.label) };
      case "signal":
        return {
          ...base,
          k: "signal",
          signal: (() => {
            const s = object(v.signal);
            return {
              kind: text(s.kind),
              tag: optional(s.tag, text),
              tool: optional(s.tool, text),
              value: optional(s.value, number),
              previous: optional(s.previous, text)
            };
          })()
        };
      case "bgend":
        return {
          ...base,
          k: "bgend",
          call: text(v.call),
          state: text(v.state),
          label: optional(v.label, text)
        };
      case "tool":
        return {
          ...base,
          k: "tool",
          name: text(v.name),
          arg: text(v.arg),
          title: optional(v.title, text),
          secs: optional(
            v.secs,
            (value2) => typeof value2 === "number" ? String(number(value2)) : text(value2)
          ),
          since: optional(v.since, number),
          exit: optional(v.exit, number),
          ok: v.ok == null ? v.ok : boolean(v.ok),
          in: optional(v.in, text),
          out: optional(v.out, text),
          cwd: optional(v.cwd, text),
          diff: optional(v.diff, diff),
          changes: optional(
            v.changes,
            (v2) => array(v2, (v3) => {
              const c = object(v3);
              return {
                path: text(c.path),
                move: optional(c.move, text),
                diff: optional(c.diff, diff)
              };
            })
          ),
          more: optional(v.more, strings3),
          script: v.script,
          scriptText: optional(v.scriptText, text),
          scriptTruncated: optional(v.scriptTruncated, boolean),
          cut: optional(v.cut, cut),
          outputTruncated: optional(v.outputTruncated, boolean)
        };
      default:
        throw new Error("Invalid transcript entry");
    }
  }

  // src/state/catalog-transcript-wire.ts
  function ordinal(value) {
    const v = number(value);
    if (!Number.isSafeInteger(v) || v < 0) throw new Error("Invalid catalog transcript ordinal");
    return v;
  }
  function entry(value) {
    const row = object(value), entryId = text(row.entry_id), slot = ordinal(row.slot), parsed = parseEntry(row);
    const provenance = row.provenance === null && parsed.k === "end" ? null : object(row.provenance);
    if (!provenance && (row.field !== void 0 || row.clipped === true))
      throw new Error("Unprovenanced boundary cannot expose native text");
    if (!entryId) throw new Error("Invalid catalog entry identity");
    let field;
    if (row.field !== void 0) {
      const reference = object(row.field);
      if (reference.name !== "text" && reference.name !== "out")
        throw new Error("Unsupported native text field");
      const layers = reference.layers ?? 1;
      if (layers !== 1 && layers !== 2) throw new Error("Unsupported native field encoding");
      field = {
        layers,
        name: reference.name,
        chunks: ordinal(reference.chunks),
        complete: boolean(reference.complete),
        provenance: reference.provenance === void 0 ? void 0 : (() => {
          const anchor = object(reference.provenance), source = parseCatalogSource(anchor.source);
          if (!provenance || JSON.stringify(source) !== JSON.stringify(parseCatalogSource(provenance.source)))
            throw Error("Native field must belong to its selected source");
          const offset = ordinal(anchor.offset), block = ordinal(anchor.block);
          if (offset > source.offset || block > 1024)
            throw Error("Invalid native field anchor");
          return {
            source,
            offset,
            block,
            native_event_id: anchor.native_event_id === null ? null : text(anchor.native_event_id)
          };
        })()
      };
    }
    let freshness2;
    if (row.freshness !== void 0) {
      const state2 = object(row.freshness).state;
      if (state2 !== "cached" && state2 !== "stale" && state2 !== "incomplete" && state2 !== "unavailable")
        throw new Error("Invalid catalog entry freshness");
      freshness2 = { state: state2 };
    }
    parsed.key = entryId;
    parsed.slot = slot;
    return {
      entry_id: entryId,
      slot,
      observed_at: row.observed_at == null ? null : number(row.observed_at),
      entry: parsed,
      clipped: row.clipped === void 0 ? void 0 : boolean(row.clipped),
      field,
      freshness: freshness2,
      native_action_text: parsed.k === "h" && row.text !== void 0 ? text(row.text) : void 0,
      provenance: provenance ? {
        source: parseCatalogSource(provenance.source),
        native_event_id: provenance.native_event_id === null ? null : text(provenance.native_event_id),
        offset: ordinal(provenance.offset),
        block: ordinal(provenance.block)
      } : null
    };
  }
  function parseCatalogTranscriptPage(value) {
    const row = object(value);
    if (!Array.isArray(row.entries) || row.entries.length > 100)
      throw new Error("Invalid catalog transcript page size");
    const identity2 = parseCatalogIdentity(row.identity), session = parseCatalogSession(row.session), projection = object(row.projection), range = object(row.range), context = object(row.relationship_context), generation2 = text(projection.generation), total = ordinal(projection.total), first = ordinal(range.first), end = ordinal(range.end), next = range.next === null ? null : ordinal(range.next), entries = array(row.entries, entry), state2 = object(row.freshness).state;
    if (row.api !== 1 || projection.version !== 5 || !/^[0-9a-f]{64}$/i.test(generation2) || session.key !== identity2.catalog_key || session.harness !== identity2.harness || end < first || end > total || end - first > 100 || entries.length !== end - first || next !== (end < total ? end : null) || entries.some((item2, i) => item2.slot !== first + i) || new Set(entries.map((item2) => item2.entry_id)).size !== entries.length || context.state !== "incomplete" && context.state !== "complete" || !Array.isArray(context.handoffs) || context.handoffs.length > 200 || state2 !== "cached" && state2 !== "stale" && state2 !== "incomplete" && state2 !== "unavailable")
      throw new Error("Invalid catalog transcript page");
    const relationships = array(context.handoffs, parseRecordedRelationship);
    const turns = array(context.turns ?? [], (value2) => {
      const row2 = object(value2);
      if (row2.sid !== identity2.catalog_key) throw Error("Invalid ranged turn owner");
      return parseRecordedTurn(row2);
    }), turnIds = new Set(entries.flatMap((item2) => item2.entry.turn ? [item2.entry.turn] : [])), starts = new Set(turns.flatMap((turn) => turn.start ? [turn.start] : []));
    if (turns.length > 100 || new Set(turns.map((turn) => turn.id)).size !== turns.length || turns.some((turn) => !turnIds.has(turn.id)))
      throw Error("Invalid ranged turn scope");
    const named = new Set(relationships.map((row2) => row2.edge.id));
    const actions = new Set(entries.flatMap((item2) => item2.entry.k === "h" ? [item2.entry.id] : []));
    if (named.size !== relationships.length || relationships.some(
      (row2) => !actions.has(row2.edge.id) && !starts.has(row2.edge.id) || row2.edge.from !== identity2.catalog_key && row2.edge.to !== identity2.catalog_key
    ) || context.state === "complete" && [...actions].some((id) => !named.has(id)) || Object.keys(object(context.sessions ?? {})).length > 201 || Object.keys(object(context.previews ?? {})).length > 128)
      throw Error("Invalid ranged relationship scope");
    return {
      api: 1,
      identity: identity2,
      session,
      projection: { version: 5, generation: generation2, total },
      range: { first, end, next },
      entries,
      freshness: { state: state2 },
      relationship_context: {
        state: context.state,
        turn_ids: array(context.turn_ids, text),
        turns,
        handoffs: relationships,
        sessions: dictionary(context.sessions ?? {}, (value2) => {
          const row2 = object(value2);
          return {
            name: text(row2.name),
            harness: text(row2.harness),
            model: text(row2.model),
            kind: row2.kind == null ? void 0 : text(row2.kind),
            state: row2.state == null ? void 0 : text(row2.state),
            start: row2.start == null ? void 0 : number(row2.start),
            last: row2.last == null ? void 0 : number(row2.last),
            calls: row2.calls == null ? void 0 : ordinal(row2.calls),
            usd: row2.usd == null ? void 0 : number(row2.usd),
            live_observation: row2.live_observation == null ? void 0 : row2.live_observation === 0 ? false : row2.live_observation === 1 ? true : boolean(row2.live_observation)
          };
        }),
        previews: dictionary(context.previews ?? {}, (value2) => {
          const generation3 = text(value2);
          if (!/^[a-f0-9]{64}$/.test(generation3)) throw Error("Invalid context generation");
          return generation3;
        })
      }
    };
  }

  // src/state/catalog-transcript.ts
  function content(item2) {
    const { freshness: freshness2, ...body } = item2;
    return JSON.stringify(body);
  }
  function emptyObservedBody(item2) {
    const entry2 = item2.entry;
    return entry2.k === "tool" ? !entry2.out : entry2.k === "h" ? !item2.native_action_text : (entry2.k === "u" || entry2.k === "a" || entry2.k === "think") && !entry2.text;
  }
  var CatalogTranscriptStore = class {
    epoch = 0;
    contentVersion = 0;
    get revision() {
      return this.contentVersion;
    }
    scope = null;
    readScope = "current";
    disposed = false;
    requests = /* @__PURE__ */ new Set();
    pages = [];
    access = /* @__PURE__ */ new Map();
    pageKey(page) {
      return `${page.range.first}:${page.range.end}`;
    }
    clock = 0;
    listeners = /* @__PURE__ */ new Set();
    select(scope, readScope2 = "current") {
      if (this.disposed) throw new Error("Catalog transcript is destroyed");
      if (typeof scope.source_key !== "string" || !scope.source_key || typeof scope.catalog_key !== "string" || !scope.catalog_key || readScope2 !== "current" && readScope2 !== "retained_history")
        throw new Error("Invalid catalog transcript scope");
      ++this.epoch;
      ++this.contentVersion;
      this.scope = { ...scope };
      this.readScope = readScope2;
      this.requests.clear();
      this.pages = [];
      this.access.clear();
      this.changed();
    }
    request(after = null, limit = 60) {
      if (this.disposed || !this.scope) throw new Error("No selected catalog transcript");
      if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100 || after !== null && (!Number.isSafeInteger(after) || after < 0))
        throw new Error("Invalid catalog transcript range");
      const request = Object.freeze({
        ...this.scope,
        epoch: this.epoch,
        after,
        limit,
        generation: this.pages[0]?.projection.generation ?? null,
        read_scope: this.readScope
      });
      this.requests.add(request);
      return request;
    }
    accept(request, value) {
      if (this.disposed || !this.requests.has(request) || request.epoch !== this.epoch) return false;
      const page = parseCatalogTranscriptPage(value);
      const generation2 = this.pages[0]?.projection.generation ?? request.generation;
      const first = request.after === null ? Math.max(0, page.projection.total - request.limit) : Math.min(request.after, page.projection.total);
      if (page.identity.source_key !== request.source_key || page.identity.catalog_key !== request.catalog_key || page.identity.read_scope !== request.read_scope || page.range.first !== first || page.range.end !== Math.min(first + request.limit, page.projection.total) || generation2 !== null && page.projection.generation !== generation2)
        throw new Error("Catalog transcript response does not match the requested range");
      this.access.set(this.pageKey(page), ++this.clock);
      const sameRange = this.pages.find(
        (old) => old.range.first === page.range.first && old.range.end === page.range.end
      );
      if (sameRange && sameRange.projection.total === page.projection.total && JSON.stringify(sameRange.entries) === JSON.stringify(page.entries)) {
        this.requests.delete(request);
        this.pages[this.pages.indexOf(sameRange)] = page;
        if (JSON.stringify(sameRange.relationship_context) !== JSON.stringify(page.relationship_context) || sameRange.session.details?.live_observation !== page.session.details?.live_observation) {
          ++this.contentVersion;
          this.changed();
        }
        return true;
      }
      const existing = /* @__PURE__ */ new Map(), ids = /* @__PURE__ */ new Map();
      for (const loaded of this.pages) {
        if (loaded.projection.total !== page.projection.total)
          throw new Error("Catalog transcript total changed within a projection");
        for (const item2 of loaded.entries) {
          existing.set(item2.slot, item2);
          ids.set(item2.entry_id, item2.slot);
        }
      }
      page.entries = page.entries.map((item2) => {
        const old = existing.get(item2.slot);
        if (old && item2.freshness?.state === "incomplete" && emptyObservedBody(item2) && old.entry_id === item2.entry_id && old.entry.k === item2.entry.k && JSON.stringify(old.provenance) === JSON.stringify(item2.provenance))
          return { ...old, freshness: item2.freshness };
        return item2;
      });
      for (const item2 of page.entries) {
        const old = existing.get(item2.slot);
        if (ids.has(item2.entry_id) && ids.get(item2.entry_id) !== item2.slot || old && content(old) !== content(item2))
          throw new Error("Catalog transcript entry changed within a projection");
      }
      if (sameRange && JSON.stringify(sameRange.entries) === JSON.stringify(page.entries)) {
        this.requests.delete(request);
        this.pages[this.pages.indexOf(sameRange)] = page;
        if (JSON.stringify(sameRange.relationship_context) !== JSON.stringify(page.relationship_context)) {
          ++this.contentVersion;
          this.changed();
        }
        return true;
      }
      const observed = new Map(page.entries.map((item2) => [item2.slot, item2]));
      this.requests.delete(request);
      this.pages = [
        ...this.pages.filter((old) => old.range.first !== first || old.range.end !== page.range.end).map((old) => ({
          ...old,
          entries: old.entries.map((item2) => observed.get(item2.slot) ?? item2)
        })),
        page
      ].sort((a, b) => a.range.first - b.range.first);
      while (this.pages.length > 5 || this.entries().length > 300) {
        const oldest = this.pages.reduce(
          (a, b) => (this.access.get(this.pageKey(a)) ?? 0) < (this.access.get(this.pageKey(b)) ?? 0) ? a : b
        );
        this.pages = this.pages.filter((value2) => value2 !== oldest);
        this.access.delete(this.pageKey(oldest));
      }
      ++this.contentVersion;
      this.changed();
      return true;
    }
    reject(request) {
      this.requests.delete(request);
    }
    retainRanges(ranges) {
      this.pages = this.pages.filter(
        (page) => ranges.some((range) => page.range.first >= range.first && page.range.end <= range.end)
      );
      const keys = new Set(this.pages.map((page) => this.pageKey(page)));
      for (const key2 of this.access.keys()) if (!keys.has(key2)) this.access.delete(key2);
      ++this.contentVersion;
      this.changed();
    }
    selectedPage() {
      return this.pages.at(-1) ?? null;
    }
    context() {
      const handoffs = /* @__PURE__ */ new Map();
      const sessions = {};
      const previews = {};
      const turns = /* @__PURE__ */ new Map();
      for (const page of this.pages) {
        for (const relationship of page.relationship_context.handoffs)
          handoffs.set(relationship.edge.id, relationship);
        Object.assign(sessions, page.relationship_context.sessions);
        Object.assign(previews, page.relationship_context.previews);
        for (const turn of page.relationship_context.turns) turns.set(turn.id, turn);
      }
      return {
        state: this.pages.some((page) => page.relationship_context.state === "incomplete") ? "incomplete" : "complete",
        turn_ids: [...new Set(this.pages.flatMap((page) => page.relationship_context.turn_ids))],
        turns: [...turns.values()],
        handoffs: [...handoffs.values()],
        sessions,
        previews
      };
    }
    loadedRanges() {
      const ranges = [];
      for (const page of this.pages) {
        const last = ranges.at(-1);
        if (last && page.range.first <= last.end) last.end = Math.max(last.end, page.range.end);
        else ranges.push({ first: page.range.first, end: page.range.end });
      }
      return ranges;
    }
    entries() {
      const bySlot = /* @__PURE__ */ new Map();
      for (const page of this.pages)
        for (const item2 of page.entries) if (!bySlot.has(item2.slot)) bySlot.set(item2.slot, item2);
      return [...bySlot.values()].sort((a, b) => a.slot - b.slot);
    }
    subscribe(listener) {
      if (this.disposed) throw new Error("Catalog transcript is destroyed");
      this.listeners.add(listener);
      return () => this.listeners.delete(listener);
    }
    destroy() {
      this.disposed = true;
      ++this.epoch;
      this.requests.clear();
      this.pages = [];
      this.access.clear();
      this.scope = null;
      this.listeners.clear();
    }
    changed() {
      for (const listener of [...this.listeners]) listener();
    }
  };

  // src/lib/catalog.tsx
  function renderCatalogList(root, snapshot, host2) {
    const filtered = !!(snapshot.query || snapshot.harness || snapshot.repo || snapshot.model);
    function commit(form) {
      const values = new FormData(form);
      host2.filters({
        q: String(values.get("q") ?? "").trim(),
        model: String(values.get("model") ?? "").trim(),
        harness: String(values.get("harness") ?? "").trim(),
        repo: values.get("no_repo") === "on" ? "__none__" : String(values.get("repo") ?? "").trim()
      });
    }
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsxs("div", { class: "ph", children: [
          /* @__PURE__ */ jsx("h1", { children: "Sessions" }),
          /* @__PURE__ */ jsxs("p", { class: "sub", children: [
            screenText(snapshot.sourceLabel),
            " \xB7 ",
            snapshot.observation ?? "Cached history"
          ] })
        ] }),
        /* @__PURE__ */ jsxs(
          "form",
          {
            class: "catalog-filters facet-filters",
            onSubmit: (event) => {
              event.preventDefault();
              commit(event.currentTarget);
            },
            children: [
              snapshot.metadataSearch && /* @__PURE__ */ jsxs("label", { class: "search catalog-query", children: [
                /* @__PURE__ */ jsx("span", { children: "Search" }),
                /* @__PURE__ */ jsx(
                  "input",
                  {
                    name: "q",
                    "aria-label": "Search session details",
                    id: "sq",
                    placeholder: "Session details\u2026",
                    value: snapshot.query ?? "",
                    onInput: (event) => {
                      if (event.currentTarget.form) commit(event.currentTarget.form);
                    }
                  }
                )
              ] }, "q"),
              /* @__PURE__ */ jsxs("label", { class: "search", children: [
                /* @__PURE__ */ jsx("span", { children: "Harness" }),
                /* @__PURE__ */ jsx(
                  "input",
                  {
                    name: "harness",
                    "aria-label": "Harness",
                    value: snapshot.harness,
                    onChange: (event) => {
                      if (event.currentTarget.form) commit(event.currentTarget.form);
                    }
                  }
                )
              ] }, "harness"),
              /* @__PURE__ */ jsxs("label", { class: "search", children: [
                /* @__PURE__ */ jsx("span", { children: "Repository" }),
                /* @__PURE__ */ jsx(
                  "input",
                  {
                    name: "repo",
                    "aria-label": "Repository",
                    value: snapshot.repo === "__none__" ? "" : snapshot.repo,
                    onChange: (event) => {
                      if (event.currentTarget.form) commit(event.currentTarget.form);
                    }
                  }
                )
              ] }, "repo"),
              /* @__PURE__ */ jsxs("label", { class: "search", children: [
                /* @__PURE__ */ jsx(
                  "input",
                  {
                    name: "no_repo",
                    type: "checkbox",
                    checked: snapshot.repo === "__none__",
                    onChange: (event) => {
                      if (event.currentTarget.form) commit(event.currentTarget.form);
                    }
                  }
                ),
                /* @__PURE__ */ jsx("span", { children: "No repository" })
              ] }, "no_repo"),
              /* @__PURE__ */ jsxs("label", { class: "search", children: [
                /* @__PURE__ */ jsx("span", { children: "Model" }),
                /* @__PURE__ */ jsx(
                  "input",
                  {
                    name: "model",
                    "aria-label": "Model",
                    value: snapshot.model ?? "",
                    onChange: (event) => {
                      if (event.currentTarget.form) commit(event.currentTarget.form);
                    }
                  }
                )
              ] }, "model"),
              /* @__PURE__ */ jsx("button", { class: "link catalog-apply", type: "submit", children: "Apply filters" })
            ]
          }
        ),
        snapshot.searchPartial && /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: "Search checked a bounded part of the index. Load more sessions to continue looking for matches." }),
        snapshot.searchIndexIncomplete && /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: "Search is incomplete while recorded sessions are being indexed." }),
        snapshot.discovering && /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: "More sessions are being discovered. This list is incomplete." }),
        snapshot.note && /* @__PURE__ */ jsxs("p", { class: "catalog-note", role: "status", children: [
          snapshot.note,
          " ",
          /* @__PURE__ */ jsx("button", { type: "button", class: "link", onClick: () => host2.retry(), children: "Retry" })
        ] }),
        snapshot.updating && /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: "Updating history\u2026" }),
        (snapshot.candidates?.length || snapshot.candidateNote || snapshot.candidateUpdating) && /* @__PURE__ */ jsxs("section", { "aria-label": "Archived sources awaiting sessions", children: [
          /* @__PURE__ */ jsx("h2", { children: "Archived sources" }),
          /* @__PURE__ */ jsx("p", { class: "catalog-note", children: "These recorded sources have not been indexed as sessions yet. Open a source to read it." }),
          snapshot.candidateNote && /* @__PURE__ */ jsxs("p", { class: "catalog-note", role: "status", children: [
            snapshot.candidateNote,
            " ",
            /* @__PURE__ */ jsx("button", { class: "link", type: "button", onClick: () => host2.candidateRetry?.(), children: "Retry archived sources" })
          ] }),
          snapshot.candidateUpdating && /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: "Preparing recorded session\u2026" }),
          snapshot.candidates?.map((candidate) => /* @__PURE__ */ jsxs(
            "button",
            {
              class: "nrow",
              type: "button",
              disabled: snapshot.candidateUpdating,
              onClick: () => host2.candidate?.(candidate),
              children: [
                /* @__PURE__ */ jsxs("span", { class: "session-row-main srow-main", children: [
                  /* @__PURE__ */ jsx("span", { class: "nm", children: screenText(candidate.native_name_hint ?? candidate.source.path) }),
                  /* @__PURE__ */ jsx("span", { class: "ag", children: "Source hint" })
                ] }),
                /* @__PURE__ */ jsxs("span", { class: "session-row-meta srow-meta for", children: [
                  screenText(candidate.source.root + " \xB7 " + candidate.source.path),
                  candidate.archive_observed_at === null ? " \xB7 Archive verification time unknown" : " \xB7 Archive verified " + new Date(candidate.archive_observed_at).toISOString()
                ] })
              ]
            },
            candidate.candidate_key
          )),
          snapshot.candidatesMore && /* @__PURE__ */ jsx(
            "button",
            {
              class: "link",
              type: "button",
              disabled: snapshot.candidateUpdating,
              onClick: () => host2.candidateMore?.(),
              children: "Load more archived sources"
            }
          )
        ] }),
        snapshot.hasReviews && /* @__PURE__ */ jsx(
          "button",
          {
            class: "link",
            type: "button",
            "data-show-approval-reviews": "",
            "aria-pressed": snapshot.reviews ?? false,
            onClick: () => host2.reviews?.(),
            children: "Show approval reviews"
          }
        ),
        /* @__PURE__ */ jsxs("div", { class: "catalog-result-header", children: [
          /* @__PURE__ */ jsxs("span", { children: [
            snapshot.items.length,
            " ",
            snapshot.items.length === 1 ? "session loaded" : "sessions loaded",
            /* @__PURE__ */ jsx("span", { class: "catalog-sort", children: " \xB7 Latest recorded activity" })
          ] }),
          filtered && /* @__PURE__ */ jsx("button", { class: "link", type: "button", onClick: () => host2.clearFilters(), children: "Clear filters" }),
          /* @__PURE__ */ jsxs("details", { class: "catalog-scope", children: [
            /* @__PURE__ */ jsx("summary", { children: "Search scope" }),
            /* @__PURE__ */ jsx("p", { class: "catalog-note", children: snapshot.metadataSearch ? "Search includes names, IDs, repositories, branches, models and harnesses. Conversation text and history across sources are unavailable." : "Text search and history across sources are unavailable." })
          ] })
        ] }),
        /* @__PURE__ */ jsx("div", { class: "session-list", "aria-label": "Recorded sessions", children: snapshot.items.map((item2) => /* @__PURE__ */ jsx(
          SessionRowView,
          {
            row: item2.presentation ?? {
              id: item2.key,
              name: item2.name,
              state: "off",
              stateLabel: "Recorded history",
              age: item2.freshness.state,
              model: item2.model,
              modelTip: item2.model,
              fields: []
            },
            host: { session: () => host2.session(item2) }
          },
          item2.key
        )) }),
        !snapshot.updating && !snapshot.items.length && !snapshot.note && !snapshot.discovering && !snapshot.searchIndexIncomplete && !snapshot.searchPartial && !snapshot.candidates?.length && !snapshot.candidateUpdating && /* @__PURE__ */ jsx("p", { class: "catalog-note", children: filtered ? "No recorded sessions match these filters." : /* @__PURE__ */ jsxs(Fragment2, { children: [
          "No recorded sessions in this source yet.",
          " ",
          /* @__PURE__ */ jsx("button", { class: "link", type: "button", onClick: () => host2.retry(), children: "Refresh history" })
        ] }) }),
        snapshot.more && /* @__PURE__ */ jsx("button", { class: "link", type: "button", disabled: snapshot.updating, onClick: () => host2.more(), children: "Load more sessions" })
      ] }),
      root
    );
    fitSessionRows(root);
  }

  // src/domain/resume.ts
  function resumeCommand(session) {
    const id = session.sessionId ?? session.id;
    if (!id.trim() || id.startsWith("-") || /[\0\r\n]/.test(id)) return null;
    const argument = /^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(id) ? id : "'" + id.replaceAll("'", `'"'"'`) + "'";
    switch (session.harness) {
      case "claude":
        return "claude --resume " + argument;
      case "codex":
        return "codex resume " + argument;
      case "copilot":
        return "copilot --resume=" + argument;
      default:
        return null;
    }
  }

  // src/app/sessionPresentation.ts
  var money2 = (value) => value == null ? "\u2014" : "$" + value.toFixed(2);
  var trademark = "Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.";
  function selectedSessionState(session) {
    const details = session.details;
    if (!details) return "off";
    return (details.state === "work" || details.state === "wait") && !details.live_observation ? "off" : details.state;
  }
  function familyCost(session) {
    const own = session.details?.cost ?? session.cost, family = session.details?.family;
    if (!family?.runs.length) return own;
    const costs = [own, ...family.runs.map((run) => run.cost)], models = {};
    for (const cost of costs)
      for (const [id, model2] of Object.entries(cost.by_model ?? {})) {
        const total = models[id] ??= { usd: 0, tokens: {}, usd_by_kind: {} };
        total.usd = total.usd == null || model2.usd == null ? null : total.usd + model2.usd;
        for (const field of ["tokens", "usd_by_kind"])
          for (const [kind, value] of Object.entries(model2[field] ?? {})) {
            const key2 = kind;
            total[field][key2] = (total[field][key2] ?? 0) + value;
          }
      }
    return {
      usd: family.state === "complete" && costs.every((cost) => cost.usd != null) ? costs.reduce((sum, cost) => sum + cost.usd, 0) : null,
      unpriced_models: [...new Set(costs.flatMap((cost) => cost.unpriced_models ?? []))],
      by_model: models
    };
  }
  function mark(harness) {
    const h2 = HARNESSES[harness];
    return h2 ? {
      id: harness,
      name: h2.name,
      light: h2.icon.light,
      dark: h2.icon.dark,
      darkTheme: matchMedia("(prefers-color-scheme: dark)").matches
    } : void 0;
  }
  function catalogSessionRow(session) {
    return {
      id: session.key,
      name: session.name,
      state: "off",
      stateLabel: "Recorded history",
      age: session.last == null ? "" : ago(session.last, Date.now()),
      harness: mark(session.harness),
      model: shortModel(session.model),
      modelTip: session.model,
      fields: [
        ...session.repo ? [
          {
            className: "repo-short",
            text: session.repo.split("/").at(-1),
            tip: session.repo,
            priority: 1,
            icon: I.repo
          }
        ] : [],
        ...session.branch ? [
          {
            className: "branch",
            text: session.branch,
            tip: session.branch,
            priority: 2,
            icon: I.branch
          }
        ] : []
      ]
    };
  }
  function selectedSessionLabels(session, machine) {
    const details = session.details, state2 = selectedSessionState(session), kind = session.presentation_kind ?? {
      subagent: "Subagent",
      "codex-run": "Codex run",
      "copilot-session": "Copilot session"
    }[session.kind], cost = familyCost(session);
    const rows = [
      {
        key: "state",
        text: STATE[state2] ?? "Recorded history",
        tip: "Recorded status \xB7 " + (details?.turns ?? 0) + " turns",
        drop: 0,
        className: "state",
        state: state2,
        stateLabel: STATE[state2] ?? "Recorded history"
      },
      {
        key: "harness",
        className: "hname",
        text: kind ?? HARNESS[session.harness] ?? session.harness,
        tip: HARNESS[session.harness] ?? session.harness,
        drop: 2
      },
      {
        key: "model",
        className: "meta-model",
        text: shortModel(session.model),
        tip: "Model: " + session.model,
        drop: 3,
        effort: session.effort ?? void 0
      }
    ];
    if (details?.errors)
      rows.push({
        key: "errors",
        text: details.errors + " failed",
        tip: "Recorded failed steps",
        drop: 0,
        className: "errs lab-errs",
        action: "errors"
      });
    if (details?.family?.runs.length)
      rows.push({
        key: "runs",
        text: details.family.runs.length + (details.family.runs.length === 1 ? " run" : " runs") + (details.family.state === "incomplete" ? "+" : ""),
        tip: "Recorded child sessions",
        drop: 1,
        action: "runs"
      });
    if (machine) rows.push({ key: "machine", text: machine, tip: "Machine: " + machine, drop: 5 });
    if (session.branch)
      rows.push({ key: "branch", text: session.branch, tip: "Branch: " + session.branch, drop: 6 });
    rows.push({
      key: "cost",
      text: money2(cost.usd),
      tip: "API-equivalent cost" + (details?.family?.runs.length ? " \xB7 this session and its runs" : " \xB7 this session"),
      drop: 7
    });
    return rows;
  }
  function selectedSessionMenu(session, machine, wide, trace) {
    const metadata = session.details, ownCost = metadata?.cost ?? session.cost, cost = familyCost(session), state2 = selectedSessionState(session), now = Date.now(), native = session.native_ids.length === 1 ? session.native_ids[0] : void 0, command = native ? resumeCommand({ harness: session.harness, id: session.key, sessionId: native }) : null, actions = [];
    if (trace)
      actions.push({ key: "trace", text: "Trace this turn", icon: I.trace, className: "menu-trace" });
    if (command) actions.push({ key: "copy", text: "Copy resume command", icon: I.copy });
    if (metadata?.errors)
      actions.push({
        key: "errors",
        text: metadata.errors + " failed",
        dot: "err",
        note: "Step through",
        className: "menu-errors"
      });
    if (metadata && Object.values(metadata.signals).some((count2) => count2 > 0))
      actions.push({
        key: "signals",
        text: "Session signals",
        icon: I.stack,
        className: "menu-signals"
      });
    actions.push({ key: "source", text: "Choose a machine", icon: I.machine });
    if (!matchMedia("(max-width: 760px)").matches)
      actions.push({ key: "wide", text: "Wide transcript", icon: I.wide, checked: wide });
    else if (metadata?.family?.runs.length)
      actions.push({
        key: "runs",
        text: "Runs \xB7 " + metadata.family.runs.length,
        icon: I.stack,
        className: "menu-runs"
      });
    const details = [
      {
        label: "Status",
        value: metadata ? (STATE[state2] ?? "Recorded history") + " \xB7 " + metadata.turns + (metadata.turns === 1 ? " turn" : " turns") : "Recorded history"
      },
      ...session.presentation_kind ? [{ label: "Kind", value: session.presentation_kind }] : [],
      {
        label: "Harness",
        value: HARNESS[session.harness] ?? session.harness,
        harness: mark(session.harness)
      },
      { label: "Model", value: session.model, mono: true },
      { label: "Machine", value: machine }
    ];
    for (const [label, value] of [
      ["Effort", session.effort],
      ["Directory", metadata?.cwd ?? session.repo],
      ["Branch", session.branch]
    ])
      if (value)
        details.push({
          label,
          value,
          mono: ["Directory", "Branch", "Session id"].includes(label)
        });
    if (metadata?.calls != null) details.push({ label: "Tool calls", value: String(metadata.calls) });
    if (metadata?.errors) details.push({ label: "Errors", value: String(metadata.errors) });
    if (metadata && Object.values(metadata.signals).some((count2) => count2 > 0))
      details.push({
        label: "Signals",
        value: Object.entries(metadata.signals).map(([kind, count2]) => kind + count2).join(" \xB7 ")
      });
    if (session.start != null)
      details.push(
        { label: "Started", value: clock(session.start, now) },
        { label: "Duration", value: dur(session.start, session.last, now) }
      );
    if (native) details.push({ label: "Session id", value: native, mono: true });
    const kinds2 = [
      ["Input", ["input"]],
      ["Output", ["output"]],
      ["Cache write", ["cache_write_5m", "cache_write_1h", "cache_write"]],
      ["Cache read", ["cache_read"]],
      ["Web search", ["web_search"]]
    ], models = Object.entries(cost.by_model ?? {}).map(([id, model2]) => ({
      id,
      rows: kinds2.flatMap(([label, keys]) => {
        const tokens2 = keys.reduce((sum, key2) => sum + (model2.tokens?.[key2] ?? 0), 0), usd = keys.reduce((sum, key2) => sum + (model2.usd_by_kind?.[key2] ?? 0), 0);
        return tokens2 || usd ? [
          {
            label,
            count: tokens2 ? compactCount(tokens2) : "",
            exact: tokens2 ? tokens2.toLocaleString() : void 0,
            cost: model2.usd == null ? "\u2014" : money2(usd)
          }
        ] : [];
      })
    }));
    const reports = metadata?.reported_runs ?? [], reported = reports.filter((row) => row.cost_usd != null), costDetails = [];
    if (metadata?.family?.runs.length)
      costDetails.push({ label: "This session", value: money2(ownCost.usd) });
    if (reports.length)
      costDetails.push({
        label: (HARNESS[session.harness] ?? session.harness) + "'s own figure",
        value: reported.length ? money2(reported.reduce((sum, row) => sum + row.cost_usd, 0)) + ", last " + reported.length + (reported.length === 1 ? " run" : " runs") : "not reported"
      });
    return {
      actions,
      path: metadata?.family?.ancestors.length ? [
        ...metadata.family.ancestors.map((row) => ({
          id: row.key,
          name: row.name,
          harness: row.harness,
          harnessName: HARNESS[row.harness] ?? row.harness
        })),
        {
          id: session.key,
          name: session.name,
          harness: session.harness,
          harnessName: HARNESS[session.harness] ?? session.harness
        }
      ] : [],
      status: [
        STATE[state2] ?? "Recorded history",
        session.presentation_kind ?? HARNESS[session.harness] ?? session.harness,
        shortModel(session.model)
      ].join(" \xB7 "),
      state: state2,
      stateLabel: STATE[state2] ?? "Recorded history",
      details,
      command: command ?? "",
      notice: trademark,
      icons: { chevron: I.chev, copy: I.copy },
      cost: {
        figure: money2(cost.usd),
        caption: "this session" + (metadata?.family?.runs.length ? " and its " + metadata.family.runs.length + (metadata.family.runs.length === 1 ? " run" : " runs") : ""),
        note: "What these tokens would cost at API rates. Subscriptions aren't billed this way." + (metadata?.copied_usage ? " Copied usage is excluded. Its original owner and the fresh usage total are unknown." : "") + (metadata?.family?.state === "incomplete" ? " The recorded family is incomplete; its total cost is unknown." : "") + (metadata?.copilot ? " Copilot usage is a cumulative saved snapshot. Fresh usage and complete cost are unknown." : "") + (cost.unpriced_models?.length ? " No price for " + cost.unpriced_models.join(", ") + "." : ""),
        details: costDetails,
        mismatch: (() => {
          const check = metadata?.cost_check.slice().reverse().find(
            (row) => row.ok === false && row.reported_usd != null && row.reported_usd > 0 && row.computed_usd != null
          );
          if (!check) return void 0;
          const difference = (check.computed_usd / check.reported_usd - 1) * 100;
          return "Semon's estimate for that run is " + Math.round(Math.abs(difference)) + "% " + (difference > 0 ? "above" : "below") + " " + (HARNESS[session.harness] ?? session.harness) + "'s figure.";
        })(),
        runs: (metadata?.family?.runs ?? []).map((run) => ({
          id: run.key,
          depth: run.parent === session.key ? 0 : 1,
          label: "Open " + run.name,
          name: run.name,
          kind: run.kind,
          state: "off",
          stateLabel: "Recorded history",
          cost: money2(run.cost.usd)
        })),
        models,
        includesRuns: !!metadata?.family?.runs.length
      }
    };
  }

  // src/navigation/routes.ts
  var NavigationController = class {
    constructor(observe) {
      this.observe = observe;
    }
    observe;
    currentRoute = { v: "home" };
    routeListeners = /* @__PURE__ */ new Set();
    disposed = false;
    get route() {
      return this.currentRoute;
    }
    set route(value) {
      if (this.disposed) return;
      this.currentRoute = value;
      this.observe?.(value);
      for (const listener of this.routeListeners) listener();
    }
    subscribeRoute(listener) {
      if (!this.disposed) this.routeListeners.add(listener);
      return () => this.routeListeners.delete(listener);
    }
    destroy() {
      this.disposed = true;
      this.routeListeners.clear();
    }
  };

  // src/app/controlObservation.ts
  function createControlObservation(host2) {
    if (!host2.viewerHost?.catalogControlStream) return { destroy() {
    } };
    let stream = null;
    let identity2 = "";
    let timer;
    let deadline;
    let delay = 1e3;
    let disposed = false;
    function selected() {
      const route2 = host2.navigation.route;
      if (host2.catalogSelection) {
        const selected2 = host2.catalogSelection.selectedIdentity();
        const accepted = route2.v === "session" && selected2?.catalog_key === route2.id && selected2.owner_qualification !== "provisional" && selected2.read_scope !== "retained_history" ? selected2 : null;
        return {
          path: accepted?.native_id ? host2.viewerHost?.catalogControlStream : void 0,
          params: accepted ? {
            source_key: accepted.source_key,
            catalog_key: accepted.catalog_key,
            native_id: accepted.native_id ?? "",
            harness: accepted.harness
          } : {},
          thread: accepted?.native_id ?? ""
        };
      }
      return { path: void 0, params: {}, thread: "" };
    }
    function cancel() {
      stream?.close();
      stream = null;
      host2.scope.clearTimeout(timer);
      host2.scope.clearTimeout(deadline);
    }
    function connect() {
      if (disposed) return;
      cancel();
      const scope = selected();
      identity2 = JSON.stringify(scope);
      const path = scope.path;
      if (!path || typeof EventSource === "undefined") return;
      if (!path.startsWith("/") || path.startsWith("//") || /[\\\s]/.test(path))
        throw new Error("Invalid control stream");
      const current = new EventSource(
        path + (path.includes("?") ? "&" : "?") + new URLSearchParams(scope.params)
      );
      stream = current;
      let revision = "";
      const valid = () => !disposed && stream === current && identity2 === JSON.stringify(selected());
      const recover = () => {
        if (!valid()) return;
        cancel();
        host2.controlOwner.unavailable();
        timer = host2.scope.timeout(connect, delay);
        delay = Math.min(15e3, delay * 2);
      };
      const heartbeat = () => {
        if (!valid()) return;
        host2.scope.clearTimeout(deadline);
        deadline = host2.scope.timeout(recover, 3e4);
      };
      heartbeat();
      current.addEventListener("heartbeat", heartbeat);
      current.addEventListener("error", recover);
      current.addEventListener("unavailable", recover);
      current.addEventListener("ended", () => {
        if (!valid()) return;
        cancel();
        host2.controlOwner.observe(null);
      });
      current.addEventListener("control", (event) => {
        if (!valid() || !(event instanceof MessageEvent)) return;
        try {
          if (String(event.data).length > 3e5) throw new Error("Control snapshot too large");
          const value = JSON.parse(String(event.data));
          if (!value || typeof value !== "object" || !("revision" in value) || typeof value.revision !== "string" || value.revision.length < 1 || value.revision.length > 128 || !Object.entries(scope.params).every(
            ([key2, expected]) => key2 in value && value[key2] === expected
          ) || !("control" in value))
            throw new Error("Control identity changed");
          const control = host2.controlOwner.prepare(value.control);
          if (control && control.thread !== scope.thread) throw new Error("Control thread changed");
          heartbeat();
          delay = 1e3;
          if (revision === value.revision) return;
          revision = value.revision;
          if (control || !scope.thread) host2.controlOwner.observe(control);
          else host2.controlOwner.unavailable();
        } catch {
          recover();
        }
      });
    }
    function changed() {
      if (JSON.stringify(selected()) === identity2) return;
      host2.controlOwner.observe(null);
      connect();
    }
    const route = host2.navigation.subscribeRoute(changed);
    const catalog = host2.catalogSelection?.subscribe(changed);
    connect();
    return {
      destroy() {
        disposed = true;
        route();
        catalog?.();
        cancel();
      }
    };
  }

  // src/lib/runtimeObservation.ts
  function parseRuntimeObservation(value) {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new Error("Invalid readonly runtime observation");
    const row = value;
    if (typeof row.state !== "string" || !["active", "disconnected", "failed", "ended", "unavailable"].includes(row.state) || typeof row.phase !== "string" || row.phase.length > 64 || typeof row.freshness !== "string" || !["current", "updating", "stale", "unavailable"].includes(row.freshness) || row.presence !== void 0 && (typeof row.presence !== "string" || !["active", "absent", "paused", "transitioning", "failed", "unknown"].includes(
      row.presence
    )) || row.observedAt !== void 0 && row.observedAt !== null && (typeof row.observedAt !== "string" || row.observedAt.length > 64 || !Number.isFinite(Date.parse(row.observedAt))) || row.observationError !== void 0 && row.observationError !== null && (typeof row.observationError !== "string" || row.observationError.length > 512) || row.updating !== void 0 && typeof row.updating !== "boolean")
      throw new Error("Invalid readonly runtime observation");
    return Object.freeze({
      state: row.state,
      phase: row.phase,
      freshness: row.freshness,
      ...row.presence === void 0 ? {} : { presence: row.presence },
      ...row.observedAt === void 0 ? {} : { observedAt: row.observedAt },
      ...row.observationError === void 0 ? {} : { observationError: row.observationError },
      ...row.updating === void 0 ? {} : { updating: row.updating }
    });
  }

  // src/app/runtimeObservation.ts
  function createRuntimeObservation(host2) {
    let disposed = false;
    let stream = null;
    let cleanups2 = [];
    let ticket = 0;
    let key2 = "";
    let frame = null;
    let timer;
    let deadline;
    let delay = 1e3;
    const listeners = /* @__PURE__ */ new Set();
    const selected = () => {
      const selected2 = host2.catalogRuntimeSelection?.selectedScope();
      const route = host2.navigation.route;
      return route.v === "session" && selected2?.catalog_key === route.id ? selected2 : null;
    };
    const selectionKey = (scope) => scope ? JSON.stringify([scope.source_key, scope.catalog_key]) : "";
    function publish(value) {
      frame = value ? Object.freeze(value) : null;
      for (const listener of listeners) listener();
    }
    function cancel() {
      ++ticket;
      for (const cleanup of cleanups2) cleanup();
      cleanups2 = [];
      stream?.close();
      stream = null;
      host2.scope.clearTimeout(timer);
      timer = void 0;
      host2.scope.clearTimeout(deadline);
      deadline = void 0;
    }
    function disconnected(reason, permanent = false) {
      cancel();
      if (frame)
        publish({
          ...frame,
          delivery: permanent ? "unavailable" : frame.observation ? "stale" : "unavailable",
          reason
        });
      if (!permanent) {
        const generation2 = ticket;
        timer = host2.scope.timeout(() => {
          if (!disposed && generation2 === ticket) connect();
        }, delay);
        delay = Math.min(15e3, delay * 2);
      }
    }
    function connect() {
      if (disposed) return;
      cancel();
      const scope = selected();
      const path = host2.viewerHost?.catalogRuntimeStream;
      key2 = selectionKey(scope);
      if (!scope || !path || !safePath(path)) {
        publish(null);
        return;
      }
      if (!frame || selectionKey(frame) !== key2)
        publish({
          ...scope,
          revision: null,
          observation: null,
          reason: "Checking environment status\u2026",
          delivery: "updating"
        });
      else publish({ ...frame, delivery: "updating" });
      const generation2 = ticket;
      const query = new URLSearchParams({
        source_key: scope.source_key,
        catalog_key: scope.catalog_key
      });
      let current;
      try {
        current = new EventSource(path + (path.includes("?") ? "&" : "?") + query);
      } catch {
        disconnected("Environment status is unavailable. Retrying.");
        return;
      }
      stream = current;
      const valid = () => !disposed && ticket === generation2 && stream === current && key2 === selectionKey(selected());
      const recover = () => {
        if (valid()) disconnected("Environment status updates were interrupted. Retrying.");
      };
      const heartbeat = () => {
        if (!valid()) return;
        host2.scope.clearTimeout(deadline);
        deadline = host2.scope.timeout(recover, 3e4);
      };
      function listen(type, listener) {
        current.addEventListener(type, listener);
        cleanups2.push(() => current.removeEventListener(type, listener));
      }
      heartbeat();
      listen("heartbeat", heartbeat);
      listen("error", recover);
      listen("unavailable", recover);
      listen("ended", () => {
        if (valid())
          disconnected(
            "Access changed. Sign in or select an authorized workspace to inspect status.",
            true
          );
      });
      listen("runtime", (event) => {
        if (!valid() || !(event instanceof MessageEvent)) return;
        try {
          if (String(event.data).length > 16384) throw new Error("Runtime status too large");
          const value = JSON.parse(String(event.data));
          if (!value || typeof value !== "object" || Array.isArray(value))
            throw new Error("Invalid runtime envelope");
          const row = value;
          if (row.source_key !== scope.source_key || row.catalog_key !== scope.catalog_key || typeof row.revision !== "string" || !/^[0-9a-f]{64}$/.test(row.revision) || !(row.reason === null || row.reason === void 0 || typeof row.reason === "string" && row.reason.length <= 512) || row.runtime === void 0)
            throw new Error("Runtime selection changed");
          const observation = row.runtime === null ? null : parseRuntimeObservation(row.runtime);
          heartbeat();
          delay = 1e3;
          if (frame?.revision === row.revision && frame.delivery === "current") return;
          publish({
            ...scope,
            revision: row.revision,
            observation,
            reason: typeof row.reason === "string" ? row.reason : null,
            delivery: "current"
          });
        } catch {
          recover();
        }
      });
    }
    const reselect = () => {
      const next = selectionKey(selected());
      if (next === key2) return;
      cancel();
      key2 = next;
      delay = 1e3;
      publish(null);
      connect();
    };
    const unsubscribe = host2.catalogRuntimeSelection?.subscribe(reselect) ?? (() => {
    });
    const unnavigate = host2.navigation.subscribeRoute(reselect);
    host2.scope.listen(document, "semon:refresh", () => {
      delay = 1e3;
      connect();
    });
    connect();
    return {
      view: () => frame,
      subscribe(listener) {
        if (disposed) return () => {
        };
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      destroy() {
        if (disposed) return;
        disposed = true;
        cancel();
        unsubscribe();
        unnavigate();
        listeners.clear();
        frame = null;
      }
    };
  }

  // src/app/localControl.ts
  function record3(value) {
    return !!value && typeof value === "object" && !Array.isArray(value);
  }
  function createLocalControl(scope, refresh) {
    const drafts = /* @__PURE__ */ new Map();
    let current = null;
    let busy2 = false;
    let note = "";
    let uncertain = false;
    let revision = 0;
    let selection = 0;
    async function write(op, extra = {}) {
      if (!current || busy2 || uncertain && op !== "reconnect") return false;
      if (op === "send" && (!current.connected || !(current.activeTurn ? current.capabilities.steer : current.capabilities.input)))
        return false;
      if (op === "reconnect" && current.runtime?.reconnectable === false) return false;
      if (op !== "reconnect" && !current.connected) return false;
      if (op === "interrupt" && !current.capabilities.interrupt) return false;
      if (op === "answer") {
        const request = current.requests.find(
          (request2) => request2.id === extra.request && request2.hash === extra.hash
        );
        if (!request || request.state.state !== "open") return false;
        const supported = request.kind === "question" ? current.capabilities.questions : request.payload.method === "item/fileChange/requestApproval" ? current.capabilities.fileApproval : current.capabilities.commandApproval;
        if (!supported) return false;
      }
      if (op === "send" && (!current.connected || !(current.activeTurn ? current.capabilities.steer : current.capabilities.input)))
        return false;
      const target = current;
      const at = ++revision;
      busy2 = true;
      note = "Sending\u2026";
      refresh();
      const controller = scope.request();
      try {
        const command = {
          id: crypto.randomUUID().replaceAll("-", ""),
          op,
          thread: target.thread,
          generation: target.generation,
          activeTurn: target.activeTurn,
          expires: Date.now() + 25e3,
          ...extra
        };
        const response = await fetch("/api/control", {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(command),
          signal: controller.signal
        });
        const receipt = await response.json();
        if (at !== revision) return false;
        if (op === "reconnect" && response.ok) uncertain = false;
        if (!response.ok || record3(receipt) && receipt.error) {
          note = record3(receipt) && typeof receipt.error === "string" ? receipt.error : "Control refused. Inspect the session before sending again.";
          uncertain = uncertain || note.includes("unknown");
          return false;
        } else {
          if (op === "send") {
            const snapshot = record3(receipt) && record3(receipt.snapshot) ? receipt.snapshot : null;
            const action = snapshot && record3(snapshot.actions) ? snapshot.actions[command.id] : null;
            if (!record3(action) || action.delivery !== "accepted") {
              uncertain = true;
              note = "Delivery unconfirmed. Inspect the session before sending again.";
              return false;
            }
          }
          note = op === "send" ? "" : op === "answer" ? "Response sent. Codex resolution means answered or cleared; the winning client is unknown." : "Native request accepted. Inspect the session for its outcome.";
          return true;
        }
      } catch {
        if (at === revision) {
          uncertain = true;
          note = "Delivery unknown. Inspect Codex and reconnect; this request will not be resent.";
        }
        return false;
      } finally {
        scope.releaseRequest(controller);
        if (at === revision) {
          busy2 = false;
          refresh();
        }
      }
    }
    function view(sid, draftKey = sid ?? current?.thread ?? "") {
      if (!current || sid && sid !== current.thread) return void 0;
      const target = current;
      const selected = selection;
      const send = (op, extra = {}) => selection === selected && current?.thread === target.thread && current.generation === target.generation ? write(op, extra) : Promise.resolve(false);
      return {
        get draft() {
          return drafts.get(draftKey) ?? "";
        },
        setDraft(text2) {
          drafts.set(draftKey, text2);
        },
        snapshot: current,
        busy: busy2,
        uncertain,
        note,
        send: (text2) => send("send", { text: text2 }),
        interrupt: () => void send("interrupt"),
        answer: (request, answer) => void send("answer", { request: request.id, hash: request.hash, answer }),
        reconnect: () => void send("reconnect")
      };
    }
    function apply(value) {
      if (current?.thread !== value?.thread) {
        ++selection;
        ++revision;
        busy2 = false;
        uncertain = false;
        note = "";
      }
      if (value?.runtime?.state === "ended" && current?.runtime?.state !== "ended") {
        ++revision;
        busy2 = false;
        note = "";
      }
      current = value;
    }
    return {
      readOnly(sid, draftKey = sid) {
        return {
          ...unavailableControl(sid),
          get draft() {
            return drafts.get(draftKey) ?? "";
          },
          setDraft(text2) {
            drafts.set(draftKey, text2);
          }
        };
      },
      prepare: parseControl,
      adopt: apply,
      unavailable() {
        if (!current) return;
        current = {
          ...current,
          connected: false,
          capabilities: {
            ...current.capabilities,
            input: false,
            steer: false,
            interrupt: false,
            commandApproval: false,
            fileApproval: false,
            questions: false
          },
          runtime: current.runtime ? { ...current.runtime, freshness: "stale" } : void 0,
          reason: current.runtime?.state === "ended" || current.runtime?.state === "failed" ? current.reason : "Current connection status is unavailable. Reconnecting\u2026"
        };
        refresh();
      },
      observe(value) {
        apply(value);
        refresh();
      },
      view,
      destroy() {
        ++selection;
        ++revision;
        current = null;
      }
    };
  }

  // src/app/effects.ts
  var EffectScope = class {
    cleanups = /* @__PURE__ */ new Set();
    requests = /* @__PURE__ */ new Map();
    timers = /* @__PURE__ */ new Map();
    frames = /* @__PURE__ */ new Map();
    disposed = false;
    own(cleanup) {
      if (this.disposed) cleanup();
      else this.cleanups.add(cleanup);
      return cleanup;
    }
    listen(target, type, listener, options2) {
      if (this.disposed) return;
      const guarded = (event) => {
        if (!this.disposed) listener(event);
      };
      target.addEventListener(type, guarded, options2);
      this.own(() => target.removeEventListener(type, guarded, options2));
    }
    retire(collection, id) {
      const cleanup = collection.get(id);
      if (cleanup) this.cleanups.delete(cleanup);
      collection.delete(id);
    }
    timeout(callback, delay = 0) {
      if (this.disposed) return 0;
      const id = window.setTimeout(() => {
        this.retire(this.timers, id);
        if (!this.disposed) callback();
      }, delay);
      this.timers.set(
        id,
        this.own(() => {
          this.retire(this.timers, id);
          window.clearTimeout(id);
        })
      );
      return id;
    }
    clearTimeout(id) {
      if (id !== void 0) this.timers.get(id)?.();
    }
    interval(callback, delay) {
      if (this.disposed) return 0;
      const id = window.setInterval(() => {
        if (!this.disposed) callback();
      }, delay);
      this.timers.set(
        id,
        this.own(() => {
          this.retire(this.timers, id);
          window.clearInterval(id);
        })
      );
      return id;
    }
    clearInterval(id) {
      if (id !== void 0) this.timers.get(id)?.();
    }
    frame(callback) {
      if (this.disposed) return 0;
      const id = requestAnimationFrame((time) => {
        this.retire(this.frames, id);
        if (!this.disposed) callback(time);
      });
      this.frames.set(
        id,
        this.own(() => {
          this.retire(this.frames, id);
          cancelAnimationFrame(id);
        })
      );
      return id;
    }
    cancelFrame(id) {
      if (id !== void 0) this.frames.get(id)?.();
    }
    request() {
      const controller = new AbortController();
      if (this.disposed) {
        controller.abort();
        return controller;
      }
      const aborted = () => this.releaseRequest(controller), cleanup = this.own(() => controller.abort());
      const release = () => {
        controller.signal.removeEventListener("abort", aborted);
        this.cleanups.delete(cleanup);
        this.requests.delete(controller);
      };
      this.requests.set(controller, release);
      controller.signal.addEventListener("abort", aborted, { once: true });
      return controller;
    }
    releaseRequest(controller) {
      this.requests.get(controller)?.();
    }
    destroy() {
      if (this.disposed) return;
      this.disposed = true;
      for (const cleanup of this.cleanups) cleanup();
      this.cleanups.clear();
      this.requests.clear();
    }
  };

  // src/domain/toolNames.ts
  var RUN = ["run", "Ran", "ran", "command", "commands"];
  var FIND = ["find", "Searched for", "searched", "time", "times"];
  var TOOLS = {
    Bash: RUN,
    shell: RUN,
    exec_command: RUN,
    local_shell: RUN,
    write_stdin: ["run", "Sent input to", "sent input to", "time", "times"],
    Grep: FIND,
    Glob: FIND,
    Read: ["read", "Read", "read", "file", "files"],
    Edit: ["edit", "Edited", "edited", "file", "files"],
    MultiEdit: ["edit", "Edited", "edited", "file", "files"],
    Write: ["edit", "Wrote", "wrote", "file", "files"],
    apply_patch: ["edit", "Patched", "patched", "file", "files"],
    NotebookEdit: ["edit", "Edited", "edited", "notebook", "notebooks"],
    AskUserQuestion: ["q", "Asked you", "asked you", "question", "questions"],
    ToolSearch: ["find", "Loaded", "loaded", "tool", "tools"],
    SendMessage: ["out", "Sent", "sent", "message", "messages"],
    SendUserFile: ["out", "Sent you", "sent you", "file", "files"],
    Agent: ["out", "Started", "started", "agent", "agents"],
    Task: ["out", "Started", "started", "agent", "agents"],
    Monitor: ["now", "Watched", "watched", "process", "processes"],
    ScheduleWakeup: ["now", "Scheduled", "scheduled", "wake-up", "wake-ups"],
    TaskStop: ["x", "Stopped", "stopped", "task", "tasks"],
    Artifact: ["ext", "Published", "published", "page", "pages"],
    WebFetch: ["ext", "Fetched", "fetched", "page", "pages"],
    WebSearch: ["search", "Searched the web for", "searched the web", "time", "times"],
    Skill: ["stack", "Used skill", "used", "skill", "skills"]
  };
  var providers = {
    github: "GitHub",
    figma: "Figma",
    google_drive: "Google Drive",
    notion: "Notion",
    slack: "Slack",
    gmail: "Gmail",
    railway: "Railway",
    linear: "Linear",
    calendar: "Calendar",
    search_service: "Search"
  };
  function toolInfo(name) {
    if (TOOLS[name]) return TOOLS[name];
    let provider, action;
    const dotted = /^codex_apps\.([^.]+)\.(.+)$/.exec(name);
    const double = /^mcp__(.+?)__(.+)$/.exec(name);
    if (dotted) [, provider, action] = dotted;
    else if (double?.[1] === "codex_apps") {
      const key2 = Object.keys(providers).sort((a, b) => b.length - a.length).find((key3) => double[2].startsWith(key3 + "_"));
      if (key2) {
        provider = key2;
        action = double[2].slice(key2.length + 1);
      }
    } else if (double) {
      [, provider, action] = double;
    }
    const known = provider && providers[provider.toLowerCase().replace(/^claude_ai_/, "")];
    if (known && action) {
      const label = known + " \xB7 " + action.replace(/[_.]+/g, " ").trim();
      return ["ext", label, label, "time", "times"];
    }
    return dotted || double ? ["ext", name, name, "time", "times"] : ["run", name, name, "step", "steps"];
  }

  // src/domain/thinking.ts
  function coalesceRecordedThoughts(entries, harness, knownTurns) {
    if (harness !== "codex") return entries;
    const out = [];
    let chain = null;
    let owner = null, sawOwner = false, unresolved = false;
    for (const item2 of entries) {
      const entry2 = item2.entry;
      if (entry2.turn !== void 0) {
        owner = knownTurns.has(entry2.turn) ? entry2.turn : null;
        unresolved = owner === null;
        if (owner) sawOwner = true;
      }
      if (entry2.k !== "think" || entry2.pending || !entry2.text || unresolved || !owner && sawOwner) {
        out.push(item2);
        chain = null;
        continue;
      }
      if (chain && chain.owner === owner && (entry2.text === chain.text || entry2.text.startsWith(chain.text + "\n") || entry2.text.startsWith(chain.text + "\r\n"))) {
        const prior = out[chain.index];
        if (prior.entry.k !== "think") throw Error("Invalid recorded thought chain");
        out[chain.index] = {
          ...prior,
          entry: { ...prior.entry, text: entry2.text, secs: entry2.secs }
        };
        chain.text = entry2.text;
      } else {
        out.push(item2);
        chain = { index: out.length - 1, owner, text: entry2.text };
      }
    }
    return out;
  }
  function recordedThoughtLabel(seconds) {
    const match = /^(\d+(?:\.\d+)?)s?$/.exec(String(seconds ?? ""));
    const n = match ? Math.round(Number(match[1])) : 0;
    return n < 1 ? "Thinking" : "Thinking \xB7 " + (n >= 60 ? Math.floor(n / 60) + "m " + n % 60 + "s" : n + "s");
  }

  // src/app/catalogTranscriptView.ts
  function recordedSignalLabel(entry2) {
    const s = entry2.signal, tag = s.tag ?? "";
    if (s.kind === "compact")
      return "Context compacted" + (s.value == null ? "" : " \xB7 " + compactCount(s.value) + " tokens before");
    if (s.kind === "interrupt") return "Interrupted by you";
    if (s.kind === "denial") return (s.tool ?? "Tool call") + " denied";
    if (s.kind === "hook")
      return "Hook " + (tag.includes("block") ? "blocked" : tag || "ran") + (s.tool ? " " + s.tool : "");
    const name = {
      model: "Model",
      effort: "Effort",
      approval: "Approval policy",
      sandbox: "Sandbox",
      permission: "Permission mode"
    }[s.kind] ?? "Signal";
    return name + (s.previous ? " " + s.previous + " \u2192 " : " \u2192 ") + tag;
  }
  function recordedBackgroundStatus(bg) {
    if (bg.state === "running") return "running " + (bg.secs ?? "");
    if (bg.state === "unknown") return "no end recorded";
    const outcome = bg.state === "failed" ? "failed" : bg.state === "killed" ? "stopped" : bg.exit != null ? "exit " + bg.exit : "done";
    return outcome + (bg.secs ? " \xB7 " + bg.secs : "");
  }
  function gatherTools(entries) {
    const output = [];
    let run = [];
    const flush = () => {
      if (!run.length) return;
      const ends = run.filter((row) => row.kind === "background");
      const counts = /* @__PURE__ */ new Map();
      for (const row of run) {
        if (row.kind !== "tool" || ends.some((end) => end.call === row.step.tid)) continue;
        const [, , verb, one, many] = toolInfo(row.step.data.name), count2 = counts.get(verb) ?? { n: 0, one, many };
        count2.n++;
        counts.set(verb, count2);
      }
      const text2 = [...counts].map(([verb, count2]) => verb + " " + count2.n + " " + (count2.n === 1 ? count2.one : count2.many)).join(", ");
      const tools = run.filter((row) => row.kind === "tool");
      const finished = ends.length ? "Finished " + ends.length + " background command" + (ends.length === 1 ? "" : "s") : "";
      const failures = ends.filter((row) => row.failed).length, stopped = ends.filter((row) => row.state === "killed").length;
      const outcome = [failures ? failures + " failed" : "", stopped ? stopped + " stopped" : ""].filter(Boolean).join(", ");
      output.push({
        kind: "group",
        key: "g:" + run[0].key,
        entries: run,
        lone: run.length === 1,
        summary: ends.length ? [
          { text: finished },
          ...outcome ? [{ text: " (" + outcome + ")", className: "tt-counts" }] : [],
          ...text2 ? [{ text: ", " + text2 }] : []
        ] : run.length > 1 ? [{ text: text2[0].toUpperCase() + text2.slice(1) }] : [],
        background: ends.length > 0,
        failed: tools.filter(
          (row) => row.step.data.ok === false && !ends.some((end) => end.call === row.step.tid)
        ).length,
        running: tools.find((row) => row.step.running)?.step.backgroundStatus?.replace(/^running /, "") ?? tools.find((row) => row.step.running)?.step.status ?? void 0,
        stack: I.stack,
        chevron: I.chev
      });
      run = [];
    };
    for (const entry2 of entries) {
      if (entry2.kind === "tool" || entry2.kind === "background") run.push(entry2);
      else if (entry2.kind === "label" && ["signal-marker", "vnote"].includes(entry2.className))
        output.push(entry2);
      else {
        flush();
        output.push(entry2);
      }
    }
    flush();
    return output;
  }
  function catalogTranscriptBlocks(entries, fieldView, relationshipView, liveObservation = false, context, attachments, harness = "", sessionKey = "", activity) {
    entries = coalesceRecordedThoughts(
      entries,
      harness,
      new Set(context?.turns.map((turn) => turn.id) ?? [])
    );
    const closingBodies = /* @__PURE__ */ new Map(), consumed = /* @__PURE__ */ new Set();
    if (relationshipView)
      for (const item2 of entries) {
        if (item2.entry.k !== "h") continue;
        const recorded = item2.entry;
        const edge = context?.handoffs.find((row) => row.edge.id === recorded.id)?.edge;
        if (edge?.kind !== "toyou" || edge.ask !== "result" || edge.from !== sessionKey || !context?.turns.some((turn) => turn.id === item2.entry.turn && turn.sent.includes(edge.id)))
          continue;
        const body = [...entries].reverse().find(
          (row) => row.slot < item2.slot && row.entry.k === "a" && row.entry.turn === item2.entry.turn
        );
        if (body?.entry.k !== "a") continue;
        closingBodies.set(item2.entry_id, body.entry.text);
        consumed.add(body.entry_id);
      }
    entries = entries.filter((item2) => !consumed.has(item2.entry_id));
    const blocks2 = entries.map((item2) => {
      const { entry_id, native_action_text, clipped, freshness: freshness2 } = item2, field = fieldView?.(item2), entry2 = field?.text === void 0 ? item2.entry : item2.entry.k === "tool" ? { ...item2.entry, out: field.text } : { ...item2.entry, text: field.text };
      const views = [];
      let notice = 0;
      const label = (text2, className = "vnote") => views.push({
        kind: "label",
        key: entry_id + ":notice:" + notice++,
        className,
        text: text2
      });
      if (entry2.k === "u" || entry2.k === "a")
        views.push({
          kind: "message",
          key: entry_id,
          entryKey: entry_id,
          flavor: entry2.k === "u" ? "user" : "assistant",
          text: entry2.text,
          images: attachments?.(item2)
        });
      else if (entry2.k === "think")
        views.push({
          kind: "thought",
          key: entry_id,
          entryKey: entry_id,
          mode: entry2.text ? "readable" : entry2.pending ? "pending" : "masked",
          label: entry2.text ? recordedThoughtLabel(entry2.secs) : void 0,
          text: entry2.text
        });
      else if (entry2.k === "tool") {
        const { more, ...preview } = entry2;
        const observedRunning = !!activity && activity.tool === entry2.name && activity.since === item2.observed_at && entry2.ok === null && !entry2.bg && !entry2.unfinished, running = liveObservation && (entry2.live === true || entry2.bg?.state === "running" || observedRunning), since = observedRunning ? activity.since : entry2.bg?.since ?? entry2.since, verb = running ? {
          Ran: "Running",
          Read: "Reading",
          Edited: "Editing",
          Wrote: "Writing",
          Patched: "Patching",
          "Searched for": "Searching for",
          "Sent input to": "Sending input to"
        }[toolInfo(entry2.name)[1]] ?? toolInfo(entry2.name)[1] : toolInfo(entry2.name)[1], status = running ? since == null ? entry2.secs : Math.max(0, Math.floor((Date.now() - since) / 1e3)) + "s" : entry2.unfinished ? "Result not recorded" : [
          entry2.exit != null ? "exit " + entry2.exit : entry2.ok === false ? "failed" : entry2.ok === null ? "exit unknown" : "",
          entry2.secs
        ].filter(Boolean).join(" \xB7 ") || null;
        views.push({
          kind: "tool",
          key: entry_id,
          entryKey: entry_id,
          step: {
            tid: entry2.tid,
            sid: running ? sessionKey : void 0,
            since: running ? since : void 0,
            className: "step" + (entry2.bg ? " background" : "") + (running ? " live" : entry2.ok === false || entry2.bg?.state === "failed" ? " err" : ""),
            key: entry_id,
            entryKey: entry_id,
            running,
            background: !!entry2.bg,
            backgroundStatus: entry2.bg ? recordedBackgroundStatus(entry2.bg) : void 0,
            waiting: false,
            label: entry2.title ?? entry2.arg,
            named: !!entry2.title,
            prefix: entry2.title ? (running ? "Running" : entry2.ok === false ? "Failed" : verb) + ": " : void 0,
            tip: entry2.title ? (entry2.in ?? entry2.arg).split(/\r\n|\n|\r/).find((line) => line.trim())?.slice(0, 200) : void 0,
            verb,
            status: entry2.bg ? null : status ?? null,
            icon: I[toolInfo(entry2.name)[0]],
            chevron: I.chev,
            data: {
              ...preview,
              expandable: !!field?.action || !!(entry2.cut && (entry2.out?.split("\n").length ?? 0) > 24)
            }
          }
        });
        if (more?.length)
          label("Full result unavailable for this source. The recorded preview is shown.");
      } else if (entry2.k === "h") {
        const related = relationshipView?.(item2);
        if (related) {
          views.push(
            ...related.map(
              (view) => view.kind === "message" ? { ...view, images: attachments?.(item2) } : view.kind === "event" && closingBodies.has(item2.entry_id) ? { ...view, brief: closingBodies.get(item2.entry_id) } : view
            )
          );
        } else {
          label("Native action \xB7 related session context is incomplete");
          if (native_action_text === void 0) label("Original action prompt is unavailable.");
          else
            views.push({
              kind: "message",
              key: entry_id,
              entryKey: entry_id,
              flavor: "incoming",
              text: native_action_text ?? ""
            });
        }
      } else if (entry2.k === "end") {
        const parent = entry2.ret && context?.sessions[entry2.ret.to], returned = entry2.ret && context?.handoffs.some(
          ({ edge }) => edge.kind === "spawn" && edge.to === sessionKey && edge.from === entry2.ret.to && edge.done === entry2.ret.at
        );
        label(
          parent && returned ? "Returned to " + parent.name + (entry2.ret.failed ? " \xB7 failed" : "") + " \xB7 " + clock(entry2.ret.at, Date.now()) : item2.provenance ? entry2.text ?? "Native session end recorded" : "Session boundary \xB7 native provenance unavailable",
          "divider"
        );
      } else if (entry2.k === "bgend")
        views.push({
          kind: "background",
          state: entry2.state,
          key: entry_id,
          entryKey: entry_id,
          failed: entry2.state === "failed",
          call: entry2.call,
          label: "Background command " + (entry2.state === "failed" ? "failed" : entry2.state === "killed" ? "stopped" : "completed") + " \xB7 " + (entry2.label ?? ""),
          loaded: entries.some((item3) => item3.entry.k === "tool" && item3.entry.tid === entry2.call)
        });
      else if (entry2.k === "harness") label(entry2.label);
      else if (entry2.k === "signal")
        views.push({
          kind: "label",
          key: entry_id,
          entryKey: entry_id,
          className: "signal-marker",
          text: recordedSignalLabel(entry2)
        });
      if (field) {
        views.push({
          kind: "label",
          key: entry_id + ":field",
          className: "vnote",
          text: field.note,
          action: field.action
        });
      } else if (clipped) label("Recorded text preview. The complete source text is not loaded.");
      if (freshness2 && freshness2.state !== "cached")
        label(
          "Source observation is " + freshness2.state + ". Previously loaded content is retained."
        );
      if (entry2.img?.length) label("Attachments unavailable for this source.");
      return { kind: "loose", key: entry_id, entries: views };
    });
    const grouped = [];
    for (let index = 0; index < entries.length; index++) {
      const item2 = entries[index], block = blocks2[index], turn = item2.entry.turn;
      if (!turn) {
        const previous2 = grouped.at(-1);
        if (previous2?.kind === "loose") previous2.entries.push(...block.entries);
        else
          grouped.push({
            ...block,
            key: "loose:" + (previous2?.kind === "turn" ? previous2.turn.id : "start")
          });
        continue;
      }
      const previous = grouped.at(-1);
      if (previous?.kind === "turn" && previous.turn.id === turn) {
        previous.turn.entries.push(...block.entries);
      } else
        grouped.push({
          kind: "turn",
          turn: {
            id: turn,
            label: "Recorded turn",
            entries: block.entries,
            traceIcon: I.trace,
            stateLabel: "Recorded"
          }
        });
    }
    for (const block of grouped) {
      if (block.kind !== "turn") continue;
      const descriptor = context?.turns.find((turn) => turn.id === block.turn.id);
      if (!descriptor) continue;
      const start = context?.handoffs.find((row) => row.edge.id === descriptor.start)?.edge;
      block.turn.label = descriptor.u || start?.kind === "ask" ? "Your message" : "Recorded turn";
      block.turn.trace = descriptor.sent.length ? descriptor.id : void 0;
      if (descriptor.end.st === "err")
        block.turn.error = descriptor.end.why === "failed" ? "Failed" : descriptor.end.why;
      if (start && start.kind !== "ask") {
        const sender = context?.sessions[start.from], harness2 = sender && HARNESSES[sender.harness];
        block.turn.header = {
          mark: harness2 ? {
            id: sender.harness,
            name: harness2.name,
            light: harness2.icon.light,
            dark: harness2.icon.dark,
            darkTheme: matchMedia("(prefers-color-scheme: dark)").matches
          } : void 0,
          parts: [
            {
              className: "soft",
              text: start.kind === "spawn" ? "Brief from " : "Message from "
            },
            {
              className: "who who-link",
              text: sender?.name ?? "Recorded session",
              action: sender ? { kind: "session", id: start.from } : void 0
            }
          ],
          time: clock(start.at, Date.now())
        };
      }
    }
    return grouped.map(
      (block) => block.kind === "turn" ? { ...block, turn: { ...block.turn, entries: gatherTools(block.turn.entries) } } : { ...block, entries: gatherTools(block.entries) }
    );
  }
  function recordedRelationshipViews(item2, relationship, selected, participants, native) {
    const edge = relationship.edge, target = participants[edge.to], source = participants[edge.from];
    const name = (id) => id === "you" ? "You" : participants[id]?.name ?? "Recorded session";
    if (edge.kind === "spawn" && edge.from === selected && target) {
      const mark2 = HARNESSES[target.harness], status = target.state === "work" || target.state === "wait" ? target.live_observation === false ? "off" : target.state : target.state ?? edge.status, kind = target.kind === "subagent" ? "Subagent" : target.kind === "codex-run" ? "Codex run" : target.kind, meta = [
        kind,
        shortModel(target.model),
        target.start == null ? void 0 : dur(
          target.start,
          target.live_observation && status === "work" ? void 0 : target.last,
          Date.now()
        ),
        target.calls == null ? void 0 : target.calls + (target.calls === 1 ? " step" : " steps"),
        target.usd == null ? void 0 : "$" + target.usd.toFixed(2)
      ].filter(Boolean).join(" \xB7 ");
      return [
        {
          kind: "child",
          key: item2.entry_id,
          entryKey: item2.entry_id,
          handoff: edge.id,
          id: edge.to,
          turn: relationship.targetTurn,
          name: target.name,
          state: status,
          stateLabel: STATE[status] ?? "Recorded history",
          meta,
          brief: native.brief ?? "",
          result: native.result,
          failed: status === "err",
          mark: mark2 ? {
            id: target.harness,
            name: mark2.name,
            light: mark2.icon.light,
            dark: mark2.icon.dark,
            darkTheme: matchMedia("(prefers-color-scheme: dark)").matches
          } : void 0,
          chevron: I.chev
        }
      ];
    }
    if (edge.to === selected && edge.from !== selected)
      return [
        {
          kind: "message",
          key: item2.entry_id,
          entryKey: item2.entry_id,
          handoff: edge.id,
          flavor: edge.kind === "ask" ? "user" : "incoming",
          text: native.brief ?? item2.native_action_text ?? ""
        },
        ...edge.kind === "ask" && relationship.targetTurn ? [
          {
            kind: "label",
            key: item2.entry_id + ":time",
            className: "msg-tm",
            text: clock(edge.at, Date.now())
          }
        ] : []
      ];
    const parts = [
      {
        className: "who who-link",
        text: name(edge.from),
        action: source ? { kind: "session", id: edge.from } : void 0
      },
      {
        className: "soft",
        text: edge.kind === "toyou" ? edge.ask === "result" ? " sent " : " asked " : edge.kind === "relay" ? " relayed to " : " asked "
      },
      {
        className: "who who-link",
        text: name(edge.to),
        action: target ? { kind: "session", id: edge.to, turn: relationship.targetTurn } : void 0
      }
    ];
    if (edge.kind === "toyou" && edge.ask === "result")
      parts.splice(1, 2, { className: "soft", text: " sent you a result" });
    return [
      {
        kind: "event",
        key: item2.entry_id,
        entryKey: item2.entry_id,
        handoff: edge.id,
        className: "event" + (edge.kind === "move" ? " move" : "") + (edge.kind === "toyou" && edge.status === "wait" ? " waiting" : ""),
        icon: edge.kind === "toyou" && edge.ask !== "result" && edge.status !== "wait" ? I.done : I[edge.kind === "toyou" ? edge.ask === "result" ? "result" : edge.ask === "decision" ? "decide" : "qc" : edge.kind === "relay" ? "relay" : "ask"],
        parts,
        time: clock(edge.at, Date.now()),
        brief: native.brief ?? item2.native_action_text ?? "",
        result: native.result,
        resultLabel: "Result",
        answers: native.answer == null ? void 0 : Array.isArray(native.answer) ? native.answer : [native.answer],
        waiting: edge.status === "wait",
        stateLabel: STATE[edge.status]
      }
    ];
  }

  // src/state/catalog-field-wire.ts
  function parseCatalogField(value, request) {
    if (!request.entry.provenance) throw new Error("Native text requires source provenance");
    const anchor = request.entry.field?.provenance ?? request.entry.provenance;
    const row = object(value), identity2 = parseCatalogIdentity(row.identity), projection = object(row.projection), field = object(row.field), provenance = object(row.provenance), source = parseCatalogSource(provenance.source), chunk = number(field.chunk), next = field.next === null ? null : number(field.next), complete = boolean(field.complete), body = text(row.text), sourceBytes = number(object(row.observation).source_bytes), layers = field.layers ?? 1;
    const sourceLimit = layers === 2 ? 65607 : 65547;
    if (row.api !== 1 || identity2.read_scope !== "retained_history" || identity2.source_key !== request.source_key || identity2.catalog_key !== request.catalog_key || projection.version !== 5 || projection.generation !== request.generation || row.slot !== request.entry.slot || field.name !== request.entry.field?.name || layers !== 1 && layers !== 2 || layers !== (request.entry.field?.layers ?? 1) || chunk !== request.chunk || !Number.isSafeInteger(chunk) || chunk < 0 || next !== (complete ? null : chunk + 1) || !request.entry.field || chunk >= request.entry.field.chunks || complete !== (chunk + 1 === request.entry.field.chunks) || object(row.freshness).state !== "cached" || !Number.isSafeInteger(sourceBytes) || sourceBytes < 0 || sourceBytes > sourceLimit || new TextEncoder().encode(body).byteLength > 131072 || JSON.stringify(source) !== JSON.stringify(anchor.source) || provenance.offset !== anchor.offset || provenance.block !== anchor.block || provenance.native_event_id !== anchor.native_event_id)
      throw new Error("Native field chunk does not match the selected source projection");
    return { text: body, next, complete };
  }

  // src/state/catalog-sources.ts
  function parseCatalogSources(value) {
    const row = object(value);
    if (row.api !== 1 || !Array.isArray(row.items) || row.items.length > 100)
      throw new Error("Invalid source inventory");
    const items = array(row.items, (value2) => {
      const item2 = object(value2), source_key = text(item2.source_key), label = text(item2.label);
      if (!source_key || !label) throw new Error("Invalid source identity");
      return { source_key, label };
    });
    if (new Set(items.map((item2) => item2.source_key)).size !== items.length)
      throw new Error("Repeated source inventory identity");
    const cursor = row.next_cursor === null ? null : text(row.next_cursor);
    if (cursor === "") throw new Error("Invalid source inventory cursor");
    return { api: 1, items, next_cursor: cursor };
  }

  // src/lib/catalogSources.tsx
  function renderCatalogSources(root, view, host2) {
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsxs("div", { class: "ph", children: [
          /* @__PURE__ */ jsx("h1", { children: "Choose a machine" }),
          /* @__PURE__ */ jsx("p", { class: "sub", children: "Browse recorded history from one authorized source." })
        ] }),
        view.note && /* @__PURE__ */ jsxs("p", { class: "catalog-note", role: "status", children: [
          screenText(view.note),
          " ",
          /* @__PURE__ */ jsx("button", { class: "link", type: "button", onClick: () => host2.retry(), children: "Retry" })
        ] }),
        view.updating && /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: "Loading machines\u2026" }),
        !!view.bookmarks?.length && /* @__PURE__ */ jsx("div", { class: "session-list", "aria-label": "Matching session origins", children: view.bookmarks.map((item2) => /* @__PURE__ */ jsxs(
          "a",
          {
            class: "nrow",
            href: "/s/" + encodeURIComponent(item2.harness) + "/" + encodeURIComponent(item2.catalog_key) + "?" + new URLSearchParams({ machine: item2.source_key }),
            children: [
              /* @__PURE__ */ jsx("span", { class: "nm", children: screenText(item2.catalog_key) }),
              /* @__PURE__ */ jsx("span", { children: screenText(
                view.items.find((source) => source.source_key === item2.source_key)?.label ?? item2.source_key
              ) })
            ]
          },
          JSON.stringify([item2.source_key, item2.catalog_key])
        )) }),
        /* @__PURE__ */ jsx("div", { class: "session-list", children: view.items.map((item2) => /* @__PURE__ */ jsx(
          "button",
          {
            class: "nrow",
            type: "button",
            "data-source-key": item2.source_key,
            onClick: () => host2.select(item2.source_key),
            children: /* @__PURE__ */ jsx("span", { class: "nm", children: screenText(item2.label) })
          },
          item2.source_key
        )) }),
        !view.items.length && !view.updating && !view.note && /* @__PURE__ */ jsx("p", { class: "catalog-note", children: "No authorized recorded sources are available." }),
        view.more && /* @__PURE__ */ jsx("button", { class: "link", type: "button", disabled: view.updating, onClick: () => host2.more(), children: "Load more machines" }),
        /* @__PURE__ */ jsx("p", { class: "catalog-note", children: /* @__PURE__ */ jsx("a", { class: "link", href: view.machinesHref, children: "Manage machines" }) })
      ] }),
      root
    );
  }

  // src/app/catalogViewer.tsx
  function createCatalogViewer(initialCapabilities, viewerHost) {
    const capabilities = initialCapabilities ?? {
      api: 1,
      read_contract: "catalog-v1",
      source_key: "",
      selected_transcript: false,
      selected_identity: false,
      selected_entry: false,
      source_candidates: false,
      metadata_search: false,
      attachment: false,
      relationship_context: false,
      large_native_records: false,
      pagination: true,
      filters: [],
      order: "last_desc_key_asc",
      full_text_search: false,
      global_union: false
    };
    const scope = new EffectScope(), selection = new CatalogSelectionStore(), navigation = new NavigationController((route) => {
      queueMicrotask(() => {
        if (!disposed && navigation.route === route)
          history.replaceState({ ...history.state, ...route }, "", location.href);
      });
    });
    const previousScrollRestoration = history.scrollRestoration;
    if (!viewerHost?.nativePage) history.scrollRestoration = "manual";
    let nameRefreshOffset = 0;
    let listRetry, listRetryDelay = 1e3;
    let disposed = false, page = null, items = [], cursor = null, listEpoch = 0, updating = false, note = "", harness = new URLSearchParams(location.search).get("harness") ?? "", repo = new URLSearchParams(location.search).get("no_repo") === "1" ? "__none__" : new URLSearchParams(location.search).get("repo") ?? "", model2 = new URLSearchParams(location.search).get("model") ?? "", query = new URLSearchParams(location.search).get("q") ?? "", listScroll = 0, listFocus = "", active = null;
    let candidates = [], candidateCursor = null, candidateLoaded = false, candidateUpdating = false, candidateNote = "", candidateEpoch = 0, candidateTimer;
    const selected = /* @__PURE__ */ new Map();
    const sourceViews = /* @__PURE__ */ new Map();
    let sourcesOpen = false, sourcesEpoch = 0, sourceItems = [], sourceCursor = null, sourcesUpdating = false, sourcesNote = "", sourcesRetryDelay = 1e3, locating = false;
    let bookmarkMatches = [];
    let pinView = null, pinUntil = 0;
    const cacheKey = (key2) => JSON.stringify([capabilities.source_key, key2]);
    let showApprovalReviews = false;
    const ordering = createOrdering();
    let orderTouch = false, orderIdle;
    const idleHook = Reflect.get(window, "__semonOrderIdleMs");
    const orderIdleMs = Number.isFinite(idleHook) && idleHook >= 200 && idleHook <= 6e4 ? idleHook : 1e4;
    function orderScope(name, signature = "") {
      const side = name === "recent", region = document.querySelector(side ? "#side-list" : "#main");
      const rows = side ? "#lanes .treeitem" : "#page .nrow";
      const first = document.querySelector(rows), focus = document.activeElement;
      const inView = side ? (region?.scrollTop ?? 0) <= 1 : !first || first.getBoundingClientRect().top >= (document.querySelector("#topbar")?.getBoundingClientRect().bottom ?? 0) - 1;
      const touched = orderTouch || !!(focus?.matches(":focus-visible") && focus.closest(side ? "#lanes" : rows)) || matchMedia("(hover: hover)").matches && !!document.querySelector(side ? "#lanes .treeitem:hover" : "#page .nrow:hover");
      if (rail && side) ordering.scopes.delete(name);
      return ordering.begin(name, signature, side ? null : navigation.route, { inView, touched });
    }
    function armOrderIdle() {
      scope.clearTimeout(orderIdle);
      orderIdle = void 0;
      if (matchMedia("(max-width: 760px)").matches || !ordering.scopes.get("recent")?.n) return;
      orderIdle = scope.timeout(() => {
        orderIdle = void 0;
        const sidebar = document.querySelector("#sidebar"), focused = document.activeElement;
        if (orderTouch || sidebar?.matches(":hover") || sessionPanel || shell.account.open || focused && sidebar?.contains(focused) && focused.matches(":focus-visible, input, textarea, select, [contenteditable]"))
          armOrderIdle();
        else {
          ordering.scopes.delete("recent");
          chrome();
        }
      }, orderIdleMs);
    }
    let account = parseAccount(viewerHost?.account);
    let standaloneNavigation;
    let restoringOverlayHistory = false, overlayBackPending = false, pendingChrome = false;
    const machinesHref = () => viewerHost?.machinesPath ?? standaloneNavigation?.paths?.machines ?? "/machines";
    let wide = readShellPreference("wide"), rail = readShellPreference("rail");
    const shell = createShellChrome({
      account: {
        place(widget, trigger) {
          const at = trigger.getBoundingClientRect();
          setGeometry(widget, "accountLeft", at.left);
          setGeometry(widget, "accountWidth", at.width);
          setGeometry(widget, "accountBottom", Math.max(0, innerHeight - at.top + 6));
        },
        opened(compact) {
          if (compact) history.pushState({ ...history.state, sheet: 1 }, "");
        },
        closed(options2) {
          if (pendingChrome && !options2.navigating && !disposed) {
            pendingChrome = false;
            scope.frame(chrome);
          }
          if (!disposed && !options2.keepEntry && !options2.navigating && !restoringOverlayHistory && history.state?.sheet) {
            overlayBackPending = true;
            history.back();
          }
        },
        navigate(href) {
          if (!history.state?.sheet) return false;
          const { sheet: sheet2, ...route } = history.state;
          history.replaceState(route, "");
          location.assign(href);
          return true;
        },
        submit(form) {
          if (!history.state?.sheet) return false;
          const { sheet: sheet2, ...route } = history.state;
          history.replaceState(route, "");
          form.submit();
          return true;
        }
      },
      navigate(destination) {
        if (viewerHost?.nativePage) return false;
        if (destination.key === "home" || destination.key === "analytics") {
          goOverview(destination.key);
          return true;
        }
        if (destination.key === "machines" && !viewerHost?.catalogSources && machinesHref() === "/machines") {
          goOverview("machines");
          return true;
        }
        if (destination.key !== "sessions") return false;
        if (capabilities.source_key) goList();
        else {
          history.pushState(null, "", "/sessions");
          fromLocation();
        }
        return true;
      },
      drawerOpened() {
        ordering.scopes.delete("recent");
        chrome();
        history.pushState({ ...history.state, drawer: 1 }, "");
      },
      drawerClosed() {
        ordering.scopes.delete("recent");
        chrome();
        if (!disposed && !restoringOverlayHistory && history.state?.drawer && !history.state?.sheet) {
          overlayBackPending = true;
          history.back();
        }
      },
      railChanged() {
        ordering.scopes.delete("recent");
        rail = !rail;
        writeShellPreference("rail", rail);
        chrome();
      }
    });
    shell.mount(document.querySelector(".app"));
    scope.listen(
      document,
      "pointerdown",
      () => {
        orderTouch = true;
      },
      true
    );
    for (const type of ["pointerup", "pointercancel"])
      scope.listen(
        document,
        type,
        () => {
          orderTouch = false;
        },
        true
      );
    scope.listen(window, "blur", () => {
      orderTouch = false;
    });
    scope.listen(document, "visibilitychange", () => {
      orderTouch = false;
      if (document.visibilityState === "visible") {
        ordering.scopes.clear();
        chrome();
        drawList();
        overview.draw();
      }
    });
    for (const type of [
      "pointermove",
      "pointerdown",
      "pointerleave",
      "focusin",
      "focusout",
      "wheel",
      "keydown"
    ])
      scope.listen(
        document.querySelector("#sidebar"),
        type,
        () => {
          if (orderIdle) armOrderIdle();
        },
        { passive: true }
      );
    const root = shell.slots.content, title = document.createElement("div"), back = document.createElement("button"), jumpActions = document.createElement("div"), jumpSlot = document.createElement("span"), composerTarget = document.createElement("span"), jumpTarget = document.createElement("div");
    const readerResize = new ResizeObserver(() => {
      if (active && active === pinView && active.following && !active.errorMode && performance.now() < pinUntil) {
        active.following = true;
        scrollEnd();
      }
    });
    readerResize.observe(root);
    readerResize.observe(document.querySelector("#main"));
    readerResize.observe(document.querySelector("#topbar"));
    scope.listen(
      document,
      "wheel",
      () => {
        pinUntil = 0;
      },
      { passive: true, capture: true }
    );
    scope.listen(
      document,
      "pointerdown",
      () => {
        pinUntil = 0;
      },
      { passive: true, capture: true }
    );
    const sessionBar = createViewerBar();
    const traceActions = document.createElement("div"), traceMenu = document.createElement("button");
    traceActions.className = "viewer-bar-actions";
    traceMenu.type = "button";
    traceMenu.id = "more-btn";
    traceMenu.className = "ibtn";
    traceMenu.setAttribute("aria-label", "Session menu");
    traceMenu.setAttribute("aria-haspopup", "dialog");
    traceMenu.setAttribute("aria-expanded", "false");
    render(/* @__PURE__ */ jsx(Glyph, { path: I.more, className: "" }), traceMenu);
    traceActions.append(traceMenu);
    scope.listen(traceMenu, "click", () => openSessionMenu(null, traceMenu));
    let sessionPanel = null;
    function panelOpened(panel) {
      sessionPanel?.destroy();
      sessionPanel = panel;
      history.pushState({ ...navigation.route, sheet: 1 }, "");
    }
    function panelClosed(panel) {
      if (sessionPanel === panel) sessionPanel = null;
      if (!disposed && !restoringOverlayHistory && history.state?.sheet) {
        overlayBackPending = true;
        history.back();
      }
    }
    let afterOverlayClose = null;
    function navigateFromPanel(panel, action) {
      if (history.state?.sheet) {
        afterOverlayClose = action;
        panel.destroy();
      } else {
        panel.destroy();
        action();
      }
    }
    let traceTitle = false;
    title.className = "ttl catalog-title";
    back.type = "button";
    back.className = "ibtn catalog-back";
    back.setAttribute("aria-label", "Back to sessions");
    back.setAttribute("data-tip", "Back to sessions");
    render(/* @__PURE__ */ jsx(Glyph, { path: I.back, className: "" }), back);
    const sources = document.createElement("a");
    sources.className = "ibtn catalog-sources";
    sources.href = "/sessions?choose_source=1";
    sources.setAttribute("aria-label", "Choose a machine");
    sources.setAttribute("data-tip", "Choose a machine");
    render(/* @__PURE__ */ jsx(Glyph, { path: I.machine, className: "" }), sources);
    scope.listen(sources, "click", (event) => {
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
        return;
      event.preventDefault();
      history.pushState(null, "", "/sessions?choose_source=1");
      void showSources();
    });
    scope.listen(back, "click", () => goList());
    jumpActions.className = "viewer-bar-actions";
    jumpSlot.className = "viewer-jump";
    jumpTarget.className = "jump-wrap";
    jumpTarget.hidden = true;
    jumpSlot.append(jumpTarget);
    jumpActions.append(jumpSlot, composerTarget);
    const recent = createRecentRenderer(shell.slots.recent.querySelector("#lanes"), {
      open(key2) {
        if (overview.openRecent(key2)) return;
        const item2 = items.find((row) => JSON.stringify([capabilities.source_key, row.key]) === key2);
        if (item2) void goSession(item2.key, item2);
        else {
          const identity2 = JSON.parse(key2);
          if (Array.isArray(identity2) && identity2.length === 2 && identity2[0] === capabilities.source_key && typeof identity2[1] === "string")
            void goSession(identity2[1]);
        }
      },
      toggle(key2, value) {
        overview.toggleRecent(key2, value);
      },
      all(key2, trigger) {
        overview.allRecent(key2, true, trigger);
      },
      fewer(key2) {
        overview.allRecent(key2, false);
      }
    });
    const control = createLocalControl(scope, () => {
      if (!active) return;
      updateSessionControl(active.root, controlFor(active));
    });
    function controlFor(view) {
      const identity2 = selection.selectedIdentity();
      const current = active === view && identity2?.source_key === view.sourceKey && identity2?.catalog_key === view.key && identity2.owner_qualification !== "provisional" && identity2.native_id ? control.view(identity2.native_id, identity2.source_key + ":" + view.key) : void 0;
      if (current) view.lastControl = current;
      const runtime = runtimeFor(view)?.observation, terminal = runtime && runtime.state !== "active";
      if (current && !terminal || !view.lastControl)
        return current ?? control.readOnly(view.key, view.sourceKey + ":" + view.key);
      const previous = current ?? view.lastControl;
      return {
        ...previous,
        get draft() {
          return previous.draft;
        },
        busy: false,
        uncertain: previous.uncertain,
        canReconnect: false,
        snapshot: {
          ...previous.snapshot,
          connected: false,
          reason: terminal ? "Native controls are unavailable for this environment state." : "Current connection status is unavailable.",
          capabilities: {
            ...previous.snapshot.capabilities,
            input: false,
            steer: false,
            interrupt: false,
            commandApproval: false,
            fileApproval: false,
            questions: false
          }
        },
        send: () => Promise.resolve(false),
        interrupt() {
        },
        answer() {
        },
        reconnect() {
        }
      };
    }
    const observation = createControlObservation({
      scope,
      navigation,
      viewerHost,
      catalogSelection: selection,
      controlOwner: control
    });
    const runtimeObservation = createRuntimeObservation({
      scope,
      navigation,
      viewerHost,
      catalogRuntimeSelection: selection
    });
    function runtimeFor(view) {
      const frame = runtimeObservation.view();
      return frame?.source_key === capabilities.source_key && frame.catalog_key === view.key ? frame : void 0;
    }
    const removeRuntimeListener = runtimeObservation.subscribe(() => {
      if (!active) return;
      updateSessionRuntime(active.root, runtimeFor(active));
      updateSessionControl(active.root, controlFor(active));
    });
    const overviewRequests = /* @__PURE__ */ new Set();
    let causal = null, causalEpoch = 0, causalLoading = false;
    const causalExpanded = /* @__PURE__ */ new Map();
    const api = async (path, body) => {
      const controller = scope.request(), deadline = scope.timeout(() => controller.abort(), 15e3);
      if (path.startsWith("/api/viewer/home?") || path.startsWith("/api/viewer/analytics?") || path.startsWith("/api/session-home?") || path.startsWith("/api/session-observe?") || path.startsWith("/api/session-trace?"))
        overviewRequests.add(controller);
      try {
        return await requestJson(
          path,
          controller.signal,
          path.startsWith("/api/viewer/analytics?"),
          body
        );
      } finally {
        scope.clearTimeout(deadline);
        scope.releaseRequest(controller);
        overviewRequests.delete(controller);
      }
    };
    const currentParams = () => new URLSearchParams({ machine: capabilities.source_key });
    const browsingParams = () => {
      const value = currentParams();
      if (harness) value.set("harness", harness);
      if (repo === "__none__") value.set("no_repo", "1");
      else if (repo) value.set("repo", repo);
      if (model2) value.set("model", model2);
      if (query) value.set("q", query);
      return value;
    };
    const saveFilters = () => history.replaceState(history.state, "", "/sessions?" + browsingParams());
    const params = () => new URLSearchParams({ machine: capabilities.source_key, scope: "retained_history" });
    const overview = createOverviewPages({
      orderScope,
      reviews: () => showApprovalReviews,
      root,
      homeIntro: viewerHost?.initialHome?.element,
      nativeContent: !!viewerHost?.nativePage,
      api,
      panelOpened,
      panelClosed,
      route: () => viewerHost?.nativePage ? "home" : navigation.route.v,
      chrome,
      session(intent, turn) {
        if (sessionPanel && history.state?.sheet) {
          navigateFromPanel(sessionPanel, () => overviewSession(intent, turn));
          return;
        }
        overviewSession(intent, turn);
      },
      trace(intent, turn) {
        if (sessionPanel && history.state?.sheet) {
          navigateFromPanel(sessionPanel, () => overviewTrace(intent, turn));
          return;
        }
        overviewTrace(intent, turn);
      },
      sessions(filters) {
        const open = () => {
          const value = new URLSearchParams();
          if (filters.machine) value.set("machine", filters.machine);
          if (filters.harness) value.set("harness", filters.harness);
          if (filters.repo === "__none__") value.set("no_repo", "1");
          else if (filters.repo) value.set("repo", filters.repo);
          if (filters.model) value.set("model", filters.model);
          history.pushState(null, "", "/sessions?" + value);
          fromLocation();
        };
        if (sessionPanel && history.state?.sheet) navigateFromPanel(sessionPanel, open);
        else open();
      }
    });
    function overviewSession(intent, turn) {
      const url = "/s/native/" + encodeURIComponent(intent.catalog_key) + "?" + new URLSearchParams({ machine: intent.source_key }) + (turn ? "#turn=" + encodeURIComponent(turn) : "");
      if (viewerHost?.nativePage) {
        location.assign(url);
        return;
      }
      history.pushState(null, "", url);
      fromLocation();
    }
    function overviewTrace(intent, turn) {
      const url = "/trace/native/" + encodeURIComponent(intent.catalog_key) + "/" + encodeURIComponent(turn) + "?" + new URLSearchParams({ machine: intent.source_key });
      if (viewerHost?.nativePage) {
        location.assign(url);
        return;
      }
      history.pushState(null, "", url);
      fromLocation();
    }
    function leaveScreen() {
      sessionPanel?.destroy();
      ++causalEpoch;
      causal = null;
      causalLoading = false;
      causalExpanded.clear();
      if (active) {
        preserveScroll();
        closeSessionComposer(active.root);
      }
      overview.leave();
      for (const request of overviewRequests) request.abort();
      overviewRequests.clear();
      ++listEpoch;
      updating = false;
      scope.clearTimeout(listRetry);
      cancelCandidate();
      active = null;
      selection.clear();
      sourcesOpen = false;
      releaseScreen(root);
      render(null, root);
      root.replaceChildren();
    }
    function goOverview(route, push = true) {
      leaveScreen();
      navigation.route = { v: route };
      if (push) history.pushState(navigation.route, "", route === "home" ? "/" : "/" + route);
      shell.closeDrawer(true);
      overview.draw();
      void overview.refresh();
    }
    function drawCausal() {
      if (!causal || navigation.route.v !== "trace") return;
      const accepted = causal;
      renderCausalTrace(root, accepted, Date.now(), causalExpanded, {
        session(key2, turn) {
          const url = "/s/native/" + encodeURIComponent(key2) + "?" + new URLSearchParams({ machine: accepted.source }) + (turn ? "#turn=" + encodeURIComponent(turn) : "");
          history.pushState(null, "", url);
          fromLocation();
        },
        fold: drawCausal
      });
      chrome();
    }
    async function loadCausal() {
      const route = navigation.route;
      if (disposed || route.v !== "trace" || causalLoading) return;
      const epoch = causalEpoch, source = capabilities.source_key;
      causalLoading = true;
      try {
        const p = params();
        p.set("sid", route.sid);
        p.set("turn", route.turn);
        const next = parseCausalTrace(
          await api("/api/session-trace?" + p),
          source,
          route.sid,
          route.turn
        );
        const metadataParams = params();
        metadataParams.set("sid", route.sid);
        const metadata = parseCatalogPage(await api("/api/sessions?" + metadataParams));
        if (metadata.machine !== source || metadata.read_scope !== "retained_history")
          throw Error("Causal metadata selection changed");
        next.metadata = metadata.items.find((item2) => item2.key === route.sid);
        if (disposed || epoch !== causalEpoch || navigation.route !== route || source !== capabilities.source_key)
          return;
        const previous = causal;
        next.previewReady = /* @__PURE__ */ new Set();
        if (previous?.source === source) {
          for (const sid of previous.previewReady ?? []) {
            if (previous.previews[sid] !== next.previews[sid]) continue;
            if ([...next.handoffs.values()].some((edge) => {
              const owner = next.previewOwners.get(edge.id), old = previous.previewOwners.get(edge.id);
              return owner?.brief === sid && old?.brief !== sid || owner?.result === sid && old?.result !== sid;
            }) || (next.turns[sid] ?? []).some(
              (turn) => !previous.turns[sid]?.some((old) => old.id === turn.id)
            ))
              continue;
            next.previewReady.add(sid);
            for (const edge of next.handoffs.values()) {
              const old = previous.handoffs.get(edge.id), owner = next.previewOwners.get(edge.id);
              if (!old) continue;
              if (owner?.brief === sid)
                Object.assign(edge, {
                  brief: old.brief,
                  answer: old.kind === "toyou" ? old.answer : void 0
                });
              if (owner?.result === sid) edge.result = old.result;
            }
            for (const turn of next.turns[sid] ?? []) {
              const old = previous.turns[sid]?.find((value) => value.id === turn.id);
              if (turn.u && old?.u) turn.u = old.u;
            }
          }
        }
        const jobs = [];
        for (const sid of /* @__PURE__ */ new Set([...Object.keys(next.previews), ...Object.keys(next.sessions)])) {
          if (next.previewReady.has(sid)) continue;
          const entries = [...next.handoffs.values()].filter((edge) => {
            const owner = next.previewOwners.get(edge.id);
            return owner?.brief === sid || owner?.result === sid;
          }).map((edge) => ({ event: edge.id })), turns = (next.turns[sid] ?? []).map((turn) => ({ turn: turn.id }));
          const selected2 = [...entries, ...turns];
          for (let start = 0; start < selected2.length; start += 8) {
            const group = selected2.slice(start, start + 8);
            jobs.push({
              sid,
              events: group.flatMap((value) => "event" in value ? [value.event] : []),
              turns: group.flatMap((value) => "turn" in value ? [value.turn] : [])
            });
          }
        }
        const remaining = /* @__PURE__ */ new Map();
        for (const job of jobs) remaining.set(job.sid, (remaining.get(job.sid) ?? 0) + 1);
        const unavailable = /* @__PURE__ */ new Set();
        const updateObservation = () => {
          next.observation = unavailable.size || next.previewsIncomplete ? "Some recorded text is unavailable. Retrying\u2026" : remaining.size ? "Loading recorded text\u2026" : void 0;
          if (causal === next && !disposed && epoch === causalEpoch && navigation.route === route)
            drawCausal();
        };
        causal = next;
        updateObservation();
        let pending = 0;
        await Promise.all(
          Array.from({ length: Math.min(4, jobs.length) }, async () => {
            while (pending < jobs.length && !disposed && epoch === causalEpoch) {
              const job = jobs[pending++], generation2 = next.previews[job.sid];
              if (!generation2) {
                unavailable.add(job.sid);
                remaining.delete(job.sid);
                updateObservation();
                continue;
              }
              try {
                const query2 = params();
                query2.set("sid", job.sid);
                query2.set("generation", generation2);
                query2.set("events", JSON.stringify(job.events));
                query2.set("turns", JSON.stringify(job.turns));
                const detail = object(await api("/api/session-home?" + query2));
                if (disposed || epoch !== causalEpoch || navigation.route !== route || source !== capabilities.source_key)
                  return;
                const identity2 = object(detail.identity);
                if (detail.api !== 1 || detail.generation !== generation2 || identity2.source_key !== source || identity2.catalog_key !== job.sid || typeof detail.incomplete !== "boolean")
                  throw Error("Causal text selection changed");
                if (detail.incomplete) {
                  const values = [
                    ...job.events.map((event) => ({ event })),
                    ...job.turns.map((turn) => ({ turn }))
                  ];
                  if (values.length > 1) {
                    const half = Math.ceil(values.length / 2);
                    for (const part of [values.slice(0, half), values.slice(half)])
                      jobs.push({
                        sid: job.sid,
                        events: part.flatMap((value) => "event" in value ? [value.event] : []),
                        turns: part.flatMap((value) => "turn" in value ? [value.turn] : [])
                      });
                    remaining.set(job.sid, (remaining.get(job.sid) ?? 0) + 2);
                  } else unavailable.add(job.sid);
                }
                const events = object(detail.events), texts = object(detail.turns);
                const eventUpdates = job.events.flatMap((id) => {
                  if (!events[id]) return [];
                  const value = object(events[id]), edge = next.handoffs.get(id);
                  if (value.brief != null && typeof value.brief !== "string")
                    throw Error("Invalid causal text");
                  if (value.result != null && typeof value.result !== "string")
                    throw Error("Invalid causal result");
                  if (value.answer != null && (!Array.isArray(value.answer) || !value.answer.every((item2) => typeof item2 === "string")))
                    throw Error("Invalid recorded answer");
                  const owner = next.previewOwners.get(id);
                  return [
                    {
                      edge,
                      ...owner?.brief === job.sid ? {
                        brief: typeof value.brief === "string" ? value.brief : "",
                        answer: edge.kind === "toyou" && Array.isArray(value.answer) ? value.answer : void 0
                      } : {},
                      ...owner?.result === job.sid ? { result: typeof value.result === "string" ? value.result : void 0 } : {}
                    }
                  ];
                });
                const turnUpdates = job.turns.map((id) => {
                  const turn = next.turns[job.sid].find((turn2) => turn2.id === id);
                  if (texts[id] != null && typeof texts[id] !== "string")
                    throw Error("Invalid recorded prompt");
                  return { turn, text: texts[id] };
                });
                for (const { edge, ...fields } of eventUpdates) Object.assign(edge, fields);
                for (const { turn, text: text2 } of turnUpdates)
                  if (typeof text2 === "string") turn.u = { k: "u", text: text2 };
              } catch {
                unavailable.add(job.sid);
              }
              const left = remaining.get(job.sid) - 1;
              if (left > 0) remaining.set(job.sid, left);
              else {
                remaining.delete(job.sid);
                if (!unavailable.has(job.sid)) next.previewReady.add(job.sid);
              }
              updateObservation();
            }
          })
        );
        if (disposed || epoch !== causalEpoch || navigation.route !== route || source !== capabilities.source_key)
          return;
        causal = next;
        drawCausal();
      } catch (error) {
        if (disposed || epoch !== causalEpoch || navigation.route !== route) return;
        if (causal) {
          causal.observation = readError(error);
          drawCausal();
        } else
          render(
            /* @__PURE__ */ jsxs("p", { class: "catalog-note", role: "status", children: [
              readError(error),
              " ",
              /* @__PURE__ */ jsx("button", { class: "link", type: "button", onClick: () => void loadCausal(), children: "Retry Trace" })
            ] }),
            root
          );
      } finally {
        if (epoch === causalEpoch) causalLoading = false;
      }
    }
    function goCausal(key2, turn) {
      leaveScreen();
      navigation.route = { v: "trace", sid: key2, turn };
      shell.closeDrawer(true);
      render(
        /* @__PURE__ */ jsx("p", { class: "catalog-note", role: "status", children: "Loading recorded Trace\u2026" }),
        root
      );
      chrome();
      void loadCausal();
    }
    function chrome() {
      if (shell.account.open) {
        pendingChrome = true;
        return;
      }
      const route = navigation.route.v;
      const name = viewerHost?.nativePage && !["home", "analytics", "sessions", "session"].includes(route) ? viewerHost.nativePage.title : route === "home" ? "Home" : route === "analytics" ? "Analytics" : route === "machines" ? "Machines" : route === "trace" ? "Trace" : sourcesOpen ? "Choose a machine" : active?.meta?.name ?? (active ? "Session" : "Sessions");
      const recentHeading = document.querySelector(".side-h");
      if (recentHeading)
        recentHeading.textContent = viewerHost?.nativePage || ["home", "analytics", "machines"].includes(route) ? "Recent" : "Recent on this machine";
      shell.update(
        projectShellNavigation(
          viewerHost?.navigation ?? standaloneNavigation ?? { leading: viewerHost?.nativeNavigation },
          viewerHost?.nativePage && !["home", "analytics", "sessions", "session"].includes(route) ? viewerHost.nativePage.nav : route === "session" || route === "trace" ? "sessions" : route,
          {
            ...viewerHost?.navigation?.leading?.find((item2) => item2.key === "new-session") ? {
              "new-session": withReturnPath(
                viewerHost.navigation.leading.find((item2) => item2.key === "new-session").href,
                location.pathname + location.search
              )
            } : {},
            home: viewerHost?.navigation?.paths?.home ?? "/",
            analytics: viewerHost?.navigation?.paths?.analytics ?? "/analytics",
            sessions: "/sessions?" + browsingParams(),
            machines: machinesHref()
          },
          overview.waitingCount() === void 0 ? {} : { home: overview.waitingCount() }
        ),
        rail
      );
      {
        const session = active?.meta;
        const content2 = active && session ? sessionBar.update(
          {
            mode: active.errorMode ? "errors" : active.findOpen ? "find" : "normal",
            name,
            session: active.key,
            state: selectedSessionState(session),
            stateLabel: STATE[selectedSessionState(session)] ?? "Recorded history",
            stateTip: "Status: " + (STATE[selectedSessionState(session)] ?? "Recorded history"),
            showState: true,
            ancestors: (session.details?.family?.ancestors ?? []).map((row) => ({
              id: row.key,
              name: row.name
            })),
            crumb: "Sessions",
            labels: selectedSessionLabels(
              session,
              page?.machine_info.label ?? active.sourceKey
            ),
            trace: false,
            analytics: false,
            days: 7,
            query: active.find ?? "",
            count: visibleEntries(active).length + (active.find ? " loaded " + (visibleEntries(active).length === 1 ? "match" : "matches") : " loaded " + (visibleEntries(active).length === 1 ? "record" : "records")),
            filter: active.filter ?? "all",
            failed: session.details?.errors ?? 0,
            signals: Object.values(session.details?.signals ?? {}).reduce(
              (sum, count2) => sum + count2,
              0
            ),
            errorMode: active.errorMode ?? "",
            errorText: active.errorMode ? (active.errorMode === "signals" ? "Signal " : "Error ") + (active.annotations?.total ? (active.errorIndex ?? 0) + 1 : 0) + " of " + (active.annotations?.total ?? 0) : "",
            errorDisabled: (active.annotations?.total ?? 0) < 2,
            icons: { search: I.search, more: I.more, back: I.back, x: I.x, up: I.up, dn: I.dn }
          },
          {
            ancestor(key2) {
              void goSession(key2);
            },
            crumb() {
              goList();
            },
            find() {
              if (!active) return;
              active.errorMode = void 0;
              for (const node of active.root.querySelectorAll(".err-current"))
                node.classList.remove("err-current");
              active.findOpen = true;
              chrome();
              document.querySelector("#find")?.focus();
            },
            closeFind() {
              if (!active) return;
              active.findOpen = false;
              active.find = "";
              active.filter = "all";
              active.renderedRevision = -1;
              chrome();
              drawSelected(active);
            },
            query(value) {
              if (!active) return;
              active.find = value.toLowerCase();
              active.renderedRevision = -1;
              chrome();
              drawSelected(active);
            },
            filter(value) {
              if (!active) return;
              if (value === "signals" || value === "failures") {
                openAnnotations(active, value === "signals" ? "signals" : "errors");
                return;
              }
              active.filter = value;
              active.renderedRevision = -1;
              chrome();
              drawSelected(active);
            },
            menu(trigger, runs) {
              if (active) openSessionMenu(active, trigger, runs);
            },
            errors() {
              if (active) openAnnotations(active, "errors");
            },
            closeErrors() {
              if (!active) return;
              closeAnnotations(active);
            },
            step(direction) {
              if (!active) return;
              const count2 = active.annotations?.total ?? 0;
              if (!count2) return;
              active.errorIndex = ((active.errorIndex ?? 0) + direction + count2) % count2;
              chrome();
              void readAnnotations(active);
            },
            range() {
            }
          }
        ) : null;
        const label = document.createElement("span");
        label.className = "t catalog-session-name l1";
        label.textContent = name;
        if (route === "trace") {
          if (!traceTitle) title.replaceChildren();
          traceTitle = true;
          const key2 = navigation.route.v === "trace" ? navigation.route.sid : "", turn = navigation.route.v === "trace" ? navigation.route.turn : void 0, session2 = causal?.sessions[key2];
          render(
            /* @__PURE__ */ jsxs("div", { class: "l1", children: [
              /* @__PURE__ */ jsx(
                "button",
                {
                  type: "button",
                  class: "crumb",
                  "aria-label": "Back to " + (session2?.name ?? "session"),
                  onClick: () => {
                    void goSession(key2, void 0, true, turn);
                  },
                  children: session2?.name ?? "Session"
                }
              ),
              /* @__PURE__ */ jsx("span", { class: "crumb-sep", children: "\u203A" }),
              /* @__PURE__ */ jsx("span", { class: "t", children: "Trace" })
            ] }),
            title
          );
        } else {
          traceTitle = false;
          render(null, title);
          title.replaceChildren(label);
          if (active) title.prepend(back);
        }
        shell.topbar({
          titleSlot: content2 && !active?.findOpen && !active?.errorMode ? content2.titleSlot : title,
          mode: (active?.findOpen || active?.errorMode) && content2 ? [content2.mode] : void 0,
          actions: route === "analytics" ? [overview.actions] : active ? [composerTarget, ...content2 ? [content2.actions] : [sources]] : route === "sessions" && !sourcesOpen ? [sources] : route === "trace" ? [traceActions] : [],
          session: !!active,
          lead: route === "trace" && navigation.route.v === "trace" ? {
            label: "Back to " + (causal?.metadata?.name ?? "session"),
            icon: I.chev,
            back: () => {
              if (navigation.route.v === "trace")
                void goSession(navigation.route.sid, void 0, true, navigation.route.turn);
            }
          } : { label: "Open menu", icon: I.menu },
          account: account ? {
            account,
            compact: false,
            wide,
            onWideChange() {
              wide = !wide;
              writeShellPreference("wide", wide);
              root.classList.toggle("wide-mode", wide);
              shell.account.updateWide(wide);
            }
          } : null,
          accountTarget: content2?.accountTarget
        });
        shell.drawerAccount(account ? { account, compact: true, wide, onWideChange() {
        } } : null);
      }
      if (active)
        scope.frame(() => measureViewerBar(document.querySelector("#topbar")));
      if (viewerHost?.nativePage || ["home", "analytics", "machines"].includes(route))
        recent.update(overview.recentSnapshot(void 0, [], void 0, rail));
      else
        recent.update(
          overview.recentSnapshot(capabilities.source_key, items, active?.key, rail, active?.meta)
        );
      const pinned = shell.slots.recent.querySelector(".tree-row.stuck");
      setGeometry(shell.slots.recent, "scrollPaddingTop", pinned ? pinned.offsetHeight + 8 : null);
      if (ordering.scopes.get("recent")?.n) {
        if (!orderIdle) armOrderIdle();
      } else {
        scope.clearTimeout(orderIdle);
        orderIdle = void 0;
      }
    }
    function visibleEntries(view) {
      const query2 = view.find ?? "", filter = view.filter ?? "all";
      return view.store.entries().filter((item2) => {
        const entry2 = item2.entry;
        if (filter === "messages" && !["u", "a", "h"].includes(entry2.k)) return false;
        if (filter === "steps" && entry2.k !== "tool") return false;
        if (filter === "failures" && !(entry2.k === "tool" && (entry2.ok === false || entry2.unfinished)))
          return false;
        if (filter === "signals" && entry2.k !== "signal") return false;
        if (!query2) return true;
        const text2 = entry2.k === "tool" ? [
          entry2.name,
          entry2.title,
          entry2.arg,
          entry2.in,
          entry2.out,
          view.fields.get(item2.entry_id)?.text
        ].join(" ") : entry2.k === "signal" ? recordedSignalLabel(entry2) : "text" in entry2 ? entry2.text ?? "" : item2.native_action_text ?? "";
        return text2.toLowerCase().includes(query2);
      });
    }
    async function readAnnotations(view, force = false) {
      const current = view.store.selectedPage(), mode = view.errorMode;
      if (disposed || active !== view || !mode || !current || view.annotationsLoading) return;
      const index = view.errorIndex ?? 0, first = Math.floor(index / 100) * 100;
      if (!force && view.annotations?.generation === current.projection.generation && view.annotations.kind === mode && view.annotations.first === first) {
        highlightAnnotation(view);
        return;
      }
      const epoch = (view.annotationsEpoch ?? 0) + 1;
      view.annotationsEpoch = epoch;
      view.annotationsLoading = true;
      const query2 = params();
      query2.set("sid", view.key);
      query2.set("kind", mode);
      query2.set("after", String(first));
      query2.set("limit", "100");
      query2.set("generation", current.projection.generation);
      chrome();
      try {
        const next = parseCatalogAnnotations(await api("/api/session-annotations?" + query2));
        if (disposed || active !== view || view.errorMode !== mode || epoch !== view.annotationsEpoch || view.store.selectedPage()?.projection.generation !== next.generation)
          return;
        if (next.source !== view.sourceKey || next.key !== view.key || next.kind !== mode || next.first !== first)
          throw Error("Annotation identity changed");
        view.annotations = next;
        view.errorIndex = Math.min(index, Math.max(0, next.total - 1));
        highlightAnnotation(view);
      } catch (error) {
        if (!disposed && active === view && epoch === view.annotationsEpoch) {
          view.note = readError(error);
          drawSelected(view);
        }
      } finally {
        if (epoch === view.annotationsEpoch) view.annotationsLoading = false;
        if (!disposed && active === view) {
          chrome();
          if (view.annotationsFocus && view.errorMode && view.annotations) {
            view.annotationsFocus = false;
            document.querySelector("#err-next")?.focus({ preventScroll: true });
          }
        }
      }
    }
    function highlightAnnotation(view) {
      if (active !== view || !view.errorMode) return;
      for (const node2 of view.root.querySelectorAll(".err-current"))
        node2.classList.remove("err-current");
      const page2 = view.annotations, target = page2?.entries[(view.errorIndex ?? 0) - page2.first];
      if (!target) return;
      const identity2 = view.store.entries().find((item2) => item2.slot === target.slot)?.entry_id;
      const node = [...view.root.querySelectorAll("[data-entry-key]")].find(
        (node2) => node2.dataset.entryKey === identity2 && (node2.classList.contains("signal-marker") || node2.classList.contains("step"))
      );
      if (!node) {
        if (view.annotationRange !== target.slot) {
          view.annotationRange = target.slot;
          void loadSelected(view, Math.max(0, target.slot - 30)).finally(() => {
            view.annotationRange = void 0;
          });
        }
        return;
      }
      view.following = false;
      node.classList.add("err-current");
      node.closest(".tgroup")?.querySelector('.tgroup > button[aria-expanded="false"]')?.click();
      const phone = matchMedia("(max-width: 760px)").matches, main = document.querySelector("#main");
      const row = (node.querySelector(":scope > button") ?? node).getBoundingClientRect();
      const top = document.querySelector("#topbar").getBoundingClientRect().bottom;
      const bottom = phone ? innerHeight : main.getBoundingClientRect().bottom;
      const delta = (row.top + row.bottom - top - bottom) / 2;
      if (phone) window.scrollBy(0, delta);
      else main.scrollTop += delta;
    }
    function closeAnnotations(view) {
      view.errorMode = void 0;
      view.errorIndex = void 0;
      view.annotationsEpoch = (view.annotationsEpoch ?? 0) + 1;
      view.annotationsLoading = false;
      const restore = view.errorRestore;
      view.findOpen = restore?.findOpen ?? false;
      view.find = restore?.find ?? "";
      view.filter = restore?.filter ?? "all";
      view.errorRestore = void 0;
      for (const node of view.root.querySelectorAll(".err-current"))
        node.classList.remove("err-current");
      if (restore) view.store.retainRanges(restore.ranges);
      view.renderedRevision = -1;
      drawSelected(view);
      const restoreScreen = () => {
        if (disposed || active !== view || view.errorMode || !restore) return;
        for (const button of view.root.querySelectorAll(
          ".tgroup > button, .step > button"
        )) {
          const key2 = button.parentElement?.dataset.entryKey ?? button.parentElement?.dataset.e;
          if (key2 && button.getAttribute("aria-expanded") === "true" !== restore.expanded.has(key2))
            button.click();
        }
        if (matchMedia("(max-width: 760px)").matches) window.scrollTo(0, restore.scroll);
        else document.querySelector("#main").scrollTop = restore.scroll;
        chrome();
        document.querySelector(restore.focus)?.focus({ preventScroll: true });
      };
      if (restore?.key && !view.store.entries().some((item2) => item2.entry_id === restore.key))
        void loadSelected(view, restore.first).finally(() => scope.frame(restoreScreen));
      else scope.frame(restoreScreen);
      chrome();
    }
    function openAnnotations(view, mode) {
      if (!view.errorMode) {
        const entries = view.store.entries(), first = entries[0];
        view.errorRestore = {
          scroll: matchMedia("(max-width: 760px)").matches ? window.scrollY : document.querySelector("#main").scrollTop,
          first: first?.slot ?? 0,
          key: first?.entry_id,
          ranges: view.store.loadedRanges(),
          findOpen: view.findOpen ?? false,
          find: view.find,
          filter: view.filter,
          focus: view.findOpen ? "#find" : matchMedia("(max-width: 760px)").matches ? "#more-btn" : ".lab-errs",
          expanded: new Set(
            [
              ...view.root.querySelectorAll(
                '.tgroup > button[aria-expanded="true"], .step > button[aria-expanded="true"]'
              )
            ].map((node) => node.parentElement?.dataset.entryKey ?? node.parentElement?.dataset.e).filter((key2) => !!key2)
          )
        };
      }
      view.find = "";
      view.filter = "all";
      view.findOpen = false;
      view.renderedRevision = -1;
      view.errorMode = mode;
      view.errorIndex = 0;
      view.annotations = void 0;
      view.annotationsFocus = true;
      drawSelected(view);
      chrome();
      void readAnnotations(view);
      document.querySelector("#err-close")?.focus();
    }
    function openSessionMenu(view, trigger, runs = false) {
      const metadata = view?.meta ?? causal?.metadata;
      if (!metadata || (view ? active !== view : navigation.route.v !== "trace")) return;
      sessionPanel?.destroy();
      const turn = view ? [...view.root.querySelectorAll("[data-turn]")].find(
        (node) => node.getBoundingClientRect().bottom > document.querySelector("#topbar").getBoundingClientRect().bottom
      )?.dataset.turn : void 0;
      const snapshot = selectedSessionMenu(
        metadata,
        page?.machine_info.label ?? capabilities.source_key,
        wide,
        !!turn
      );
      const panel = createPanelChrome(
        {
          title: metadata.name,
          sub: snapshot.status,
          label: "Session menu for " + metadata.name,
          className: "anchored session-menu"
        },
        {
          opened() {
            history.pushState({ ...navigation.route, sheet: 1 }, "");
            trigger.setAttribute("aria-expanded", "true");
          },
          closed() {
            render(null, panel.body);
            if (sessionPanel === panel) sessionPanel = null;
            trigger.setAttribute("aria-expanded", "false");
            if (!disposed && !restoringOverlayHistory && history.state?.sheet) {
              overlayBackPending = true;
              history.back();
            }
            if (trigger.isConnected) trigger.focus({ preventScroll: true });
          }
        }
      );
      sessionPanel = panel;
      renderSessionMenu(panel.body, snapshot, {
        wide: () => wide,
        session(key2) {
          navigateFromPanel(panel, () => {
            void goSession(key2);
          });
        },
        action(key2) {
          if (key2 === "runs") {
            panel.body.querySelector(".runs")?.scrollIntoView({ block: "start" });
          } else if (key2 === "wide") {
            wide = !wide;
            writeShellPreference("wide", wide);
            root.classList.toggle("wide-mode", wide);
            shell.account.updateWide(wide);
          } else if (key2 === "source") {
            navigateFromPanel(panel, () => sources.click());
          } else if (key2 === "signals" || key2 === "errors") {
            navigateFromPanel(panel, () => {
              const kind = key2 === "errors" ? "errors" : "signals";
              if (view) openAnnotations(view, kind);
              else
                void goSession(metadata.key).then(() => {
                  if (active?.key === metadata.key) openAnnotations(active, kind);
                });
            });
          } else if (key2 === "trace" && turn && view) {
            navigateFromPanel(panel, () => {
              history.pushState(
                null,
                "",
                "/trace/native/" + encodeURIComponent(view.key) + "/" + encodeURIComponent(turn) + "?" + browsingParams()
              );
              goCausal(view.key, turn);
            });
          }
        }
      });
      panel.show();
      if (runs) panel.body.querySelector(".runs")?.scrollIntoView({ block: "nearest" });
    }
    function openRecordedTool(view, key2, inputLabel, script = false) {
      if (active !== view || disposed || !view.meta) return;
      const item2 = view.store.entries().find((item3) => item3.entry_id === key2);
      if (!item2 || item2.entry.k !== "tool") return;
      const entry2 = item2.entry, trigger = document.activeElement;
      sessionPanel?.destroy();
      const panel = createPanelChrome(
        {
          title: toolInfo(entry2.name)[1] + " " + entry2.arg,
          sub: entry2.name + " \xB7 " + (entry2.ok === false ? "Failed" : "Recorded result"),
          className: "full"
        },
        {
          opened() {
            history.pushState({ ...navigation.route, sheet: 1 }, "");
          },
          closed() {
            render(null, panel.body);
            if (sessionPanel === panel) sessionPanel = null;
            if (!disposed && !restoringOverlayHistory && history.state?.sheet) {
              overlayBackPending = true;
              history.back();
            }
            if (active === view && trigger instanceof HTMLElement && trigger.isConnected)
              trigger.focus({ preventScroll: true });
          }
        }
      );
      panel.body.classList.add("viewer-b");
      sessionPanel = panel;
      let scriptResult = {};
      let outputResult, outputNote = "";
      const draw = () => {
        if (disposed || active !== view || sessionPanel !== panel) return;
        const field = script ? null : fieldView(view, item2), data = {
          ...entry2,
          ...script ? scriptResult : { scriptText: void 0 },
          ...field?.text === void 0 ? {} : { out: field.text },
          ...field ? { boundedPreview: true } : {},
          ...outputResult
        };
        render(
          /* @__PURE__ */ jsxs(Fragment2, { children: [
            script && scriptResult.scriptText === void 0 && !scriptResult.scriptFailed ? /* @__PURE__ */ jsx("p", { role: "status", class: "vnote", children: "Loading recorded script\u2026" }) : /* @__PURE__ */ jsx(
              FullTool,
              {
                data,
                inputLabel,
                waiting: selectedSessionState(view.meta) === "wait"
              }
            ),
            field && (field.action || view.fields.get(key2)?.note) && /* @__PURE__ */ jsxs("p", { class: "vnote", children: [
              field.note,
              field.action && /* @__PURE__ */ jsx(
                "button",
                {
                  class: "btn sh-quiet",
                  type: "button",
                  disabled: field.action.busy,
                  onClick: () => {
                    void loadField(view, item2).then(draw);
                    draw();
                  },
                  children: field.action.label
                }
              )
            ] }),
            outputNote && /* @__PURE__ */ jsx("p", { class: "vnote", role: "status", children: outputNote })
          ] }),
          panel.body
        );
      };
      draw();
      panel.show();
      if (script) {
        const generation2 = view.store.selectedPage().projection.generation, p = params();
        p.set("sid", view.key);
        p.set("after", String(item2.slot));
        p.set("limit", "1");
        p.set("generation", generation2);
        p.set("part", "script");
        void api("/api/session-entry?" + p).then((value) => {
          if (disposed || active !== view || sessionPanel !== panel || view.store.selectedPage()?.projection.generation !== generation2)
            return;
          const page2 = parseCatalogTranscriptPage(value), saved = page2.entries[0];
          if (page2.identity.source_key !== view.sourceKey || page2.identity.catalog_key !== view.key || page2.identity.read_scope !== view.store.selectedPage().identity.read_scope || page2.projection.generation !== generation2 || page2.range.first !== item2.slot || page2.entries.length !== 1 || saved.entry_id !== item2.entry_id || JSON.stringify(saved.provenance) !== JSON.stringify(item2.provenance) || saved.entry.k !== "tool" || typeof saved.entry.scriptText !== "string")
            throw Error("Invalid recorded script scope");
          scriptResult = {
            scriptText: saved.entry.scriptText,
            scriptTruncated: saved.entry.scriptTruncated
          };
          draw();
        }).catch(() => {
          scriptResult = { scriptFailed: true };
          draw();
        });
      } else if (fieldView(view, item2)?.action) {
        void loadField(view, item2).then(draw);
        draw();
      } else if (entry2.cut) {
        const generation2 = view.store.selectedPage().projection.generation, p = params();
        p.set("sid", view.key);
        p.set("after", String(item2.slot));
        p.set("limit", "1");
        p.set("generation", generation2);
        p.set("part", "output");
        outputNote = "Loading recorded output\u2026";
        draw();
        void api("/api/session-entry?" + p).then((value) => {
          if (disposed || active !== view || sessionPanel !== panel || view.store.selectedPage()?.projection.generation !== generation2)
            return;
          const page2 = parseCatalogTranscriptPage(value), saved = page2.entries[0];
          if (page2.identity.source_key !== view.sourceKey || page2.identity.catalog_key !== view.key || page2.identity.read_scope !== view.store.selectedPage().identity.read_scope || page2.projection.generation !== generation2 || page2.range.first !== item2.slot || page2.entries.length !== 1 || saved.entry_id !== item2.entry_id || JSON.stringify(saved.provenance) !== JSON.stringify(item2.provenance) || saved.entry.k !== "tool")
            throw Error("Invalid recorded output scope");
          outputResult = { out: saved.entry.out, cut: saved.entry.cut };
          outputNote = saved.entry.outputTruncated ? "The output preview is bounded; the remaining text is not shown." : "";
          draw();
        }).catch(() => {
          outputNote = "Couldn't load the full text: this is the preview.";
          draw();
        });
      }
    }
    function drawList() {
      if (disposed || navigation.route.v !== "sessions" || active || sourcesOpen) return;
      chrome();
      const focused = document.activeElement;
      const draft = focused instanceof HTMLInputElement && root.contains(focused) ? {
        node: focused,
        value: focused.value,
        start: focused.selectionStart,
        end: focused.selectionEnd
      } : null;
      renderCatalogList(
        root,
        {
          items: orderRows(
            orderScope(
              "sessions",
              JSON.stringify([
                capabilities.source_key,
                harness,
                repo,
                model2,
                query,
                showApprovalReviews
              ])
            ),
            "sessions",
            items.filter((item2) => showApprovalReviews || item2.presentation_kind !== "Approval review").map((item2) => ({ ...item2, id: item2.key })),
            (a, b) => (b.last ?? 0) - (a.last ?? 0) || a.key.localeCompare(b.key)
          ).map((item2) => ({ ...item2, presentation: catalogSessionRow(item2) })),
          reviews: showApprovalReviews,
          hasReviews: showApprovalReviews || items.some((item2) => item2.presentation_kind === "Approval review"),
          sourceLabel: page?.machine_info.label ?? (capabilities.source_key || "This source"),
          updating,
          candidates,
          candidatesMore: candidateCursor !== null,
          candidateUpdating,
          candidateNote,
          discovering: page?.completeness.state === "partial",
          observation: {
            cached: "Cached history",
            updating: "Discovering history",
            stale: "History observation is stale",
            incomplete: "History is incomplete",
            unavailable: "History observation is unavailable"
          }[page?.freshness ?? "cached"],
          note,
          harness,
          repo,
          model: model2,
          query,
          metadataSearch: capabilities.metadata_search,
          searchPartial: page?.search?.partial ?? false,
          searchIndexIncomplete: page?.search ? !page.search.index_complete : false,
          more: cursor !== null
        },
        {
          session(item2) {
            void goSession(
              item2.key,
              items.find((row) => row.key === item2.key)
            );
          },
          reviews() {
            showApprovalReviews = !showApprovalReviews;
            ordering.scopes.clear();
            drawList();
            chrome();
          },
          candidate(item2) {
            void prepareCandidate(item2);
          },
          candidateMore() {
            void loadCandidates(true);
          },
          candidateRetry() {
            void loadCandidates(false);
          },
          filters(values) {
            if (harness === values.harness && repo === values.repo && model2 === values.model && query === values.q)
              return;
            cancelCandidate();
            harness = values.harness;
            repo = values.repo;
            model2 = values.model;
            query = values.q;
            saveFilters();
            void loadList(false);
          },
          clearFilters() {
            cancelCandidate();
            harness = repo = model2 = query = "";
            saveFilters();
            void loadList(false);
            root.querySelector(".catalog-filters input")?.focus();
          },
          more() {
            void loadList(true);
          },
          retry() {
            void loadList(false);
          }
        }
      );
      if (draft?.node.isConnected && document.activeElement === draft.node) {
        draft.node.value = draft.value;
        if (draft.start !== null && draft.end !== null)
          draft.node.setSelectionRange(draft.start, draft.end);
      }
    }
    async function loadList(append) {
      if (disposed || !capabilities.source_key || navigation.route.v !== "sessions" && !active || append && updating)
        return;
      scope.clearTimeout(listRetry);
      const epoch = ++listEpoch, p = params();
      p.set("limit", "60");
      if (harness) p.set("harness", harness);
      if (repo === "__none__") p.set("no_repo", "1");
      else if (repo) p.set("repo", repo);
      if (model2) p.set("model", model2);
      if (query && capabilities.metadata_search) p.set("q", query);
      if (append && cursor !== null) p.set("cursor", cursor);
      updating = true;
      note = "";
      drawList();
      try {
        const next = parseCatalogPage(await api("/api/sessions?" + p));
        if (disposed || epoch !== listEpoch) return;
        if (query && capabilities.metadata_search && next.search === null || next.machine !== capabilities.source_key || next.read_scope !== "retained_history" || append && page?.generation !== next.generation)
          throw new Error("History changed; refresh this list before loading another page.");
        if (append && next.items.some((item2) => items.some((old) => old.key === item2.key)))
          throw new Error("History page repeated a session. Refresh this list.");
        listRetryDelay = 1e3;
        page = next;
        if (capabilities.source_candidates && !candidateLoaded) void loadCandidates(false);
        cursor = next.next_cursor;
        items = append ? [...items, ...next.items].slice(-300) : next.items;
        updating = false;
        drawList();
        chrome();
        if ((next.completeness.state === "partial" || next.search?.index_complete === false) && items.length <= 60) {
          const refresh = () => {
            if (disposed || epoch !== listEpoch || active || sourcesOpen) return;
            void loadList(false);
          };
          listRetry = scope.timeout(refresh, 1e3);
        }
      } catch (error) {
        if (disposed || epoch !== listEpoch) return;
        updating = false;
        if (append && error && typeof error === "object" && "status" in error && error.status === 409) {
          void loadList(false);
          return;
        }
        note = readError(error);
        drawList();
        if (retryable(error)) {
          listRetry = scope.timeout(() => void loadList(append), listRetryDelay);
          listRetryDelay = Math.min(8e3, listRetryDelay * 2);
        }
      }
    }
    function cancelCandidate() {
      candidateEpoch++;
      scope.clearTimeout(candidateTimer);
      candidateUpdating = false;
    }
    async function loadCandidates(append) {
      if (!capabilities.source_candidates || disposed || append && candidateUpdating) return;
      cancelCandidate();
      const epoch = candidateEpoch, sourceKey = capabilities.source_key;
      candidateLoaded = true;
      candidateUpdating = true;
      candidateNote = "";
      drawList();
      const p = new URLSearchParams({ machine: sourceKey, limit: "60" });
      if (append && candidateCursor) p.set("cursor", candidateCursor);
      try {
        const result = parseCatalogSourceCandidates(await api("/api/session-source-candidates?" + p));
        if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
        if (result.source_key !== sourceKey || append && result.items.some(
          (item2) => candidates.some((old) => old.candidate_key === item2.candidate_key)
        ))
          throw new Error("Invalid archive source scope");
        candidates = append ? [...candidates, ...result.items] : result.items;
        candidateCursor = result.next_cursor;
      } catch (error) {
        if (disposed || epoch !== candidateEpoch) return;
        candidateNote = readError(error);
      } finally {
        if (!disposed && epoch === candidateEpoch) {
          candidateUpdating = false;
          drawList();
        }
      }
    }
    async function prepareCandidate(candidate) {
      cancelCandidate();
      const epoch = candidateEpoch, sourceKey = capabilities.source_key;
      candidateUpdating = true;
      candidateNote = "";
      drawList();
      let delay = 1e3;
      const poll = async () => {
        if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
        try {
          const p = new URLSearchParams({
            machine: sourceKey,
            candidate: candidate.candidate_key,
            generation: candidate.generation
          });
          const result = parseCatalogSourceCandidateProgress(
            await api("/api/session-source-candidate?" + p)
          );
          if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
          if (result.source_key !== sourceKey || result.candidate_key !== candidate.candidate_key || result.generation !== candidate.generation)
            throw new Error("Archive source changed; resynchronize source hints");
          if (result.state === "ready") {
            const metadata = parseCatalogPage(
              await api(
                "/api/sessions?" + new URLSearchParams({
                  machine: sourceKey,
                  scope: "retained_history",
                  sid: result.catalog_key
                })
              )
            );
            if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
            if (metadata.machine !== sourceKey || metadata.read_scope !== "retained_history" || metadata.items.length !== 1 || metadata.items[0].key !== result.catalog_key)
              throw new Error("Parsed archive session identity is unavailable");
            candidateUpdating = false;
            await goSession(result.catalog_key, metadata.items[0]);
            return;
          }
          candidateNote = result.reason ?? (result.state === "updating" ? "Recorded source is being indexed. Its session will open automatically." : "This archived source cannot currently be read.");
          if (result.state === "unavailable" && !result.retryable) {
            candidateUpdating = false;
            drawList();
            return;
          }
          drawList();
          candidateTimer = scope.timeout(() => void poll(), delay);
          delay = Math.min(delay * 2, 8e3);
        } catch (error) {
          if (disposed || epoch !== candidateEpoch) return;
          candidateNote = readError(error);
          candidateUpdating = retryable(error);
          drawList();
          if (candidateUpdating) {
            candidateTimer = scope.timeout(() => void poll(), delay);
            delay = Math.min(delay * 2, 8e3);
          }
        }
      };
      await poll();
    }
    function readError(error) {
      if (!(error && typeof error === "object" && "status" in error) && !(error instanceof Error && error.name === "AbortError"))
        return "The source returned an invalid history view. Retry after checking this source.";
      const status = error && typeof error === "object" && "status" in error ? error.status : 0;
      if (status === 401 || status === 403)
        return "Authorization is required to read this source. Reconnect your account and retry.";
      if (status === 400) return "The requested filter or range is unsupported by this source.";
      if (status === 404) return "This recorded session is unavailable from the selected source.";
      if (status === 409) return "Recorded history changed. Resynchronizing\u2026";
      if (status === 503) return "The source is updating or temporarily unavailable. Retrying\u2026";
      if (status === 0) return "Source observation is unavailable. Retrying\u2026";
      return "History could not be read. Retry after checking your connection.";
    }
    function retryable(error) {
      if (!(error && typeof error === "object" && "status" in error))
        return error instanceof Error && error.name === "AbortError";
      const status = error && typeof error === "object" && "status" in error ? error.status : 0;
      return status === 0 || status === 429 || status === 500 || status === 502 || status === 503 || status === 504;
    }
    function preserveScroll() {
      const scroll = window.matchMedia("(max-width: 760px)").matches ? window.scrollY : document.querySelector("#main").scrollTop;
      if (active) active.scroll = scroll;
      else if (!sourcesOpen) listScroll = scroll;
    }
    function goList(push = true) {
      leaveScreen();
      navigation.route = { v: "sessions" };
      if (push) history.pushState(navigation.route, "", "/sessions?" + browsingParams());
      root.replaceChildren();
      drawList();
      const row = [...root.querySelectorAll('[data-session-row="list"]')].find(
        (node) => node.dataset.id === listFocus
      );
      row?.focus({ preventScroll: true });
      if (window.matchMedia("(max-width: 760px)").matches) window.scrollTo(0, listScroll);
      else document.querySelector("#main").scrollTop = listScroll;
      if ((!page || page.completeness.state === "partial" && items.length <= 60) && !updating)
        void loadList(false);
      shell.closeDrawer(true);
    }
    function drawSelected(view) {
      if (disposed || active !== view) return;
      const current = view.store.selectedPage(), identity2 = selection.selectedIdentity(), observationNote = [
        view.anchorNote,
        view.note,
        view.meta?.source_refs.some(
          (reference) => reference.state === "unavailable" || reference.state === "incomplete"
        ) ? "History is incomplete. Previously loaded text is retained." : void 0
      ].filter(Boolean).join(" ");
      if (!current) {
        renderSessionScreen(
          view.root,
          {
            composerTarget,
            id: view.key,
            name: view.meta?.name ?? "Session",
            control: controlFor(view),
            runtime: runtimeFor(view),
            blocks: [],
            order: [],
            observation: "Recorded history is updating",
            retry: view.note && !view.loading ? {
              label: "Retry selected transcript",
              run() {
                scope.clearTimeout(view.retry);
                void loadSelected(view);
              }
            } : void 0,
            empty: observationNote || (capabilities.selected_transcript ? "Loading recorded session\u2026" : "Recorded transcript is updating. The source refresh will appear automatically.")
          },
          {
            committed() {
            },
            trace(turn) {
              const url = "/trace/native/" + encodeURIComponent(view.key) + "/" + encodeURIComponent(turn) + "?" + new URLSearchParams({ machine: view.sourceKey });
              history.pushState(null, "", url);
              goCausal(view.key, turn);
            },
            session(key2) {
              void goSession(key2);
            },
            machine() {
            },
            sender() {
            },
            toolAll() {
            },
            script() {
            },
            image() {
            },
            background() {
            },
            jump() {
            },
            pager() {
            }
          }
        );
        return;
      }
      if (view.lastTotal !== null && !view.following && current.projection.total > view.lastTotal)
        view.newCount += current.projection.total - view.lastTotal;
      view.lastTotal = current.projection.total;
      overview.markResults(
        view.sourceKey,
        view.store.context().handoffs.filter(
          (row) => row.edge.kind === "toyou" && row.edge.ask === "result" && row.edge.from === view.key
        ).map((row) => row.edge.id)
      );
      if (view.renderedRevision === view.store.revision && view.renderedNote === observationNote) {
        measureSessionScreen(view.root);
        updateSessionControl(view.root, controlFor(view));
        updateSessionRuntime(view.root, runtimeFor(view));
        syncJump(view);
        if ((view.store.loadedRanges()[0]?.first ?? 0) > 0)
          updateSessionPager(view.root, {
            sid: view.key,
            where: "before",
            text: "Load earlier records",
            disabled: view.loading,
            busy: view.loading
          });
        return;
      }
      view.renderedRevision = view.store.revision;
      view.renderedNote = observationNote;
      const anchor = [...view.root.querySelectorAll("[data-entry-key]")].find(
        (node) => node.getBoundingClientRect().bottom > (window.matchMedia("(max-width: 760px)").matches ? 0 : document.querySelector("#main").getBoundingClientRect().top)
      );
      const anchorTop = anchor?.getBoundingClientRect().top;
      renderSessionScreen(
        view.root,
        {
          composerTarget,
          id: view.key,
          name: view.meta?.name ?? "Session",
          control: controlFor(view),
          runtime: runtimeFor(view),
          observation: observationNote || {
            cached: "Cached history",
            stale: "Stale history",
            incomplete: "History is incomplete",
            unavailable: "Source history is unavailable"
          }[current.freshness.state] + (view.store.context().state === "incomplete" ? " \xB7 related session context is incomplete" : ""),
          started: view.store.loadedRanges()[0]?.first === 0 && view.meta?.start != null ? {
            lead: "Started " + clock(view.meta.start, Date.now()) + " on ",
            machine: page?.machine_info.label ?? view.sourceKey
          } : void 0,
          blocks: catalogTranscriptBlocks(
            visibleEntries(view),
            (entry2) => fieldView(view, entry2),
            (item2) => {
              if (item2.entry.k !== "h") return null;
              const context = view.store.context(), relationship = context.handoffs.find(
                (row) => row.edge.id === item2.entry.id
              );
              if (!relationship) return null;
              const preview = (owner) => owner ? view.contextText?.get(
                JSON.stringify([owner, context.previews[owner], relationship.edge.id])
              ) : void 0;
              return recordedRelationshipViews(item2, relationship, view.key, context.sessions, {
                brief: preview(relationship.briefOwner)?.brief,
                result: preview(relationship.resultOwner)?.result,
                answer: preview(relationship.briefOwner)?.answer
              });
            },
            view.meta?.details?.live_observation === true,
            view.store.context(),
            (item2) => (item2.entry.img ?? []).map((image2, index) => {
              const p = params();
              p.set("sid", view.key);
              p.set("after", String(item2.slot));
              p.set("limit", "1");
              p.set("generation", current.projection.generation);
              p.set("part", "attachment");
              p.set("image", String(index));
              const scale = image2.w && image2.h ? Math.min(1, 200 / image2.w, 160 / image2.h) : null;
              const size = image2.size == null ? "?" : image2.size >= 1048576 ? (image2.size / 1048576).toFixed(1) + " MB" : image2.size >= 1024 ? Math.round(image2.size / 1024) + " KB" : image2.size + " bytes";
              return {
                unavailable: !!image2.na || !capabilities.attachment,
                url: "/api/session-entry?" + p,
                label: "Attached image " + (index + 1) + " (" + (image2.type?.replace("image/", "").toUpperCase() ?? "image") + ", " + size + ")",
                width: scale && image2.w ? Math.max(1, Math.round(scale * image2.w)) : void 0,
                height: scale && image2.h ? Math.max(1, Math.round(scale * image2.h)) : void 0
              };
            }),
            view.meta?.harness,
            view.key,
            view.meta?.details?.state === "work" ? view.meta.details.activity : void 0
          ),
          order: [...new Set(view.store.entries().map((item2) => item2.entry.turn ?? item2.entry_id))],
          before: (view.store.loadedRanges()[0]?.first ?? 0) > 0 ? {
            sid: view.key,
            where: "before",
            text: "Load earlier records",
            disabled: view.loading,
            busy: view.loading
          } : void 0,
          empty: visibleEntries(view).length ? void 0 : view.store.entries().length ? "No loaded records match your search or filter." : "No native records in this range."
        },
        {
          committed() {
          },
          composerOpened(panel) {
            panelOpened(panel);
          },
          composerClosed(panel) {
            panelClosed(panel);
          },
          trace(turn) {
            const url = "/trace/native/" + encodeURIComponent(view.key) + "/" + encodeURIComponent(turn) + "?" + new URLSearchParams({ machine: view.sourceKey });
            history.pushState(null, "", url);
            goCausal(view.key, turn);
          },
          session(key2, turn) {
            void goSession(key2, void 0, true, turn);
          },
          machine() {
          },
          sender() {
          },
          toolAll(key2, label) {
            openRecordedTool(view, key2, label);
          },
          script(key2) {
            openRecordedTool(view, key2, "Script", true);
          },
          image(url, label, trigger) {
            sessionPanel?.destroy();
            const image2 = createImageViewer(url, label, {
              opened() {
                history.pushState({ ...navigation.route, sheet: 1 }, "");
              },
              closed() {
                if (sessionPanel === image2) sessionPanel = null;
                if (!disposed && !restoringOverlayHistory && history.state?.sheet) {
                  overlayBackPending = true;
                  history.back();
                }
                if (active === view && trigger.isConnected) trigger.focus({ preventScroll: true });
              }
            });
            sessionPanel = image2;
            image2.show();
          },
          background(call) {
            const saved = view.store.entries().find((item2) => item2.entry.k === "tool" && item2.entry.tid === call);
            const target = saved && [...view.root.querySelectorAll("[data-entry-key]")].find(
              (node) => node.dataset.entryKey === saved.entry_id
            );
            if (!target) return;
            const group = target.closest(".tgroup");
            const summary = group?.querySelector('.tsum[aria-expanded="false"]');
            summary?.click();
            target.scrollIntoView({ block: "center" });
            target.classList.add("flash");
            scope.timeout(() => target.classList.remove("flash"), 1200);
          },
          jump() {
            jumpLatest(view);
          },
          pager() {
            void loadSelected(view, Math.max(0, view.store.loadedRanges()[0].first - 60));
          }
        }
      );
      measureSessionScreen(view.root);
      const main = document.querySelector("#main");
      if (view.following) {
        if (view.opening) {
          pinView = view;
          pinUntil = performance.now() + 2e3;
          scope.frame(() => {
            if (active === view && pinView === view && performance.now() < pinUntil) {
              scrollEnd();
              scope.frame(() => {
                if (active === view && pinView === view && performance.now() < pinUntil) scrollEnd();
              });
            }
          });
        }
        view.opening = false;
        scrollEnd();
      } else if (anchor?.isConnected && anchorTop !== void 0) {
        const shift = anchor.getBoundingClientRect().top - anchorTop;
        if (window.matchMedia("(max-width: 760px)").matches) window.scrollBy(0, shift);
        else main.scrollTop += shift;
      } else if (view.opening) {
        view.opening = false;
        if (window.matchMedia("(max-width: 760px)").matches)
          window.scrollTo(0, document.documentElement.scrollHeight);
        else main.scrollTop = main.scrollHeight;
      }
      syncJump(view);
      if (view.errorMode)
        scope.frame(() => {
          if (view.annotations?.generation !== view.store.selectedPage()?.projection.generation)
            void readAnnotations(view);
          else highlightAnnotation(view);
        });
    }
    function scrollEnd() {
      if (window.matchMedia("(max-width: 760px)").matches)
        window.scrollTo(0, document.documentElement.scrollHeight);
      else {
        const main = document.querySelector("#main");
        main.scrollTop = main.scrollHeight;
      }
    }
    function syncJump(view) {
      if (active !== view || disposed) return;
      updateSessionJump(view.root, !view.following, view.newCount, false, jumpTarget);
    }
    function jumpLatest(view) {
      if (active !== view || disposed) return;
      view.following = true;
      view.newCount = 0;
      syncJump(view);
      scrollEnd();
      void loadSelected(view).finally(() => {
        if (active === view && !disposed) scrollEnd();
      });
    }
    function fieldView(view, entry2) {
      if (!entry2.clipped || !entry2.field) return null;
      const progress = view.fields.get(entry2.entry_id), generation2 = view.store.selectedPage().projection.generation, changed = progress && progress.generation !== generation2;
      return {
        text: progress?.text,
        note: changed ? "Source projection changed. Previously loaded text is retained. " : progress?.note || (progress?.next === null ? "Complete recorded text loaded." : progress?.text === void 0 ? "Recorded text preview. " : "Part of the recorded text is loaded. "),
        action: capabilities.selected_entry ? {
          label: progress?.loading ? "Loading text\u2026" : changed ? "Reload recorded text" : progress?.text === void 0 ? "Load recorded text" : "Load more text",
          busy: !!progress?.loading,
          run() {
            void loadField(view, entry2);
          }
        } : void 0,
        ...progress?.next === null && !changed ? { action: void 0 } : {}
      };
    }
    async function loadField(view, entry2) {
      if (disposed || view.sourceKey !== capabilities.source_key || !capabilities.selected_entry || !entry2.field)
        return;
      const generation2 = view.store.selectedPage()?.projection.generation;
      if (!generation2) return;
      let progress = view.fields.get(entry2.entry_id);
      if (progress?.loading) return progress.promise;
      if (!progress || progress.generation !== generation2) {
        progress = { generation: generation2, next: 0, loading: false, note: "" };
        view.fields.set(entry2.entry_id, progress);
      }
      if (progress.next === null) return;
      const chunk = progress.next, p = params(), pending = progress;
      p.set("sid", view.key);
      p.set("after", String(entry2.slot));
      p.set("limit", "1");
      p.set("generation", generation2);
      p.set("field_chunk", String(chunk));
      pending.loading = true;
      pending.note = "";
      view.renderedRevision = -1;
      drawSelected(view);
      pending.promise = (async () => {
        try {
          const value = await api("/api/session-entry?" + p);
          if (disposed || view.fields.get(entry2.entry_id) !== pending) return;
          if (view.store.selectedPage()?.projection.generation !== generation2) {
            pending.note = "Source projection changed. Reload recorded text. ";
            return;
          }
          const field = parseCatalogField(value, {
            source_key: capabilities.source_key,
            catalog_key: view.key,
            generation: generation2,
            entry: entry2,
            chunk
          });
          pending.text = (chunk === 0 ? "" : pending.text ?? "") + field.text;
          pending.next = field.next;
        } catch (error) {
          if (disposed) return;
          pending.note = error && typeof error === "object" && "status" in error && error.status === 422 ? "Complete text is unavailable for this native format. " : "Couldn't load the full text. " + readError(error) + " ";
          if (error && typeof error === "object" && "status" in error && error.status === 409)
            void loadSelected(view);
        } finally {
          pending.loading = false;
          if (!disposed) {
            view.renderedRevision = -1;
            drawSelected(view);
          }
        }
      })();
      return pending.promise;
    }
    async function resynchronize(view) {
      const candidate = new CatalogTranscriptStore();
      candidate.select(
        { source_key: capabilities.source_key, catalog_key: view.key },
        "retained_history"
      );
      const read = async (after, limit) => {
        const ticket = candidate.request(after, limit), p = params();
        p.set("sid", view.key);
        p.set("limit", String(limit));
        if (after !== null) p.set("after", String(after));
        if (ticket.generation !== null) p.set("generation", ticket.generation);
        candidate.accept(ticket, await api("/api/session-transcript?" + p));
      };
      try {
        await read(null, 60);
        const total = candidate.selectedPage().projection.total;
        for (const range of view.store.loadedRanges())
          for (let after = range.first; after < Math.min(range.end, total); after += 100)
            await read(after, Math.min(100, Math.min(range.end, total) - after));
        if (disposed) {
          candidate.destroy();
          return;
        }
        view.store.destroy();
        view.store = candidate;
        view.renderedRevision = -1;
        view.meta = {
          ...candidate.selectedPage().session,
          ...view.observedName === void 0 ? {} : { name: view.observedName }
        };
        view.note = "";
        view.retryDelay = 1e3;
        if (active === view) {
          void loadIdentity(view);
        }
      } catch (error) {
        candidate.destroy();
        throw error;
      }
    }
    async function loadSelected(view, after = null) {
      if (view.loading && !disposed && capabilities.selected_transcript) {
        if (view.queuedAfter === void 0 || after !== null) view.queuedAfter = after;
        return;
      }
      if (disposed || view.sourceKey !== capabilities.source_key || !capabilities.selected_transcript) {
        drawSelected(view);
        return;
      }
      if (after !== null) view.following = false;
      scope.clearTimeout(view.retry);
      const request = view.store.request(after), p = params();
      p.set("sid", view.key);
      p.set("limit", String(request.limit));
      if (after !== null) p.set("after", String(after));
      if (request.generation !== null) p.set("generation", request.generation);
      view.loading = true;
      view.root.setAttribute("aria-busy", "true");
      view.note = "";
      drawSelected(view);
      try {
        const value = await api("/api/session-transcript?" + p);
        if (disposed) return;
        if (view.store.accept(request, value)) {
          void hydrateContext(view);
          view.retryDelay = 1e3;
          const current = view.store.selectedPage();
          const retainedEntries = new Set(view.store.entries().map((item2) => item2.entry_id));
          for (const key2 of view.fields.keys())
            if (!retainedEntries.has(key2)) view.fields.delete(key2);
          view.meta = {
            ...current.session,
            ...view.observedName === void 0 ? {} : { name: view.observedName }
          };
          if (active === view) {
            if (view.errorMode && view.annotations?.generation !== current.projection.generation)
              void readAnnotations(view, true);
            void loadIdentity(view);
            chrome();
          }
        }
      } catch (error) {
        if (disposed) return;
        view.store.reject(request);
        if (error && typeof error === "object" && "status" in error && error.status === 409) {
          view.note = "Recorded history changed. Resynchronizing\u2026";
          try {
            await resynchronize(view);
            return;
          } catch (failure) {
            error = failure;
          }
        }
        view.note = readError(error);
        if (retryable(error) || error && typeof error === "object" && "status" in error && error.status === 409) {
          view.retry = scope.timeout(() => void loadSelected(view, after), view.retryDelay);
          view.retryDelay = Math.min(8e3, view.retryDelay * 2);
        }
      } finally {
        view.loading = false;
        if (!disposed) view.root.setAttribute("aria-busy", "false");
        drawSelected(view);
        const queued = view.queuedAfter;
        view.queuedAfter = void 0;
        if (!disposed && active === view && queued !== void 0) void loadSelected(view, queued);
      }
    }
    async function observeSelected(view) {
      if (disposed || active !== view || view.loading || view.observing) return;
      const source = view.sourceKey, p = params();
      p.set("sid", view.key);
      p.set("limit", "1");
      view.observing = true;
      try {
        const next = parseCatalogPage(await api("/api/sessions?" + p));
        if (disposed || active !== view || capabilities.source_key !== source) return;
        const metadata = next.items[0], current = view.store.selectedPage();
        if (!metadata || metadata.key !== view.key || next.machine !== source)
          throw Error("Selected observation changed");
        if (!metadata.transcript || metadata.transcript.generation !== current?.projection.generation || metadata.transcript.total !== current.projection.total)
          await loadSelected(view);
        else if (JSON.stringify(metadata) !== JSON.stringify(view.meta)) {
          view.meta = metadata;
          view.renderedRevision = -1;
          drawSelected(view);
          chrome();
        }
        void loadIdentity(view);
      } catch (error) {
        if (!disposed && active === view) {
          view.note = readError(error);
          drawSelected(view);
        }
      } finally {
        view.observing = false;
      }
    }
    async function hydrateContext(view) {
      if (disposed || active !== view || view.contextLoading) return;
      const store = view.store, generation2 = store.selectedPage()?.projection.generation, context = store.context();
      const cache = view.contextText ??= /* @__PURE__ */ new Map();
      const owners2 = /* @__PURE__ */ new Map();
      for (const relationship of context.handoffs)
        for (const owner of [relationship.briefOwner, relationship.resultOwner]) {
          if (!owner || !context.previews[owner]) continue;
          const key2 = JSON.stringify([owner, context.previews[owner], relationship.edge.id]);
          if (cache.has(key2)) continue;
          if (!owners2.has(owner)) owners2.set(owner, /* @__PURE__ */ new Set());
          owners2.get(owner).add(relationship.edge.id);
        }
      const jobs = [...owners2].flatMap(([sid, ids]) => {
        const values = [...ids], result = [];
        for (let start = 0; start < values.length; start += 8)
          result.push({ sid, ids: values.slice(start, start + 8) });
        return result;
      });
      if (!jobs.length) return;
      view.contextLoading = true;
      const current = () => !disposed && active === view && view.store === store && store.selectedPage()?.projection.generation === generation2;
      let pending = 0;
      try {
        await Promise.all(
          Array.from({ length: Math.min(4, jobs.length) }, async () => {
            while (current() && pending < jobs.length) {
              const job = jobs[pending++];
              try {
                const params2 = new URLSearchParams({
                  machine: view.sourceKey,
                  scope: "retained_history",
                  sid: job.sid,
                  generation: context.previews[job.sid],
                  events: JSON.stringify(job.ids)
                });
                const data = object(await api("/api/session-home?" + params2));
                if (!current()) return;
                const identity2 = object(data.identity);
                if (data.api !== 1 || identity2.catalog_key !== job.sid || identity2.source_key !== view.sourceKey || data.generation !== context.previews[job.sid] || typeof data.incomplete !== "boolean")
                  throw Error("Native relationship generation changed");
                const parsed = parseNativeRelationships(data, job.ids);
                if (data.incomplete === true) {
                  if (job.ids.length > 1) {
                    const mid = Math.ceil(job.ids.length / 2);
                    jobs.push(
                      { sid: job.sid, ids: job.ids.slice(0, mid) },
                      { sid: job.sid, ids: job.ids.slice(mid) }
                    );
                  }
                  continue;
                }
                for (const row of parsed)
                  cache.set(JSON.stringify([job.sid, context.previews[job.sid], row.id]), row);
                const allowed = new Set(
                  context.handoffs.flatMap(
                    (row) => [row.briefOwner, row.resultOwner].filter((sid) => !!sid).map((sid) => JSON.stringify([sid, context.previews[sid], row.edge.id]))
                  )
                );
                for (const key2 of cache.keys()) if (!allowed.has(key2)) cache.delete(key2);
                view.renderedRevision = -1;
                drawSelected(view);
              } catch {
              }
            }
          })
        );
      } finally {
        view.contextLoading = false;
      }
    }
    async function loadIdentity(view) {
      if (disposed || view.sourceKey !== capabilities.source_key || view.identityLoading || !view.ticket || !capabilities.selected_identity)
        return;
      const ticket = view.ticket, p = currentParams();
      p.set("sid", view.key);
      view.identityLoading = true;
      let invalidated = false;
      try {
        const reply = object(await api("/api/session-identity?" + p));
        if (reply.api !== 1) throw new Error("Unsupported source identity response");
        if (active === view && selection.accept(ticket, reply.identity)) {
          if (selection.selectedIdentity()?.owner_qualification === "provisional")
            view.note = "Session identity is provisional while history is being discovered. Native controls are unavailable.";
          else if (view.note.startsWith("Session identity is provisional")) view.note = "";
          if (!viewerHost) {
            const value = object(await api("/api/session-control?" + p));
            if (!disposed && active === view && view.ticket === ticket) {
              const identity2 = selection.selectedIdentity();
              const snapshot = control.prepare(value.control);
              if (snapshot && snapshot.thread === identity2?.native_id) control.observe(snapshot);
              else control.unavailable();
            }
          }
          drawSelected(view);
        }
      } catch (error) {
        if (disposed || active !== view || ticket !== view.ticket) return;
        if (error && typeof error === "object" && "status" in error && [401, 403, 404].includes(Number(error.status)) || error instanceof Error && error.message === "Historical identity cannot supply current catalog authority") {
          invalidated = true;
          view.ticket = selection.begin({
            source_key: capabilities.source_key,
            catalog_key: view.key
          });
        }
        view.note = error && typeof error === "object" && "status" in error && error.status === 404 ? "Current native session selection is unavailable. Retained history remains readable." : readError(error);
        drawSelected(view);
      } finally {
        view.identityLoading = false;
        if (!disposed && active === view && !invalidated && view.ticket !== ticket)
          scope.timeout(() => void loadIdentity(view), 1e3);
      }
    }
    async function goSession(key2, meta, push = true, anchor) {
      if (disposed || !capabilities.source_key) return;
      cancelCandidate();
      preserveScroll();
      if (active) closeSessionComposer(active.root);
      if (!active && !sourcesOpen) listFocus = key2;
      sourcesOpen = false;
      let view = selected.get(cacheKey(key2));
      if (!view) {
        const store = new CatalogTranscriptStore();
        store.select({ source_key: capabilities.source_key, catalog_key: key2 }, "retained_history");
        view = {
          sourceKey: capabilities.source_key,
          key: key2,
          root: document.createElement("div"),
          store,
          meta: meta ?? null,
          scroll: 0,
          loading: false,
          note: "",
          opening: true,
          identityLoading: false,
          retryDelay: 1e3,
          renderedRevision: -1,
          renderedNote: "",
          fields: /* @__PURE__ */ new Map(),
          following: true,
          newCount: 0,
          lastTotal: null
        };
        view.root.className = "catalog-session";
        readerResize.observe(view.root);
        selected.set(cacheKey(key2), view);
      }
      selected.delete(cacheKey(key2));
      selected.set(cacheKey(key2), view);
      while (selected.size > 5) {
        const oldest = selected.entries().next().value;
        selected.delete(oldest[0]);
        scope.clearTimeout(oldest[1].retry);
        readerResize.unobserve(oldest[1].root);
        oldest[1].store.destroy();
        releaseScreen(oldest[1].root);
        render(null, oldest[1].root);
      }
      sessionPanel?.destroy();
      active = view;
      view.errorMode = void 0;
      view.annotationsEpoch = (view.annotationsEpoch ?? 0) + 1;
      view.annotationsLoading = false;
      view.errorRestore = void 0;
      view.findOpen = false;
      view.find = "";
      view.filter = "all";
      view.renderedRevision = -1;
      for (const node of view.root.querySelectorAll(".err-current"))
        node.classList.remove("err-current");
      if (push && !anchor) {
        view.following = true;
        view.opening = true;
      }
      sourcesOpen = false;
      view.ticket = selection.begin({ source_key: capabilities.source_key, catalog_key: key2 });
      overview.leave();
      for (const request of overviewRequests) request.abort();
      overviewRequests.clear();
      navigation.route = {
        v: "session",
        id: key2,
        turn: anchor ?? (push ? void 0 : new URLSearchParams(location.hash.slice(1)).get("turn") ?? void 0)
      };
      releaseScreen(root);
      render(null, root);
      root.replaceChildren(view.root);
      root.classList.toggle("wide-mode", wide);
      shell.closeDrawer(true);
      if (push)
        history.pushState(
          navigation.route,
          "",
          "/s/" + encodeURIComponent(view.meta?.harness ?? "native") + "/" + encodeURIComponent(key2) + "?" + browsingParams() + (anchor ? "#turn=" + encodeURIComponent(anchor) : "")
        );
      chrome();
      drawSelected(view);
      const turn = new URLSearchParams(location.hash.slice(1)).get("turn");
      view.anchorNote = void 0;
      if (turn) {
        view.following = false;
        view.opening = false;
        const sourceKey = view.sourceKey;
        const p = params();
        p.set("sid", key2);
        p.set("turn", turn);
        try {
          const anchor2 = object(await api("/api/session-anchor?" + p));
          if (disposed || active !== view || sourceKey !== capabilities.source_key || new URLSearchParams(location.hash.slice(1)).get("turn") !== turn)
            return;
          if (anchor2.api !== 1 || anchor2.source_key !== sourceKey || anchor2.catalog_key !== key2 || anchor2.turn !== turn || anchor2.slot !== null && (!Number.isSafeInteger(anchor2.slot) || Number(anchor2.slot) < 0))
            throw Error("Invalid turn anchor");
          if (anchor2.slot === null) {
            view.anchorNote = "This recorded turn is unavailable in the retained history.";
            drawSelected(view);
          } else {
            await loadSelected(view, Number(anchor2.slot));
            if (active === view && !disposed)
              scope.timeout(() => {
                if (disposed || active !== view) return;
                const target = [...view.root.querySelectorAll("[data-turn]")].find(
                  (node) => node.dataset.turn === turn
                );
                target?.scrollIntoView({ block: "start" });
                target?.classList.add("flash");
                scope.timeout(() => target?.classList.remove("flash"), 1400);
              }, 0);
            return;
          }
        } catch (error) {
          if (active === view && !disposed) {
            view.anchorNote = readError(error);
            drawSelected(view);
          }
        }
      }
      if (view.store.selectedPage()) {
        void loadIdentity(view);
        view.opening = false;
        if (push && !turn) scrollEnd();
        else if (window.matchMedia("(max-width: 760px)").matches) window.scrollTo(0, view.scroll);
        else document.querySelector("#main").scrollTop = view.scroll;
      } else {
        void loadIdentity(view);
        if (capabilities.selected_transcript) await loadSelected(view);
      }
    }
    async function refreshMetadata(sourceKey, epoch) {
      const p = params();
      p.set("limit", "60");
      if (harness) p.set("harness", harness);
      if (repo === "__none__") p.set("no_repo", "1");
      else if (repo) p.set("repo", repo);
      if (model2) p.set("model", model2);
      if (query && capabilities.metadata_search) p.set("q", query);
      const value = parseCatalogPage(await api("/api/sessions?" + p));
      if (disposed || epoch !== sourcesEpoch || sourceKey !== capabilities.source_key || value.machine !== sourceKey || value.read_scope !== "retained_history")
        return;
      const observed = new Map(value.items.map((item2) => [item2.key, item2]));
      const names = new Map(value.items.map((item2) => [item2.key, item2.name]));
      const view = active;
      if (view && !names.has(view.key)) {
        const selectedParams = params();
        selectedParams.set("sid", view.key);
        const selectedMeta = parseCatalogPage(await api("/api/sessions?" + selectedParams));
        if (disposed || epoch !== sourcesEpoch || sourceKey !== capabilities.source_key || selectedMeta.machine !== sourceKey || selectedMeta.read_scope !== "retained_history")
          return;
        const match = selectedMeta.items.find((item2) => item2.key === view.key);
        if (match) names.set(match.key, match.name);
      }
      const remaining = [
        .../* @__PURE__ */ new Set([
          ...items.map((item2) => item2.key),
          ...[...selected.values()].filter((view2) => view2.sourceKey === sourceKey).map((view2) => view2.key)
        ])
      ].filter((key2) => !names.has(key2));
      for (let i = 0; i < Math.min(8, remaining.length); i++) {
        const key2 = remaining[(nameRefreshOffset + i) % remaining.length];
        const lookup = params();
        lookup.set("sid", key2);
        const metadata = parseCatalogPage(await api("/api/sessions?" + lookup));
        if (disposed || epoch !== sourcesEpoch || sourceKey !== capabilities.source_key || metadata.machine !== sourceKey || metadata.read_scope !== "retained_history")
          return;
        const item2 = metadata.items.find((item3) => item3.key === key2);
        if (item2) names.set(key2, item2.name);
      }
      if (remaining.length) nameRefreshOffset = (nameRefreshOffset + 8) % remaining.length;
      const before = JSON.stringify(items);
      const previousKeys = new Set(items.map((item2) => item2.key));
      const refreshed = items.map((item2) => observed.get(item2.key) ?? item2);
      items = [...value.items.filter((item2) => !previousKeys.has(item2.key)), ...refreshed].slice(
        0,
        300
      );
      const changedPage = !page || page.generation !== value.generation && items.length <= 60;
      if (changedPage) {
        page = value;
        cursor = value.next_cursor;
      }
      let changed = before !== JSON.stringify(items);
      for (const cached of selected.values()) {
        const name = names.get(cached.key);
        if (cached.sourceKey === sourceKey && cached.meta && name !== void 0 && cached.meta.name !== name) {
          cached.observedName = name;
          cached.meta = { ...cached.meta, name };
          updateSessionName(cached.root, name);
          changed = true;
        }
      }
      if (changed) {
        if (!active && !sourcesOpen) drawList();
        chrome();
      }
    }
    async function recheckCapabilities() {
      const observedSource = capabilities.source_key, observedEpoch = sourcesEpoch;
      try {
        if (viewerHost?.nativePage || navigation.route.v === "home" || navigation.route.v === "analytics" || navigation.route.v === "machines") {
          await overview.refresh();
          return;
        }
        if (navigation.route.v === "trace") {
          await loadCausal();
          return;
        }
        if (!capabilities.source_key) return;
        const next = parseCatalogCapabilities(
          await api("/api/session-capabilities?" + currentParams())
        );
        if (disposed || observedEpoch !== sourcesEpoch || observedSource !== capabilities.source_key)
          return;
        if (next.source_key !== capabilities.source_key)
          throw new Error("Source selection changed. Choose the source again.");
        const ready = !capabilities.selected_transcript && next.selected_transcript;
        Object.assign(capabilities, next);
        if (next.source_candidates && !candidateLoaded && !active && !sourcesOpen)
          void loadCandidates(false);
        if (active) void loadIdentity(active);
        if (ready && active && !active.store.selectedPage()) void loadSelected(active);
        await refreshMetadata(observedSource, observedEpoch);
      } catch (error) {
        if (disposed || observedEpoch !== sourcesEpoch || observedSource !== capabilities.source_key)
          return;
        if (active && !active.store.selectedPage()) {
          active.note = readError(error);
          drawSelected(active);
        }
      } finally {
        if (!disposed) scope.timeout(() => void recheckCapabilities(), 8e3);
      }
    }
    function drawSources() {
      if (disposed || !sourcesOpen) return;
      chrome();
      renderCatalogSources(
        root,
        {
          items: sourceItems,
          updating: sourcesUpdating,
          note: sourcesNote,
          more: sourceCursor !== null,
          machinesHref: machinesHref(),
          bookmarks: bookmarkMatches
        },
        {
          select(key2) {
            if (sourceItems.some((item2) => item2.source_key === key2)) void switchSource(key2);
          },
          more() {
            void loadSources(true);
          },
          retry() {
            void loadSources(false);
          }
        }
      );
    }
    async function loadSources(append) {
      if (disposed || !sourcesOpen || sourcesUpdating) return;
      const epoch = ++sourcesEpoch, p = new URLSearchParams({ limit: "60" });
      if (append && sourceCursor !== null) p.set("cursor", sourceCursor);
      sourcesUpdating = true;
      sourcesNote = "";
      drawSources();
      try {
        const next = parseCatalogSources(await api("/api/session-sources?" + p));
        if (disposed || !sourcesOpen || epoch !== sourcesEpoch) return;
        if (append && next.items.some((item2) => sourceItems.some((old) => old.source_key === item2.source_key)))
          throw new Error("Source page repeated a machine");
        sourceItems = append ? [...sourceItems, ...next.items] : next.items;
        sourceCursor = next.next_cursor;
        sourcesRetryDelay = 1e3;
      } catch (error) {
        if (disposed || !sourcesOpen || epoch !== sourcesEpoch) return;
        sourcesNote = readError(error);
        if (retryable(error)) {
          scope.timeout(() => {
            if (sourcesOpen && epoch === sourcesEpoch) void loadSources(append);
          }, sourcesRetryDelay);
          sourcesRetryDelay = Math.min(8e3, sourcesRetryDelay * 2);
        }
      } finally {
        if (!disposed && epoch === sourcesEpoch) {
          sourcesUpdating = false;
          drawSources();
        }
      }
    }
    function saveSource() {
      preserveScroll();
      if (!capabilities.source_key) return;
      sourceViews.delete(capabilities.source_key);
      sourceViews.set(capabilities.source_key, {
        page,
        items,
        cursor,
        harness,
        repo,
        model: model2,
        query,
        listScroll,
        listFocus,
        selectedKey: active?.key ?? null
      });
      while (sourceViews.size > 5) sourceViews.delete(sourceViews.keys().next().value);
    }
    async function showSources() {
      if (disposed) return;
      bookmarkMatches = [];
      if (!sourcesOpen) saveSource();
      leaveScreen();
      navigation.route = { v: "sessions" };
      sourcesOpen = true;
      root.replaceChildren();
      shell.closeDrawer(true);
      drawSources();
      if (!sourceItems.length) await loadSources(false);
    }
    async function switchSource(key2, push = true) {
      if (disposed) return;
      const epoch = ++sourcesEpoch;
      sourcesUpdating = true;
      sourcesNote = "";
      drawSources();
      try {
        const next = parseCatalogCapabilities(
          await api("/api/session-capabilities?" + new URLSearchParams({ machine: key2 }))
        );
        if (disposed || epoch !== sourcesEpoch) return;
        if (next.source_key !== key2)
          throw new Error("Source capability identity does not match this selection");
        if (!sourcesOpen) saveSource();
        ++listEpoch;
        scope.clearTimeout(listRetry);
        selection.clear();
        Object.assign(capabilities, next);
        const retained = sourceViews.get(key2);
        page = retained?.page ?? null;
        items = retained?.items ?? [];
        cursor = retained?.cursor ?? null;
        cancelCandidate();
        candidates = [];
        candidateCursor = null;
        candidateLoaded = false;
        candidateNote = "";
        const requested = new URLSearchParams(location.search);
        harness = requested.get("harness") ?? retained?.harness ?? "";
        repo = requested.get("no_repo") === "1" ? "__none__" : requested.get("repo") ?? retained?.repo ?? "";
        model2 = requested.get("model") ?? retained?.model ?? "";
        query = requested.get("q") ?? retained?.query ?? "";
        listScroll = retained?.listScroll ?? 0;
        listFocus = retained?.listFocus ?? "";
        updating = false;
        note = "";
        sourcesOpen = false;
        active = null;
        const bookmark = location.pathname.startsWith("/s/") ? location.pathname.split("/")[3] : null;
        if (push && bookmark) {
          await goSession(decodeURIComponent(bookmark), void 0, true);
          chrome();
          return;
        }
        if (push && retained?.selectedKey) await goSession(retained.selectedKey, void 0, false);
        else goList(false);
        const url = retained?.selectedKey ? "/s/" + encodeURIComponent(
          selected.get(cacheKey(retained.selectedKey))?.meta?.harness ?? "native"
        ) + "/" + encodeURIComponent(retained.selectedKey) : "/sessions";
        if (push) history.pushState(navigation.route, "", url + "?" + browsingParams());
        else fromLocation();
        chrome();
        void loadList(false);
      } catch (error) {
        if (disposed || epoch !== sourcesEpoch) return;
        sourcesNote = readError(error);
        if (retryable(error))
          scope.timeout(() => {
            if (!disposed && epoch === sourcesEpoch && new URLSearchParams(location.search).get("machine") === key2)
              void switchSource(key2, false);
          }, 1e3);
      } finally {
        if (!disposed && epoch === sourcesEpoch) {
          sourcesUpdating = false;
          drawSources();
        }
      }
    }
    async function locateBookmark(key2, sourceKey) {
      if (locating) return;
      locating = true;
      const path = location.pathname;
      const current = () => !disposed && location.pathname === path && (sourceKey ? capabilities.source_key === sourceKey : !new URLSearchParams(location.search).has("machine"));
      const retry = () => scope.timeout(() => {
        if (current()) void locateBookmark(key2, sourceKey);
      }, 1e3);
      try {
        const reply = object(
          await api(
            "/api/session-locate?" + new URLSearchParams({ sid: key2, ...sourceKey ? { machine: sourceKey } : {} })
          )
        );
        if (!current()) return;
        if (reply.api !== 1 || !Array.isArray(reply.matches) || !Number.isSafeInteger(reply.unavailable) || Number(reply.unavailable) < 0)
          throw Error("Bookmark lookup unavailable");
        if (reply.matches.length === 1 && reply.unavailable === 0) {
          const match = object(reply.matches[0]);
          if (typeof match.source_key !== "string" || typeof match.catalog_key !== "string")
            throw Error("Invalid bookmark selection");
          const url = new URL(location.href);
          url.searchParams.set("machine", match.source_key);
          if (match.catalog_key !== key2) {
            url.pathname = "/s/" + encodeURIComponent(typeof match.harness === "string" ? match.harness : "native") + "/" + encodeURIComponent(match.catalog_key);
            url.hash = "";
          }
          history.replaceState(history.state, "", url.pathname + url.search + url.hash);
          fromLocation();
        } else {
          bookmarkMatches = reply.matches.slice(0, 100).map((value) => {
            const match = object(value);
            return {
              source_key: text(match.source_key),
              catalog_key: text(match.catalog_key),
              harness: typeof match.harness === "string" ? match.harness : "native"
            };
          });
          leaveScreen();
          navigation.route = { v: "sessions" };
          sourcesOpen = true;
          sourcesNote = Number(reply.unavailable) > 0 ? "The recorded session index is updating. Retrying\u2026" : reply.matches.length > 1 ? "This bookmark has several recorded origins. Choose a session." : "Choose a machine to resolve this recorded session.";
          drawSources();
          if (Number(reply.unavailable) > 0) retry();
        }
      } catch (error) {
        if (current()) {
          sourcesNote = readError(error);
          drawSources();
          if (retryable(error)) retry();
        }
      } finally {
        locating = false;
      }
    }
    function fromLocation() {
      const queryParams = new URLSearchParams(location.search);
      const machineBookmark = location.pathname.match(/^\/(?:m|machines)\/([^/]+)$/);
      if (machineBookmark && !viewerHost?.nativePage) {
        void resolveMachineBookmark(machineBookmark[1]);
        return;
      }
      if (location.pathname === "/machines" && !viewerHost?.nativePage) {
        if (machinesHref() !== "/machines") {
          location.assign(machinesHref());
          return;
        }
        goOverview("machines", false);
        return;
      }
      if (location.pathname === "/" || location.pathname === "/analytics" || location.pathname === "/timeline") {
        goOverview(location.pathname === "/" ? "home" : "analytics", false);
        return;
      }
      if (viewerHost?.nativePage && location.pathname !== "/sessions" && !location.pathname.startsWith("/s/")) {
        navigation.route = { v: "machines" };
        chrome();
        void overview.refresh();
        return;
      }
      const machine = queryParams.get("machine");
      if (queryParams.get("choose_source") === "1" || viewerHost?.catalogSources && !machine) {
        void showSources().then(() => {
          const parts2 = location.pathname.split("/");
          if ((parts2[1] === "s" || parts2[1] === "trace") && parts2[3])
            void locateBookmark(decodeURIComponent(parts2[3]));
        });
        return;
      }
      if (!capabilities.source_key || machine && machine !== capabilities.source_key) {
        if (machine || viewerHost?.catalogSources) void showSources();
        else {
          leaveScreen();
          navigation.route = { v: "sessions" };
          sourcesOpen = true;
          drawSources();
        }
        if (machine) void switchSource(machine, false);
        else
          void api("/api/session-capabilities").then((value) => {
            if (disposed) return;
            Object.assign(capabilities, parseCatalogCapabilities(value));
            fromLocation();
          }).catch((error) => {
            if (disposed) return;
            sourcesNote = readError(error);
            drawSources();
            if (retryable(error))
              scope.timeout(() => {
                if (!disposed && !capabilities.source_key && location.pathname !== "/" && location.pathname !== "/analytics")
                  fromLocation();
              }, 1e3);
          });
        return;
      }
      harness = queryParams.get("harness") ?? "";
      repo = queryParams.get("no_repo") === "1" ? "__none__" : queryParams.get("repo") ?? "";
      model2 = queryParams.get("model") ?? "";
      query = queryParams.get("q") ?? "";
      const parts = location.pathname.split("/").filter(Boolean);
      if (parts[0] === "trace" && parts[2] && parts[3]) {
        goCausal(decodeURIComponent(parts[2]), decodeURIComponent(parts[3]));
      } else if (parts[0] === "s" && parts[2]) {
        try {
          const key2 = decodeURIComponent(parts[2]);
          if (key2.startsWith("unsent:")) void locateBookmark(key2, capabilities.source_key);
          else void goSession(key2, void 0, false);
        } catch {
          goList(false);
        }
      } else goList(false);
    }
    scope.listen(window, "popstate", () => {
      if (overlayBackPending) {
        overlayBackPending = false;
        const action = afterOverlayClose;
        afterOverlayClose = null;
        if (!disposed) action?.();
        return;
      }
      restoringOverlayHistory = true;
      try {
        if (sessionPanel && !history.state?.sheet) {
          sessionPanel.destroy();
          if (history.state?.v === navigation.route.v && history.state?.sid === ("sid" in navigation.route ? navigation.route.sid : void 0) && history.state?.id === ("id" in navigation.route ? navigation.route.id : void 0) && history.state?.turn === ("turn" in navigation.route ? navigation.route.turn : void 0) && (!active || new URLSearchParams(location.search).get("machine") === active.sourceKey))
            return;
        }
        if (shell.account.open && !history.state?.sheet) {
          shell.account.close();
          return;
        }
        if (shell.drawerOpen && !history.state?.drawer) {
          shell.closeDrawer(true);
          return;
        }
        fromLocation();
      } finally {
        restoringOverlayHistory = false;
      }
    });
    async function resolveMachineBookmark(encoded) {
      const path = location.pathname;
      await showSources();
      if (disposed || path !== location.pathname) return;
      let key2;
      try {
        key2 = decodeURIComponent(encoded);
      } catch {
        sourcesNote = "This machine bookmark is invalid.";
        drawSources();
        return;
      }
      const exact = sourceItems.find((source) => source.source_key === key2);
      const matches = exact ? [exact] : sourceItems.filter((source) => source.label === key2);
      if (matches.length === 1 && (exact || !sourceCursor)) {
        const url = new URL(location.href);
        url.pathname = "/sessions";
        url.searchParams.set("machine", matches[0].source_key);
        url.searchParams.delete("choose_source");
        history.replaceState(history.state, "", url.pathname + url.search + url.hash);
        fromLocation();
      } else {
        sourcesNote = matches.length > 1 ? "Several machines share this name. Choose the source for this bookmark." : "Choose the source for this machine bookmark.";
        drawSources();
      }
    }
    const readingScroll = () => {
      if (!active || active.opening || !active.store.selectedPage()) return;
      const main = document.querySelector("#main"), gap = window.matchMedia("(max-width: 760px)").matches ? document.documentElement.scrollHeight - window.scrollY - window.innerHeight : main.scrollHeight - main.scrollTop - main.clientHeight;
      if (active === pinView && performance.now() < pinUntil && !active.errorMode && gap > 1 && gap <= 80) {
        active.following = true;
        scrollEnd();
        return;
      }
      active.following = gap <= 80;
      if (!active.following) pinUntil = 0;
      if (active.following) active.newCount = 0;
      syncJump(active);
    };
    scope.listen(window, "scroll", readingScroll, { passive: true });
    scope.listen(document.querySelector("#main"), "scroll", readingScroll, {
      passive: true
    });
    scope.listen(document, "keydown", (event) => {
      if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(event.key) && !(event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement))
        pinUntil = 0;
      if (active?.errorMode && !sessionPanel && !event.altKey && !event.ctrlKey && !event.metaKey && !(event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) && ["n", "p", "Enter"].includes(event.key)) {
        event.preventDefault();
        const direction = event.key === "p" || event.key === "Enter" && event.shiftKey ? -1 : 1;
        const count2 = active.annotations?.total ?? 0;
        if (count2) {
          active.errorIndex = ((active.errorIndex ?? 0) + direction + count2) % count2;
          void readAnnotations(active);
          chrome();
        }
      }
      if (event.key === "Escape") {
        if (sessionPanel) return;
        if (active?.errorMode) {
          closeAnnotations(active);
        } else if (active?.findOpen) {
          active.findOpen = false;
          active.find = "";
          active.filter = "all";
          active.renderedRevision = -1;
          drawSelected(active);
          chrome();
          document.querySelector("#find-btn")?.focus();
        } else if (shell.account.open) shell.account.escape();
        else shell.closeDrawer();
      }
    });
    scope.interval(() => {
      if (active?.meta?.details?.live_observation) updateSessionClock(active.root, Date.now(), {});
    }, 1e3);
    const removeNavigation = viewerHost?.subscribeNavigation?.(chrome);
    if (!viewerHost)
      void api("/api/viewer/chrome").then((value) => {
        if (disposed) return;
        const data = object(value);
        if (data.api !== 1) throw Error("Unsupported chrome metadata");
        account = parseAccount(data.account);
        standaloneNavigation = parseShellNavigation(data.navigation) ?? void 0;
        if (location.pathname === "/machines" && machinesHref() !== "/machines") {
          fromLocation();
          return;
        }
        chrome();
      }).catch(() => {
      });
    fromLocation();
    scope.timeout(() => void recheckCapabilities(), 8e3);
    scope.interval(() => {
      if (active && capabilities.selected_transcript && active.store.selectedPage() && document.visibilityState === "visible")
        void observeSelected(active);
    }, 3e3);
    return {
      destroy() {
        if (disposed) return;
        disposed = true;
        afterOverlayClose = null;
        if (!viewerHost?.nativePage && history.scrollRestoration === "manual")
          history.scrollRestoration = previousScrollRestoration;
        ++listEpoch;
        observation.destroy();
        removeRuntimeListener();
        runtimeObservation.destroy();
        removeNavigation?.();
        scope.destroy();
        readerResize.disconnect();
        sessionPanel?.destroy();
        sessionBar.destroy();
        viewerHost?.initialMachines?.destroy();
        viewerHost?.initialHome?.destroy();
        selection.destroy();
        navigation.destroy();
        control.destroy();
        for (const view of selected.values()) {
          view.store.destroy();
          releaseScreen(view.root);
          render(null, view.root);
        }
        overview.destroy();
        releaseGeometry(shell.slots.recent, ["scrollPaddingTop"]);
        recent.destroy();
        releaseScreen(root);
        render(null, root);
        shell.destroy();
        selected.clear();
      }
    };
  }

  // src/app/viewer.ts
  var mounted = null;
  function mountViewerApplication(host2 = getViewerHost()) {
    mounted?.destroy();
    const url = new URL(location.href);
    const before = url.href;
    if (url.pathname === "/timeline") url.pathname = "/analytics";
    if (url.pathname.startsWith("/s/") && url.searchParams.has("turn")) {
      url.hash = "turn=" + encodeURIComponent(url.searchParams.get("turn"));
      url.searchParams.delete("turn");
    }
    if (url.searchParams.has("compat")) url.searchParams.delete("compat");
    if (url.href !== before) {
      history.replaceState(history.state, "", url.pathname + url.search + url.hash);
    }
    const owner = createCatalogViewer(null, host2);
    const application = {
      destroy() {
        owner.destroy();
        if (mounted === application) mounted = null;
      }
    };
    mounted = application;
    return application;
  }

  // src/viewer.ts
  queueMicrotask(mountViewerApplication);
})();
