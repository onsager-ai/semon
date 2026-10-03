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

  // ../../semon/ui/node_modules/preact/dist/preact.module.js
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

  // ../../semon/ui/node_modules/preact/hooks/dist/hooks.module.js
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

  // ../../semon/ui/node_modules/preact/jsx-runtime/dist/jsxRuntime.module.js
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
  var swallow = 0;
  var sheetList = null;
  var touchY = 0;
  var installed2 = false;
  var phone = () => window.matchMedia("(max-width: 760px)").matches;
  function orphaned() {
    if (!sheetList || sheetList.isConnected) return false;
    lock(null);
    sheet = null;
    return true;
  }
  function refuse(event) {
    if (orphaned() || !sheetList) return;
    if (event instanceof TouchEvent && event.type === "touchstart") {
      touchY = event.touches[0]?.clientY ?? 0;
      return;
    }
    if (!(event.target instanceof Node) || !sheetList.contains(event.target)) {
      event.preventDefault();
      return;
    }
    const room = sheetList.scrollHeight - sheetList.clientHeight, delta = event instanceof WheelEvent ? event.deltaY : event instanceof TouchEvent ? touchY - (event.touches[0]?.clientY ?? touchY) : 0;
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
          swallow = 0;
          opened.entry = true;
          sheet = {
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
        if (was.entry) {
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
    for (const [key, value] of Object.entries({
      viewBox: "0 0 24 24",
      "aria-hidden": "true",
      fill: "none",
      stroke: "currentColor",
      "stroke-width": stroke,
      "stroke-linecap": "round",
      "stroke-linejoin": "round"
    }))
      node.setAttribute(key, value);
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
    let recent, content;
    let desktopAccount = null, phoneAccount = null;
    let lead = null, destroyed = false, sx = null;
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
        return { recent, content };
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
        content = required("#page");
        app = container;
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
                    if (active(event.currentTarget)) host2.fewer(item2.id);
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

  // src/lib/screens.tsx
  var owned = /* @__PURE__ */ new WeakSet();
  var kinds = /* @__PURE__ */ new WeakMap();
  var screenKind = (root) => kinds.get(root);
  var cleanups = /* @__PURE__ */ new WeakMap();
  function claimScreen(root, kind, cleanup) {
    owned.add(root);
    kinds.set(root, kind);
    cleanups.set(root, cleanup);
  }
  function ownsScreen(root) {
    return owned.has(root);
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
  function renderMachinesScreen(root, snapshot, host2) {
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
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
      ] }),
      root
    );
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
    mark,
    size = 14,
    lead = true
  }) {
    return /* @__PURE__ */ jsx(
      "span",
      {
        class: "hicon " + (size === 16 ? "hi-screen-16" : "hi-screen") + (lead ? " hi-lead" : ""),
        "data-harness": mark.id,
        "aria-hidden": "true",
        children: mark.light === mark.dark ? /* @__PURE__ */ jsx("img", { src: mark.light, alt: "", draggable: false, decoding: "async" }) : /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(
            "img",
            {
              class: "hi-light",
              loading: mark.darkTheme ? "lazy" : void 0,
              src: mark.light,
              alt: "",
              draggable: false,
              decoding: "async"
            }
          ),
          /* @__PURE__ */ jsx(
            "img",
            {
              class: "hi-dark",
              loading: mark.darkTheme ? void 0 : "lazy",
              src: mark.dark,
              alt: "",
              draggable: false,
              decoding: "async"
            }
          )
        ] })
      }
    );
  }
  function Section({ heading, count }) {
    return /* @__PURE__ */ jsxs("div", { class: "sec-h", children: [
      screenText(heading),
      /* @__PURE__ */ jsx("span", { class: "n", children: count })
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
  function renderHomeScreen(root, snapshot, host2) {
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
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
        /* @__PURE__ */ jsx(Section, { heading: "Needs you", count: snapshot.waiting }),
        /* @__PURE__ */ jsxs("div", { class: "list", children: [
          snapshot.inbox.map((row) => /* @__PURE__ */ jsx(Inbox, { row, host: host2 }, row.id)),
          !snapshot.waiting && /* @__PURE__ */ jsx("p", { class: "empty", children: "Nothing is waiting on you." })
        ] }),
        /* @__PURE__ */ jsx(Section, { heading: "Working now", count: snapshot.working }),
        /* @__PURE__ */ jsxs("div", { class: "list", children: [
          snapshot.live.map((row) => /* @__PURE__ */ jsx(Live, { row, host: host2 }, row.id)),
          !snapshot.working && /* @__PURE__ */ jsx("p", { class: "empty", children: "Nothing is running." })
        ] }),
        snapshot.totalAnswered > 0 && /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Section, { heading: "Answered", count: snapshot.totalAnswered }),
          /* @__PURE__ */ jsxs("div", { class: "list", children: [
            snapshot.answered.map((row) => /* @__PURE__ */ jsx(Inbox, { row, host: host2 }, row.id)),
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
      ] }),
      root
    );
    owned.add(root);
    kinds.set(root, "home");
    host2.committed();
  }
  function renderMachineScreen(root, snapshot, host2) {
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx("div", { class: "ph sr", children: /* @__PURE__ */ jsx("h1", { children: screenText(snapshot.name) }) }),
        snapshot.totalSessions > 0 && /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Section, { heading: "Sessions", count: snapshot.totalSessions }),
          /* @__PURE__ */ jsx("div", { class: "list", children: snapshot.sessions.map((row) => /* @__PURE__ */ jsx(Live, { row, host: host2 }, row.id)) })
        ] }),
        snapshot.off.length > 0 && /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Section, { heading: "Moved off", count: snapshot.off.length }),
          /* @__PURE__ */ jsx("div", { class: "list", children: snapshot.off.map((row) => /* @__PURE__ */ jsx(Live, { row, host: host2 }, row.id)) })
        ] }),
        snapshot.moves.length > 0 && /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Section, { heading: "Moves", count: snapshot.moves.length }),
          /* @__PURE__ */ jsx("div", { class: "list", children: snapshot.moves.map((row) => /* @__PURE__ */ jsx(Inbox, { row, host: host2 }, row.id)) })
        ] }),
        !snapshot.totalSessions && !snapshot.off.length && /* @__PURE__ */ jsx("p", { class: "empty", children: "No sessions have run here." })
      ] }),
      root
    );
    owned.add(root);
    kinds.set(root, "machine");
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
  function Session({ row, host: host2 }) {
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
  function renderSessionsScreen(root, snapshot, host2, focusSearch = false) {
    let state2 = sessionsState.get(root);
    if (!state2) {
      state2 = { input: null, frame: 0, resize: () => fitSessionRows(root) };
      sessionsState.set(root, state2);
      window.addEventListener("resize", state2.resize, { passive: true });
    }
    const current = state2;
    const fieldValue = current.input?.value.trim() === snapshot.query ? current.input.value : snapshot.query;
    const facet = host2.facets();
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsxs("div", { class: "ph", children: [
          /* @__PURE__ */ jsx("h1", { children: "Sessions" }),
          /* @__PURE__ */ jsxs("div", { class: "sub", children: [
            /* @__PURE__ */ jsxs("span", { children: [
              /* @__PURE__ */ jsx("b", { children: snapshot.total }),
              snapshot.total === 1 ? "session" : "sessions"
            ] }),
            /* @__PURE__ */ jsxs("span", { children: [
              /* @__PURE__ */ jsx("b", { children: snapshot.working }),
              "working"
            ] }),
            /* @__PURE__ */ jsxs("span", { children: [
              /* @__PURE__ */ jsx("b", { children: snapshot.waiting }),
              "waiting on you"
            ] })
          ] })
        ] }),
        /* @__PURE__ */ jsx(
          "div",
          {
            class: "session-facet-slot",
            ref: (node) => {
              if (node && facet.parentElement !== node) node.append(facet);
            }
          }
        ),
        /* @__PURE__ */ jsxs("label", { class: "find", children: [
          /* @__PURE__ */ jsx(Glyph, { path: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4" }),
          /* @__PURE__ */ jsx(
            "input",
            {
              id: "sq",
              type: "search",
              placeholder: "Search sessions",
              "aria-label": "Search sessions",
              value: fieldValue,
              ref: (node) => {
                current.input = node;
              },
              onInput: (event) => {
                if (event.currentTarget.isConnected) host2.search(event.currentTarget.value);
              }
            }
          )
        ] }),
        /* @__PURE__ */ jsx("div", { class: "groupby", role: "group", "aria-label": "Group by", children: [
          ["recent", "Recent"],
          ["project", "Project"],
          ["machine", "Machine"],
          ["harness", "Harness"]
        ].map(([key, label]) => /* @__PURE__ */ jsx(
          "button",
          {
            type: "button",
            "data-g": key,
            "aria-pressed": snapshot.groupBy === key,
            onClick: (event) => {
              if (event.currentTarget.isConnected) host2.group(key);
            },
            children: label
          },
          key
        )) }),
        snapshot.showReviews && /* @__PURE__ */ jsx("div", { class: "groupby", role: "group", "aria-label": "Session visibility", children: /* @__PURE__ */ jsx(
          "button",
          {
            type: "button",
            "data-show-approval-reviews": "",
            "aria-pressed": snapshot.reviews,
            onClick: (event) => {
              if (event.currentTarget.isConnected) host2.reviews();
            },
            children: "Show approval reviews"
          }
        ) }),
        /* @__PURE__ */ jsxs("div", { class: "sess session-groups", children: [
          snapshot.groups.map((group) => /* @__PURE__ */ jsxs("div", { class: "session-group", children: [
            group.title && /* @__PURE__ */ jsxs("div", { class: "sec-h", children: [
              screenText(group.title),
              /* @__PURE__ */ jsx("span", { class: "n", children: group.total }),
              group.harness && /* @__PURE__ */ jsx(Harness, { mark: group.harness, lead: false })
            ] }),
            /* @__PURE__ */ jsx("div", { class: "list", children: group.rows.map((row) => /* @__PURE__ */ jsx(Session, { row, host: host2 }, row.id)) })
          ] }, group.title)),
          !snapshot.matches && /* @__PURE__ */ jsx("p", { class: "empty", children: "No sessions match \u201C" + screenText(snapshot.query) + "\u201D." })
        ] })
      ] }),
      root
    );
    owned.add(root);
    kinds.set(root, "sessions");
    host2.committed();
    cancelAnimationFrame(current.frame);
    current.frame = requestAnimationFrame(current.resize);
    if (focusSearch) current.input?.focus({ preventScroll: true });
  }

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
    const lines = (data.out ?? "").split("\n"), cut2 = lines.length > 12, tail = data.ok === false, parts = !!data.cut?.parts?.length, more = !!data.more?.length;
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
    return data.cut ? /* @__PURE__ */ jsx("p", { class: "vnote", children: screenText(cutNoteText(data.cut)) }) : !data.fullFailed && /…(\(truncated\))?\s*$/.test(text2 ?? "") ? /* @__PURE__ */ jsx("p", { class: "vnote", children: "Cut short in this copy of the logs." }) : null;
  }
  function renderFullTool(root, data, inputLabel, waiting) {
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        data.scriptText !== void 0 ? /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Header, { className: "vs", label: "Script", text: data.scriptText }),
          /* @__PURE__ */ jsx("pre", { class: "script", children: data.scriptText }),
          data.scriptTruncated && /* @__PURE__ */ jsx("p", { class: "vnote", children: "Cut at 8 MB: the rest isn't shown." })
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
        !!data.fullCut?.length && /* @__PURE__ */ jsx("p", { class: "vnote", children: "Cut at 8 MB: the rest isn't shown." })
      ] }),
      root
    );
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

  // src/lib/analytics.tsx
  var states = /* @__PURE__ */ new WeakMap();
  var COST_INFO = "M9 9a3 3 0 1 1 4 2.8c-.7.3-1 .9-1 1.7V14M12 18h.01";
  function Info({ text: text2 }) {
    return /* @__PURE__ */ jsx("span", { class: "cost-info", tabIndex: 0, role: "img", "aria-label": text2, "data-tip": text2, children: /* @__PURE__ */ jsx(Glyph, { path: COST_INFO, className: "" }) });
  }
  function Row({ row, open, widthClass }) {
    const content = /* @__PURE__ */ jsxs(Fragment2, { children: [
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
        children: content
      }
    ) : /* @__PURE__ */ jsx("button", { class: cls, type: "button", onClick: open, children: content }) : /* @__PURE__ */ jsx("div", { class: cls, children: content });
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
    render(
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
            ].map(([key, label]) => /* @__PURE__ */ jsx(
              "button",
              {
                type: "button",
                "data-measure": key,
                "aria-pressed": snapshot.measure === key,
                onClick: () => active(() => host2.measure(key)),
                children: label
              },
              key
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
      ] }),
      root
    );
    host2.committed();
  }
  function renderSliceBody(root, rows, more, empty, open) {
    render(
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
      ] }) }),
      root
    );
  }
  function renderModelItems(root, count, items, more, host2) {
    render(
      /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx("p", { class: "panel-sub", children: screenText(count) }),
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
      ] }),
      root
    );
  }

  // src/lib/sentence.tsx
  function Sentence({ parts, host: host2 }) {
    return /* @__PURE__ */ jsx(Fragment2, { children: parts.map(
      (part, i) => part.action ? /* @__PURE__ */ jsx(
        "button",
        {
          class: part.className,
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
      ) : /* @__PURE__ */ jsx("span", { class: part.className, "data-tip": part.tip, children: screenText(part.text) }, i)
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
      for (const [key, pair] of owner.briefs) {
        if (owner.open.has(key) || !pair.node.clientHeight) continue;
        const clipped = pair.node.scrollHeight > pair.node.clientHeight + 1;
        if (owner.clipped.has(key) !== clipped) {
          changed = true;
          if (clipped) owner.clipped.add(key);
          else owner.clipped.delete(key);
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
      render(
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
                      hop.answers.length === 1 ? hop.answers[0] : /* @__PURE__ */ jsx("ol", { children: hop.answers.map((answer2, i) => /* @__PURE__ */ jsx("li", { children: screenText(answer2) }, i)) })
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
          ] })
        ] }),
        root
      );
      if (owner.chart) owner.observer.observe(owner.chart);
      for (const pair of owner.briefs.values()) owner.observer.observe(pair.node);
      cancelAnimationFrame(owner.frame);
      owner.frame = requestAnimationFrame(owner.measure);
    };
    owner.paint();
    host2.committed();
  }
  function measureTraceScreen(root) {
    states2.get(root)?.measure();
  }

  // src/lib/transcript.tsx
  var owners = /* @__PURE__ */ new WeakMap();
  var identity = (entry) => entry.entryKey ?? entry.key ?? "";
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
  function Entry({ entry, owner }) {
    const key = identity(entry), open = owner.open.has(key), active = (action) => {
      if (!owner.disposed) action();
    };
    switch (entry.kind) {
      case "message":
        return /* @__PURE__ */ jsxs(
          "div",
          {
            class: entry.flavor === "incoming" ? "bubble in" : "msg " + entry.flavor,
            "data-e": entry.key,
            "data-entry-key": entry.entryKey,
            "data-h": entry.handoff,
            children: [
              entry.images?.length ? /* @__PURE__ */ jsx("div", { class: "attach-row", children: entry.images.map(
                (image2, i) => image2.unavailable || owner.failedImages.has(key + ":" + image2.url) ? /* @__PURE__ */ jsx("span", { class: "attach-na", children: "Image not available" }, i) : /* @__PURE__ */ jsx(
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
                          owner.failedImages.add(key + ":" + image2.url);
                          owner.change(key);
                        })
                      }
                    )
                  },
                  i
                )
              ) }) : null,
              (!entry.images?.length || entry.text) && /* @__PURE__ */ jsx(Markdown, { text: entry.text })
            ]
          }
        );
      case "thought":
        return entry.mode === "pending" ? /* @__PURE__ */ jsxs("div", { class: "think-pending", "data-e": entry.key, "data-entry-key": entry.entryKey, children: [
          /* @__PURE__ */ jsx("span", { class: "spin" }),
          /* @__PURE__ */ jsx("span", { children: "Thinking\u2026" })
        ] }) : /* @__PURE__ */ jsxs(
          "div",
          {
            class: "thought" + (entry.mode === "masked" ? " masked" : ""),
            "data-e": entry.key,
            "data-entry-key": entry.entryKey,
            children: [
              /* @__PURE__ */ jsx("div", { class: "think-label", children: screenText(entry.label ?? "") }),
              entry.mode === "readable" && /* @__PURE__ */ jsx(Markdown, { text: entry.text ?? "", className: "think-text" })
            ]
          }
        );
      case "label":
        return /* @__PURE__ */ jsx("div", { class: entry.className, "data-e": entry.key, "data-entry-key": entry.entryKey, children: screenText(entry.text) });
      case "tool":
        return /* @__PURE__ */ jsx(
          "div",
          {
            class: entry.step.className,
            "data-e": entry.key,
            "data-entry-key": entry.entryKey,
            "data-tid": entry.step.tid,
            "data-live": entry.step.sid,
            "data-since": entry.step.since,
            children: /* @__PURE__ */ jsx(
              ToolStepContents,
              {
                snapshot: entry.step,
                expanded: open,
                mounted: owner.mounted.has(key),
                toggle: () => active(() => {
                  owner.mounted.add(key);
                  if (open) owner.open.delete(key);
                  else owner.open.add(key);
                  owner.change(key);
                }),
                host: {
                  all(label) {
                    owner.host.toolAll(key, label);
                  },
                  script() {
                    owner.host.script(key);
                  }
                }
              }
            )
          }
        );
      case "background": {
        const content = /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(Glyph, { path: "M4 17l5-5-5-5M12 19h8", className: "" }),
          /* @__PURE__ */ jsx("span", { class: "bg-label", children: screenText(entry.label) })
        ] });
        return /* @__PURE__ */ jsx(
          "div",
          {
            class: "step bgend" + (entry.failed ? " err" : ""),
            "data-e": entry.key,
            "data-entry-key": entry.entryKey,
            children: entry.loaded ? /* @__PURE__ */ jsx(
              "button",
              {
                class: "bg-line",
                type: "button",
                onClick: (event) => {
                  if (event.currentTarget.isConnected)
                    owner.host.background(entry.call, event.currentTarget);
                },
                children: content
              }
            ) : /* @__PURE__ */ jsx("div", { class: "bg-line", children: content })
          }
        );
      }
      case "group": {
        const steps = /* @__PURE__ */ jsx(
          "div",
          {
            class: "steps" + (entry.lone ? " lone" : ""),
            hidden: entry.summary.length > 0 && !open,
            children: entry.entries.map((item2, i) => /* @__PURE__ */ jsx(Entry, { entry: item2, owner }, identity(item2) || i))
          }
        );
        return !entry.summary.length ? steps : /* @__PURE__ */ jsxs("div", { class: "tgroup", "data-e": entry.key, "data-entry-key": entry.entryKey, children: [
          /* @__PURE__ */ jsxs(
            "button",
            {
              class: "tsum",
              type: "button",
              "aria-expanded": open,
              onClick: (event) => {
                if (!event.currentTarget.isConnected) return;
                if (open) owner.open.delete(key);
                else owner.open.add(key);
                owner.change(key);
              },
              children: [
                entry.running ? /* @__PURE__ */ jsx("span", { class: "spin" }) : /* @__PURE__ */ jsx(Glyph, { path: entry.stack, className: "" }),
                /* @__PURE__ */ jsx("span", { class: "tt" + (entry.background ? " bgsum" : ""), children: entry.summary.map(
                  (part, i) => part.className ? /* @__PURE__ */ jsx("span", { class: part.className, children: screenText(part.text) }, i) : part.text
                ) }),
                entry.failed > 0 && /* @__PURE__ */ jsx("span", { class: "tf", children: screenText("\xB7 " + entry.failed + " failed") }),
                entry.running && /* @__PURE__ */ jsx("span", { class: "tl tick", children: entry.clockTick ? "\xB7 running " + entry.running : screenText("\xB7 running " + entry.running) }),
                /* @__PURE__ */ jsx(Glyph, { path: entry.chevron, className: "chev" })
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
            "data-e": entry.key,
            "data-entry-key": entry.entryKey,
            "data-h": entry.handoff,
            children: [
              /* @__PURE__ */ jsxs("span", { class: "cc-head", children: [
                /* @__PURE__ */ jsx(
                  "button",
                  {
                    class: "cc-name who-link",
                    type: "button",
                    "aria-label": "Open " + entry.name,
                    onClick: (event) => {
                      if (event.currentTarget.isConnected) owner.host.session(entry.id, entry.turn);
                    },
                    children: screenText(entry.name)
                  }
                ),
                /* @__PURE__ */ jsx(State, { state: entry.state, label: entry.stateLabel, text: entry.stateLabel }),
                /* @__PURE__ */ jsx(Glyph, { path: entry.chevron, className: "chev" })
              ] }),
              /* @__PURE__ */ jsxs("span", { class: "cc-meta", children: [
                entry.mark && /* @__PURE__ */ jsx(Harness, { mark: entry.mark }),
                screenText(entry.meta)
              ] }),
              /* @__PURE__ */ jsx("span", { class: "cc-brief", children: /* @__PURE__ */ jsx(Inline, { text: entry.brief }) }),
              entry.result ? /* @__PURE__ */ jsxs("span", { class: "cc-result" + (entry.failed ? " err" : ""), children: [
                /* @__PURE__ */ jsx("span", { class: "rl", children: entry.failed ? "Result: " : "Returned: " }),
                /* @__PURE__ */ jsx(Inline, { text: entry.result })
              ] }) : entry.activity && /* @__PURE__ */ jsxs("span", { class: "cc-now", children: [
                /* @__PURE__ */ jsx("span", { class: "spin" }),
                /* @__PURE__ */ jsx("span", { children: screenText(entry.activity[0]) }),
                /* @__PURE__ */ jsx("code", { children: entry.activity[1] })
              ] }),
              entry.trace && /* @__PURE__ */ jsx(
                "button",
                {
                  class: "link cc-run",
                  type: "button",
                  "aria-label": entry.traceLabel,
                  onClick: (event) => {
                    if (event.currentTarget.isConnected) owner.host.trace(entry.trace);
                  },
                  children: "Run view"
                }
              )
            ]
          }
        );
      case "event":
        return /* @__PURE__ */ jsxs(
          "div",
          {
            class: entry.className,
            "data-e": entry.key,
            "data-entry-key": entry.entryKey,
            "data-h": entry.handoff,
            children: [
              /* @__PURE__ */ jsxs("div", { class: "ev-head", children: [
                /* @__PURE__ */ jsx(Glyph, { path: entry.icon, className: "" }),
                /* @__PURE__ */ jsx("span", { class: "ln", children: /* @__PURE__ */ jsx(Sentence, { parts: entry.parts, host: owner.host }) }),
                /* @__PURE__ */ jsx("span", { class: "tm", children: entry.time })
              ] }),
              /* @__PURE__ */ jsx(
                "div",
                {
                  class: "ev-text" + (open ? " open" : owner.clipped.has(key) ? " clipped" : ""),
                  ref: (node) => {
                    if (node) owner.clamps.set(key, node);
                  },
                  children: /* @__PURE__ */ jsx(Inline, { text: entry.brief })
                }
              ),
              /* @__PURE__ */ jsx(
                "button",
                {
                  class: "link ev-more",
                  type: "button",
                  hidden: !owner.clipped.has(key) && !open,
                  "aria-expanded": open,
                  onClick: (event) => {
                    event.stopPropagation();
                    if (!event.currentTarget.isConnected) return;
                    if (open) owner.open.delete(key);
                    else owner.open.add(key);
                    owner.change(key);
                  },
                  children: open ? "Show less" : "Show more"
                }
              ),
              entry.result && /* @__PURE__ */ jsxs("div", { class: "ev-result", children: [
                /* @__PURE__ */ jsx("span", { class: "rl", children: entry.resultLabel }),
                /* @__PURE__ */ jsx(Inline, { text: entry.result })
              ] }),
              entry.answers && /* @__PURE__ */ jsx("div", { class: "ev-result answer" + (entry.answers.length ? "" : " none"), children: /* @__PURE__ */ jsx(Answer, { values: entry.answers }) }),
              entry.waiting && /* @__PURE__ */ jsx(State, { state: "wait", label: entry.stateLabel, text: "Waiting on you" })
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
        !view.bare && /* @__PURE__ */ jsx("div", { class: "tx", children: view.entries.map((entry, i) => /* @__PURE__ */ jsx(Entry, { entry, owner }, identity(entry) || i)) }),
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
          disabled: owner.jumpBusy,
          onClick: (event) => {
            if (event.currentTarget.isConnected) owner.host.jump();
          },
          children: [
            owner.jumpCount > 0 && /* @__PURE__ */ jsx("span", { class: "new-count", children: owner.jumpCount + " new" }),
            /* @__PURE__ */ jsx(Glyph, { path: "M12 4v15M5 12l7 7 7-7", className: "" })
          ]
        }
      ),
      owner.jump
    );
  }
  function updateSessionJump(root, visible, count, busy2 = false) {
    const owner = owners.get(root);
    if (!owner) return;
    owner.jumpVisible = visible;
    owner.jumpCount = count;
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
        measure() {
        },
        change() {
        },
        observer: new ResizeObserver(() => {
        })
      };
      owner = state3;
      owners.set(root, state3);
      state3.measure = () => {
        if (state3.disposed) return;
        let changed = false;
        for (const [key, node] of state3.clamps) {
          if (!node.isConnected || state3.open.has(key) || !node.clientHeight) continue;
          const clipped = node.scrollHeight > node.clientHeight + 1;
          if (state3.clipped.has(key) !== clipped) {
            if (clipped) state3.clipped.add(key);
            else state3.clipped.delete(key);
            changed = true;
            for (const block of state3.blocks)
              if (block.kind === "turn" && block.turn.entries.some((entry) => identity(entry) === key))
                state3.revisions.set(block.turn.id, (state3.revisions.get(block.turn.id) ?? 0) + 1);
          }
        }
        if (changed) state3.paint();
      };
      state3.observer.disconnect();
      state3.observer = new ResizeObserver(state3.measure);
      claimScreen(root, "session", () => {
        state3.disposed = true;
        state3.observer.disconnect();
        cancelAnimationFrame(state3.frame);
        if (state3.jump) render(null, state3.jump);
        owners.delete(root);
      });
    }
    const state2 = owner;
    state2.host = host2;
    state2.blocks = mergeBlocks(state2.blocks, snapshot);
    state2.snapshot = snapshot;
    state2.change = (key) => {
      for (const block of state2.blocks)
        if (block.kind === "turn" && block.turn.entries.some(
          (entry) => identity(entry) === key || entry.kind === "group" && entry.entries.some((item2) => identity(item2) === key)
        ))
          state2.revisions.set(block.turn.id, (state2.revisions.get(block.turn.id) ?? 0) + 1);
      state2.paint();
    };
    function pager(view) {
      return /* @__PURE__ */ jsx("div", { class: "list", children: /* @__PURE__ */ jsxs(
        "button",
        {
          class: "more",
          type: "button",
          "data-load-earlier": view.where === "before" ? "" : void 0,
          "data-pager-sid": view.sid,
          "data-pager-where": view.where,
          disabled: view.disabled,
          "aria-busy": view.busy || void 0,
          onClick: (event) => {
            if (event.currentTarget.isConnected) state2.host.pager(event.currentTarget);
          },
          children: [
            view.busy && /* @__PURE__ */ jsx("span", { class: "spin", "aria-hidden": "true" }),
            /* @__PURE__ */ jsx("span", { class: "pager-label", "aria-live": "polite", children: screenText(view.text) })
          ]
        }
      ) }, view.where);
    }
    state2.paint = () => {
      if (state2.disposed) return;
      const held = document.activeElement instanceof HTMLElement && document.activeElement.closest(".session-foot") ? document.activeElement : null, heldKind = held?.dataset.foot;
      const view = state2.snapshot;
      render(
        /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx("div", { class: "ph sr", children: /* @__PURE__ */ jsx("h1", { children: screenText(view.name) }) }),
          /* @__PURE__ */ jsxs("section", { class: "transcript", "aria-label": "Transcript", children: [
            /* @__PURE__ */ jsxs("div", { class: "turns", children: [
              view.before ? pager(view.before) : view.started && /* @__PURE__ */ jsx("div", { class: "divider started", children: /* @__PURE__ */ jsxs("span", { class: "dv-text", children: [
                /* @__PURE__ */ jsx("span", { class: "dv-lead", children: view.started.lead }),
                /* @__PURE__ */ jsx("span", { class: "dv-machine", "data-tip": view.started.machine, "data-tip-clipped": "", children: screenText(view.started.machine) })
              ] }) }),
              state2.blocks.map(
                (block) => block.kind === "turn" ? /* @__PURE__ */ jsx(
                  Turn,
                  {
                    view: block.turn,
                    owner: state2,
                    revision: state2.revisions.get(block.turn.id) ?? 0
                  },
                  block.turn.id
                ) : /* @__PURE__ */ jsx(Fragment, { children: block.entries.map((entry, i) => /* @__PURE__ */ jsx(Entry, { entry, owner: state2 }, identity(entry) || block.key + i)) }, block.key)
              ),
              view.after && pager(view.after),
              view.empty && /* @__PURE__ */ jsx("p", { class: "empty", children: screenText(view.empty) })
            ] }),
            /* @__PURE__ */ jsx(
              "div",
              {
                class: "jump-wrap",
                ref: (node) => {
                  state2.jump = node;
                  paintJump(state2);
                }
              }
            )
          ] }),
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
        ] }),
        root
      );
      if (heldKind && held && !held.isConnected)
        (root.querySelector('[data-foot="' + CSS.escape(heldKind) + '"]') ?? root.querySelector('[data-foot="time"]'))?.focus({ preventScroll: true });
      for (const [key, node] of state2.clamps) {
        if (node.isConnected) state2.observer.observe(node);
        else {
          state2.observer.unobserve(node);
          state2.clamps.delete(key);
        }
      }
    };
    state2.paint();
    host2.committed();
  }
  function updateSessionPager(root, view) {
    const owner = owners.get(root);
    if (!owner || owner.snapshot.id !== view.sid) return;
    owner.snapshot = { ...owner.snapshot, [view.where]: view };
    owner.paint();
  }
  function updateSessionClock(root, now, starts) {
    const owner = owners.get(root);
    if (!owner || owner.disposed) return;
    const elapsed = (since) => {
      const n = Math.max(0, Math.floor((now - since) / 1e3));
      return n < 60 ? n + "s" : Math.floor(n / 60) + "m " + n % 60 + "s";
    };
    function update(entry) {
      if (entry.kind === "tool" && entry.step.running && entry.step.sid) {
        const since = entry.step.since ?? starts[entry.step.sid];
        if (!Number.isFinite(since)) return entry;
        const text2 = elapsed(since), step = entry.step;
        const status2 = step.background ? step.status : text2, backgroundStatus = step.background ? "running " + text2 : step.backgroundStatus;
        return status2 === step.status && backgroundStatus === step.backgroundStatus ? entry : { ...entry, step: { ...step, status: status2, backgroundStatus } };
      }
      if (entry.kind === "group") {
        const entries = entry.entries.map((item2) => update(item2)), since = entries.filter(
          (item2) => item2.kind === "tool" && item2.step.running && !!item2.step.sid
        ).map((item2) => item2.step.since ?? starts[item2.step.sid]).filter((value) => value !== void 0 && Number.isFinite(value));
        const running = since.length ? elapsed(Math.min(...since)) : entry.running;
        return running === entry.running && entries.every((item2, i) => item2 === entry.entries[i]) ? entry : { ...entry, entries, running, clockTick: true };
      }
      return entry;
    }
    let changed = false;
    owner.blocks = owner.blocks.map((block) => {
      const previous = block.kind === "turn" ? block.turn.entries : block.entries, entries = previous.map(update);
      if (entries.every((entry, i) => entry === previous[i])) return block;
      changed = true;
      return block.kind === "turn" ? { ...block, turn: { ...block.turn, entries } } : { ...block, entries };
    });
    if (changed) owner.paint();
  }
  function measureSessionScreen(root) {
    owners.get(root)?.measure();
  }

  // src/lib/viewer-bar.tsx
  function createViewerBar() {
    const title = document.createElement("div"), actions = document.createElement("div"), mode = document.createElement("div");
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
        view.mode === "normal" ? view.analytics ? /* @__PURE__ */ jsx("div", { class: "analytics-range", role: "group", "aria-label": "Analytics range", children: [
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
        ] }) : null : null,
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
          ].map(([key, label, count]) => /* @__PURE__ */ jsxs(
            "button",
            {
              class: "chip",
              type: "button",
              "data-filter": key,
              "aria-pressed": view.filter === key,
              onClick: (event) => active(event.currentTarget, () => host2.filter(String(key))),
              children: [
                /* @__PURE__ */ jsx("span", { children: label }),
                count !== void 0 && /* @__PURE__ */ jsx("span", { class: "n", children: count })
              ]
            },
            key
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
                /* @__PURE__ */ jsx("span", { class: "errnav-count", children: view.errorText })
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

  // src/lib/placeholder.tsx
  var shapes = [
    [true, [1, 3, 5], true],
    [false, [1, 2, 4], true],
    [true, [2, 1, 3], false],
    [false, [1, 1, 5], true]
  ];
  function renderPlaceholder(root, error, retry) {
    releaseScreen(root);
    claimScreen(root, "placeholder", () => {
    });
    render(
      error ? /* @__PURE__ */ jsxs("div", { class: "load-error", role: "alert", children: [
        /* @__PURE__ */ jsx("p", { class: "empty", children: screenText(error) }),
        retry && /* @__PURE__ */ jsx(
          "button",
          {
            class: "more",
            type: "button",
            onClick: (event) => {
              if (event.currentTarget.isConnected) retry();
            },
            children: "Try again"
          }
        )
      ] }) : /* @__PURE__ */ jsx("div", { class: "skeleton", "aria-hidden": "true", children: Array.from({ length: 6 }, (_, i) => {
        const [bubble, lines, step] = shapes[i % shapes.length];
        return /* @__PURE__ */ jsxs("div", { class: "sk-turn", children: [
          bubble && /* @__PURE__ */ jsx("span", { class: "sk-line sk-bubble sk-w3" }),
          /* @__PURE__ */ jsx("div", { class: "sk-text", children: lines.map((width, j) => /* @__PURE__ */ jsx("span", { class: "sk-line sk-w" + width }, j)) }),
          step && /* @__PURE__ */ jsx("span", { class: "sk-line sk-step sk-w2" })
        ] }, i);
      }) }),
      root
    );
  }
  function createStatusNote(text2, className, role = "status") {
    const root = document.createElement("div");
    root.className = "viewer-status-slot";
    render(
      /* @__PURE__ */ jsx("p", { class: className, role, children: screenText(text2) }),
      root
    );
    return root;
  }
  function createLiveRegion() {
    const root = document.createElement("div");
    root.className = "sr-only";
    root.setAttribute("role", "status");
    root.setAttribute("aria-live", "polite");
    return root;
  }

  // src/lib/model.ts
  function object(value) {
    return !!value && typeof value === "object" && !Array.isArray(value);
  }
  var strings = (value) => Array.isArray(value) && value.every((item2) => typeof item2 === "string");
  function machine(value) {
    return object(value) && typeof value.id === "string" && typeof value.name === "string" && typeof value.up === "boolean";
  }
  function parseModel(value) {
    if (!object(value) || !Number.isFinite(value.now) || typeof value.version !== "string" || !machine(value.machine) || !object(value.sessions) || !Array.isArray(value.handoffs) || !Array.isArray(value.turns))
      throw new Error("Invalid model response");
    if (value.machines !== void 0 && (!Array.isArray(value.machines) || !value.machines.every(machine)))
      throw new Error("Invalid machine response");
    for (const session of Object.values(value.sessions))
      if (!object(session) || !["name", "harness", "state", "machine"].every(
        (field) => typeof session[field] === "string"
      ) || !Number.isFinite(session.start) || !Number.isFinite(session.last))
        throw new Error("Invalid session response");
    for (const handoff of value.handoffs)
      if (!object(handoff) || !["id", "kind", "from", "status"].every((field) => typeof handoff[field] === "string") || !Number.isFinite(handoff.at))
        throw new Error("Invalid handoff response");
    for (const turn of value.turns)
      if (!object(turn) || typeof turn.id !== "string" || typeof turn.sid !== "string" || !strings(turn.sent))
        throw new Error("Invalid turn response");
    return value;
  }
  var ModelStore = class {
    current = null;
    adopt(value) {
      const model2 = parseModel(value);
      this.current = structuredClone(model2);
      return model2;
    }
    apply(value) {
      if (!object(value)) throw new Error("Invalid model response");
      if (value.delta !== 1) return parseModel(value);
      if (!this.current || value.from !== this.current.version)
        throw new Error("Model delta base expired");
      if (value.set !== void 0 && !object(value.set)) throw new Error("Invalid model delta");
      const next = { ...this.current, ...object(value.set) ? value.set : {} };
      if (value.remove !== void 0 && !strings(value.remove))
        throw new Error("Invalid model delta removals");
      for (const field of value.remove ?? []) delete next[field];
      if (value.collections !== void 0 && !object(value.collections))
        throw new Error("Invalid model delta collections");
      for (const [field, patch] of Object.entries(
        object(value.collections) ? value.collections : {}
      )) {
        if (!object(patch) || patch.set !== void 0 && !object(patch.set) || patch.remove !== void 0 && !strings(patch.remove) || patch.order !== void 0 && !strings(patch.order))
          throw new Error("Invalid collection patch");
        const array2 = field === "handoffs" || field === "turns", previous = this.current[field];
        const rows = /* @__PURE__ */ new Map();
        if (array2) {
          if (!Array.isArray(previous)) throw new Error("Invalid delta base");
          for (const row of previous) {
            if (!object(row) || typeof row.id !== "string") throw new Error("Invalid delta row");
            rows.set(row.id, row);
          }
        } else {
          if (previous !== void 0 && !object(previous)) throw new Error("Invalid delta base");
          for (const [id, row] of Object.entries(object(previous) ? previous : {})) rows.set(id, row);
        }
        for (const id of patch.remove ?? []) rows.delete(id);
        for (const [id, row] of Object.entries(object(patch.set) ? patch.set : {})) rows.set(id, row);
        if (array2) {
          const order = patch.order ?? [...rows.keys()];
          if (order.length !== rows.size || new Set(order).size !== rows.size || order.some((id) => !rows.has(id)))
            throw new Error("Incomplete model delta");
          next[field] = order.map((id) => rows.get(id));
        } else next[field] = Object.fromEntries(rows);
      }
      next.version = value.version ?? null;
      return parseModel(next);
    }
  };
  var ApiError = class extends Error {
    constructor(message, status2) {
      super(message);
      this.status = status2;
    }
    status;
  };
  async function requestJson(path, signal, unchanged = false) {
    let response;
    try {
      response = await fetch(path, { credentials: "same-origin", signal });
    } catch (error) {
      if (error instanceof Error && error.name === "AbortError") throw error;
      throw new ApiError(error instanceof Error ? error.message : "No response", 0);
    }
    if (unchanged && response.status === 304) return null;
    if (!response.ok)
      throw new ApiError(response.status + " " + response.statusText, response.status);
    return response.json();
  }

  // src/lib/live.ts
  function createLiveController(host2) {
    const state2 = {
      late: null,
      lateTries: 0,
      retry: false,
      version: null,
      timer: null,
      due: 0,
      busy: false,
      started: -Infinity,
      delay: 2e3,
      ended: false,
      again: false,
      pending: false,
      fresh: 0,
      turns: /* @__PURE__ */ new Map()
    };
    let disposed = false;
    const visible = () => document.visibilityState === "visible", floorWait = () => Math.max(0, state2.started + 1e3 - performance.now());
    function cancel() {
      if (state2.timer !== null) clearTimeout(state2.timer);
      state2.timer = null;
    }
    function schedule(ms) {
      cancel();
      if (!disposed && !state2.ended && visible()) {
        state2.due = performance.now() + ms;
        state2.timer = window.setTimeout(poll, ms);
      }
    }
    function refresh() {
      if (disposed || !state2.version || state2.ended) return;
      if (state2.busy) {
        state2.again = true;
        return;
      }
      const wait = floorWait();
      if (!state2.timer || performance.now() + wait < state2.due) schedule(wait);
    }
    async function poll() {
      state2.timer = null;
      if (disposed || state2.busy || state2.ended || !visible()) return;
      state2.busy = true;
      state2.started = performance.now();
      state2.retry = false;
      let ok = false;
      try {
        await host2.poll();
        state2.delay = state2.retry ? Math.min(3e4, state2.delay * 2) : 2e3;
        ok = true;
      } catch (error) {
        if (disposed) return;
        const status2 = typeof error === "object" && error !== null && "status" in error ? error.status : void 0;
        if (host2.failed(error)) {
          state2.ended = true;
          cancel();
        } else if (status2 === 403) host2.ended(403);
        else {
          state2.delay = Math.min(3e4, state2.delay * 2);
          if (status2 === void 0)
            window.setTimeout(() => {
              throw error;
            });
        }
      } finally {
        state2.busy = false;
        if (!disposed) {
          schedule(state2.again ? floorWait() : state2.delay);
          state2.again = false;
          window.dispatchEvent(new CustomEvent("semon:polled", { detail: { ok } }));
        }
      }
    }
    function visibility() {
      if (!visible()) cancel();
      else if (state2.version && !state2.busy && !state2.timer)
        schedule(state2.delay > 2e3 ? state2.delay : 0);
    }
    document.addEventListener("visibilitychange", visibility);
    window.addEventListener("semon:refresh", refresh);
    return {
      state: state2,
      schedule,
      stop() {
        state2.ended = true;
        cancel();
      },
      destroy() {
        if (disposed) return;
        disposed = true;
        cancel();
        document.removeEventListener("visibilitychange", visibility);
        window.removeEventListener("semon:refresh", refresh);
      }
    };
  }
  function createPagingStore() {
    const states3 = /* @__PURE__ */ new Map();
    return {
      states: states3,
      get(sid, direction) {
        let pair = states3.get(sid);
        if (!pair) {
          pair = { before: { busy: false, failed: false }, after: { busy: false, failed: false } };
          states3.set(sid, pair);
        }
        return pair[direction];
      },
      clear(sid) {
        for (const state2 of Object.values(states3.get(sid) ?? {})) state2.controller?.abort();
        states3.delete(sid);
      },
      destroy() {
        for (const sid of states3.keys()) this.clear(sid);
      }
    };
  }

  // src/lib/geometry.ts
  var properties2 = {
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
  var serial2 = 0;
  var records = /* @__PURE__ */ new Map();
  function paint() {
    if (!sheet) {
      sheet = new CSSStyleSheet();
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    }
    sheet.replaceSync("");
    for (const [node, record2] of records) {
      if (!node.isConnected) {
        records.delete(node);
        continue;
      }
      const declarations = [...record2.values].map(
        ([slot, value]) => properties2[slot] + ":" + (slot === "intrinsicHeight" ? "auto " : "") + value + "px !important"
      ).join(";");
      if (declarations)
        sheet.insertRule(
          '[data-semon-geometry="' + record2.id + '"]{' + declarations + "}",
          sheet.cssRules.length
        );
    }
  }
  function setGeometry(node, slot, value) {
    if (!Object.hasOwn(properties2, slot) || value !== null && (!Number.isFinite(value) || Math.abs(value) > 1e8))
      throw new Error("Invalid host geometry");
    let record2 = records.get(node);
    if (!record2) {
      record2 = { id: ++serial2, values: /* @__PURE__ */ new Map() };
      records.set(node, record2);
      node.dataset.semonGeometry = String(record2.id);
    }
    if (value === null) record2.values.delete(slot);
    else record2.values.set(slot, value);
    paint();
    if (!observer) {
      observer = new MutationObserver(() => {
        if ([...records.keys()].some((node2) => !node2.isConnected)) paint();
      });
      observer.observe(document.documentElement, { childList: true, subtree: true });
    }
  }
  function releaseGeometry(root, slots) {
    for (const [node, record2] of records) {
      if (slots ? node === root : node === root || root.contains(node) || !node.isConnected) {
        if (slots) for (const slot of slots) record2.values.delete(slot);
        if (!slots || !record2.values.size) {
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
  function revealMeasuredTurn(node, visible) {
    node.classList.toggle("semon-measuring-turn", visible);
  }

  // src/lib/routes.ts
  function routeUrl(route, model2) {
    const enc = encodeURIComponent, harness = (id) => model2.session(id)?.harness ?? "claude";
    switch (route.v) {
      case "home":
        return "/";
      case "analytics":
        return "/analytics";
      case "sessions":
        return "/sessions";
      case "machines":
        return model2.machinesPath ?? "/machines";
      case "machine":
        return "/machines/" + enc(route.id);
      case "session":
        return "/s/" + harness(route.id) + "/" + enc(route.id) + (route.turn ? "?turn=" + enc(route.turn) : "");
      case "trace":
        return "/trace/" + harness(route.sid) + "/" + enc(route.sid) + "/" + enc(route.turn);
    }
  }
  function parseRoute(location2, model2) {
    if (model2.machinesPath && location2.pathname === model2.machinesPath) return { v: "machines" };
    const decode = (text2) => {
      try {
        return decodeURIComponent(text2);
      } catch {
        return text2;
      }
    }, parts = location2.pathname.split("/").filter(Boolean).map(decode), sid = parts[2], turn = new URLSearchParams(location2.search).get("turn");
    if (parts[0] === "timeline" || parts[0] === "analytics") return { v: "analytics" };
    if (parts[0] === "sessions") return { v: "sessions" };
    if (parts[0] === "machines")
      return parts[1] && model2.machine(parts[1]) ? { v: "machine", id: parts[1] } : { v: "machines" };
    if (parts[0] === "s" && sid && model2.session(sid)) {
      const candidate = model2.turn(turn ?? decode(location2.hash.slice(1)).split("#")[0] ?? ""), target = candidate?.sid === sid ? candidate.id : null;
      return target ? { v: "session", id: sid, turn: target } : { v: "session", id: sid };
    }
    if (parts[0] === "trace" && sid && model2.session(sid) && parts[3] && model2.turn(parts[3])?.sid === sid)
      return { v: "trace", sid, turn: parts[3] };
    return { v: "home" };
  }
  var TranscriptCache = class extends Map {
    constructor(maxEntries = 5, maxBytes = 2 * 1024 * 1024) {
      super();
      this.maxEntries = maxEntries;
      this.maxBytes = maxBytes;
    }
    maxEntries;
    maxBytes;
    keep(sid, entries, meta) {
      this.delete(sid);
      let weight = 0;
      for (const entry of entries)
        for (const value of Object.values(entry))
          weight += typeof value === "string" ? value.length : value && typeof value === "object" ? JSON.stringify(value).length : 4;
      const bytes = weight * 2;
      if (bytes > this.maxBytes) return;
      this.set(sid, { entries, meta, bytes });
      let sum = 0;
      for (const record2 of this.values()) sum += record2.bytes;
      for (const [id, record2] of this) {
        if (this.size <= this.maxEntries && sum <= this.maxBytes) break;
        this.delete(id);
        sum -= record2.bytes;
      }
    }
  };

  // src/lib/paging.ts
  function createPagerController(host2) {
    let disposed = false, frame = null;
    const observers = /* @__PURE__ */ new Map();
    const requests = /* @__PURE__ */ new Set();
    function disconnect() {
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null;
      for (const observer2 of observers.values()) observer2.disconnect();
      observers.clear();
    }
    async function load(button, manual) {
      const sid = button.dataset.pagerSid, direction = button.dataset.pagerWhere;
      if (disposed || !sid || direction !== "before" && direction !== "after" || !button.isConnected)
        return;
      const route = host2.route(sid), state2 = host2.state(sid, direction);
      if (!route || state2.busy || !manual && (state2.failed || !host2.automatic())) return;
      if (manual) host2.beginManual();
      const range = host2.range(sid);
      if (!range || (direction === "before" ? range.from <= 0 : range.to >= range.total)) return;
      const boundary = direction === "before" ? range.from : range.to;
      if (!manual) host2.countAutomatic();
      state2.busy = true;
      state2.controller = new AbortController();
      const controller = state2.controller;
      requests.add(controller);
      host2.paint(button);
      try {
        let applied = false;
        await host2.load(sid, direction, boundary, controller.signal, () => {
          applied = true;
        });
        if (disposed || !applied || !host2.current(sid, direction, state2, route) || host2.range(sid) !== range)
          return;
        state2.failed = false;
        if (host2.route(sid) === route) host2.commit(route, direction, manual);
      } catch (error) {
        if (!(error instanceof Error && error.name === "AbortError") && host2.current(sid, direction, state2, route))
          state2.failed = true;
      } finally {
        requests.delete(controller);
        state2.busy = false;
        state2.controller = null;
        if (!disposed && host2.current(sid, direction, state2, route)) {
          for (const next of document.querySelectorAll("#page [data-pager-where]"))
            if (next.dataset.pagerSid === sid && next.dataset.pagerWhere === direction)
              host2.paint(next);
          host2.queue();
        }
      }
    }
    function queue(root, scrollRoot, enabled) {
      if (disposed || frame !== null) return;
      frame = requestAnimationFrame(() => {
        frame = null;
        disconnect();
        if (disposed || !root.isConnected || !enabled() || typeof IntersectionObserver === "undefined")
          return;
        for (const button of root.querySelectorAll("[data-pager-where]")) {
          const sid = button.dataset.pagerSid, direction = button.dataset.pagerWhere;
          if (!sid || direction !== "before" && direction !== "after") continue;
          const state2 = host2.state(sid, direction);
          if (state2.busy || state2.failed) continue;
          const observer2 = new IntersectionObserver(
            (entries) => {
              if (observers.get(button) === observer2 && entries.some((entry) => entry.isIntersecting))
                void load(button, false);
            },
            {
              root: scrollRoot,
              rootMargin: direction === "before" ? "800px 0px 0px 0px" : "0px 0px 800px 0px"
            }
          );
          observers.set(button, observer2);
          observer2.observe(button);
        }
      });
    }
    return {
      load,
      queue,
      disconnect,
      destroy() {
        if (disposed) return;
        disposed = true;
        for (const controller of requests) controller.abort();
        requests.clear();
        disconnect();
      }
    };
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
  function orderRows(scope, key, items, compare, {
    must = null,
    limit = Infinity,
    quiet = false,
    seed = false
  } = {}) {
    const sorted = [...items].sort(compare), by = new Map(items.map((row) => [row.id, row]));
    let ids = sorted.map((row) => row.id);
    if (scope.keep && !quiet && !(seed && !scope.prev.has(key))) {
      const existed = scope.prev.has(key), old = (scope.prev.get(key) ?? []).filter((id) => by.has(id)), have = new Set(old), fresh = sorted.filter((row) => !have.has(row.id)), up = fresh.filter((row) => must?.has(row.id) || scope.calm && existed);
      ids = [...up.map((row) => row.id), ...old];
      const shown = ids.slice(0, limit), inShown = new Set(shown), top = new Set(sorted.slice(0, limit).map((row) => row.id)), held = fresh.filter((row) => top.has(row.id) && !up.includes(row)).length;
      scope.n += held + sorted.slice(0, limit).filter((row) => have.has(row.id) && !inShown.has(row.id)).length + moves(
        shown.map((id) => by.get(id)),
        compare
      );
    }
    scope.lists.set(key, ids);
    return ids.map((id) => by.get(id));
  }

  // src/lib/index.ts
  installPropGuard();

  // src/viewer-host.ts
  var host = null;
  function getViewerHost() {
    return host;
  }

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
  var shortName = (name) => {
    const h2 = String(name).split(".")[0];
    return h2.length > 14 ? h2.slice(0, 14) + "\u2026" : h2;
  };
  var machineShorts = (names) => {
    const forms = (id, full) => {
      const first = String(full).split(".")[0];
      return [
        shortName(first),
        first.length > 14 ? first.slice(0, 6) + "\u2026" + first.slice(-7) : first,
        first,
        id
      ];
    };
    const opts = new Map([...names].map(([id, full]) => [id, forms(id, full)])), level = new Map([...opts.keys()].map((id) => [id, 0]));
    for (let step = 0; step < 3; step++) {
      const groups = /* @__PURE__ */ new Map();
      for (const [id, o] of opts) {
        const l = o[level.get(id)];
        if (!groups.has(l)) groups.set(l, []);
        groups.get(l).push(id);
      }
      let clash = false;
      for (const ids of groups.values())
        if (ids.length > 1) {
          clash = true;
          for (const id of ids) level.set(id, level.get(id) + 1);
        }
      if (!clash) break;
    }
    return new Map([...opts].map(([id, o]) => [id, o[level.get(id)]]));
  };
  var shortModel = (model2) => String(model2 ?? "Unknown model").replace(/^gpt-(\d+\.\d+)-(.+)$/i, "$2 $1").replace(/^gpt-\d+-/i, "").replace(/^claude-/i, "").replace(/^(opus|sonnet|haiku)-(\d+)-(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)\.(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)$/i, "$1 $2");
  var clean = (t) => t.replace(/[`*]/g, "");
  var liveUrl = (u) => {
    try {
      return /^https?:$/.test(new URL(u).protocol) && /^https?:\/\//i.test(u) ? u : null;
    } catch {
      return null;
    }
  };
  var preview = (t) => t.split("\n").map((l) => l.replace(/^\s*(#{1,6}\s+|>\s?|[-*]\s+|\d{1,3}[.)]\s+)/, "").trim()).filter((l) => l && !/^\s*\|?\s*:?-{2,}/.test(l)).join(" ");
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

  // src/domain/calculations.ts
  function createDomain(state2, now, SEEN_RESULTS) {
    const {
      sessions: SESS,
      machines: MACHINE,
      handoffs: H,
      turns: TURNS,
      turn: TURN,
      starts: STARTS,
      holds: HOLDS,
      handoff: HID,
      transcriptMeta: TXM
    } = state2;
    const clock2 = (at) => clock(at, now());
    const nameOf = (id) => id === "you" ? "You" : SESS[id].name;
    const hcls = (id) => id === "you" ? "h-you" : "h-" + SESS[id].harness;
    const where = (s) => s.repo ? s.repo + (s.branch && s.branch !== "main" && s.branch !== s.name ? " \xB7 " + s.branch : "") : "No repo";
    const hostOf = (s) => s.host ?? MACHINE[s.machine] ?? s.machine ?? "Unknown machine";
    const machineLabels = (scope = Object.values(SESS)) => {
      const names = /* @__PURE__ */ new Map();
      for (const x of scope) {
        const s = typeof x === "string" ? SESS[x] : x;
        if (s?.machine != null && !names.has(s.machine))
          names.set(s.machine, s.host ?? MACHINE[s.machine] ?? s.machine);
      }
      return names.size > 1 ? machineShorts([...names]) : /* @__PURE__ */ new Map();
    };
    const machineLabel = (s, scope) => s ? machineLabels(scope).get(s.machine) ?? "" : "";
    const branchOf = (s) => s.worktree ?? s.branch ?? "No branch";
    const shortHost = (s) => s.host == null && MACHINE[s.machine] == null && s.machine == null ? hostOf(s) : shortName(hostOf(s));
    const parentOf = (sid) => SESS[sid]?.parent ?? H.find((h2) => h2.kind === "spawn" && h2.to === sid)?.from;
    const originHandoff = (sid) => H.find(
      (h2) => (h2.kind === "spawn" || h2.kind === "relay") && h2.to === sid && h2.from !== sid && (h2.kind === "spawn" || SESS[sid]?.kind === "Relayed" || !SESS[sid]?.lane)
    );
    const RANK = { wait: 0, work: 1, err: 2, done: 3 };
    const isResult = (h2) => h2.kind === "toyou" && h2.ask === "result";
    const inbox = () => {
      const entries = H.filter(
        (h2) => h2.kind === "toyou" && (h2.status === "wait" || isResult(h2) && !SEEN_RESULTS.has(h2.id))
      );
      const waiting = new Set(entries.filter((h2) => h2.status === "wait").map((h2) => h2.from));
      for (const s of Object.values(SESS))
        if (s.state === "wait" && !waiting.has(s.id))
          entries.push({
            id: "wait:" + s.id,
            kind: "toyou",
            from: s.id,
            to: "you",
            ask: "decision",
            status: "wait",
            at: s.waiting_since ?? s.last,
            brief: s.waiting_for ?? "Waiting for your input or permission"
          });
      return entries.sort((a, b) => b.at - a.at);
    };
    const working = () => Object.values(SESS).filter((s) => s.state === "work");
    const answersOf = (h2) => h2.kind === "toyou" && h2.ask !== "result" && h2.status === "done" ? (Array.isArray(h2.answers) ? h2.answers.map((a) => (a.values ?? []).join(", ")) : Array.isArray(h2.answer) ? h2.answer : h2.answer != null ? [h2.answer] : []).map((a) => String(a).trim()).filter(Boolean) : null;
    const statWord = (h2) => isResult(h2) ? SEEN_RESULTS.has(h2.id) ? "read" : "new" : {
      new: void 0,
      work: "working",
      wait: "waiting on you",
      err: "failed",
      done: h2.kind === "toyou" ? "answered" : h2.result ? "returned" : "delivered"
    }[h2.status];
    const hasTurn = (t) => !!t.end || !!(t.start || t.u) || t.entries.some((e) => e.k === "a" || e.k === "tool" || e.k === "h");
    const oneLine = (s) => clean(s).replace(/\s+/g, " ").trim();
    const TOYOU = {
      question: "Asked you",
      result: "Sent you a result",
      decision: "Needs your decision"
    };
    function turnEnd2(t) {
      if (!hasTurn(t)) return null;
      if (!t.end) {
        if (t.last && SESS[t.sid]?.state === "wait")
          return { st: "wait", text: "Waiting on permission or input" };
        if (t.last && SESS[t.sid]?.state === "work") return { st: "work", text: "Still working" };
        const ty = t.out.filter((h3) => h3.kind === "toyou").at(-1);
        if (ty)
          return {
            st: ty.status === "wait" ? "wait" : "done",
            text: (TOYOU[ty.kind === "toyou" ? ty.ask : ""] ?? "Sent you a message") + " \xB7 " + statWord(ty) + " \xB7 " + clock2(ty.at)
          };
        const entries = t.entries.filter(
          (e) => e.k === "a" || e.k === "tool" || e.k === "h" && t.out.includes(HID.get(e.id))
        );
        const last = entries.at(-1), h2 = last?.k === "h" ? HID.get(last.id) : null;
        if (t.start?.status === "err")
          return { st: "err", text: "Failed" + (t.start.done ? " \xB7 " + clock2(t.start.done) : "") };
        if (last?.k === "tool" && last.ok === false && !last.live)
          return {
            st: "err",
            text: last.unfinished ? "Stopped on a step with no result" : "Stopped on a failed step"
          };
        if (h2?.status === "err") return { st: "err", text: "Handoff to " + nameOf(h2.to) + " failed" };
        if (t.start?.kind === "spawn" && t.start.status === "done")
          return {
            st: "done",
            text: "Returned to " + nameOf(t.start.from) + (t.start.done ? " \xB7 " + clock2(t.start.done) : "")
          };
        if (entries.some((e) => e.k === "a")) return { st: "done", text: "Replied" };
        return { st: "idle", text: "No reply in these logs" };
      }
      const { st, why } = t.end, mh = t.end.h ? HID.get(t.end.h) : null;
      if (why === "input") return { st: "wait", text: "Waiting for your input" };
      if (why === "permission" || why === "waiting" || why === "waiting_permission")
        return { st: "wait", text: "Waiting on permission" };
      if (why === "working") return { st: "work", text: "Still working" };
      if (why === "toyou" && mh)
        return {
          st: mh.status === "wait" ? "wait" : "done",
          text: (TOYOU[mh.kind === "toyou" ? mh.ask : ""] ?? "Sent you a message") + " \xB7 " + statWord(mh) + " \xB7 " + clock2(mh.at)
        };
      if (why === "failed")
        return { st: "err", text: "Failed" + (t.start?.done ? " \xB7 " + clock2(t.start.done) : "") };
      if (why === "unfinished_step") return { st: "err", text: "Stopped on a step with no result" };
      if (why === "failed_step") return { st: "err", text: "Stopped on a failed step" };
      if (why === "handoff_failed" && mh)
        return { st: "err", text: "Handoff to " + nameOf(mh.to) + " failed" };
      if (why === "returned" && t.start)
        return {
          st: "done",
          text: "Returned to " + nameOf(t.start.from) + (t.start.done ? " \xB7 " + clock2(t.start.done) : "")
        };
      if (why === "replied") return { st: "done", text: "Replied" };
      return { st: st ?? "idle", text: "No reply in these logs" };
    }
    function traceRoot(t) {
      const seen = /* @__PURE__ */ new Set();
      while (!seen.has(t.id)) {
        seen.add(t.id);
        const up = t.start && t.start.from !== "you" ? HOLDS.get(t.start.id) : null;
        if (!up) break;
        t = up;
      }
      return t;
    }
    const countOf = (s, key) => {
      const m = TXM[s.id];
      return (m && m.to >= m.total ? m[key] : void 0) ?? s[key] ?? m?.[key] ?? null;
    };
    const callsText = (calls) => (calls == null ? "\u2014" : calls) + (calls === 1 ? " tool call" : " tool calls");
    let childrenCache = null;
    const invalidate = () => {
      childrenCache = null;
    };
    const sessionChildren = () => {
      if (childrenCache) return childrenCache;
      const children = /* @__PURE__ */ new Map();
      for (const session of Object.values(SESS)) {
        const parent = parentOf(session.id);
        if (parent && SESS[parent]) {
          const rows = children.get(parent) ?? [];
          rows.push(session);
          children.set(parent, rows);
        }
      }
      for (const rows of children.values()) rows.sort((a, b) => b.last - a.last);
      return childrenCache = children;
    };
    const childSessions = (sid) => [...sessionChildren().get(sid) ?? []].sort(
      (a, b) => (originHandoff(a.id)?.at ?? a.last) - (originHandoff(b.id)?.at ?? b.last)
    );
    const descendantsOf = (sid, children, out = [], seen = /* @__PURE__ */ new Set([sid])) => {
      for (const child of children.get(sid) ?? [])
        if (!seen.has(child.id)) {
          seen.add(child.id);
          out.push(child);
          descendantsOf(child.id, children, out, seen);
        }
      return out;
    };
    const TOTAL_TOKEN_KINDS = ["input", "output", "cache_write", "cache_read"];
    const TOKEN_KINDS = [
      ["input", "Input"],
      ["output", "Output"],
      ["cache_read", "Cache read"],
      ["cache_write_5m", "Cache write \xB7 5m"],
      ["cache_write_1h", "Cache write \xB7 1h"],
      ["web_search", "Web search"]
    ];
    const asMoney = (usd) => "$" + usd.toFixed(2), shortMoney = (usd) => "$" + usd.toFixed(1);
    const usageTotal = (s) => Object.values(s.tokens_by_model ?? {}).reduce(
      (sum, usage2) => sum + TOTAL_TOKEN_KINDS.reduce((n, key) => n + (Number(usage2[key]) || 0), 0),
      0
    );
    function costForSessions(sessions) {
      const total = {
        usd: 0,
        unpriced_models: [],
        split_unknown_messages: 0,
        by_model: {},
        by_day: {}
      }, unpriced = /* @__PURE__ */ new Set();
      let allPriced = true;
      for (const s of sessions) {
        const cost2 = s.cost ?? {};
        if (cost2.usd == null) allPriced = false;
        else total.usd = (total.usd ?? 0) + (Number(cost2.usd) || 0);
        for (const model2 of cost2.unpriced_models ?? []) unpriced.add(model2);
        total.split_unknown_messages += Number(cost2.split_unknown_messages) || 0;
        for (const [day2, amount] of Object.entries(cost2.by_day ?? {}))
          total.by_day[day2] = (total.by_day[day2] ?? 0) + (Number(amount) || 0);
        for (const [modelId, model2] of Object.entries(cost2.by_model ?? {})) {
          const current = Object.assign(
            { tokens: {}, usd_by_kind: {} },
            total.by_model[modelId] ?? { usd: 0, tokens: {}, usd_by_kind: {} }
          );
          if (model2.usd == null) current.usd = null;
          else if (current.usd != null) current.usd += Number(model2.usd) || 0;
          for (const [key, amount] of Object.entries(model2.tokens ?? {}))
            current.tokens[key] = (current.tokens[key] ?? 0) + (Number(amount) || 0);
          for (const [key, amount] of Object.entries(model2.usd_by_kind ?? {}))
            current.usd_by_kind[key] = (current.usd_by_kind[key] ?? 0) + (Number(amount) || 0);
          total.by_model[modelId] = current;
        }
      }
      total.unpriced_models = [...unpriced].sort();
      if (!allPriced || unpriced.size) total.usd = null;
      return total;
    }
    const costForSession = (sid, includeRuns = false) => costForSessions(
      SESS[sid] ? [SESS[sid], ...includeRuns ? descendantsOf(sid, sessionChildren()) : []] : []
    );
    const costText = (cost2) => cost2.usd == null || cost2.unpriced_models?.length ? "\u2014" : asMoney(cost2.usd);
    const costMissing = (cost2) => cost2.unpriced_models ?? [];
    const TREE_RANK = { wait: 0, work: 1, err: 2, idle: 3, done: 4 };
    const urgentDescendant = (sid, children) => descendantsOf(sid, children).filter((s) => s.state in TREE_RANK).sort((a, b) => TREE_RANK[a.state] - TREE_RANK[b.state] || b.last - a.last)[0]?.state;
    const childParts = (all) => {
      const n = (state3) => all.filter((x) => x.state === state3).length, wait = n("wait"), work = n("work"), err = n("err");
      return [
        all.length + (all.length === 1 ? " run" : " runs"),
        wait && wait + " needs you",
        work && work + " working",
        err && err + " failed"
      ].filter(Boolean);
    };
    const defaultTreeOpen = (sid, children) => descendantsOf(sid, children).some((s) => s.state === "wait" || s.state === "work");
    function kidRank(c, children) {
      let rank = c.state === "wait" ? 0 : c.state === "work" ? 1 : 2;
      if (rank)
        for (const d of descendantsOf(c.id, children)) {
          if (d.state === "wait") return 0;
          if (d.state === "work") rank = 1;
        }
      return rank;
    }
    function lineageOf(sid) {
      const path = [], seen = /* @__PURE__ */ new Set();
      let id = sid;
      while (id && SESS[id] && !seen.has(id)) {
        seen.add(id);
        path.push(SESS[id]);
        id = parentOf(id);
      }
      return path.reverse();
    }
    const byState = (a, b) => (RANK[a.state] ?? 3) - (RANK[b.state] ?? 3) || b.last - a.last;
    const onMachine = (m) => Object.values(SESS).filter((s) => s.machine === m).sort(byState);
    const movedOff = (m) => Object.values(SESS).filter((s) => s.movedFrom === m);
    const movesOf = (m) => H.filter((h2) => h2.kind === "move" && (h2.fromMachine === m || h2.toMachine === m)).sort(
      (a, b) => b.at - a.at
    );
    return {
      invalidate,
      nameOf,
      hcls,
      where,
      hostOf,
      machineLabels,
      machineLabel,
      branchOf,
      shortHost,
      parentOf,
      originHandoff,
      RANK,
      isResult,
      inbox,
      working,
      answersOf,
      statWord,
      hasTurn,
      oneLine,
      TOYOU,
      turnEnd: turnEnd2,
      traceRoot,
      countOf,
      callsText,
      sessionChildren,
      childSessions,
      descendantsOf,
      TOTAL_TOKEN_KINDS,
      TOKEN_KINDS,
      asMoney,
      usageTotal,
      costForSessions,
      costForSession,
      costText,
      costMissing,
      TREE_RANK,
      urgentDescendant,
      childParts,
      defaultTreeOpen,
      kidRank,
      lineageOf,
      byState,
      onMachine,
      movedOff,
      movesOf,
      shortMoney
    };
  }

  // src/navigation/routes.ts
  var NavigationController = class {
    constructor(host2, initial = null) {
      this.host = host2;
      this.content = initial;
    }
    host;
    route = { v: "home" };
    rendered = null;
    content;
    nativePending = null;
    routePending = null;
    disposed = false;
    cancelNative() {
      this.nativePending?.abort();
      this.nativePending = null;
    }
    cancelRoute() {
      this.routePending?.abort();
      this.routePending = null;
    }
    loadNative(route, ready, failed) {
      if (this.disposed || !this.host.loadMachines) return;
      this.cancelNative();
      const request = new AbortController();
      this.nativePending = request;
      this.host.loadMachines(request.signal).then(
        (content) => {
          if (this.disposed || request.signal.aborted || this.nativePending !== request) {
            content.destroy();
            return;
          }
          this.nativePending = null;
          ready(content);
        },
        () => {
          if (!this.disposed && !request.signal.aborted && this.nativePending === request) {
            this.nativePending = null;
            failed();
          }
        }
      );
    }
    replaceContent(route, next) {
      if (next) {
        this.content?.destroy();
        this.content = next;
      } else if (route.v !== "machines" && this.content) {
        this.content.destroy();
        this.content = null;
      }
    }
    load(route, request, done, failed) {
      if (this.disposed) return;
      this.cancelRoute();
      const controller = new AbortController();
      this.routePending = controller;
      const loaded = request(controller.signal);
      const current = () => !this.disposed && !controller.signal.aborted && this.routePending === controller && this.route === route;
      if (loaded)
        loaded.then(
          () => {
            if (current()) {
              this.routePending = null;
              done();
            }
          },
          (error) => {
            if (current()) {
              this.routePending = null;
              failed(error);
            }
          }
        );
      else if (current()) {
        this.routePending = null;
        done();
      }
    }
    historyRoute(value, fallback) {
      if (!value || typeof value !== "object") return fallback;
      let route = fallback;
      if ("v" in value) {
        const v = value.v;
        if (v === "home" || v === "analytics" || v === "sessions" || v === "machines") route = { v };
        else if (v === "timeline") route = { v: "analytics" };
        else if (v === "session" && "id" in value && typeof value.id === "string" && this.host.model?.session(value.id)) {
          const turn = "turn" in value && typeof value.turn === "string" ? this.host.model.turn(value.turn) : void 0;
          route = turn?.sid === value.id ? { v, id: value.id, turn: turn.id } : { v, id: value.id };
        } else if (v === "machine" && "id" in value && typeof value.id === "string" && this.host.model?.machine(value.id))
          route = { v, id: value.id };
        else if (v === "trace" && "sid" in value && typeof value.sid === "string" && "turn" in value && typeof value.turn === "string" && this.host.model?.session(value.sid) && this.host.model.turn(value.turn)?.sid === value.sid)
          route = { v, sid: value.sid, turn: value.turn };
      }
      const out = { ...route };
      if ("scrollTop" in value && typeof value.scrollTop === "number" && Number.isFinite(value.scrollTop))
        out.scrollTop = value.scrollTop;
      if ("hostFocus" in value && value.hostFocus && typeof value.hostFocus === "object") {
        const focus = value.hostFocus;
        out.hostFocus = {
          id: "id" in focus && typeof focus.id === "string" ? focus.id : void 0,
          row: "row" in focus && typeof focus.row === "string" ? focus.row : void 0,
          label: "label" in focus && typeof focus.label === "string" ? focus.label : void 0
        };
      }
      return out;
    }
    destroy() {
      if (this.disposed) return;
      this.disposed = true;
      this.cancelNative();
      this.cancelRoute();
      this.content?.destroy();
      this.content = null;
      this.rendered = null;
    }
  };

  // src/domain/validate.ts
  function object2(value) {
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
    return Object.fromEntries(Object.entries(object2(value)).map(([id, row]) => [id, parse(row)]));
  }
  function state(value) {
    switch (value) {
      case "work":
      case "wait":
      case "idle":
      case "done":
      case "err":
        return value;
      default:
        throw new Error("Invalid session state");
    }
  }
  function status(value) {
    switch (value) {
      case "work":
      case "wait":
      case "done":
      case "err":
      case "new":
        return value;
      default:
        throw new Error("Invalid handoff status");
    }
  }

  // src/domain/normalize.ts
  var strings2 = (v) => array(v, text);
  var numbers = (v) => dictionary(v, number);
  function usage(value) {
    const v = object2(value);
    return {
      input: optional(v.input, number),
      output: optional(v.output, number),
      cache_write: optional(v.cache_write, number),
      cache_write_5m: optional(v.cache_write_5m, number),
      cache_write_1h: optional(v.cache_write_1h, number),
      cache_read: optional(v.cache_read, number),
      web_search: optional(v.web_search, number)
    };
  }
  function modelCost(value) {
    const v = object2(value);
    return {
      usd: v.usd == null ? null : number(v.usd),
      tokens: optional(v.tokens, usage),
      usd_by_kind: optional(v.usd_by_kind, usage)
    };
  }
  function cost(value) {
    const v = object2(value);
    return {
      usd: v.usd == null ? null : number(v.usd),
      unpriced_models: optional(v.unpriced_models, strings2),
      split_unknown_messages: optional(v.split_unknown_messages, number),
      by_model: optional(v.by_model, (v2) => dictionary(v2, modelCost)),
      by_day: optional(v.by_day, numbers)
    };
  }
  function activity(value) {
    if (!Array.isArray(value) || value.length < 3 || value.length > 4)
      throw new Error("Invalid activity");
    return [text(value[0]), text(value[1]), number(value[2]), optional(value[3], number)];
  }
  function parseSession(id, value) {
    const v = object2(value);
    return {
      id,
      name: text(v.name),
      harness: text(v.harness),
      state: state(v.state),
      machine: text(v.machine),
      start: number(v.start),
      last: number(v.last),
      effort: optional(v.effort, text),
      cwd: optional(v.cwd, text),
      dir: optional(v.dir, text),
      directory: optional(v.directory, text),
      sessionId: optional(v.sessionId, text),
      pid: optional(v.pid, number),
      wait_edges_truncated: optional(v.wait_edges_truncated, boolean),
      busy: optional(
        v.busy,
        (v2) => array(v2, (v3) => {
          if (!Array.isArray(v3) || v3.length !== 2) throw new Error("Invalid interval");
          return [number(v3[0]), number(v3[1])];
        })
      ),
      wait_edges: optional(
        v.wait_edges,
        (v2) => array(v2, (v3) => {
          const w = object2(v3);
          return {
            call: text(w.call),
            tool: text(w.tool),
            targets: strings2(w.targets),
            start: number(w.start),
            end: optional(w.end, number),
            turn: optional(w.turn, text)
          };
        })
      ),
      reported_runs: optional(
        v.reported_runs,
        (v2) => array(v2, (v3) => {
          const r = object2(v3);
          return { start: number(r.start), cost_usd: optional(r.cost_usd, number) };
        })
      ),
      cost_check: optional(
        v.cost_check,
        (v2) => array(v2, (v3) => {
          const c = object2(v3);
          return {
            start: number(c.start),
            computed_usd: optional(c.computed_usd, number),
            reported_usd: optional(c.reported_usd, number),
            ok: optional(c.ok, boolean)
          };
        })
      ),
      kind: optional(v.kind, text),
      host: optional(v.host, text),
      repo: optional(v.repo, text),
      branch: optional(v.branch, text),
      worktree: optional(v.worktree, text),
      parent: optional(v.parent, text),
      lane: optional(v.lane, boolean),
      model: optional(v.model, text),
      modelId: optional(v.modelId, text),
      role: optional(v.role, boolean),
      stub: optional(v.stub, boolean),
      movedFrom: optional(v.movedFrom, text),
      calls: optional(v.calls, number),
      errors: optional(v.errors, number),
      waiting_since: optional(v.waiting_since, number),
      waiting_for: optional(v.waiting_for, text),
      tokens: optional(v.tokens, (v2) => array(v2, number)),
      tokens_by_model: optional(v.tokens_by_model, (v2) => dictionary(v2, usage)),
      cost: optional(v.cost, cost),
      activity: optional(v.activity, activity),
      tool_calls: optional(v.tool_calls, numbers),
      signals: optional(v.signals, numbers)
    };
  }
  function answer(value) {
    return typeof value === "string" ? value : strings2(value);
  }
  function parseHandoff(value) {
    const v = object2(value), common = {
      id: text(v.id),
      from: text(v.from),
      to: v.to == null ? "" : text(v.to),
      at: number(v.at),
      status: status(v.status),
      brief: text(v.brief),
      done: optional(v.done, number),
      result: optional(v.result, text),
      target: optional(v.target, text),
      declined: optional(v.declined, boolean)
    };
    switch (v.kind) {
      case "ask":
      case "spawn":
      case "relay":
        return { ...common, kind: v.kind };
      case "move":
        return {
          ...common,
          kind: "move",
          fromMachine: text(v.fromMachine),
          toMachine: text(v.toMachine)
        };
      case "toyou": {
        if (v.ask !== "question" && v.ask !== "decision" && v.ask !== "result")
          throw new Error("Invalid ask");
        return {
          ...common,
          kind: "toyou",
          ask: v.ask,
          answer: optional(v.answer, answer),
          answers: optional(
            v.answers,
            (v2) => array(v2, (v3) => ({ values: optional(object2(v3).values, strings2) }))
          )
        };
      }
      default:
        throw new Error("Invalid handoff kind");
    }
  }
  function turnEnd(value) {
    const v = object2(value);
    return {
      st: optional(v.st, state),
      why: text(v.why),
      h: optional(v.h, text),
      at: optional(v.at, number)
    };
  }
  function normalizeModel(model2) {
    const sessions = Object.fromEntries(
      Object.entries(model2.sessions).map(([id, v]) => [id, parseSession(id, v)])
    );
    const handoffs = model2.handoffs.map(parseHandoff);
    for (const h2 of handoffs)
      if (!h2.to) {
        const machine2 = sessions[h2.from]?.machine ?? model2.machine.id, id = "unsent:" + (h2.target ?? "") + (model2.machines ? "@" + machine2 : "");
        const s = sessions[id] ??= {
          id,
          name: h2.target || "unknown",
          harness: "claude",
          stub: true,
          machine: machine2,
          state: "err",
          model: "\u2014",
          tokens: [0, 0, 0],
          start: h2.at,
          last: h2.at
        };
        s.start = Math.min(s.start, h2.at);
        s.last = Math.max(s.last, h2.at);
        h2.to = id;
      }
    const handoff = new Map(handoffs.map((h2) => [h2.id, h2])), turns = /* @__PURE__ */ Object.create(null), turn = /* @__PURE__ */ new Map(), starts = /* @__PURE__ */ new Map(), holds = /* @__PURE__ */ new Map();
    for (const row of model2.turns) {
      const x = object2(row), id = text(x.id), sid = text(x.sid), start = optional(x.start, text), sent = array(x.sent, text).flatMap((id2) => {
        const h2 = handoff.get(id2);
        return h2 ? [h2] : [];
      });
      const t = {
        id,
        sid,
        start: start ? handoff.get(start) ?? null : null,
        at: optional(x.at, number),
        u: x.u ? { k: "u", text: optional(x.text, text) ?? "" } : null,
        entries: [],
        sent,
        out: sent.filter((h2) => h2.kind !== "move"),
        end: optional(x.end, turnEnd),
        last: optional(x.last, boolean)
      };
      (turns[sid] ??= []).push(t);
      turn.set(id, t);
      if (t.start) starts.set(t.start.id, t);
      for (const h2 of t.sent) holds.set(h2.id, t);
    }
    return { sessions, handoffs, handoff, turns, turn, starts, holds };
  }

  // src/state/model.ts
  function replaceRecord(target, next) {
    for (const id of Object.keys(target)) delete target[id];
    Object.assign(target, next);
  }
  function replaceMap(target, next) {
    target.clear();
    for (const [id, row] of next) target.set(id, row);
  }
  var ViewerModelStore = class {
    raw = new ModelStore();
    sessions = /* @__PURE__ */ Object.create(null);
    machines = /* @__PURE__ */ Object.create(null);
    machineUp = /* @__PURE__ */ Object.create(null);
    machineLast = /* @__PURE__ */ Object.create(null);
    handoffs = [];
    turns = /* @__PURE__ */ Object.create(null);
    turn = /* @__PURE__ */ new Map();
    starts = /* @__PURE__ */ new Map();
    holds = /* @__PURE__ */ new Map();
    handoff = /* @__PURE__ */ new Map();
    apply(value) {
      return this.raw.apply(value);
    }
    adopt(value) {
      const model2 = parseModel(value), normalized = normalizeModel(model2);
      const machines = model2.machines == null ? [parseModelMachine(model2.machine)] : array(model2.machines, parseModelMachine);
      this.raw.adopt(model2);
      replaceRecord(this.sessions, normalized.sessions);
      replaceRecord(this.turns, normalized.turns);
      this.handoffs.splice(0, this.handoffs.length, ...normalized.handoffs);
      replaceMap(this.turn, normalized.turn);
      replaceMap(this.starts, normalized.starts);
      replaceMap(this.holds, normalized.holds);
      replaceMap(this.handoff, normalized.handoff);
      for (const table of [this.machines, this.machineUp, this.machineLast])
        for (const id of Object.keys(table)) delete table[id];
      for (const row of machines) {
        this.machines[row.id] = row.name;
        this.machineUp[row.id] = row.up;
        if (row.last !== void 0) this.machineLast[row.id] = row.last;
      }
      return model2;
    }
    domain(transcriptMeta) {
      return {
        sessions: this.sessions,
        machines: this.machines,
        handoffs: this.handoffs,
        turns: this.turns,
        turn: this.turn,
        starts: this.starts,
        holds: this.holds,
        handoff: this.handoff,
        transcriptMeta
      };
    }
  };
  function parseModelMachine(value) {
    const v = object2(value);
    return { id: text(v.id), name: text(v.name), up: boolean(v.up), last: optional(v.last, number) };
  }

  // src/state/transcript-wire.ts
  var strings3 = (value) => array(value, text);
  function background(value) {
    const v = object2(value), state2 = v.state;
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
    const v = object2(value), base = {
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
    const v = object2(value);
    return {
      original_tokens: optional(v.original_tokens, number),
      parts: optional(
        v.parts,
        (v2) => array(v2, (v3) => {
          const p = object2(v3);
          return {
            text: optional(p.text, text),
            gap: optional(p.gap, (v4) => {
              const g = object2(v4);
              return { unit: text(g.unit), n: number(g.n), of: optional(g.of, number) };
            })
          };
        })
      )
    };
  }
  function parseEntry(value) {
    const v = object2(value), base = {
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
            const r = object2(v2);
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
            const s = object2(v.signal);
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
              const c = object2(v3);
              return {
                path: text(c.path),
                move: optional(c.move, text),
                diff: optional(c.diff, diff)
              };
            })
          ),
          more: optional(v.more, strings3),
          script: v.script,
          cut: optional(v.cut, cut)
        };
      default:
        throw new Error("Invalid transcript entry");
    }
  }
  function parseTranscriptPage(value) {
    const v = object2(value), from = number(v.from), to = number(v.to), total = number(v.total);
    if (![from, to, total].every(Number.isInteger) || from < 0 || to < from || total < to)
      throw new Error("Invalid transcript range");
    return {
      entries: array(v.entries, parseEntry),
      from,
      to,
      total,
      calls: optional(v.calls, number),
      errors: optional(v.errors, number),
      bg_running: optional(v.bg_running, strings3)
    };
  }

  // src/state/transcript.ts
  var TranscriptStore = class {
    constructor(host2) {
      this.host = host2;
    }
    host;
    entries = /* @__PURE__ */ Object.create(null);
    meta = /* @__PURE__ */ Object.create(null);
    marks = /* @__PURE__ */ Object.create(null);
    staleBriefs = /* @__PURE__ */ new Set();
    cache = new TranscriptCache();
    paging = createPagingStore();
    requests = /* @__PURE__ */ new Set();
    replacements = /* @__PURE__ */ new Map();
    disposed = false;
    spread(sid) {
      for (const turn2 of this.host.turns(sid)) turn2.entries = [];
      let turn, pre = 0;
      for (const entry of this.entries[sid] ?? []) {
        if (entry.turn) turn = this.host.turn(entry.turn);
        if (turn && turn.sid === sid) {
          entry.key = turn.id + "#" + turn.entries.length;
          turn.entries.push(entry);
        } else entry.key = sid + "#" + pre++;
      }
    }
    clearPaging(sid) {
      this.paging.clear(sid);
      this.host.cleared(sid);
    }
    cancelReplacement(sid) {
      this.replacements.get(sid)?.abort();
      this.replacements.delete(sid);
    }
    drop(sid) {
      this.cancelReplacement(sid);
      this.clearPaging(sid);
      delete this.entries[sid];
      delete this.meta[sid];
    }
    async fetch(sid, query = "", direction, signal, onPage) {
      if (this.disposed || signal?.aborted) return;
      const controller = new AbortController(), abort = () => controller.abort();
      signal?.addEventListener("abort", abort, { once: true });
      this.requests.add(controller);
      if (!direction) {
        this.cancelReplacement(sid);
        this.replacements.set(sid, controller);
      }
      const mark = this.marks[sid], range = this.meta[sid], boundary = direction === "before" ? range?.from : range?.to;
      try {
        const response = await this.host.request(
          "/api/tx?sid=" + encodeURIComponent(sid) + (query ? "&" + query : ""),
          controller.signal
        );
        if (this.disposed || controller.signal.aborted || direction && (this.meta[sid] !== range || (direction === "before" ? this.meta[sid]?.from : this.meta[sid]?.to) !== boundary))
          return;
        const page = parseTranscriptPage(response), entries = page.entries.map((entry) => this.host.entry({ ...entry, sid })), meta = this.meta[sid];
        if (direction === "before" && meta) {
          this.entries[sid] = entries.concat(this.entries[sid]);
          meta.from = page.from;
        } else if (direction === "after" && meta) {
          this.entries[sid] = this.entries[sid].concat(entries);
          meta.to = page.to;
        } else {
          this.clearPaging(sid);
          this.entries[sid] = entries;
          this.meta[sid] = { from: page.from, to: page.to, total: page.total };
          this.staleBriefs.delete(sid);
        }
        Object.assign(this.meta[sid], {
          total: page.total,
          calls: page.calls,
          errors: page.errors,
          watchTok: mark
        });
        if (direction !== "before" && page.to >= page.total) {
          this.meta[sid].tok = mark;
          this.meta[sid].newer = 0;
        }
        this.spread(sid);
        onPage?.();
      } catch (error) {
        if (!this.disposed && !controller.signal.aborted) throw error;
      } finally {
        this.requests.delete(controller);
        signal?.removeEventListener("abort", abort);
        if (this.replacements.get(sid) === controller) this.replacements.delete(sid);
      }
    }
    keep(sid, entries, meta, origin) {
      if (!entries || !meta || meta.to < meta.total || meta.tok == null || this.staleBriefs.has(sid) || meta.origin !== origin)
        return;
      this.cache.keep(sid, entries, meta);
    }
    adoptCached(sid, turn) {
      const cached = this.cache.get(sid);
      if (!cached) return false;
      this.cancelReplacement(sid);
      this.cache.delete(sid);
      this.entries[sid] = cached.entries;
      this.meta[sid] = cached.meta;
      if (!turn) return true;
      this.spread(sid);
      const target = this.host.turn(turn);
      if (target && target.sid === sid && !target.entries.length) {
        this.drop(sid);
        return false;
      }
      return true;
    }
    destroy() {
      if (this.disposed) return;
      this.disposed = true;
      for (const request of this.requests) request.abort();
      this.requests.clear();
      this.replacements.clear();
      this.paging.destroy();
      this.cache.clear();
      this.staleBriefs.clear();
      for (const id of Object.keys(this.entries)) delete this.entries[id];
      for (const id of Object.keys(this.meta)) delete this.meta[id];
    }
  };

  // src/app/accountControls.ts
  function createAccountControls(host2) {
    const accountHost = {
      place(widget, trigger) {
        const at = trigger.getBoundingClientRect();
        setGeometry(widget, "accountLeft", at.left);
        setGeometry(widget, "accountWidth", at.width);
        setGeometry(widget, "accountBottom", Math.max(0, innerHeight - at.top + 6));
      },
      opened(compact) {
        if (compact)
          try {
            history.pushState(
              { ...host2.navigation.route, sheet: 1, scrollTop: host2.currentScroll() },
              ""
            );
            host2.accountSheet = true;
          } catch {
          }
      },
      closed({ keepEntry, navigating }) {
        if (host2.disposed) return;
        if (host2.accountSheet) {
          host2.accountSheet = false;
          if (!keepEntry && history.state?.sheet) {
            host2.toolViewsOwner.skipPop = true;
            history.back();
          }
        }
        if (host2.LIVE.pending && !navigating)
          host2.scope.timeout(() => {
            if (host2.LIVE.pending && !host2.toolViewsOwner.viewerEl && !accountChrome.open)
              host2.refresh();
          }, 0);
      },
      navigate(href) {
        if (!host2.accountSheet) return false;
        leaveAccountSheet(() => location.assign(href));
        return true;
      },
      submit(form) {
        if (!host2.accountSheet) return false;
        leaveAccountSheet(() => form.submit());
        return true;
      }
    };
    const shellChrome = host2.SIDEBAR_ONLY ? null : createShellChrome({
      account: accountHost,
      navigate(destination) {
        host2.go(host2.navigation.historyRoute({ v: destination.key }, { v: "home" }));
        return true;
      },
      drawerOpened() {
        host2.orderApply("side");
      },
      drawerClosed() {
        host2.scope.timeout(() => {
          if (host2.phone.matches && !document.body.classList.contains("drawer-open"))
            host2.orderApply("side");
        }, host2.ORD_DRAWER_MS);
      },
      railChanged() {
        host2.setRailMode(!host2.layoutOwner.railMode);
        host2.renderNav();
      }
    });
    if (shellChrome) shellChrome.mount(host2.app);
    const accountChrome = shellChrome?.account ?? createAccountChrome(accountHost);
    function closeAccountMenu(keepEntry = void 0, navigating = void 0) {
      accountChrome.close({ keepEntry, navigating });
    }
    function leaveAccountSheet(go) {
      host2.accountSheet = false;
      if (history.state?.sheet) {
        host2.toolViewsOwner.skipPop = true;
        host2.afterPop = go;
        history.back();
      } else go();
    }
    function accountWidget(compact) {
      return host2.ACCOUNT ? accountChrome.mount({
        account: host2.ACCOUNT,
        compact,
        wide: host2.layoutOwner.wideMode,
        onWideChange: () => host2.setWideMode(!host2.layoutOwner.wideMode)
      }) : null;
    }
    function renderDrawerAccount() {
      if (shellChrome) {
        shellChrome.drawerAccount(
          host2.ACCOUNT ? {
            account: host2.ACCOUNT,
            compact: true,
            wide: host2.layoutOwner.wideMode,
            onWideChange: () => host2.setWideMode(!host2.layoutOwner.wideMode)
          } : null
        );
        return;
      }
      const old = host2.$("#account-drawer");
      if (old) {
        accountChrome.unmount(old);
        old.remove();
      }
      if (!host2.ACCOUNT) return;
      const widget = accountWidget(true);
      if (!widget) return;
      widget.id = "account-drawer";
      host2.$("#sidebar").append(widget);
    }
    return { shellChrome, accountChrome, closeAccountMenu, renderDrawerAccount };
  }

  // src/state/analytics-wire.ts
  function shape(fields) {
    return (value) => {
      const source = object2(value), parsed = Object.fromEntries(
        Object.entries(fields).map(([key, parse]) => [key, parse(source[key])])
      );
      return parsed;
    };
  }
  var list = (parse) => (value) => array(value, parse);
  var nullable = (parse) => (value) => value == null ? null : parse(value);
  var optional2 = (parse) => (value) => value == null ? void 0 : parse(value);
  var strings4 = list(text);
  var money = shape({ usd: nullable(number), unpriced_models: strings4 });
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
  var priced = shape({ sid: text, usd: nullable(number), unpriced_models: strings4 });
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
    unpriced_models: strings4
  });
  var tokens = shape({
    input: optional2(number),
    output: optional2(number),
    cache_read: optional2(number),
    cache_write: optional2(number)
  });
  var item = shape({
    sid: text,
    models: optional2(strings4),
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
    cost: nullable(shape({ days: list(day), unpriced_models: strings4 })),
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
    facets: shape({ repo: list(nullable(text)), machine: strings4, harness: strings4, model: strings4 })
  });

  // src/app/analytics.ts
  function createAnalytics(host2) {
    const MIN = 6e4, HOUR = 60 * MIN, AN_EVERY = 1e4, AN_KEEP = 8;
    const AN = {
      answers: /* @__PURE__ */ new Map(),
      inflight: null,
      again: false,
      againAsked: false,
      timer: void 0,
      error: null,
      failedAt: 0
    };
    function analyticsQuery() {
      const q = ["range=" + (host2.analyticsRange === 1 ? "24h" : host2.analyticsRange + "d")];
      if (host2.sessionFilters.repo)
        q.push(
          "repo=" + (host2.sessionFilters.repo === "__none__" ? "" : host2.enc(host2.sessionFilters.repo))
        );
      for (const key of ["machine", "harness", "model"])
        if (host2.sessionFilters[key]) q.push(key + "=" + host2.enc(host2.sessionFilters[key]));
      return q.join("&");
    }
    const analyticsData = () => AN.answers.get(analyticsQuery())?.data ?? null;
    function fetchAnalytics(askedByUser = false) {
      if (AN.inflight) {
        AN.again = true;
        AN.againAsked ||= askedByUser;
        return AN.inflight;
      }
      const key = analyticsQuery(), kept = AN.answers.get(key);
      const controller = host2.scope.request();
      const asked = fetch("/api/analytics?" + key, {
        signal: controller.signal,
        credentials: "same-origin",
        headers: kept?.etag ? { "If-None-Match": kept.etag } : {}
      }).then((r) => {
        if (host2.disposed) throw new DOMException("Viewer destroyed", "AbortError");
        if (r.status === 304) {
          const cleared = AN.error != null;
          AN.error = null;
          return cleared;
        }
        if (r.status === 403) {
          host2.ended(403);
          return false;
        }
        if (!r.ok)
          throw Object.assign(new Error(r.status + " " + r.statusText), { status: r.status });
        const etag = r.headers.get("ETag");
        return r.json().then((value) => {
          if (host2.disposed) return false;
          const data = parseAnalytics(value);
          AN.answers.delete(key);
          AN.answers.set(key, { etag, data });
          while (AN.answers.size > AN_KEEP) AN.answers.delete(AN.answers.keys().next().value);
          const changed = AN.error != null || kept?.etag !== etag;
          AN.error = null;
          return changed;
        });
      }).catch((e) => {
        if (host2.disposed) return false;
        const changed = AN.error !== e.message;
        AN.error = e.message;
        AN.failedAt = performance.now();
        return changed;
      });
      AN.inflight = asked.then((changed) => {
        host2.scope.releaseRequest(controller);
        AN.inflight = null;
        if (host2.disposed || !AN.again) return changed;
        const askedAgain = AN.againAsked;
        AN.again = AN.againAsked = false;
        if (!askedAgain && backingOff()) return changed;
        return fetchAnalytics(askedAgain).then((more) => changed || more);
      });
      return AN.inflight;
    }
    const backingOff = () => AN.error != null && performance.now() - AN.failedAt < AN_EVERY;
    function scheduleAnalytics() {
      host2.scope.clearTimeout(AN.timer);
      AN.timer = void 0;
      if (host2.navigation.route.v !== "analytics" || host2.LIVE.ended || !host2.visible()) return;
      const data = analyticsData(), behind = !AN.error && data && host2.LIVE.version && data.version !== host2.LIVE.version;
      AN.timer = host2.scope.timeout(
        () => {
          AN.timer = void 0;
          refreshAnalytics();
        },
        AN.error ? Math.max(0, AN.failedAt + AN_EVERY - performance.now()) : behind ? 1200 : AN_EVERY
      );
    }
    function refreshAnalytics(asked = false) {
      if (host2.navigation.route.v !== "analytics") return Promise.resolve();
      if (asked !== true && backingOff()) {
        if (!AN.timer) scheduleAnalytics();
        return Promise.resolve();
      }
      host2.scope.clearTimeout(AN.timer);
      AN.timer = void 0;
      return fetchAnalytics(asked === true).then((changed) => {
        if (changed && host2.navigation.route.v === "analytics" && host2.navigation.rendered === host2.navigation.route) {
          if (host2.viewerEl || host2.accountChrome.open)
            host2.LIVE.pending = true;
          else {
            const st = host2.capture();
            host2.render();
            host2.restore(st);
          }
        }
        scheduleAnalytics();
      });
    }
    if (!host2.SIDEBAR_ONLY)
      host2.scope.listen(document, "visibilitychange", () => {
        if (host2.visible() && host2.navigation.route.v === "analytics") refreshAnalytics();
        else if (!host2.visible()) {
          host2.scope.clearTimeout(AN.timer);
          AN.timer = void 0;
        }
      });
    const nameOfSid = (A, sid) => host2.SESS[sid]?.name ?? A.sessions[sid]?.name ?? sid;
    const harnessOfSid = (A, sid) => host2.SESS[sid]?.harness ?? A.sessions[sid]?.harness ?? "";
    const sessionFacetValue = (s, key) => key === "repo" ? s.repo ?? "__none__" : key === "model" ? s.model ?? s.modelId ?? "Unknown model" : s[key] ?? "";
    function matchesSessionFacets(s) {
      return Object.keys(host2.sessionFilters).every(
        (key) => !host2.sessionFilters[key] || sessionFacetValue(s, key) === host2.sessionFilters[key]
      );
    }
    const rangeFacet = (key) => host2.navigation.route.v !== "analytics" ? [] : (analyticsData()?.facets?.[key] ?? []).map((v) => v ?? "__none__");
    const FACETS = [
      [
        "repo",
        "Repo",
        "All repos",
        () => [
          .../* @__PURE__ */ new Set([
            ...Object.values(host2.SESS).map((s) => sessionFacetValue(s, "repo")),
            ...rangeFacet("repo")
          ])
        ].sort((a, b) => a === "__none__" ? 1 : b === "__none__" ? -1 : a.localeCompare(b)),
        (v) => v === "__none__" ? "No repo" : v
      ],
      [
        "machine",
        "Machine",
        "All machines",
        () => [
          .../* @__PURE__ */ new Set([
            ...Object.values(host2.SESS).map((s) => s.machine ?? ""),
            ...rangeFacet("machine")
          ])
        ].sort(),
        (v) => host2.MACHINE[v] ?? v
      ],
      [
        "harness",
        "Harness",
        "All harnesses",
        () => [
          .../* @__PURE__ */ new Set([
            ...Object.values(host2.SESS).map((s) => s.harness ?? ""),
            ...rangeFacet("harness")
          ])
        ].sort(),
        (v) => host2.HARNESS[v] ?? v
      ],
      [
        "model",
        "Model",
        "All models",
        () => [
          .../* @__PURE__ */ new Set([
            ...Object.values(host2.SESS).map((s) => sessionFacetValue(s, "model")),
            ...rangeFacet("model")
          ])
        ].sort(),
        shortModel
      ]
    ];
    function renderFacetFilters(box, onChange) {
      const s = host2.slot("facets", box, (ctx) => {
        let before = "";
        const control = createFacetChrome({
          select(label, value, onChange2) {
            return createSelect({ label, value, options: [], onChange: onChange2 });
          },
          change(key, value) {
            host2.sessionFilters[key] = value;
          },
          cleared(key) {
            host2.sessionFilters[key] = "";
            ctx.sync();
            ctx.onChange();
          },
          canOpen() {
            return !host2.viewerEl;
          },
          opened(d) {
            ctx.sync();
            before = JSON.stringify(host2.sessionFilters);
            host2.viewerEl = d;
            try {
              history.pushState(
                { ...host2.navigation.route, sheet: 1, scrollTop: host2.currentScroll() },
                ""
              );
            } catch {
            }
          },
          closed(d, reason) {
            if (host2.disposed || reason === "destroyed") return;
            if (host2.viewerEl === d) {
              host2.viewerEl = null;
              if (history.state?.sheet) {
                host2.skipPop = true;
                history.back();
              }
            }
            if (JSON.stringify(host2.sessionFilters) !== before) {
              host2.LIVE.pending = false;
              ctx.onChange();
            } else if (host2.LIVE.pending) host2.refresh();
          },
          clear() {
            for (const key of Object.keys(host2.sessionFilters))
              host2.sessionFilters[key] = "";
          }
        });
        ctx.destroy = () => control.destroy();
        ctx.sync = () => control.update(
          FACETS.map(([key, label, allLabel, valuesOf, showValue]) => {
            const current = host2.sessionFilters[key], values = valuesOf().filter((value) => value !== ""), gone = current !== "" && !values.includes(current);
            if (gone) values.push(current);
            return {
              key,
              label,
              value: current,
              display: showValue(current),
              options: [
                { value: "", label: allLabel },
                ...values.map((value) => ({
                  value,
                  label: showValue(value) + (gone && value === current ? " (no sessions)" : "")
                }))
              ]
            };
          })
        );
        return control.element;
      });
      s.ctx.onChange = onChange;
      s.ctx.sync();
      return s.el;
    }
    const hoursText = (ms) => (ms / HOUR).toFixed(1) + " h", rangeName = () => host2.analyticsRange === 1 ? "24 h" : host2.analyticsRange + " d";
    function chartWidth() {
      const page = host2.$("#page"), style = getComputedStyle(page);
      return Math.max(
        280,
        Math.round(page.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight))
      );
    }
    const rangeAgo = (A) => A.days === 1 ? "24 h ago" : A.days + " d ago";
    function openModelItems(group) {
      const {
        d,
        body,
        show: open
      } = host2.panel(shortModel(group.model) + " \xB7 " + group.band, {
        label: "Work items for " + group.model + ", " + group.band
      });
      renderModelItems(
        body,
        group.n + " work items" + (group.small_sample ? " \xB7 small sample" : ""),
        (group.items ?? []).map((item2) => ({
          id: item2.sid,
          name: host2.SESS[item2.sid]?.name,
          description: (item2.models ?? []).map(shortModel).join(" \u2192 ") + " \xB7 " + (item2.cost_usd == null ? "cost unknown" : host2.asMoney(item2.cost_usd)),
          trace: (host2.TURNS[item2.sid] ?? []).find((t) => t.out.length)?.id,
          url: liveUrl(item2.pr_url ?? "") ?? void 0
        })),
        group.items_more ? group.items_more + " more items in the selected range" : void 0,
        {
          session(id) {
            host2.pendingSessionOpen = id;
            d.close();
          },
          trace(id) {
            host2.afterPop = () => host2.goTrace(id);
            d.close();
          }
        }
      );
      open();
    }
    function openAnalyticsSlice(A, a, b, items, more, costMode = false) {
      const when = new Date(a).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" }) + "\u2013" + new Date(b).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }), heading = costMode ? "Sessions with cost" : "Sessions busy";
      const sheet2 = createNativeSheet(
        {
          className: "analytics-slice",
          heading: heading + " \xB7 " + when,
          label: heading + " " + when,
          closeLabel: "Close sessions list"
        },
        {
          opened(d) {
            host2.viewerEl = d;
            document.documentElement.classList.add("viewer-open");
            try {
              history.pushState(
                { ...host2.navigation.route, sheet: 1, scrollTop: host2.currentScroll() },
                ""
              );
            } catch {
            }
          },
          closed(d) {
            host2.dialogs.delete(d);
            if (host2.disposed) return;
            document.documentElement.classList.remove("viewer-open");
            if (host2.viewerEl === d) {
              host2.viewerEl = null;
              if (history.state?.sheet) {
                host2.skipPop = true;
                history.back();
              } else if (host2.pendingSessionOpen) {
                const id = host2.pendingSessionOpen;
                host2.pendingSessionOpen = null;
                host2.goSession(id);
              }
            }
          }
        }
      );
      const rows = items.map((item2) => {
        const harness = harnessOfSid(A, item2.sid);
        return {
          id: item2.sid,
          name: nameOfSid(A, item2.sid),
          harness,
          harnessName: host2.HARNESS_SHORT[harness] ?? harness,
          harnessTip: host2.HARNESS[harness] && host2.HARNESS[harness] !== host2.HARNESS_SHORT[harness] ? host2.HARNESS[harness] : void 0,
          mark: host2.harnessSnapshot(harness),
          value: costMode ? host2.asMoney("usd" in item2 ? item2.usd ?? 0 : 0) : timeText("ms" in item2 ? item2.ms : 0) + " busy",
          href: host2.SESS[item2.sid] ? host2.urlOf({ v: "session", id: item2.sid }) : void 0,
          className: "analytics-session analytics-slice",
          missing: costMode && "unpriced_models" in item2 && item2.unpriced_models.length ? "no price for " + item2.unpriced_models.join(", ") : void 0
        };
      });
      renderSliceBody(
        sheet2.body,
        rows,
        more ?? 0,
        costMode ? "No sessions had a recorded cost then." : "No sessions were busy then.",
        (id) => {
          host2.pendingSessionOpen = id;
          sheet2.dialog.close();
        }
      );
      sheet2.cleanup(() => render(null, sheet2.body));
      host2.dialogs.set(sheet2.dialog, sheet2);
      sheet2.show();
    }
    function renderAnalytics(page) {
      const A = analyticsData(), metrics = [], charts = [], breakdowns = [], lists = [], modelGroups = /* @__PURE__ */ new Map(), slices = /* @__PURE__ */ new Map();
      const heading = "Measured activity \xB7 Last " + (host2.analyticsRange === 1 ? "24 hours" : host2.analyticsRange + " days");
      let error = AN.error ? A ? "Couldn't update Analytics: " + AN.error + ". Showing the last answer." : "Couldn't load Analytics: " + AN.error : A ? void 0 : "Loading\u2026";
      const note = (value, previous, format) => {
        const delta = value - previous;
        return Math.abs(delta) < 1e-9 ? { text: "No change vs previous " + rangeName() } : {
          lead: (delta > 0 ? "+" : "\u2212") + format(Math.abs(delta)),
          tone: delta > 0 ? "up" : "down",
          text: " vs previous " + rangeName()
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
        const key = costMode ? "cost" : "agents", W = chartWidth(), left = costMode ? 46 : 40, right = W - 4;
        if (costMode && !A.cost)
          return {
            key,
            heading: "Cost over time",
            info: host2.COST_TIP,
            empty: "Cost is recorded per UTC day, so there is no hourly series. Pick 7 d or 30 d for a daily chart.",
            width: W,
            left,
            right,
            grid: [],
            bins: [],
            legend: [],
            role: "img",
            label: "",
            ago: rangeAgo(A)
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
          const id = key + ":" + i, total = bin.claude + bin.codex, when = host2.clock(bin.a) + "\u2013" + host2.clock(bin.b), tip = when + ": " + hLabel(total);
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
            label: costMode ? host2.clock(bin.a) + " to " + host2.clock(bin.b) + ": " + host2.asMoney(total) : tip + ". Open the sessions busy then",
            when,
            value: costMode ? host2.asMoney(total) : hLabel(total) + " agent-hours"
          };
        });
        return {
          key,
          heading: costMode ? "Cost over time" : "Agents at work",
          info: costMode ? host2.COST_TIP : void 0,
          sub: costMode ? "API-equivalent cost per UTC day \xB7 today so far \xB7 stacked by harness" : "Agent-hours " + A.agents.unit + " \xB7 stacked by harness",
          width: W,
          left,
          right,
          grid,
          bins,
          label: costMode ? "API-equivalent cost per day, stacked by harness" : "Agent-hours " + A.agents.unit + " over the selected range, stacked by harness",
          role: costMode ? "img" : "group",
          ago: rangeAgo(A),
          legend: [
            ["claude", "Claude"],
            ["codex", "Codex"]
          ].map(([id, label]) => ({ id, label, mark: host2.harnessSnapshot(id) })),
          missing: costMode && A.cost.unpriced_models.length ? "no price for " + A.cost.unpriced_models.join(", ") + "; unpriced usage is omitted from bars." : void 0
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
        ["API cost", "median_cost_usd", "cost_n", "cost", host2.asMoney],
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
          now.cost.usd == null ? "\u2014" : host2.asMoney(now.cost.usd),
          now.cost.usd == null || previous.cost.usd == null ? {
            text: "no price for " + [.../* @__PURE__ */ new Set([...now.cost.unpriced_models, ...previous.cost.unpriced_models])].join(
              ", "
            )
          } : note(now.cost.usd, previous.cost.usd, host2.asMoney),
          A.days === 1 ? "API-equivalent cost. Cost is recorded per UTC day: this is the whole current UTC day so far, compared with the whole day before." : "API-equivalent cost. Cost is recorded per UTC day: the last " + A.days + " UTC days count, today so far, compared with the " + A.days + " whole UTC days before.",
          host2.COST_TIP
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
          countText(now.errors) + " failed (" + pct(now.errors, now.tools) + ") \xB7 previous " + rangeName() + ": " + countText(previous.errors) + " failed (" + pct(previous.errors, previous.tools) + ")"
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
          "Median wait " + timeText(now.median_wait_ms) + " \xB7 previous " + rangeName() + ": " + timeText(previous.median_wait_ms)
        );
        const wait = A.longest_current_wait;
        metric(
          "Longest current wait",
          wait ? timeText(wait.ms) : "\u2014",
          note(wait ? wait.ms : 0, previous.longest_wait_ms, timeText),
          wait ? nameOfSid(A, wait.sid) + " has waited on you for " + timeText(wait.ms) : "No session is waiting on you"
        );
        charts.push(makeChart(false), makeChart(true));
        for (const [key, title, groups] of [
          ["repo", "By repo", A.breakdown.repo],
          ["machine", "By machine", A.breakdown.machine],
          ["harness", "By harness and model", A.breakdown.model]
        ]) {
          const keyFor = (g) => key === "repo" ? g.repo ?? "__none__" : key === "machine" ? g.machine ?? "" : g.harness + "\0" + g.model;
          const labelFor = (id) => key === "repo" ? id === "__none__" ? "No repo (roles)" : id : key === "machine" ? host2.MACHINE[id] ?? id : (host2.HARNESS[id.split("\0")[0]] ?? id.split("\0")[0]) + " \xB7 " + shortModel(id.split("\0")[1]);
          const selected = (g) => host2.analyticsMeasure === "cost" ? g.usd : g.ms, rows = [...groups].sort(
            (a, b) => selected(b) - selected(a) || labelFor(keyFor(a)).localeCompare(labelFor(keyFor(b)))
          ), max = Math.max(1, ...rows.map(selected));
          breakdowns.push({
            key,
            heading: title,
            rows: rows.map((g) => ({
              key: keyFor(g),
              name: labelFor(keyFor(g)),
              count: g.sessions + (g.sessions === 1 ? " session" : " sessions"),
              width: widthOf(selected(g), max),
              color: key === "harness" ? keyFor(g).startsWith("claude") ? "claude" : "codex" : void 0,
              hours: hoursText(g.ms),
              cost: g.unpriced_models.length ? "\u2014" : host2.asMoney(g.usd),
              missing: g.unpriced_models.length ? "no price for " + g.unpriced_models.join(", ") : void 0
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
            (x) => "usd" in x && x.usd != null ? host2.asMoney(x.usd) : "\u2014",
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
                harnessName: host2.HARNESS_SHORT[harness] ?? harness,
                harnessTip: host2.HARNESS[harness] && host2.HARNESS[harness] !== host2.HARNESS_SHORT[harness] ? host2.HARNESS[harness] : void 0,
                mark: host2.harnessSnapshot(harness),
                value: value(item2),
                href: host2.SESS[item2.sid] ? host2.urlOf({ v: "session", id: item2.sid }) : void 0,
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
            ), W = Math.max(280, chartWidth()), maxCost = Math.max(0.01, ...points.map((g) => g.median_cost_usd ?? 0));
            models.push({
              key: band,
              heading: band === "unknown" ? "Unknown difficulty" : band[0].toUpperCase() + band.slice(1),
              width: W,
              rows: rows.map((g) => {
                const key = band + "\0" + g.model;
                modelGroups.set(key, g);
                const tokens2 = g.tokens ?? {};
                return {
                  key,
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
                tip: g.model + ": " + host2.asMoney(g.median_cost_usd ?? 0) + ", " + (g.first_pass_acceptance * 100).toFixed(0) + "% accepted; acceptance n=" + g.acceptance_n + ", cost n=" + g.cost_n
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
          query: analyticsQuery(),
          metrics,
          charts,
          breakdowns,
          lists,
          models,
          measure: host2.analyticsMeasure,
          allowance,
          modelHeaders: [
            "Model",
            "Work items",
            ...measures.map((m) => m[0]),
            "Tokens \xB7 input / output / cache"
          ]
        },
        {
          committed: host2.observeTitle,
          facets: () => renderFacetFilters(page, () => {
            host2.render();
            refreshAnalytics(true);
          }),
          session: host2.goSession,
          measure(value) {
            if (host2.analyticsMeasure === value) return;
            const top = host2.currentScroll();
            host2.analyticsMeasure = value;
            host2.render();
            host2.restoreScroll(top);
          },
          breakdown(group, key) {
            if (group === "repo") host2.sessionFilters.repo = key;
            else if (group === "machine") host2.sessionFilters.machine = key;
            else {
              const [harness, model2] = key.split("\0");
              host2.sessionFilters.harness = harness ?? "";
              host2.sessionFilters.model = model2 ?? "";
            }
            host2.query = "";
            host2.groupBy = "recent";
            host2.go({ v: "sessions" });
          },
          slice(key) {
            const slice = slices.get(key);
            if (!slice || !A) return;
            const { bin, costMode } = slice;
            openAnalyticsSlice(A, bin.a, bin.b, bin.sessions, bin.more, costMode);
          },
          model(key) {
            const model2 = modelGroups.get(key);
            if (model2) openModelItems(model2);
          }
        }
      );
    }
    return {
      fetchAnalytics,
      scheduleAnalytics,
      refreshAnalytics,
      renderAnalytics,
      matchesSessionFacets,
      renderFacetFilters
    };
  }

  // src/app/applicationRefresh.ts
  function createApplicationRefresh(host2) {
    function refresh(dirty = null) {
      if (host2.disposed) return;
      if (host2.accountChrome.open && !host2.$(".account-popover")?.isConnected)
        host2.closeAccountMenu();
      if (host2.toolViewsOwner.viewerEl || host2.accountChrome.open) {
        host2.LIVE.pending = true;
        return;
      }
      if (host2.SIDEBAR_ONLY) {
        host2.LIVE.pending = false;
        host2.render();
        return;
      }
      host2.LIVE.pending = false;
      const r = host2.navigation.route;
      if (host2.navigation.rendered !== r || r.v === "session" && !host2.SESS[r.id] || r.v === "trace" && !host2.SESS[r.sid] || r.v === "machine" && !host2.MACHINE[r.id])
        return;
      const st = host2.capture();
      setGeometry(host2.$("#page"), "paddingBottom", null);
      if (r.v !== "session") {
        host2.render();
        host2.restore(st);
        return;
      }
      const n = host2.patchSession(dirty);
      host2.restore(st, st.bottom);
      if (!st.bottom && n) host2.LIVE.fresh += n;
      host2.syncJump();
    }
    return { refresh };
  }

  // src/app/bootstrap.ts
  function createBootstrap(host2) {
    const routeModel = {
      session(id) {
        return host2.SESS[id];
      },
      machine(id) {
        return !!host2.MACHINE[id];
      },
      turn(id) {
        return host2.TURN.get(id);
      },
      get machinesPath() {
        return host2.viewerHost?.machinesPath;
      }
    };
    const urlOf = (r) => routeUrl(r, routeModel), routeOf = (location2) => parseRoute(location2, routeModel);
    function boot() {
      host2.api("/api/model?delta=1").then(
        (m) => {
          const adopted = host2.adopt(m);
          host2.LIVE.version = adopted.version;
          host2.remember(adopted);
          if (host2.SIDEBAR_ONLY || host2.NATIVE_PAGE) {
            if (host2.NATIVE_PAGE)
              host2.navigation.route = host2.navigation.historyRoute(
                { v: host2.NATIVE_PAGE.nav },
                { v: "home" }
              );
            host2.render();
            host2.schedule(2e3);
            return;
          }
          host2.navigation.route = routeOf(location);
          if (host2.navigation.route.v === "sessions")
            host2.query = (new URLSearchParams(location.search).get("q") ?? "").trim();
          if (host2.navigation.route.v === "machines" && host2.NAV_MACHINES && !host2.viewerHost) {
            location.assign(host2.NAV_MACHINES);
            return;
          }
          try {
            history.replaceState(
              { ...host2.navigation.route, scrollTop: 0 },
              "",
              urlOf(host2.navigation.route) + (host2.navigation.route.v === "session" ? location.hash : host2.navigation.route.v === "sessions" && host2.query ? "?q=" + host2.enc(host2.query) : "")
            );
          } catch {
          }
          const done = () => {
            host2.render();
            if (host2.navigation.route.v === "session" && ("turn" in host2.navigation.route ? host2.navigation.route.turn : void 0)) {
              host2.revealTurn(
                ("turn" in host2.navigation.route ? host2.navigation.route.turn : void 0) ?? "",
                true
              );
              if (location.hash) host2.scope.frame(() => host2.scope.frame(host2.revealEntryHash));
            } else if (host2.navigation.route.v === "session" && location.hash) host2.revealEntryHash();
            else if (host2.navigation.route.v === "session") {
              host2.openSessionAtEnd();
              host2.syncJump();
            } else host2.quietTop();
            host2.schedule(2e3);
            host2.scope.interval(host2.ticker, 1e3);
          };
          const initialRoute = host2.navigation.route, p = host2.load(initialRoute);
          if (p)
            p.then(() => {
              done();
            }, done);
          else {
            done();
          }
        },
        (err) => {
          if (host2.disposed) return;
          if (host2.viewerHost?.modelFailed?.(err?.status ?? 0)) return;
          if (host2.viewerHost) {
            console.warn("semon: model unavailable", err.status);
            return;
          }
          renderPlaceholder(
            host2.$(host2.SIDEBAR_ONLY ? "#lanes" : "#page"),
            "Couldn't load the sessions: " + err.message
          );
        }
      );
    }
    return { routeModel, urlOf, routeOf, boot };
  }

  // src/app/destination.ts
  function createDestination(host2) {
    function openSessionAtEnd() {
      if (location.hash) return;
      host2.startOpeningEndPin();
    }
    const SKELETON_MS = 150;
    let skeletonTimer;
    let lanesFor = null;
    function paintPending(r) {
      const page = host2.$("#page"), hadFocus = host2.$("#sidebar").contains(document.activeElement);
      host2.renderNav();
      host2.renderLanes();
      host2.drawSessionBar();
      lanesFor = { r, version: host2.LIVE.version };
      if (hadFocus)
        host2.$("#lanes .srow[data-id='" + CSS.escape(r.id) + "']")?.focus({ preventScroll: true });
      page.setAttribute("aria-busy", "true");
      page.inert = true;
      page.classList.add("loading");
      host2.scope.clearTimeout(skeletonTimer);
      skeletonTimer = host2.scope.timeout(() => {
        if (host2.navigation.route !== r || !page.classList.contains("loading")) return;
        page.classList.remove("loading");
        page.classList.remove("child-page");
        setGeometry(page, "paddingBottom", null);
        renderPlaceholder(page);
        host2.quietTop();
        host2.syncBarLine();
      }, SKELETON_MS);
    }
    function endLoading() {
      host2.scope.clearTimeout(skeletonTimer);
      skeletonTimer = void 0;
      const page = host2.$("#page");
      page.removeAttribute("aria-busy");
      page.inert = false;
      page.classList.remove("loading");
    }
    function focusTitle() {
      const a = document.activeElement;
      if (a && a !== document.body && !host2.$("#sidebar").contains(a)) return;
      const h2 = host2.$("#page .ph h1");
      if (h2) {
        h2.tabIndex = -1;
        h2.focus({ preventScroll: true });
      }
    }
    function failLoad(r, err) {
      if (host2.navigation.route !== r || err instanceof Error && err.name === "AbortError") return;
      endLoading();
      const page = host2.$("#page");
      page.classList.remove("child-page");
      setGeometry(page, "paddingBottom", null);
      renderPlaceholder(
        page,
        "Couldn't load this session: " + (err instanceof Error ? err.message : "no response"),
        () => go({ ...r }, true)
      );
    }
    const isDeep = (r) => {
      const t = r.turn ? host2.TURN.get(r.turn) : null;
      return !!t && t.sid === r.id && !t.entries.length;
    };
    function go(r, fromHistory = false, prepared = false, nextContent = null) {
      host2.navigation.cancelNative();
      if (host2.NATIVE_PAGE) {
        if (!fromHistory)
          location.assign(r.v === "machines" ? host2.NAV_MACHINES ?? host2.urlOf(r) : host2.urlOf(r));
        return;
      }
      if (r.v === "machines" && host2.viewerHost && !prepared) {
        host2.closeDrawer(true);
        host2.closeAccountMenu(true, true);
        host2.navigation.loadNative(
          r,
          (content) => go(r, fromHistory, true, content),
          () => {
            if (host2.viewerHost) location.assign(host2.viewerHost.machinesPath);
          }
        );
        return;
      }
      if (r.v !== "sessions" || r !== host2.focusSessionsSearchOnRender)
        host2.focusSessionsSearchOnRender = null;
      if (host2.SIDEBAR_ONLY) {
        if (!fromHistory) {
          host2.closeDrawer(true);
          location.assign(
            r.v === "machines" && host2.NAV_MACHINES ? host2.NAV_MACHINES : host2.urlOf(r)
          );
        }
        return;
      }
      if (host2.navigation.route.v === "session")
        host2.clearPaging("id" in host2.navigation.route ? host2.navigation.route.id : "");
      host2.resetPagerInput();
      host2.stopOpeningEndPin();
      host2.navigation.cancelRoute();
      if (r.v === "machines" && host2.NAV_MACHINES && !host2.viewerHost) {
        location.assign(host2.NAV_MACHINES);
        return;
      }
      if (!fromHistory) host2.saveHistoryScroll();
      host2.navigation.replaceContent(r, nextContent);
      host2.closeAccountMenu(true, true);
      host2.dropErrors(true);
      if (host2.navigation.route.v === "session" && (r.v !== "session" || r.v !== "session" || r.id !== ("id" in host2.navigation.route ? host2.navigation.route.id : "")) && host2.TX["id" in host2.navigation.route ? host2.navigation.route.id : ""] && host2.TXM["id" in host2.navigation.route ? host2.navigation.route.id : ""]) {
        const sid = "id" in host2.navigation.route ? host2.navigation.route.id : "", entries = host2.TX[sid], meta = { ...host2.TXM[sid], origin: !!host2.originHandoff(sid) };
        host2.scope.frame(
          () => host2.scope.frame(() => host2.scope.timeout(() => host2.cacheTx(sid, entries, meta), 0))
        );
      }
      if (r.v !== "session" || r.v !== "session" || r.id !== ("id" in host2.navigation.route ? host2.navigation.route.id : ""))
        host2.show = { ...host2.SHOW_ALL };
      if (r.v !== "sessions") host2.ORD.delete("page");
      host2.navigation.route = r;
      host2.find = "";
      host2.findOpen = false;
      host2.closeDrawer(true);
      host2.clearNewEntries();
      if (!fromHistory) {
        const state2 = { ...r };
        delete state2.scrollTop;
        try {
          history.pushState(state2, "", host2.urlOf(r));
        } catch {
        }
      }
      const done = () => {
        if (host2.navigation.route !== r) return;
        endLoading();
        host2.render();
        if (r.v === "session" || host2.viewerHost && r.v === "machines") focusTitle();
        if (fromHistory && Number.isFinite(r.scrollTop)) {
          const turns = [...host2.$("#page").querySelectorAll(".turn")];
          for (const turn of turns) revealMeasuredTurn(turn, true);
          const heights = turns.map((turn) => turn.getBoundingClientRect().height);
          turns.forEach((turn, i) => {
            setGeometry(turn, "intrinsicHeight", Math.ceil(heights[i]));
            revealMeasuredTurn(turn, false);
          });
          host2.restoreScroll(r.scrollTop);
          host2.restoreHostFocus(r.hostFocus);
        } else if (r.v === "session" && r.turn) {
          revealTurn(r.turn, !fromHistory);
          if (location.hash) host2.scope.frame(() => host2.scope.frame(revealEntryHash));
        } else if (r.v === "session" && location.hash) revealEntryHash();
        else if (r.v === "session") openSessionAtEnd();
        else host2.quietTop();
        host2.syncJump();
      };
      if (r.v === "session" && host2.SESS[r.id]) {
        if (host2.STALE_BRIEFS.has(r.id)) {
          delete host2.TX[r.id];
          delete host2.TXM[r.id];
          host2.TXCACHE.delete(r.id);
        }
        const kept = !host2.TX[r.id];
        if (kept ? host2.adoptCached(r) : !isDeep(r)) {
          host2.TXCACHE.delete(r.id);
          paintPending(r);
          host2.scope.frame(
            () => host2.scope.timeout(() => {
              if (host2.navigation.route !== r) return;
              if (kept && !r.turn) host2.spread(r.id);
              done();
              host2.revalidate(r);
            }, 0)
          );
          return;
        }
      }
      if (r.v === "session") paintPending(r);
      host2.navigation.load(
        r,
        (signal) => host2.load(r, signal),
        done,
        (error) => failLoad(r, error)
      );
    }
    if (!host2.SIDEBAR_ONLY)
      host2.scope.listen(window, "popstate", (e) => {
        if (host2.skipPop) {
          host2.skipPop = false;
          if (host2.afterPop) {
            const leave = host2.afterPop;
            host2.afterPop = null;
            leave();
            return;
          }
          if (host2.pendingSessionOpen) {
            const id = host2.pendingSessionOpen;
            host2.pendingSessionOpen = null;
            goSession(id);
          }
          return;
        }
        if (host2.accountSheet) {
          host2.accountSheet = false;
          host2.closeAccountMenu(true);
          return;
        }
        if (host2.viewerEl) {
          const d = host2.viewerEl;
          host2.viewerEl = null;
          d.close();
          return;
        }
        if (e.state?.v) go(host2.navigation.historyRoute(e.state, host2.routeOf(location)), true);
      });
    const goSession = (id, turn = void 0) => go(turn ? { v: "session", id, turn } : { v: "session", id });
    const goTrace = (turn) => {
      const found = host2.TURN.get(turn);
      if (found) go({ v: "trace", sid: found.sid, turn });
    };
    const openSender = (h2) => {
      if (host2.SESS[h2.from]) goSession(h2.from, host2.HOLDS.get(h2.id)?.id);
    };
    function revealTurn(id, flash) {
      host2.resetPagerInput();
      const b = [...document.querySelectorAll(".turn")].find(
        (x) => x.dataset.turn === id
      );
      if (!b) {
        host2.quietTop();
        return;
      }
      const place = () => {
        if (!b.isConnected) return;
        const gap = host2.$("#topbar").offsetHeight + 8;
        host2.scrollProgrammatically(() => {
          if (host2.phone.matches)
            window.scrollTo(0, Math.max(0, window.scrollY + b.getBoundingClientRect().top - gap));
          else {
            const m = host2.$("#main");
            m.scrollTop += b.getBoundingClientRect().top - m.getBoundingClientRect().top - gap;
          }
        });
        host2.syncJump();
        host2.saveHistoryScroll();
      };
      place();
      host2.scope.frame(() => host2.scope.frame(place));
      if (flash) {
        b.classList.add("flash");
        host2.scope.timeout(() => b.classList.remove("flash"), 1500);
      }
    }
    function revealEntryHash() {
      host2.resetPagerInput();
      if (!location.hash) return;
      let key = "";
      try {
        key = decodeURIComponent(location.hash.slice(1));
      } catch {
        key = location.hash.slice(1);
      }
      const target = document.getElementById(key) ?? [...document.querySelectorAll("[data-e]")].find((n) => n.dataset.e === key) ?? [...document.querySelectorAll(".turn[data-turn]")].find(
        (n) => n.dataset.turn === key
      );
      if (!target) return;
      const place = () => {
        if (!target.isConnected) return;
        const gap = host2.$("#topbar").offsetHeight + 8;
        host2.scrollProgrammatically(() => {
          if (host2.phone.matches)
            window.scrollTo(
              0,
              Math.max(0, window.scrollY + target.getBoundingClientRect().top - gap)
            );
          else {
            const m = host2.$("#main");
            m.scrollTop += target.getBoundingClientRect().top - m.getBoundingClientRect().top - gap;
          }
        });
        host2.syncJump();
        host2.saveHistoryScroll();
      };
      place();
      host2.scope.frame(() => host2.scope.frame(place));
    }
    return {
      goSession,
      go,
      openSender,
      revealTurn,
      revealEntryHash,
      openSessionAtEnd,
      goTrace,
      get lanesFor() {
        return lanesFor;
      },
      set lanesFor(value) {
        lanesFor = value;
      }
    };
  }

  // src/app/documentEvents.ts
  function createDocumentEvents(host2) {
    const sidebar = host2.$("#sidebar");
    function openDrawer() {
      host2.shellChrome?.openDrawer();
    }
    function closeDrawer(quiet = void 0) {
      host2.shellChrome?.closeDrawer(quiet);
    }
    if (host2.SIDEBAR_ONLY)
      host2.scope.listen(window, "semon:drawer-open", () => host2.orderApply("side"));
    if (!host2.SIDEBAR_ONLY)
      host2.scope.listen(document, "keydown", (e) => {
        if (e.key === "Escape" && host2.accountSheet) host2.accountChrome.escape();
        else if (e.key === "Escape" && !host2.toolViewsOwner.viewerEl) {
          closeDrawer();
          host2.closeAccountMenu();
          host2.$(".session-menu")?.remove();
          host2.$("#more-btn")?.setAttribute("aria-expanded", "false");
        }
        if (e.key === "/" && !/INPUT|TEXTAREA/.test(document.activeElement?.tagName ?? "") && !(document.activeElement instanceof HTMLElement && document.activeElement.isContentEditable) && !host2.toolViewsOwner.viewerEl) {
          e.preventDefault();
          if (host2.navigation.route.v === "session") {
            host2.findOpen = true;
            host2.render();
            host2.$("#find")?.focus();
          } else if (host2.navigation.route.v === "sessions") {
            host2.$("#sq")?.focus();
          } else {
            const r = { v: "sessions", q: host2.query };
            host2.focusSessionsSearchOnRender = r;
            host2.go(r);
          }
        }
      });
    host2.scope.listen(host2.phone, "change", () => {
      closeDrawer(true);
      host2.syncLayoutPrefs();
      host2.recentNavigation.expandedAll = null;
      host2.renderLanes();
      if (!host2.phone.matches && host2.toolViewsOwner.viewerEl?.classList.contains("kids-sheet"))
        host2.toolViewsOwner.viewerEl.close();
      if (host2.navigation.route.v === "session" || host2.navigation.route.v === "analytics") {
        const top = host2.currentScroll();
        host2.render();
        host2.restoreScroll(top);
      } else host2.renderLanes();
      const l2 = host2.$("#topbar .meta-line");
      if (l2 && host2.navigation.route.v === "session") measureViewerBar(host2.$("#topbar"));
      host2.syncJump();
    });
    return { closeDrawer };
  }

  // src/app/documentRenderer.ts
  function createDocumentRenderer(host2) {
    const SLOTS = /* @__PURE__ */ new Map();
    function slot(key, box, build) {
      let s = SLOTS.get(key);
      if (!s || s.route !== host2.navigation.route || s.box !== box || !box.contains(s.el)) {
        s?.ctx.destroy?.();
        const ctx = { sync() {
        }, onChange() {
        } };
        s = { route: host2.navigation.route, box, ctx, el: build(ctx) };
        SLOTS.set(key, s);
      }
      return s;
    }
    function clearBox(box, r = null) {
      for (const [key, s] of SLOTS)
        if (s.route !== r) {
          s.ctx.destroy?.();
          SLOTS.delete(key);
        }
      if (ownsScreen(box)) {
        if (screenKind(box) === r?.v && !(r?.v === "machines" && host2.viewerHost) && !host2.NATIVE_PAGE)
          return;
        releaseScreen(box);
      }
      const kept = new Set([...SLOTS.values()].filter((s) => s.box === box).map((s) => s.el));
      for (const n of [...box.childNodes]) if (!(n instanceof HTMLElement && kept.has(n))) n.remove();
    }
    function placer(box) {
      let cur = box.firstChild;
      const put = (...nodes) => {
        for (const n of nodes) {
          if (n === cur) cur = cur?.nextSibling ?? null;
          else box.insertBefore(n, cur);
        }
      };
      put.done = () => {
        while (cur) {
          const next = cur.nextSibling;
          cur.remove();
          cur = next;
        }
      };
      return put;
    }
    function render2() {
      if (host2.disposed) return;
      const focusSearch = host2.focusSessionsSearchOnRender === host2.navigation.route;
      host2.focusSessionsSearchOnRender = null;
      if (host2.SIDEBAR_ONLY) {
        host2.domain.invalidate();
        host2.tick();
        host2.navigation.rendered = host2.navigation.route;
        host2.renderNav();
        host2.renderLanes();
        return;
      }
      if (host2.NATIVE_PAGE || host2.navigation.route.v === "machines" && host2.viewerHost) {
        host2.tick();
        host2.navigation.rendered = host2.navigation.route;
        if (host2.navigation.content && !host2.navigation.content.element.isConnected) {
          clearBox(host2.$("#page"), host2.navigation.route);
          host2.$("#page").append(host2.navigation.content.element);
        }
        document.title = (host2.NATIVE_PAGE?.title ?? "Machines") + " \xB7 Semon";
        host2.renderTopbar(host2.NATIVE_PAGE?.title ?? "Machines");
        host2.syncLayoutPrefs();
        host2.syncBarLine();
        host2.renderNav();
        host2.renderLanes();
        host2.renderDrawerAccount();
        host2.syncJump();
        return;
      }
      if (host2.viewerHost)
        document.title = ({
          home: "Home",
          sessions: "Sessions",
          analytics: "Analytics",
          machines: "Machines",
          machine: "Machine",
          session: "Session",
          trace: "Trace"
        }[host2.navigation.route.v] ?? "Semon") + " \xB7 Semon";
      host2.resetPagerInput();
      host2.holdProgrammaticScroll();
      host2.closeAccountMenu();
      host2.stopOpeningEndPin();
      host2.domain.invalidate();
      host2.orderingControlsOwner.ordPageState = host2.ordState("page");
      host2.tick();
      const page = host2.$("#page"), r = host2.navigation.route;
      host2.navigation.rendered = r;
      setGeometry(page, "paddingBottom", null);
      clearBox(page, r);
      page.classList.remove("child-page");
      if (r.v === "home") {
        host2.renderHome(page);
        host2.renderTopbar("Home");
      } else if (r.v === "analytics") {
        host2.renderAnalytics(page);
        host2.renderTopbar("Analytics", null, { analytics: true });
      } else if (r.v === "sessions") {
        host2.renderSessions(page, focusSearch);
        host2.renderTopbar("Sessions");
      } else if (r.v === "machines") {
        host2.renderMachines(page);
        host2.renderTopbar("Machines");
      } else if (r.v === "machine") {
        host2.renderMachine(page, r.id);
        host2.renderTopbar(
          host2.MACHINE[r.id],
          { label: "Machines", go: () => host2.go({ v: "machines" }) },
          { line2: host2.machineLine(r.id) }
        );
      } else if (r.v === "trace") {
        host2.renderTrace(page, r.turn);
        host2.renderTopbar(
          "Trace",
          { label: host2.SESS[r.sid].name, go: () => host2.goSession(r.sid, r.turn) },
          { traceSession: host2.SESS[r.sid] }
        );
      } else if (r.v === "session") {
        const s = host2.SESS[r.id], lineage = host2.lineageOf(r.id).slice(0, -1);
        host2.renderSession(page, r.id);
        host2.renderTopbar(s.name, null, { session: s, lineage, line2: host2.sessionLine(s) });
      }
      if (r.v === "session" && host2.errOn(r.id)) host2.markError(false);
      setGeometry(document.documentElement, "barHeight", host2.$("#topbar").offsetHeight);
      const lanesKept = host2.destination.lanesFor && host2.destination.lanesFor.r === r && host2.destination.lanesFor.version === host2.LIVE.version;
      host2.destination.lanesFor = null;
      host2.syncLayoutPrefs();
      host2.syncBarLine();
      host2.renderNav();
      if (!lanesKept) host2.renderLanes();
      host2.renderDrawerAccount();
      host2.syncJump();
      host2.orderingControlsOwner.ordPageState = null;
    }
    return { SLOTS, render: render2, slot };
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

  // src/app/historyScroll.ts
  function createHistoryScroll(host2) {
    const currentScroll = () => host2.phone.matches ? window.scrollY : host2.$("#main").scrollTop;
    const restoreScroll = (top) => host2.scrollProgrammatically(() => {
      if (host2.phone.matches) window.scrollTo(0, top);
      else host2.$("#main").scrollTop = top;
    });
    let hostFocus;
    if (host2.viewerHost)
      host2.scope.listen(document, "focusin", (event) => {
        const node = event.target;
        if (!(node instanceof HTMLElement) || !host2.$("#page").contains(node)) return;
        hostFocus = node.id ? { id: node.id } : node.dataset.id ? { row: node.dataset.id } : node.getAttribute("aria-label") ? { label: node.getAttribute("aria-label") ?? void 0 } : void 0;
      });
    function restoreHostFocus(saved) {
      if (!host2.viewerHost || !saved) return;
      const selector = saved.id ? "#" + CSS.escape(saved.id) : saved.row ? '[data-id="' + CSS.escape(saved.row) + '"]' : saved.label ? '[aria-label="' + CSS.escape(saved.label) + '"]' : null;
      if (selector)
        host2.$("#page")?.querySelector(selector)?.focus({ preventScroll: true });
    }
    const saveHistoryScroll = () => {
      if (host2.toolViewsOwner.viewerEl) return;
      try {
        if (history.state?.v)
          history.replaceState(
            {
              ...history.state,
              scrollTop: currentScroll(),
              ...host2.viewerHost ? { hostFocus } : {}
            },
            ""
          );
      } catch {
      }
    };
    let scrollSaveFrame = false;
    const queueScrollSave = () => {
      if (scrollSaveFrame) return;
      scrollSaveFrame = true;
      host2.scope.frame(() => {
        scrollSaveFrame = false;
        saveHistoryScroll();
      });
    };
    if (!host2.SIDEBAR_ONLY) {
      host2.scope.listen(window, "scroll", queueScrollSave, { passive: true });
      host2.scope.listen(host2.$("#main"), "scroll", queueScrollSave, { passive: true });
    }
    const quietTop = () => restoreScroll(0);
    return { saveHistoryScroll, quietTop, restoreScroll, restoreHostFocus, currentScroll };
  }

  // src/app/layout.ts
  function createLayout(host2) {
    let wideMode = false, railMode = false;
    let treePrefs = {};
    try {
      wideMode = localStorage.getItem("semon.wide") === "1";
    } catch {
    }
    try {
      railMode = !host2.SIDEBAR_ONLY && localStorage.getItem("semon.rail") === "1";
    } catch {
    }
    try {
      const saved = JSON.parse(localStorage.getItem("semon.tree") ?? "{}");
      if (saved && typeof saved === "object" && !Array.isArray(saved))
        treePrefs = pruneTreePrefs(saved);
    } catch {
    }
    const app = host2.$(".app");
    const syncLayoutPrefs = () => {
      if (host2.SIDEBAR_ONLY) return;
      app.classList.toggle("rail", railMode && !host2.phone.matches);
      host2.$("#page").classList.toggle(
        "wide-mode",
        wideMode && !host2.phone.matches && host2.navigation.route.v === "session"
      );
    };
    function setWideMode(on) {
      wideMode = on;
      try {
        localStorage.setItem("semon.wide", on ? "1" : "0");
      } catch {
      }
      syncLayoutPrefs();
      host2.$(".wide-toggle")?.setAttribute("aria-pressed", String(on));
      host2.accountChrome.updateWide(on);
    }
    function setRailMode(on) {
      railMode = on;
      host2.ORD.delete("side");
      try {
        localStorage.setItem("semon.rail", on ? "1" : "0");
      } catch {
      }
      syncLayoutPrefs();
      host2.recentNavigation.expandedAll = null;
      host2.renderLanes();
      const b = host2.$("#rail-toggle");
      b?.setAttribute("aria-expanded", String(!on));
      b?.setAttribute("aria-label", on ? "Expand sidebar" : "Collapse sidebar");
      b?.setAttribute("data-tip", on ? "Expand sidebar" : "Collapse sidebar");
    }
    function pruneTreePrefs(saved) {
      const kept = {};
      if (!saved || typeof saved !== "object") return kept;
      for (const [id, pref] of Object.entries(saved))
        if (pref && typeof pref === "object" && "open" in pref && typeof pref.open === "boolean")
          kept[id] = { open: pref.open, at: "at" in pref ? Number(pref.at) || 0 : 0 };
      return kept;
    }
    function saveTreePref(id, open) {
      treePrefs[id] = { open, at: Date.now() };
      treePrefs = Object.fromEntries(
        Object.entries(treePrefs).sort((a, b) => (b[1]?.at ?? 0) - (a[1]?.at ?? 0)).slice(0, 500)
      );
      try {
        localStorage.setItem("semon.tree", JSON.stringify(treePrefs));
      } catch {
      }
    }
    const railToggle = host2.SIDEBAR_ONLY ? host2.$("#rail-toggle") ?? document.createElement("button") : document.createElement("button");
    railToggle.setAttribute("aria-expanded", String(!railMode));
    railToggle.setAttribute("data-tip", railMode ? "Expand sidebar" : "Collapse sidebar");
    railToggle.setAttribute("aria-label", railMode ? "Expand sidebar" : "Collapse sidebar");
    host2.scope.listen(railToggle, "click", () => setRailMode(!railMode));
    syncLayoutPrefs();
    return {
      app,
      setRailMode,
      get railMode() {
        return railMode;
      },
      set railMode(value) {
        railMode = value;
      },
      get wideMode() {
        return wideMode;
      },
      set wideMode(value) {
        wideMode = value;
      },
      setWideMode,
      saveTreePref,
      get treePrefs() {
        return treePrefs;
      },
      set treePrefs(value) {
        treePrefs = value;
      },
      syncLayoutPrefs
    };
  }

  // src/app/liveModel.ts
  function createLiveModel(host2) {
    const liveController = createLiveController({
      async poll() {
        const response = await host2.api(
          "/api/model?delta=1&since=" + host2.enc(LIVE.late ? "" : LIVE.version ?? ""),
          void 0,
          true
        );
        if (!response) return;
        let model2;
        try {
          model2 = host2.applyModelDelta(response);
        } catch {
          await host2.api("/api/model?delta=1").then(host2.update);
          return;
        }
        await host2.update(model2);
      },
      failed(error) {
        return !!host2.viewerHost?.modelFailed?.(error instanceof ApiError ? error.status : 0);
      },
      ended
    });
    const LIVE = liveController.state;
    const remember = (_m) => {
      LIVE.turns = new Map(
        Object.values(host2.TURNS).flat().map((x) => [x.id, turnKey(x)])
      );
    };
    const turnKey = (x) => [
      x.start?.id,
      x.end?.st,
      x.end?.why,
      x.end?.h,
      x.sent.map((h2) => h2.id).join(","),
      x.last ? 1 : 0
    ].join("|");
    const handKey = (h2) => [
      h2.status,
      h2.to,
      h2.done,
      h2.result?.length,
      h2.kind === "toyou" ? h2.answer?.length : void 0,
      h2.kind === "toyou" ? h2.answers?.length : void 0,
      h2.declined ? 1 : 0
    ].join("|");
    const visible = () => document.visibilityState === "visible";
    const schedule = (ms) => liveController.schedule(ms);
    function ended(status2) {
      if (host2.disposed) return;
      liveController.stop();
      if (host2.$(".livenote, .livenote-side")) return;
      if (!window.dispatchEvent(
        new CustomEvent("semon:ended", { cancelable: true, detail: { status: status2 } })
      ))
        return;
      const n = createStatusNote(
        host2.SIDEBAR_ONLY ? "Sessions stopped updating: reload the page" : "Session ended: reload with the printed URL",
        host2.SIDEBAR_ONLY ? "ghead livenote-side" : "livenote"
      );
      if (host2.SIDEBAR_ONLY) host2.$("#lanes").after(n);
      else document.body.append(n);
    }
    const soft = (p) => p.catch((e) => {
      if (e instanceof ApiError && (e.status === 403 || e.status === 0)) throw e;
    });
    const viewed = () => {
      const v = /* @__PURE__ */ new Set();
      if (host2.navigation.route.v === "session")
        v.add("id" in host2.navigation.route ? host2.navigation.route.id : "");
      return v;
    };
    const cardKeys = () => new Map(
      host2.H.filter((h2) => h2.kind === "spawn" && host2.SESS[h2.to]).map((h2) => {
        const c = host2.SESS[h2.to];
        return [
          h2.id,
          [
            c.name,
            c.state,
            c.kind,
            c.model,
            host2.countOf(c, "calls"),
            c.activity?.join("|"),
            h2.status,
            h2.result
          ].join("")
        ];
      })
    );
    return {
      liveController,
      LIVE,
      remember,
      schedule,
      visible,
      ended,
      handKey,
      cardKeys,
      viewed,
      soft
    };
  }

  // src/app/liveUpdates.ts
  function createLiveUpdates(host2) {
    const applyModelDelta = (value) => host2.modelStore.apply(value);
    function update(value) {
      const m = applyModelDelta(value);
      const oldH = new Map(host2.H.map((h2) => [h2.id, host2.handKey(h2)])), oldT = host2.LIVE.turns, oldCards = host2.cardKeys(), names = new Map(Object.values(host2.SESS).map((x) => [x.id, x.name]));
      const hadOrigins = new Set([...host2.TXCACHE.keys()].filter((sid) => !!host2.originHandoff(sid)));
      const hadOrigin = host2.navigation.route.v === "session" && !!host2.SESS["id" in host2.navigation.route ? host2.navigation.route.id : ""] && !!host2.originHandoff("id" in host2.navigation.route ? host2.navigation.route.id : "");
      host2.adopt(m);
      host2.remember(m);
      for (const sid of host2.STALE_BRIEFS) if (!host2.SESS[sid]) host2.STALE_BRIEFS.delete(sid);
      for (const sid of [...host2.TXCACHE.keys()])
        if (!hadOrigins.has(sid) && host2.originHandoff(sid)) host2.TXCACHE.delete(sid);
      if (host2.LIVE.late !== ("id" in host2.navigation.route ? host2.navigation.route.id : "")) {
        if (host2.LIVE.late) host2.TXCACHE.delete(host2.LIVE.late);
        host2.LIVE.late = null;
      }
      if (host2.navigation.route.v === "session" && !hadOrigin && !!host2.SESS["id" in host2.navigation.route ? host2.navigation.route.id : ""] && !!host2.originHandoff("id" in host2.navigation.route ? host2.navigation.route.id : "")) {
        host2.LIVE.late = "id" in host2.navigation.route ? host2.navigation.route.id : "";
        host2.LIVE.lateTries = 0;
        host2.STALE_BRIEFS.add("id" in host2.navigation.route ? host2.navigation.route.id : "");
        host2.TXCACHE.delete("id" in host2.navigation.route ? host2.navigation.route.id : "");
      }
      if (host2.LIVE.late && !host2.TX[host2.LIVE.late]) host2.LIVE.late = null;
      const changedH = new Set(
        host2.H.filter((h2) => oldH.get(h2.id) !== host2.handKey(h2)).map((h2) => h2.id)
      );
      const newCards = host2.cardKeys(), changedCards = new Set(
        [...newCards].filter(([id, k]) => oldCards.has(id) && oldCards.get(id) !== k).map(([id]) => id)
      );
      const view = host2.viewed(), grown = /* @__PURE__ */ new Set(), cuts = /* @__PURE__ */ new Map(), patched = /* @__PURE__ */ new Map();
      let full = Object.values(host2.SESS).some((x) => names.has(x.id) && names.get(x.id) !== x.name);
      for (const sid of Object.keys(host2.TX)) {
        if (view.has(sid) && host2.SESS[sid]) host2.spread(sid);
        else host2.dropTx(sid);
      }
      let chain = Promise.resolve();
      for (const sid of view)
        if (host2.TX[sid] && host2.LIVE.late === sid)
          chain = chain.then(
            () => host2.soft(
              reloadLate(sid).then(() => {
                grown.add(sid);
                full = true;
              })
            )
          );
        else if (host2.TX[sid] && host2.TXM[sid].tok != null && host2.TOK[sid] != null && host2.shrank(host2.TXM[sid].tok, host2.TOK[sid]))
          chain = chain.then(
            () => host2.soft(
              reload(sid).then(() => {
                grown.add(sid);
                full = true;
              })
            )
          );
        else if (host2.TX[sid] && host2.TXM[sid].to >= host2.TXM[sid].total && host2.TXM[sid].tok !== host2.TOK[sid])
          chain = chain.then(
            () => host2.soft(
              tail(sid).then((r) => {
                grown.add(sid);
                if (r.cut != null) cuts.set(sid, r.cut);
                if (r.patched?.length) patched.set(sid, r.patched);
                if (r.reload) full = true;
              })
            )
          );
      for (const sid of view)
        if (host2.TX[sid] && host2.TXM[sid].to < host2.TXM[sid].total && host2.TXM[sid].watchTok !== host2.TOK[sid])
          chain = chain.then(
            () => host2.soft(
              watchLater(sid).then((r) => {
                if (r?.reload) full = true;
              })
            )
          );
      return chain.then(() => {
        host2.LIVE.version = m.version;
        host2.refresh(full ? null : dirtyTurns(cuts, grown, changedH, oldT, changedCards, patched));
        if (host2.navigation.route.v === "analytics") host2.refreshAnalytics();
        const e = host2.errorsLive();
        return e && host2.soft(e);
      });
    }
    function dirtyTurns(cuts, grown, changedH, oldT, changedCards, patched) {
      if (host2.navigation.route.v !== "session" || !host2.TX["id" in host2.navigation.route ? host2.navigation.route.id : ""])
        return null;
      const sid = "id" in host2.navigation.route ? host2.navigation.route.id : "", dirty = /* @__PURE__ */ new Set(), owner = new Map((host2.TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e, t.id])));
      if (cuts.has(sid))
        for (const e of host2.TX[sid].slice(cuts.get(sid))) {
          if (host2.isGap(e)) return null;
          if (owner.has(e)) dirty.add(owner.get(e));
        }
      for (const e of patched?.get(sid) ?? []) {
        if (!owner.has(e)) return null;
        dirty.add(owner.get(e));
      }
      for (const t of host2.TURNS[sid] ?? []) {
        if (oldT.get(t.id) !== host2.LIVE.turns.get(t.id) || [t.start, ...t.sent].some((h2) => h2 && changedH.has(h2.id)) || t.entries.some((e) => e.k === "h" && changedH.has(e.id)))
          dirty.add(t.id);
      }
      for (const h2 of host2.H)
        if (h2.kind === "spawn" && h2.from === sid && changedCards.has(h2.id) && host2.HOLDS.get(h2.id))
          dirty.add(host2.HOLDS.get(h2.id).id);
      return dirty;
    }
    function watchLater(sid) {
      const m = host2.TXM[sid], tok2 = host2.TOK[sid];
      return host2.api("/api/tx?sid=" + host2.enc(sid) + "&after=" + m.total).then((value) => {
        const p = parseTranscriptPage(value);
        if (host2.TXM[sid] !== m) return;
        if (p.total < m.total) return reload(sid);
        m.newer = (m.newer ?? 0) + Math.max(0, p.total - m.total);
        Object.assign(m, { total: p.total, calls: p.calls, errors: p.errors, watchTok: tok2 });
      });
    }
    function tail(sid) {
      const es = host2.TX[sid], m = host2.TXM[sid], tok2 = host2.TOK[sid];
      let cut2 = es.findIndex((e) => e.live && e.slot != null);
      if (cut2 < 0) cut2 = es.length;
      for (let i = es.length - 1; i >= 0; i--) {
        if (es[i].unfinished && es[i].slot != null) cut2 = Math.min(cut2, i);
        if (es[i].turn) break;
      }
      let got = [], last = null, n = 0;
      const page = (after) => host2.api("/api/tx?sid=" + host2.enc(sid) + "&after=" + after).then((value) => {
        const p = parseTranscriptPage(value);
        got = got.concat(p.entries.map((e) => host2.txEntry({ ...e, sid })));
        last = p;
        if (p.to < p.total && p.entries.length && ++n < 5) return page(p.to);
      });
      return page(cut2 < es.length ? es[cut2].slot : m.to).then(() => {
        if (host2.TX[sid] !== es) return { cut: null };
        if (!last) return { cut: null };
        if (last.total < m.total || last.to < last.total || cut2 < es.length && got[0]?.slot !== es[cut2].slot)
          return reload(sid);
        const ends = new Map(
          got.flatMap((e) => e.k === "bgend" && e.bg ? [[e.call, e.bg]] : [])
        ), still = new Set(last.bg_running ?? []), patched = [];
        for (const e of es.slice(0, cut2))
          if (e.bg) {
            const next = ends.get(e.tid ?? "") ?? (still.has(e.tid ?? "") ? { ...e.bg, state: "running", secs: e.bg.secs ?? "\u2014" } : e.bg.state === "running" ? { state: "unknown" } : e.bg);
            if (JSON.stringify(next) !== JSON.stringify(e.bg)) {
              e.bg = next;
              patched.push(e);
            }
          }
        host2.TX[sid] = es.slice(0, cut2).concat(got);
        Object.assign(m, {
          to: last.to,
          total: last.total,
          calls: last.calls,
          errors: last.errors,
          tok: tok2
        });
        host2.spread(sid);
        return { cut: cut2, patched };
      });
    }
    function reload(sid) {
      if (sid !== ("id" in host2.navigation.route ? host2.navigation.route.id : "")) {
        host2.dropTx(sid);
        return Promise.resolve({ cut: null, reload: true });
      }
      return host2.fetchTx(sid, "").then(() => ({ cut: null, reload: true }));
    }
    const LATE_TRIES = 4;
    function reloadLate(sid) {
      return reload(sid).then(
        (r) => {
          if (host2.LIVE.late === sid) host2.LIVE.late = null;
          return r;
        },
        (err) => {
          if (host2.LIVE.late === sid) {
            if (++host2.LIVE.lateTries >= LATE_TRIES) {
              host2.LIVE.late = null;
              console.warn(
                "semon: gave up reloading the transcript of " + sid + " after its origin arrived"
              );
            } else host2.LIVE.retry = true;
          }
          throw err;
        }
      );
    }
    return { reload, tail, applyModelDelta, update };
  }

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

  // src/app/navigationView.ts
  function createNavigationView(host2) {
    function renderNav() {
      const nav = host2.$("#nav"), destinations = [];
      const under = {
        home: ["home"],
        analytics: ["analytics"],
        sessions: ["sessions", "session", "trace"],
        machines: ["machines", "machine"]
      };
      const item2 = (v, label, ic, count, hot) => {
        destinations.push({
          key: v,
          label,
          icon: ic,
          href: v === "machines" && host2.NAV_MACHINES ? host2.NAV_MACHINES : host2.urlOf({ v }),
          current: under[v].includes(
            (host2.SIDEBAR_ONLY ? host2.app.dataset.viewerNav : host2.NATIVE_PAGE?.nav) ?? host2.navigation.route.v
          ),
          count,
          hot
        });
      };
      item2("home", "Home", I.home, host2.inbox().length, true);
      item2("sessions", "Sessions", I.sessions);
      item2("analytics", "Analytics", I.chart);
      item2(
        "machines",
        "Machines",
        I.machine,
        Object.keys(host2.MACHINE).filter((m) => !host2.MACHINE_UP[m]).length,
        true
      );
      if (host2.shellChrome) host2.shellChrome.update(destinations, host2.layoutOwner.railMode);
      else
        renderShellNavigation(nav, destinations, (destination) => {
          host2.go(host2.navigation.historyRoute({ v: destination.key }, { v: "home" }));
          return true;
        });
    }
    const sessMatch = (s, q) => !q || [
      s.name,
      s.repo,
      s.branch,
      host2.MACHINE[s.machine],
      s.movedFrom ? host2.MACHINE[s.movedFrom] : "",
      HARNESS[s.harness],
      s.role ? "role no repo" : "",
      ...(host2.TURNS[s.id] ?? []).map((t) => t.start?.brief ?? t.u?.text ?? "")
    ].join(" ").toLowerCase().includes(q.toLowerCase());
    return { renderNav, sessMatch };
  }

  // src/app/orderingControls.ts
  function createOrderingControls(host2) {
    const ordering = createOrdering(), ORD = ordering.scopes, ordTouch = { down: false }, byLast = (a, b) => b.last - a.last;
    const pageSig = () => JSON.stringify([host2.query, host2.groupBy, host2.sessionFilters]);
    host2.scope.listen(
      document,
      "pointerdown",
      () => {
        ordTouch.down = true;
      },
      true
    );
    for (const t of ["pointerup", "pointercancel"])
      host2.scope.listen(
        document,
        t,
        () => {
          ordTouch.down = false;
        },
        true
      );
    host2.scope.listen(window, "blur", () => {
      ordTouch.down = false;
    });
    host2.scope.listen(document, "visibilitychange", () => {
      ordTouch.down = false;
      if (host2.visible()) {
        orderApply("page");
        orderApply("side");
      }
    });
    function ordState(name) {
      const side = name === "side", rows = side ? "#lanes .treeitem" : "#page .nrow", a = document.activeElement;
      const inView = side ? sideRegion().scrollTop <= 1 : !host2.$(rows) || host2.$(rows).getBoundingClientRect().top >= host2.$("#topbar").getBoundingClientRect().bottom - 1;
      const touched = ordTouch.down || !!a?.matches?.(":focus-visible") && !!a.closest(side ? "#lanes" : rows) || matchMedia("(hover: hover)").matches && !!host2.$(rows + ":hover");
      return { inView, touched };
    }
    let ordPageState = null;
    const pageState = () => ordPageState ?? ordState("page");
    const orderScope = (name, sig, tie, state2) => ordering.begin(name, sig, tie, state2), orderList = orderRows;
    const sideRegion = () => host2.$("#side-list") ?? host2.$("#sidebar");
    const ordIdleMs = Reflect.get(window, "__semonOrderIdleMs");
    const ORD_IDLE_MS = Number.isFinite(ordIdleMs) && ordIdleMs >= 200 && ordIdleMs <= 6e4 ? ordIdleMs : 1e4;
    const ORD_DRAWER_MS = 320;
    function orderApply(name) {
      const sc = ORD.get(name);
      if (!sc?.n || name === "page" && (sc.tie !== host2.navigation.route || host2.navigation.rendered !== host2.navigation.route || host2.toolViewsOwner.viewerEl || host2.$("#page").hasAttribute("aria-busy")))
        return;
      ORD.delete(name);
      if (name === "side") host2.renderLanes();
      else {
        const st = host2.capture();
        host2.render();
        host2.restore(st);
      }
    }
    let ordIdle;
    function ordIdleArm() {
      host2.scope.clearTimeout(ordIdle);
      ordIdle = void 0;
      if (host2.phone.matches || !ORD.get("side")?.n) return;
      ordIdle = host2.scope.timeout(() => {
        ordIdle = void 0;
        const bar = host2.$("#sidebar"), a = document.activeElement;
        const menu = host2.$(
          ".session-menu, .account-popover, .runs-popover, .filters.pop:not([hidden])"
        );
        if (ordTouch.down || bar.matches(":hover") || a && bar.contains(a) && (a.matches(":focus-visible") || a.matches("input, textarea, select, [contenteditable]")) || menu || host2.toolViewsOwner.viewerEl)
          ordIdleArm();
        else orderApply("side");
      }, ORD_IDLE_MS);
    }
    for (const t of [
      "pointermove",
      "pointerdown",
      "pointerleave",
      "focusin",
      "focusout",
      "wheel",
      "keydown"
    ])
      host2.scope.listen(
        host2.$("#sidebar"),
        t,
        () => {
          if (ordIdle) ordIdleArm();
        },
        { passive: true }
      );
    return {
      ORD,
      orderApply,
      ORD_DRAWER_MS,
      byLast,
      orderList,
      ordState,
      orderScope,
      get ordIdle() {
        return ordIdle;
      },
      set ordIdle(value) {
        ordIdle = value;
      },
      ordIdleArm,
      pageSig,
      pageState,
      get ordPageState() {
        return ordPageState;
      },
      set ordPageState(value) {
        ordPageState = value;
      }
    };
  }

  // src/app/paging.ts
  function createPaging(host2) {
    const pagingStore = host2.transcripts.paging, PAGING = pagingStore.states;
    let pagerArmed = false, automaticLoads = 0, scrollRevision = 0, programmaticScrollPending = false, programmaticScrollTimer = void 0;
    const clearPaging = (sid) => host2.transcripts.clearPaging(sid);
    const dropTx = (sid) => host2.transcripts.drop(sid);
    function resetPagerInput() {
      pagerArmed = false;
      automaticLoads = 0;
      scrollRevision++;
      disconnectPagerObservers();
    }
    function holdProgrammaticScroll() {
      programmaticScrollPending = true;
      host2.scope.clearTimeout(programmaticScrollTimer);
      programmaticScrollTimer = host2.scope.timeout(() => {
        programmaticScrollPending = false;
        programmaticScrollTimer = void 0;
      }, 120);
    }
    function scrollProgrammatically(fn, jump = true) {
      if (jump) resetPagerInput();
      holdProgrammaticScroll();
      fn();
    }
    function readerScrollInput() {
      if (host2.navigation.route.v !== "session" || host2.navigation.rendered !== host2.navigation.route || host2.$("#page").hasAttribute("aria-busy"))
        return;
      host2.scope.clearTimeout(programmaticScrollTimer);
      programmaticScrollTimer = void 0;
      programmaticScrollPending = false;
      host2.stopOpeningEndPin();
      pagerArmed = true;
      automaticLoads = 0;
      scrollRevision++;
      queuePagerObservers();
    }
    const automaticPagingAllowed = () => pagerArmed && automaticLoads < 3 && !host2.viewport.openingEndUntil && !host2.findOpen && !host2.find && host2.show.messages && host2.show.tools && host2.show.thinking;
    const pagingState = (sid, where) => pagingStore.get(sid, where);
    function pagerSnapshot(sid, where) {
      const state2 = pagingState(sid, where), direction = where === "before" ? "earlier" : "later";
      return {
        sid,
        where,
        disabled: state2.busy,
        busy: state2.busy,
        text: state2.busy ? "Loading " + direction + "\u2026" : state2.failed ? "Couldn't load " + direction + " entries \xB7 Retry" : "Load " + direction + (where === "after" && host2.TXM[sid]?.newer ? " \xB7 " + host2.TXM[sid].newer + " new" : "")
      };
    }
    function paintPager(b) {
      updateSessionPager(
        host2.$("#page"),
        pagerSnapshot(b.dataset.pagerSid, b.dataset.pagerWhere === "before" ? "before" : "after")
      );
    }
    const pagerController = createPagerController({
      route(sid) {
        return host2.navigation.route.v === "session" && ("id" in host2.navigation.route ? host2.navigation.route.id : "") === sid && host2.navigation.rendered === host2.navigation.route ? host2.navigation.route : null;
      },
      range(sid) {
        return host2.TXM[sid];
      },
      state: pagingState,
      automatic: automaticPagingAllowed,
      current(sid, where, state2, r) {
        return PAGING.get(sid)?.[where] === state2;
      },
      beginManual: () => host2.stopOpeningEndPin(),
      countAutomatic() {
        automaticLoads++;
      },
      paint: paintPager,
      load(sid, where, boundary, signal, applied) {
        return host2.fetchTx(sid, where + "=" + boundary, where, signal, applied);
      },
      commit(r, where, manual) {
        const box = host2.scroller(), top = host2.phone.matches ? 0 : box.getBoundingClientRect().top, st = host2.capture();
        const entry = [
          ...host2.$("#page").querySelectorAll(".turns [data-e][data-entry-key]:not(.tgroup)")
        ].find((n) => {
          const rect = n.getBoundingClientRect();
          return rect.height && rect.top >= top;
        });
        st.paging = {
          anchor: entry ? { key: entry.dataset.entryKey, off: entry.getBoundingClientRect().top - top } : null,
          height: box.scrollHeight,
          before: where === "before"
        };
        const armed = pagerArmed, used = automaticLoads;
        host2.render();
        host2.restore(st);
        if (!manual) {
          pagerArmed = armed;
          automaticLoads = used;
        }
        host2.syncJump();
        host2.saveHistoryScroll();
      },
      queue: queuePagerObservers
    });
    function disconnectPagerObservers() {
      pagerController.disconnect();
    }
    function queuePagerObservers() {
      if (host2.disposed || host2.SIDEBAR_ONLY) return;
      pagerController.queue(
        host2.$("#page"),
        host2.phone.matches ? null : host2.$("#main"),
        () => host2.navigation.route.v === "session" && host2.navigation.rendered === host2.navigation.route && automaticPagingAllowed() && !host2.$("#page").hasAttribute("aria-busy")
      );
    }
    function loadPager(button, manual) {
      return pagerController.load(button, manual);
    }
    return {
      pagerController,
      resetPagerInput,
      scrollProgrammatically,
      clearPaging,
      dropTx,
      loadPager,
      pagerSnapshot,
      holdProgrammaticScroll,
      queuePagerObservers,
      disconnectPagerObservers,
      get scrollRevision() {
        return scrollRevision;
      },
      set scrollRevision(value) {
        scrollRevision = value;
      },
      paintPager,
      readerScrollInput,
      get programmaticScrollPending() {
        return programmaticScrollPending;
      },
      set programmaticScrollPending(value) {
        programmaticScrollPending = value;
      }
    };
  }

  // src/app/recentNavigation.ts
  function createRecentNavigation(host2) {
    const isApprovalReview = (s) => s.kind === "Approval review";
    const visibleInNavigation = (s, path = routedPath()) => host2.showApprovalReviews || !isApprovalReview(s) || s.id === path.current || path.ancestors.has(s.id);
    const navigationTree = (path = routedPath()) => {
      const canonical = host2.sessionChildren(), roots = [], children = /* @__PURE__ */ new Map(), visited = /* @__PURE__ */ new Set();
      const visit = (session, parent) => {
        if (visited.has(session.id)) return;
        visited.add(session.id);
        let nearest = parent;
        if (visibleInNavigation(session, path)) {
          if (parent) {
            if (!children.has(parent)) children.set(parent, []);
            children.get(parent).push(session);
          } else roots.push(session);
          nearest = session.id;
        }
        for (const child of canonical.get(session.id) ?? []) visit(child, nearest);
      };
      for (const root of Object.values(host2.SESS).filter((s) => s.lane && !host2.parentOf(s.id)))
        visit(root, null);
      roots.sort(host2.byLast);
      for (const kids of children.values()) kids.sort(host2.byLast);
      return { roots, children };
    };
    const COST_TIP = "What these tokens would cost at API rates. Subscriptions (Claude Max, ChatGPT plans) aren't billed this way.";
    const TREE_ACTIVE = 8, TREE_ROWS = 3;
    function routedPath() {
      const current = host2.navigation.route.v === "session" ? "id" in host2.navigation.route ? host2.navigation.route.id : "" : host2.navigation.route.v === "trace" ? host2.navigation.route.sid : null, ancestors = /* @__PURE__ */ new Set();
      for (let id = current && host2.SESS[current] ? host2.parentOf(current) : null; id && host2.SESS[id] && id !== current && !ancestors.has(id); id = host2.parentOf(id))
        ancestors.add(id);
      return { current, ancestors };
    }
    let forcedOpen = {
      route: null,
      ids: /* @__PURE__ */ new Set()
    };
    function forcedOpenIds() {
      if (forcedOpen.route !== host2.navigation.route)
        forcedOpen = { route: host2.navigation.route, ids: routedPath().ancestors };
      return forcedOpen.ids;
    }
    let expandedAll = null, revealedFor = null;
    let expandedPath = /* @__PURE__ */ new Set(), expandedUnder = /* @__PURE__ */ new Set();
    let sideOrder = null;
    const ancestorsOf = (id) => {
      const out = /* @__PURE__ */ new Set();
      for (let p = id && host2.SESS[id] ? host2.parentOf(id) : null; p && host2.SESS[p] && p !== id && !out.has(p); p = host2.parentOf(p))
        out.add(p);
      return out;
    };
    function treeGroupSnapshot(parent, kids, children, depth, rail, open) {
      const { current, ancestors } = routedPath(), rank = new Map(kids.map((c) => [c.id, host2.kidRank(c, children)]));
      const byRank = (a, b) => rank.get(a.id) - rank.get(b.id) || b.last - a.last, sorted = [...kids].sort(byRank), keep = /* @__PURE__ */ new Set();
      for (const c of sorted) if (rank.get(c.id) < 2 && keep.size < TREE_ACTIVE) keep.add(c.id);
      for (const c of sorted)
        if (c.id === current || ancestors.has(c.id) || expandedPath.has(c.id)) keep.add(c.id);
      for (const c of sorted) if (keep.size < TREE_ROWS) keep.add(c.id);
      const listed = sorted.filter((c) => keep.has(c.id)), hidden = kids.length - listed.length;
      if (!hidden && expandedAll === parent.id) expandedAll = null;
      const full = hidden > 0 && !rail && (expandedAll === parent.id || expandedUnder.has(parent.id));
      const key = (full ? "a:" : "k:") + parent.id, unseen = !!sideOrder?.keep && open && !full && !sideOrder.reseed && sideOrder.seen.has(parent.id) && !sideOrder.prev.has(key);
      const shown = sideOrder ? host2.orderList(sideOrder, key, full ? sorted : listed, byRank, {
        must: /* @__PURE__ */ new Set([...current ? [current] : [], ...ancestors, ...expandedPath]),
        quiet: !open,
        seed: full || sideOrder.reseed || !sideOrder.seen.has(parent.id)
      }) : full ? sorted : listed;
      const items = shown.map((child) => buildLaneSnapshot(child, depth + 1, children, rail));
      return {
        items,
        all: kids.length === shown.length || full || unseen ? void 0 : host2.descendantsOf(parent.id, children).length,
        full: full && expandedAll === parent.id
      };
    }
    let recentSnapshot = { items: [], empty: false };
    const recentRenderer = createRecentRenderer(host2.$("#lanes"), {
      open: (id) => host2.goSession(id),
      toggle(id, value) {
        if (!value) forcedOpenIds().delete(id);
        host2.saveTreePref(id, value);
        if (!value && (expandedAll === id || expandedPath.has(id))) {
          expandedAll = null;
          renderLanes();
          host2.$('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] > .tree-row .tree-toggle')?.focus();
        } else {
          const change = (items) => items.map((item2) => ({
            ...item2,
            open: item2.id === id ? value : item2.open,
            children: item2.children ? change(item2.children) : void 0
          }));
          recentSnapshot = { ...recentSnapshot, items: change(recentSnapshot.items) };
          recentRenderer.update(recentSnapshot);
        }
      },
      all(id, trigger) {
        if (!host2.SESS[id]) return;
        if (host2.phone.matches) {
          openKidsSheet(host2.SESS[id], trigger);
          return;
        }
        expandedAll = id;
        renderLanes();
        host2.$('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] > .tree-row .tree-fewer')?.focus();
      },
      fewer(id) {
        expandedAll = null;
        renderLanes();
        const row = host2.$('#lanes .srow[data-id="' + CSS.escape(id) + '"]');
        host2.scrollProgrammatically(() => row?.scrollIntoView({ block: "nearest" }));
        row?.focus({ preventScroll: true });
      }
    });
    function openKidsSheet(parent, trigger) {
      const all = host2.descendantsOf(parent.id, navigationTree().children), bucket = (s) => s.state === "wait" ? 0 : s.state === "work" ? 1 : 2;
      let picked = null;
      const rows = [...all].sort((a, b) => b.last - a.last).map((s) => {
        const above = host2.parentOf(s.id);
        return {
          id: s.id,
          name: s.name,
          state: s.state,
          stateLabel: host2.STATE[s.state] ?? s.state,
          age: host2.ago(s.last),
          bucket: bucket(s),
          under: above && above !== parent.id && host2.SESS[above] ? host2.SESS[above].name : void 0
        };
      });
      const sheet2 = createKidsSheet(parent.name, rows, {
        matches(id, query) {
          return host2.sessMatch(host2.SESS[id], query);
        },
        select(id) {
          picked = id;
          host2.pendingSessionOpen = id;
          sheet2.dialog.close();
        },
        opened(d) {
          host2.viewerEl = d;
          document.documentElement.classList.add("viewer-open");
          if (!host2.SIDEBAR_ONLY)
            try {
              history.pushState(
                { ...host2.navigation.route, sheet: 1, scrollTop: host2.currentScroll() },
                ""
              );
            } catch {
            }
        },
        closed(d) {
          host2.dialogs.delete(d);
          if (host2.disposed) return;
          document.documentElement.classList.remove("viewer-open");
          if (host2.viewerEl === d) {
            host2.viewerEl = null;
            if (!host2.SIDEBAR_ONLY && history.state?.sheet) {
              host2.skipPop = true;
              history.back();
            } else if (host2.pendingSessionOpen) {
              const id = host2.pendingSessionOpen;
              host2.pendingSessionOpen = null;
              host2.goSession(id);
            }
          }
          if (!picked)
            (host2.$('#lanes .tree-all[data-id="' + CSS.escape(parent.id) + '"]') ?? trigger).focus();
          if (host2.LIVE.pending) host2.refresh();
        }
      });
      host2.dialogs.set(sheet2.dialog, sheet2);
      sheet2.show();
    }
    function buildLaneSnapshot(s, depth, children, rail) {
      const kids = children.get(s.id) ?? [], allKids = host2.descendantsOf(s.id, children);
      const { current, ancestors } = routedPath(), saved = host2.treePrefs[s.id];
      const open = forcedOpenIds().has(s.id) || expandedPath.has(s.id) || (typeof saved?.open === "boolean" ? saved.open : host2.defaultTreeOpen(s.id, children) || expandedUnder.has(s.id));
      const group = kids.length && !rail ? treeGroupSnapshot(s, kids, children, depth, rail, open) : null;
      const parts = allKids.length ? host2.childParts(allKids) : [], harness = Object.hasOwn(host2.HARNESSES, s.harness) ? host2.HARNESSES[s.harness] : null;
      const flag = kids.length && !rail ? allKids.some((x) => x.state === "wait") ? "wait" : allKids.some((x) => x.state === "err") ? "err" : null : null;
      const fields = [
        {
          className: "row-duration",
          text: host2.dur(s.start, s.state === "work" || s.state === "wait" ? null : s.last),
          priority: 1,
          tip: "Duration",
          icon: host2.I.duration
        }
      ];
      if (Object.keys(host2.MACHINE).length > 1)
        fields.push({
          className: "row-machine host",
          text: host2.shortHost(s),
          priority: 2,
          tip: "Machine: " + host2.hostOf(s),
          icon: host2.I.machine
        });
      if (s.repo)
        fields.push({
          className: "repo-short",
          text: s.repo,
          priority: 3,
          tip: "Repo: " + s.repo,
          icon: host2.I.repo
        });
      return {
        id: s.id,
        name: s.name,
        label: [
          s.name,
          host2.STATE[s.state] ?? s.state,
          host2.HARNESS[s.harness] ?? s.harness,
          host2.shortHost(s),
          ...parts
        ].join(", "),
        state: s.state,
        stateLabel: host2.STATE[s.state] ?? s.state,
        age: host2.ago(s.last),
        model: shortModel(s.model ?? s.modelId),
        modelTip: "Model: " + host2.modelIdOf(s),
        harness: harness ? {
          id: s.harness,
          name: harness.name,
          light: harness.icon.light,
          dark: harness.icon.dark,
          darkTheme: host2.darkTheme()
        } : void 0,
        fields,
        current: current === s.id ? "page" : rail && ancestors.has(s.id) ? "true" : void 0,
        rail,
        childState: rail && allKids.some((x) => x.state === "work" || x.state === "wait") ? host2.urgentDescendant(s.id, children) ?? "work" : void 0,
        flag: flag ? {
          state: flag,
          tip: allKids.filter((x) => x.state === "wait").length + " needs you \xB7 " + allKids.filter((x) => x.state === "err").length + " failed"
        } : void 0,
        open,
        depth,
        children: group?.items,
        all: group?.all,
        stuck: !!group?.full && open
      };
    }
    function laneFocus() {
      const a = document.activeElement instanceof HTMLElement ? document.activeElement : null, item2 = a?.closest(".treeitem");
      if (!a || !host2.$("#lanes").contains(a)) return null;
      const kind = ["srow", "tree-all", "tree-fewer", "tree-toggle"].find((c) => a.classList.contains(c)) ?? (a === item2 ? "treeitem" : null);
      const id = kind === "tree-toggle" ? a.dataset.treeToggle : kind === "srow" || kind === "tree-all" ? a.dataset.id : item2?.dataset.id;
      return kind && id ? { kind, id, visible: a.matches(":focus-visible") } : null;
    }
    function restoreLaneFocus(f) {
      if (!f || document.activeElement !== document.body) return;
      const q = (sel) => host2.$("#lanes " + sel), id = CSS.escape(f.id);
      const target = f.kind === "srow" ? q('.srow[data-id="' + id + '"]') : f.kind === "tree-all" ? q('.tree-all[data-id="' + id + '"]') : f.kind === "tree-fewer" ? q('.treeitem[data-id="' + id + '"] > .tree-row .tree-fewer') : f.kind === "tree-toggle" ? q('.tree-toggle[data-tree-toggle="' + id + '"]') : q('.treeitem[data-id="' + id + '"]');
      (target ?? q('.srow[data-id="' + id + '"]'))?.focus({
        preventScroll: true,
        focusVisible: f.visible
      });
    }
    function renderLanes() {
      const rail = host2.railMode && !host2.phone.matches, prevSide = host2.ORD.get("side");
      if (rail || prevSide?.rail !== rail) host2.ORD.delete("side");
      const sideState = host2.ordState("side"), focus = laneFocus(), path = routedPath(), { current, ancestors } = path, { roots: lanes, children: everyone } = navigationTree(path);
      sideOrder = Object.assign(host2.orderScope("side", "", null, sideState), {
        seen: /* @__PURE__ */ new Set()
      });
      sideOrder.rail = rail;
      const box = host2.$("#lanes"), drawn = [...box.querySelectorAll(".treeitem")];
      sideOrder.seen = new Set(drawn.map((r) => r.dataset.id));
      const seeing = new Set(
        drawn.filter((r) => r.querySelector(":scope > .tree-row .srow")?.getClientRects().length).map((r) => r.dataset.id)
      );
      let children = everyone;
      if (sideOrder.keep) {
        const held = [...everyone.keys()].filter(
          (p) => !sideOrder.prevKids.has(p) && seeing.has(p) && p !== current && !ancestors.has(p)
        );
        if (held.length) {
          children = new Map([...everyone].filter(([p]) => !held.includes(p)));
          sideOrder.n += held.reduce((n, p) => n + everyone.get(p).length, 0);
        }
      }
      sideOrder.kids = new Set(children.keys());
      if (expandedAll && (host2.phone.matches || host2.railMode || !host2.SESS[expandedAll]))
        expandedAll = null;
      sideOrder.exp = JSON.stringify([expandedAll, host2.railMode && !host2.phone.matches]);
      sideOrder.reseed = sideOrder.keep && sideOrder.prevExp !== sideOrder.exp;
      expandedPath = expandedAll ? ancestorsOf(expandedAll) : /* @__PURE__ */ new Set();
      expandedUnder = expandedAll ? new Set(host2.descendantsOf(expandedAll, children).map((x) => x.id)) : /* @__PURE__ */ new Set();
      recentSnapshot = {
        items: host2.orderList(sideOrder, "lanes", lanes, host2.byLast, {
          limit: 8,
          must: /* @__PURE__ */ new Set([...current ? [current] : [], ...ancestors])
        }).slice(0, 8).map((s) => buildLaneSnapshot(s, 0, children, rail)),
        empty: !lanes.length
      };
      recentRenderer.update(recentSnapshot);
      if (!sideOrder.n) {
        host2.scope.clearTimeout(host2.ordIdle);
        host2.ordIdle = void 0;
      } else if (!host2.ordIdle) host2.ordIdleArm();
      const q = host2.$("#q");
      if (q && document.activeElement !== q) q.value = host2.query;
      restoreLaneFocus(focus);
      const stuck = box.querySelector(".tree-row.stuck"), navigated = revealedFor !== host2.navigation.route;
      revealedFor = host2.navigation.route;
      setGeometry(
        host2.$("#side-list") ?? host2.$("#sidebar"),
        "scrollPaddingTop",
        stuck ? stuck.offsetHeight + 8 : null
      );
      if (stuck && navigated)
        host2.scrollProgrammatically(
          () => box.querySelector('.srow[aria-current="page"]')?.scrollIntoView({ block: "nearest" })
        );
    }
    return {
      recentRenderer,
      get expandedAll() {
        return expandedAll;
      },
      set expandedAll(value) {
        expandedAll = value;
      },
      renderLanes,
      COST_TIP,
      isApprovalReview
    };
  }

  // src/domain/trace.ts
  var finite = (value) => typeof value === "number" && Number.isFinite(value);
  function createTraceCalculations(state2, domain, now, RUN_EXPANDED, HARNESS2, STATE2) {
    const { sessions: SESS, turns: TURNS, starts: STARTS } = state2;
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
      const upper = root.end?.at ?? nextTurn?.at ?? (session.state === "work" ? now() : session.last) ?? start;
      const end = Math.max(
        start,
        upper,
        ...model2.handoffs.map((h2) => h2.done).filter(finite),
        ...allRunNodes(model2).filter((n) => n.kind !== "root").map(
          (n) => SESS[n.sid]?.state === "work" ? now() : n.handoff?.done ?? SESS[n.sid]?.last
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
        const handoff = node.handoff, from = node.kind === "root" ? start : handoff.at, to = node.kind === "root" ? end : s.state === "work" ? now() : handoff.done ?? s.last ?? from;
        const duration = dur2(from, to), ownCost = costForSession(node.sid), rollup = costForSessions(
          allNodes(node).map((x) => SESS[x.sid]).filter(Boolean)
        ), status2 = STATE2[s.state] ?? s.state;
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
          label: [s.name, s.kind ?? HARNESS2[s.harness], status2, duration, costText(rollup)].join(
            ", "
          ),
          name: s.name,
          relay: node.kind === "relay",
          critical: !!handoff && critical.has(handoff.id),
          harnessClass: hcls(node.sid),
          model: [kindText(s), shortModel(s.model)].filter(Boolean).join(" \xB7 "),
          state: s.state,
          stateLabel: status2,
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
            done: pct(handoff.done ?? (s.state === "work" ? now() : s.last)),
            returned: Number.isFinite(handoff.done)
          } : void 0
        });
        const folded = foldRows && node.children.length > 8 && !expanded.has(node.sid);
        for (const child of folded ? node.children.slice(0, 8) : node.children)
          add(child, Math.min(3, depth + 1), node.sid);
        if (folded) {
          const hidden = node.children.slice(8), ids = new Set(hidden.flatMap((n) => allNodes(n).map((x) => x.sid))), cost2 = costForSessions([...ids].map((id) => SESS[id]).filter(Boolean));
          rows.push({
            kind: "more",
            id: "more:" + node.sid,
            fold: node.sid,
            depth: Math.min(3, depth + 1),
            name: "+" + hidden.length + " more",
            label: "Show " + hidden.length + " more agents under " + s.name,
            cost: cost2.usd == null ? "\u2014" : shortMoney(cost2.usd),
            costTip: "Hidden agents' rollup cost: " + costText(cost2)
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

  // src/app/screenViews.ts
  function createScreenViews(host2) {
    const MENU_KINDS = [
      ["Input", ["input"]],
      ["Output", ["output"]],
      ["Cache write", ["cache_write_5m", "cache_write_1h"]],
      ["Cache read", ["cache_read"]],
      ["Web search", ["web_search"]]
    ];
    const COST_NOTE = "What these tokens would cost at API rates. Subscriptions aren't billed this way.";
    function costSnapshot(s, kids) {
      const own = host2.costForSession(s.id), all = host2.costForSession(s.id, true), missing = host2.costMissing(all), details = [];
      if (kids.length)
        details.push(
          { label: "This session", value: host2.costText(own) },
          {
            label: kids.length === 1 ? "Its run" : "Its " + kids.length + " runs",
            value: host2.costText(host2.costForSessions(kids))
          }
        );
      const reports = s.reported_runs ?? [], reported = reports.filter((r) => Number.isFinite(r.cost_usd));
      if (reports.length)
        details.push({
          label: host2.HARNESS[s.harness] + "'s own figure",
          value: reported.length ? host2.asMoney(reported.reduce((n, r) => n + (r.cost_usd ?? 0), 0)) + (reported.length === 1 ? ", last run" : ", last " + reported.length + " runs") : "not reported"
        });
      const check = [...s.cost_check ?? []].reverse().find(
        (c) => c.ok === false && Number.isFinite(c.computed_usd) && Number.isFinite(c.reported_usd)
      );
      const mismatch = check ? "Semon's estimate for that run is " + (check.reported_usd === 0 ? 100 : Math.round(
        Math.abs((check.computed_usd ?? 0) - (check.reported_usd ?? 0)) / Math.abs(check.reported_usd ?? 0) * 100
      )) + "% " + ((check.computed_usd ?? 0) > (check.reported_usd ?? 0) ? "above" : "below") + " " + host2.HARNESS[s.harness] + "'s figure: API rates differ from what a plan is charged." : void 0;
      const children = host2.sessionChildren(), runs = [], walk = (id, depth) => {
        for (const c of [...children.get(id) ?? []].sort((a, b) => b.last - a.last)) {
          const cost2 = host2.costText(host2.costForSession(c.id));
          runs.push({
            id: c.id,
            depth,
            label: "Open " + c.name + ", " + host2.kindText(c) + ", " + host2.STATE[c.state] + ", " + cost2,
            name: c.name,
            kind: host2.kindText(c),
            state: c.state,
            stateLabel: host2.STATE[c.state],
            cost: cost2
          });
          walk(c.id, depth + 1);
        }
      };
      if (kids.length) walk(s.id, 0);
      const models = Object.entries(all.by_model ?? {}).map(([id, model2]) => {
        const priced2 = model2.usd != null && !missing.includes(id), rows = [];
        for (const [label, keys] of MENU_KINDS) {
          const tokens2 = keys.reduce((n, k) => n + (Number(model2.tokens?.[k]) || 0), 0), usd = keys.reduce((n, k) => n + (Number(model2.usd_by_kind?.[k]) || 0), 0);
          if (tokens2 === 0 && (!priced2 || usd < 5e-3)) continue;
          rows.push({
            label,
            count: tokens2 ? compactCount(tokens2) : "",
            exact: tokens2 ? tokens2.toLocaleString() : void 0,
            cost: priced2 ? host2.asMoney(usd) : "\u2014"
          });
        }
        return { id, rows };
      });
      return {
        figure: host2.costText(kids.length ? all : own),
        caption: kids.length ? "this session and its " + kids.length + (kids.length === 1 ? " run" : " runs") : "this session",
        note: COST_NOTE + (missing.length ? " No price for " + missing.join(", ") + "." : ""),
        details,
        mismatch,
        runs,
        models,
        includesRuns: !!kids.length
      };
    }
    const upCount = () => Object.keys(host2.MACHINE).filter((m) => host2.MACHINE_UP[m]).length;
    let allAnswered = false;
    function harnessSnapshot(id) {
      const h2 = Object.hasOwn(host2.HARNESSES, id) ? host2.HARNESSES[id] : null;
      return h2 ? { id, name: h2.name, light: h2.icon.light, dark: h2.icon.dark, darkTheme: host2.darkTheme() } : void 0;
    }
    function liveSnapshot(s, showMachine) {
      const cur = (host2.TURNS[s.id] ?? []).at(-1), msg = cur?.start?.brief ?? cur?.u?.text;
      const inb = cur?.start ?? (cur?.u ? { from: "you" } : host2.H.find((h2) => h2.to === s.id && h2.kind !== "move"));
      return {
        id: s.id,
        name: s.name,
        state: s.state,
        stateLabel: host2.STATE[s.state] ?? s.state,
        status: s.state === "work" ? host2.HARNESS[s.harness] : host2.ago(s.last),
        harness: s.state === "work" ? harnessSnapshot(s.harness) : void 0,
        detail: [
          showMachine ? host2.MACHINE[s.machine] : null,
          inb ? inb.from === "you" ? "for you" : "for " + host2.nameOf(inb.from) : null,
          msg ? host2.oneLine(msg) : null
        ].filter(Boolean).join(" \xB7 "),
        activity: s.state === "work" && s.activity ? [s.activity[0], s.activity[1], s.activity[2]] : void 0
      };
    }
    function inboxSnapshot(h2, quiet = false) {
      const sid = h2.kind === "move" ? h2.to : h2.from, t = host2.HOLDS.get(h2.id), s = host2.SESS[sid];
      const parts = [], part = (className, text2, tip) => parts.push({ className, text: text2, tip });
      let path = host2.I.more;
      if (h2.kind === "ask") {
        path = host2.I.ask;
        part("who", host2.nameOf("you"));
        part("verb", " asked ");
        part("who", host2.nameOf(h2.to));
      } else if (h2.kind === "spawn" || h2.kind === "relay") {
        path = host2.I.out;
        part("who", host2.nameOf(h2.from));
        part(
          "verb",
          h2.kind === "spawn" ? " handed off to " + (host2.SESS[h2.to]?.kind === "Subagent" ? "subagent" : host2.SESS[h2.to]?.kind ?? "") + " " : " relayed to "
        );
        part("who", host2.nameOf(h2.to));
      } else if (h2.kind === "move") {
        path = host2.I.move;
        const short = machineShorts(
          [h2.fromMachine, h2.toMachine].map((id) => [id, host2.MACHINE[id] ?? id])
        );
        part("verb", "Semon moved ");
        part("who", host2.nameOf(h2.to));
        part("verb", " from ");
        part(
          "verb mach",
          short.get(h2.fromMachine) ?? h2.fromMachine,
          "Machine: " + (host2.MACHINE[h2.fromMachine] ?? h2.fromMachine)
        );
        part("verb", " to ");
        part(
          "verb mach",
          short.get(h2.toMachine) ?? h2.toMachine,
          "Machine: " + (host2.MACHINE[h2.toMachine] ?? h2.toMachine)
        );
      } else if (h2.kind === "toyou") {
        path = h2.status === "done" && (h2.ask === "question" || h2.ask === "decision") ? host2.I.done : h2.ask === "question" ? host2.I.qc : h2.ask === "decision" ? host2.I.decide : host2.I.result;
        part("who", host2.nameOf(h2.from));
        part(
          "verb",
          { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h2.ask]
        );
      }
      const answer2 = quiet ? host2.answersOf(h2) : null, root = t ? host2.traceRoot(t) : null, msg = root?.start?.from === "you" ? root.start.brief : root?.u?.text;
      return {
        id: h2.id,
        quiet,
        icon: quiet && h2.kind === "toyou" ? host2.I.done : path,
        parts,
        age: host2.ago(h2.at),
        preview: preview(h2.brief ?? ""),
        answer: answer2 ? answer2.length ? "You answered: " + answer2.join(" \xB7 ") : "Answered \xB7 reply not in these logs" : void 0,
        origin: root ? msg ? { message: host2.oneLine(msg) } : { text: "Started by " + host2.nameOf(root.start ? root.start.from : root.sid) } : void 0,
        context: [host2.HARNESS[s.harness], host2.MACHINE[s.machine]].join(" \xB7 "),
        harness: harnessSnapshot(s.harness),
        trace: t?.out.length ? t.id : void 0
      };
    }
    const activityHost = {
      session: host2.goSession,
      committed: host2.observeTitle,
      trace: host2.goTrace,
      inbox(id) {
        const h2 = host2.H.find((h3) => h3.id === id) ?? host2.inbox().find((h3) => h3.id === id);
        if (!h2) return;
        if (host2.isResult(h2)) host2.markSeenResults([h2]);
        host2.goSession(h2.kind === "move" ? h2.to : h2.from, host2.HOLDS.get(h2.id)?.id);
      },
      answered() {
        allAnswered = true;
        host2.render();
      }
    };
    function renderHome(page) {
      const open = host2.inbox(), running = host2.working(), many = Object.keys(host2.MACHINE).length > 1;
      const rows = host2.orderList(
        host2.orderScope("page", host2.pageSig(), host2.navigation.route, host2.pageState()),
        "working",
        running,
        host2.byLast
      );
      const done = host2.H.filter((h2) => h2.kind === "toyou" && h2.status === "done").sort(
        (a, b) => b.at - a.at
      );
      renderHomeScreen(
        page,
        {
          waiting: open.length,
          working: running.length,
          up: upCount(),
          machines: Object.keys(host2.MACHINE).length,
          inbox: open.map((h2) => inboxSnapshot(h2, false)),
          live: rows.map((s) => liveSnapshot(s, many)),
          answered: (allAnswered ? done : done.slice(0, 3)).map((h2) => inboxSnapshot(h2, true)),
          totalAnswered: done.length,
          allAnswered
        },
        activityHost
      );
    }
    function renderMachines(page) {
      const ms = Object.keys(host2.MACHINE).sort(
        (a, b) => Number(host2.MACHINE_UP[a]) - Number(host2.MACHINE_UP[b])
      );
      const rows = ms.map((m) => {
        const here = host2.onMachine(m), w = here.filter((s) => s.state === "work").length, up = host2.MACHINE_UP[m], state2 = !up ? "err" : w ? "work" : "idle";
        const mv = host2.movedOff(m).length, mh = host2.movesOf(m).find((h2) => h2.kind === "move" && h2.fromMachine === m);
        return {
          id: m,
          name: host2.MACHINE[m],
          state: state2,
          stateLabel: host2.STATE[state2] ?? state2,
          status: !up ? "offline" : w ? "up" : "idle",
          detail: up ? [w + " working", here.length + (here.length === 1 ? " session" : " sessions")].join(
            " \xB7 "
          ) : [
            "Not responding" + (mh ? " since " + host2.clock(mh.at) : host2.MACHINE_LAST[m] != null ? " since " + host2.clock(host2.MACHINE_LAST[m]) : ""),
            mv ? mv + (mv === 1 ? " session" : " sessions") + " moved off" : null
          ].filter(Boolean).join(" \xB7 ")
        };
      });
      renderMachinesScreen(
        page,
        { rows, up: upCount(), working: host2.working().length, admin: host2.ADMIN },
        {
          machine(id) {
            host2.go({ v: "machine", id });
          },
          admin(href) {
            location.assign(href);
          },
          committed: host2.observeTitle
        }
      );
    }
    function renderMachine(page, m) {
      const here = host2.onMachine(m), off = host2.movedOff(m), moves2 = host2.movesOf(m);
      const ordered = host2.orderList(
        host2.orderScope("page", host2.pageSig(), host2.navigation.route, host2.pageState()),
        "machine:" + m,
        here,
        host2.byState
      );
      renderMachineScreen(
        page,
        {
          name: host2.MACHINE[m],
          totalSessions: here.length,
          sessions: ordered.map((s) => liveSnapshot(s, false)),
          off: off.map((s) => ({
            ...liveSnapshot(s, false),
            detail: "Now on " + host2.MACHINE[s.machine]
          })),
          moves: moves2.map((h2) => inboxSnapshot(h2, true))
        },
        activityHost
      );
    }
    const RUN_EXPANDED = /* @__PURE__ */ new Map();
    const { agentSnapshot } = createTraceCalculations(
      { sessions: host2.SESS, turns: host2.TURNS, starts: host2.STARTS },
      host2.domain,
      () => host2.NOW,
      RUN_EXPANDED,
      host2.HARNESS,
      host2.STATE
    );
    function renderTrace(page, id) {
      const root = host2.TURN.get(id);
      if (!root) {
        renderTraceScreen(
          page,
          { empty: true, hops: [] },
          { ...host2.sentenceHost, committed: host2.observeTitle, fold() {
          } }
        );
        return;
      }
      const visited = /* @__PURE__ */ new Set(), read = (turn) => {
        if (!turn || visited.has(turn.id)) return;
        visited.add(turn.id);
        host2.markSeenResults(turn.out);
        for (const h2 of turn.out)
          if (h2.kind === "spawn" || h2.kind === "relay") read(host2.STARTS.get(h2.id));
      };
      read(root);
      const seen = /* @__PURE__ */ new Set([root.id]), sessions = /* @__PURE__ */ new Set([root.sid]), scope = /* @__PURE__ */ new Set([root.sid]), reached = /* @__PURE__ */ new Set([root.id]);
      let n = 0;
      const reach = (turn) => {
        for (const h2 of turn.sent) {
          scope.add(h2.kind === "toyou" ? h2.from : h2.to);
          const child = h2.kind === "spawn" || h2.kind === "relay" ? host2.STARTS.get(h2.id) : null;
          if (child && !reached.has(child.id)) {
            reached.add(child.id);
            reach(child);
          }
        }
      };
      reach(root);
      const meta = (state2, text3, sid, turn, note) => {
        const s = host2.SESS[sid], label = host2.machineLabel(s, scope);
        return {
          state: state2,
          stateLabel: host2.STATE[state2] ?? state2,
          text: text3,
          chip: s ? [s.kind ?? host2.HARNESS[s.harness], label].filter(Boolean).join(" \xB7 ") : void 0,
          chipClass: s ? host2.hcls(sid) : void 0,
          tip: label ? "Machine: " + host2.hostOf(s) : void 0,
          harness: s ? harnessSnapshot(s.harness) : void 0,
          note: note ?? void 0,
          session: s && !s.stub ? sid : void 0,
          turn: turn?.id,
          name: s?.name
        };
      };
      const start = root.start, text2 = start ? start.brief : root.u?.text, initial = start ? host2.sentenceSnapshot(start, null) : root.u ? host2.sentenceSnapshot(
        {
          kind: "ask",
          id: root.id,
          from: "you",
          to: root.sid,
          at: root.at ?? host2.NOW,
          status: "done",
          brief: ""
        },
        null
      ) : {
        icon: host2.I.more,
        parts: [
          { className: "who", text: host2.SESS[root.sid].name },
          { className: "verb", text: " \xB7 a turn whose start isn't in these logs" }
        ]
      };
      const outcome = host2.turnEnd(root), hops = [
        {
          key: "root:" + root.id,
          className: "k-root",
          icon: initial.icon,
          parts: initial.parts,
          nodeClass: host2.hcls(start ? start.from : root.u ? "you" : root.sid),
          turn: root.id,
          handoff: start?.id,
          time: start ? host2.clock(start.at) : void 0,
          brief: text2 || void 0,
          meta: meta(outcome?.st ?? "idle", outcome?.text ?? "Nothing recorded", root.sid, root)
        }
      ];
      const walk = (turn) => {
        for (const h2 of turn.sent) {
          const result = h2.kind === "toyou" && h2.ask === "result", child = h2.kind === "spawn" || h2.kind === "relay" ? host2.STARTS.get(h2.id) : null, target = h2.kind === "toyou" ? h2.from : h2.to;
          const sentence = result ? { icon: host2.I.result, parts: [{ className: "verb", text: host2.statWord(h2) ?? "" }] } : host2.sentenceSnapshot(h2, null);
          const hop = {
            key: h2.id,
            className: "child k-" + h2.kind + " s-" + h2.status + (child || h2.kind === "toyou" || h2.kind === "move" ? "" : " stub"),
            icon: sentence.icon,
            parts: sentence.parts,
            nodeClass: host2.hcls(target),
            handoff: h2.id,
            turn: child?.id,
            time: host2.clock(h2.at)
          };
          hops.push(hop);
          if (result) {
            n++;
            continue;
          }
          hop.brief = h2.brief;
          hop.answers = host2.answersOf(h2) ?? void 0;
          hop.result = h2.result || void 0;
          if (h2.kind === "move") {
            hop.meta = meta("done", "Moved", h2.to, turn);
            continue;
          }
          n++;
          if (h2.kind === "toyou") {
            hop.meta = meta(
              host2.isResult(h2) ? host2.SEEN_RESULTS.has(h2.id) ? "read" : "new" : h2.status === "done" ? "done" : h2.status,
              host2.statWord(h2) ?? "",
              h2.from,
              turn
            );
            continue;
          }
          sessions.add(h2.to);
          const end = child && host2.turnEnd(child);
          hop.meta = meta(
            end ? end.st ?? "idle" : h2.status === "done" ? "done" : h2.status,
            end ? end.text ?? "Nothing recorded" : host2.statWord(h2) ?? "",
            h2.to,
            child,
            child ? void 0 : "Its turn isn't in these logs"
          );
          if (child && !seen.has(child.id)) {
            seen.add(child.id);
            walk(child);
          }
        }
      };
      walk(root);
      renderTraceScreen(
        page,
        {
          empty: false,
          hops,
          agents: agentSnapshot(root),
          summary: [
            sessions.size + (sessions.size === 1 ? " session" : " sessions"),
            n + (n === 1 ? " handoff" : " handoffs"),
            [...host2.machineLabels(scope).values()].join(", ")
          ].filter(Boolean).join(" \xB7 ")
        },
        {
          ...host2.sentenceHost,
          committed: host2.observeTitle,
          fold(id2) {
            RUN_EXPANDED.get(root.id)?.add(id2);
            renderTrace(page, root.id);
          }
        }
      );
      return [
        sessions.size + (sessions.size === 1 ? " session" : " sessions"),
        n + (n === 1 ? " handoff" : " handoffs"),
        [...host2.machineLabels(scope).values()].join(", ")
      ].filter(Boolean).join(" \xB7 ");
    }
    function renderSession(page, sid, opts = {}) {
      host2.markSeenResults(host2.H.filter((h2) => host2.isResult(h2) && h2.from === sid));
      const raw = new Map(
        host2.transcriptEntries(host2.TX[sid] ?? [], sid).map((e) => [e.slot != null ? sid + "#slot:" + e.slot : e.key, e])
      );
      renderSessionScreen(page, host2.transcriptSnapshot(sid, opts), {
        ...host2.sentenceHost,
        committed: host2.observeTitle,
        trace: host2.goTrace,
        toolAll(key, label) {
          const e = raw.get(key);
          if (e?.k === "tool") {
            const [ic, v] = host2.verb(e.name);
            host2.openStepViewer(e, v, ic, label);
          }
        },
        script(key) {
          const e = raw.get(key);
          if (e?.k === "tool") host2.openScript(e);
        },
        image: host2.openImage,
        background(call, trigger) {
          const target = trigger.closest('section[aria-label="Transcript"]')?.querySelector('.step[data-tid="' + CSS.escape(call) + '"]');
          if (!target) return;
          host2.stopOpeningEndPin();
          for (let parent = target.parentElement; parent; parent = parent.parentElement) {
            const toggle = host2.opener(parent);
            if (toggle?.getAttribute("aria-expanded") === "false") toggle.click();
          }
          host2.centre(target);
          target.classList.add("flash");
          host2.scope.timeout(() => target.classList.remove("flash"), 1500);
        },
        pager(button) {
          host2.loadPager(button, true);
        },
        jump: host2.jumpToLatest
      });
    }
    const showsFooter = (s, origin) => origin ? s.state === "work" || s.state === "done" || s.state === "err" || origin.status === "done" || origin.status === "err" : !s.stub && !s.role && s.state in host2.STATE;
    return {
      harnessSnapshot,
      costSnapshot,
      showsFooter,
      renderHome,
      renderMachines,
      renderMachine,
      renderTrace,
      renderSession
    };
  }

  // src/app/seenPersistence.ts
  function createSeenPersistence(host2) {
    function markSeenResults(handoffs) {
      let changed = false;
      for (const h2 of handoffs)
        if (host2.isResult(h2) && typeof h2.id === "string" && !host2.SEEN_RESULTS.has(h2.id)) {
          host2.SEEN_RESULTS.add(h2.id);
          changed = true;
        }
      while (host2.SEEN_RESULTS.size > host2.SEEN_LIMIT)
        host2.SEEN_RESULTS.delete(host2.SEEN_RESULTS.values().next().value);
      if (changed)
        try {
          window.localStorage.setItem(host2.SEEN_KEY, JSON.stringify([...host2.SEEN_RESULTS]));
        } catch {
        }
    }
    return { markSeenResults };
  }

  // src/app/seenResults.ts
  function createSeenResults(host2) {
    const SEEN_KEY = "semon.seen", SEEN_LIMIT = 2e3;
    const SEEN_RESULTS = (() => {
      try {
        const ids = JSON.parse(window.localStorage.getItem(SEEN_KEY) ?? "[]");
        if (!Array.isArray(ids)) return /* @__PURE__ */ new Set();
        const clean2 = ids.filter((id) => typeof id === "string").slice(-SEEN_LIMIT), seen = new Set(clean2);
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

  // src/app/sentences.ts
  function createSentences(host2) {
    function sentenceSnapshot(h2, viewer, links = false) {
      const parts = [], text2 = (className, text3) => parts.push({ className, text: text3 });
      const who = (id, action, label) => {
        const name = host2.nameOf(id), linked = links && action && id !== "you" && id !== viewer && host2.SESS[id];
        parts.push({
          text: name,
          className: linked ? "who-link" : "who",
          action: linked ? action : void 0,
          label: linked ? label?.(name) : void 0
        });
      };
      const sender = { kind: "sender", id: h2.id }, recipient = {
        kind: "session",
        id: h2.to,
        turn: host2.STARTS.get(h2.id)?.id
      };
      const senderLabel = (name) => "Open " + name + " where it sent this", recipientLabel = (name) => "Open " + name + " at the turn this started";
      let path;
      if (h2.kind === "ask") {
        path = I.ask;
        who("you");
        text2("verb", " asked ");
        who(h2.to, recipient, recipientLabel);
      } else if (h2.kind === "spawn" || h2.kind === "relay") {
        if (viewer === h2.to) {
          path = I.in;
          text2("verb", h2.kind === "spawn" ? "Brief from " : "Relay from ");
          who(h2.from, sender, senderLabel);
        } else {
          path = I.out;
          who(h2.from, sender, senderLabel);
          text2(
            "verb",
            h2.kind === "spawn" ? " handed off to " + (host2.SESS[h2.to]?.kind === "Subagent" ? "subagent" : host2.SESS[h2.to]?.kind ?? "") + " " : " relayed to "
          );
          who(h2.to, recipient, recipientLabel);
        }
      } else if (h2.kind === "move") {
        path = I.move;
        const short = machineShorts(
          [h2.fromMachine, h2.toMachine].map((id) => [id, host2.MACHINE[id] ?? id])
        );
        const machine2 = (id) => parts.push({
          className: "verb mach",
          text: short.get(id) ?? id,
          tip: "Machine: " + (host2.MACHINE[id] ?? id),
          action: links ? { kind: "machine", id } : void 0,
          label: links ? "Open machine " + (host2.MACHINE[id] ?? id) : void 0
        });
        text2("verb", "Semon moved ");
        who(h2.to, { kind: "session", id: h2.to }, (name) => "Open " + name);
        text2("verb", " from ");
        machine2(h2.fromMachine);
        text2("verb", " to ");
        machine2(h2.toMachine);
      } else if (h2.kind === "toyou") {
        path = h2.status === "done" && (h2.ask === "question" || h2.ask === "decision") ? I.done : h2.ask === "question" ? I.qc : h2.ask === "decision" ? I.decide : I.result;
        who(h2.from, sender, senderLabel);
        text2(
          "verb",
          { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h2.ask]
        );
      }
      return { icon: path ?? "", parts };
    }
    const sentenceHost = {
      session(id, turn) {
        host2.goSession(id, turn);
      },
      machine(id) {
        host2.go({ v: "machine", id });
      },
      sender(id) {
        const h2 = host2.HID.get(id);
        if (h2) host2.openSender(h2);
      }
    };
    return { sentenceHost, sentenceSnapshot };
  }

  // src/navigation/errors.ts
  function createErrorNavigation(host2) {
    const {
      navigation,
      TX,
      TXM,
      SESS,
      drawSessionBar,
      render: render2,
      keepFocus,
      countOf,
      capture,
      restore,
      opener,
      resetPagerInput,
      stopOpeningEndPin,
      centre,
      fetchTx,
      dropTx,
      spread,
      tail
    } = host2;
    const scope = new EffectScope();
    let pending = null, modeRequest = null;
    const $ = (selector) => host2.page().ownerDocument.querySelector(selector);
    const enc = encodeURIComponent, SIDEBAR_ONLY = host2.sidebarOnly;
    const ERR = {
      mode: "errors",
      on: false,
      sid: null,
      slots: [],
      listed: false,
      count: 0,
      version: null,
      k: -1,
      slot: null,
      saved: null,
      range: null,
      tools: true,
      chain: Promise.resolve(),
      gen: 0
    };
    const ERR_NEAR = 400, ERR_AROUND = 40;
    const errLive = createLiveRegion();
    if (!SIDEBAR_ONLY) document.body.append(errLive);
    const signalCount = (s) => Object.values(s?.signals ?? {}).reduce((n, x) => n + x, 0);
    const errText = () => ERR.k < 0 ? ERR.count ? "Finding " + ERR.mode + "\u2026" : "No " + ERR.mode : (ERR.mode === "signals" ? "Signal " : "Error ") + (ERR.k + 1) + " of " + ERR.count;
    const errOn = (sid) => ERR.on && ERR.sid === sid;
    function errLabel(announce) {
      ERR.notice = null;
      if (navigation.route.v === "session" && ERR.on) drawSessionBar();
      if (announce) errLive.textContent = errText();
    }
    function releaseMode() {
      if (modeRequest) {
        modeRequest.abort();
        scope.releaseRequest(modeRequest);
        modeRequest = null;
      }
    }
    function releasePending() {
      if (pending) {
        pending.abort();
        scope.releaseRequest(pending);
        pending = null;
      }
    }
    function openErrors(sid, mode = "errors") {
      if (ERR.on || navigation.route.v !== "session" || navigation.route.id !== sid || !TXM[sid])
        return;
      releaseMode();
      modeRequest = scope.request();
      resetPagerInput();
      stopOpeningEndPin();
      host2.clearFind();
      Object.assign(ERR, {
        mode,
        on: true,
        sid,
        slots: [],
        listed: false,
        count: mode === "signals" ? signalCount(SESS[sid]) : countOf(SESS[sid], "errors") ?? 0,
        version: null,
        k: -1,
        slot: null,
        saved: capture(),
        range: { tx: TX[sid], m: { ...TXM[sid] } },
        tools: host2.show.tools,
        gen: ERR.gen + 1
      });
      if (!host2.show.tools) {
        host2.show.tools = true;
        render2();
      } else drawSessionBar();
      document.getElementById("err-next")?.focus({ preventScroll: true });
      errLabel(true);
      const gen = ERR.gen;
      fetchErrors(sid).then(
        () => {
          if (ERR.gen !== gen || !ERR.on) return;
          if (ERR.slots.length) {
            ERR.k = 0;
            showError(true);
          } else errLabel(true);
        },
        () => {
          if (ERR.gen === gen && ERR.on) {
            errLive.textContent = "Couldn't list " + ERR.mode;
            ERR.notice = "Couldn't list " + ERR.mode;
            drawSessionBar();
          }
        }
      );
    }
    function fetchErrors(sid) {
      releasePending();
      const request = scope.request();
      pending = request;
      const gen = ERR.gen, mode = ERR.mode;
      return requestJson(
        "/api/tx?sid=" + enc(sid) + "&" + mode + "=1" + (ERR.version ? "&since=" + enc(ERR.version) : ""),
        request.signal,
        true
      ).then((x) => {
        if (!x || request.signal.aborted || ERR.gen !== gen || !errOn(sid) || typeof x !== "object")
          return;
        const slots = "slots" in x && Array.isArray(x.slots) ? x.slots.filter(
          (value) => typeof value === "number" && Number.isInteger(value) && value >= 0
        ) : [];
        const count = mode === "errors" ? "errors" in x ? x.errors : void 0 : "signals" in x ? x.signals : void 0;
        ERR.listed = true;
        ERR.slots = slots;
        ERR.count = typeof count === "number" && Number.isInteger(count) ? Math.max(count, slots.length) : slots.length;
        ERR.version = "version" in x && typeof x.version === "string" ? x.version : null;
        if (ERR.slot != null) {
          const currentSlot = ERR.slot, at = ERR.slots.indexOf(currentSlot), after = ERR.slots.findIndex((slot) => slot > currentSlot);
          ERR.k = at >= 0 ? at : !ERR.slots.length ? -1 : after >= 0 ? after : ERR.slots.length - 1;
          if (at < 0) ERR.slot = ERR.k >= 0 ? ERR.slots[ERR.k] : null;
        }
      }).catch((error) => {
        if (!request.signal.aborted && pending === request && ERR.gen === gen && errOn(sid))
          throw error;
      }).finally(() => {
        scope.releaseRequest(request);
        if (pending === request) pending = null;
      });
    }
    function stepErrors(delta) {
      if (!ERR.on || !ERR.slots.length) return;
      const n = ERR.slots.length;
      ERR.k = ERR.k < 0 ? 0 : ((ERR.k + delta) % n + n) % n;
      showError(true);
    }
    const hasSlot = (sid, slot) => {
      const m = TXM[sid];
      return !!m && slot >= m.from && slot < m.to;
    };
    function loadSlot(sid, slot) {
      if (hasSlot(sid, slot)) return Promise.resolve(false);
      const m = TXM[sid];
      if (!m || modeRequest?.signal.aborted) return Promise.resolve(false);
      const up = slot < m.from, near = up ? m.from - slot <= ERR_NEAR : slot - m.to < ERR_NEAR;
      let tries = 0;
      const extend = () => modeRequest?.signal.aborted || hasSlot(sid, slot) || tries++ >= 3 ? null : fetchTx(
        sid,
        up ? "before=" + TXM[sid].from : "after=" + TXM[sid].to,
        up ? "before" : "after",
        modeRequest?.signal
      ).then(extend);
      const around = () => modeRequest?.signal.aborted || hasSlot(sid, slot) ? null : fetchTx(
        sid,
        "after=" + Math.max(0, slot - ERR_AROUND),
        void 0,
        modeRequest?.signal
      ).then(
        () => hasSlot(sid, slot) ? null : fetchTx(sid, "after=" + slot, void 0, modeRequest?.signal)
      );
      return Promise.resolve(near ? extend() : null).then(around).then(() => true);
    }
    function errNode(sid, slot) {
      const e = (TX[sid] ?? []).find(
        (x) => x.k === (ERR.mode === "signals" ? "signal" : "tool") && x.slot === slot
      );
      if (!e?.key) return null;
      return [...host2.page().querySelectorAll(".turns [data-e]")].find(
        (n) => n.dataset.e === e.key
      ) ?? null;
    }
    function markError(ring) {
      for (const n of host2.page().querySelectorAll("[data-e].err-current"))
        n.classList.remove("err-current", "err-ring");
      if (!ERR.on || ERR.slot == null || navigation.route.v !== "session" || navigation.route.id !== ERR.sid)
        return null;
      const node = errNode(navigation.route.id, ERR.slot);
      if (!node) return null;
      const g = node.closest(".tgroup"), sum = g && opener(g);
      if (sum?.getAttribute("aria-expanded") === "false") sum.click();
      node.classList.add("err-current");
      if (ring) {
        node.classList.remove("err-ring");
        void node.offsetWidth;
        node.classList.add("err-ring");
        scope.timeout(() => node.classList.remove("err-ring"), 1500);
      }
      return node;
    }
    function showError(announce) {
      resetPagerInput();
      const sid = ERR.sid;
      if (!sid) return;
      const slot = ERR.slots[ERR.k], gen = ERR.gen;
      ERR.slot = slot;
      errLabel(announce);
      ERR.chain = ERR.chain.then(() => {
        if (!ERR.on || ERR.gen !== gen || ERR.slot !== slot) return null;
        stopOpeningEndPin();
        return loadSlot(sid, slot).then(
          (moved) => {
            if (!ERR.on || ERR.gen !== gen || ERR.slot !== slot || navigation.route.v !== "session" || navigation.route.id !== sid)
              return;
            if (moved) keepFocus(render2);
            const node = markError(true);
            if (!node) {
              if (announce) errLive.textContent = errText() + ", not shown in this transcript";
              return;
            }
            centre(node);
            scope.frame(
              () => scope.frame(() => {
                if (ERR.on && ERR.slot === slot && node.isConnected) centre(node);
              })
            );
          },
          () => {
            if (ERR.on && ERR.gen === gen) errLive.textContent = "Couldn't load " + errText();
          }
        );
      }).catch((e) => {
        scope.timeout(() => {
          throw e;
        });
      });
    }
    function dropErrors(away = false) {
      if (!ERR.on) return;
      const sid = ERR.sid;
      if (!sid) return;
      const range = ERR.range, m = TXM[sid];
      releasePending();
      releaseMode();
      ERR.on = false;
      ERR.gen++;
      host2.show.tools = ERR.tools;
      ERR.saved = ERR.range = null;
      errLive.textContent = "";
      if (away && range && m && (m.from !== range.m.from || m.to < range.m.to)) dropTx(sid);
    }
    function closeErrors() {
      if (!ERR.on) return;
      const sid = ERR.sid;
      if (!sid) return;
      const saved = ERR.saved, range = ERR.range, m = TXM[sid];
      dropErrors();
      const generation = ERR.gen;
      let p = Promise.resolve(void 0);
      if (range && m && TX[sid] && (m.from !== range.m.from || m.to < range.m.to)) {
        TX[sid] = range.tx;
        TXM[sid] = range.m;
        spread(sid);
        if (range.m.to >= range.m.total && range.m.tok !== host2.TOK[sid])
          p = tail(sid).catch(() => null);
      }
      p.then(() => {
        if (navigation.route.v !== "session" || navigation.route.id !== sid || ERR.on || ERR.gen !== generation)
          return;
        render2();
        if (saved) restore(saved);
        const b0 = $("#topbar .lab-errs") ?? $('#topbar .chip[data-filter="failures"]'), b = b0 && !b0.getClientRects().length ? $("#more-btn") : b0;
        if (b && !b.hidden && document.activeElement !== b && (!document.activeElement || document.activeElement === document.body || !document.activeElement.isConnected))
          b.focus({ preventScroll: true });
      });
    }
    if (!SIDEBAR_ONLY)
      scope.listen(document, "keydown", (e) => {
        if (!ERR.on || e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey || document.querySelector("dialog[open]"))
          return;
        if (!(e.target instanceof HTMLElement)) return;
        if (document.body.classList.contains("drawer-open") || document.querySelector(".menu"))
          return;
        if (e.target.closest?.("input, textarea, select, [contenteditable='true']")) return;
        const inBar = !!e.target.closest?.("#topbar .errnav-bar"), onButton = e.target.tagName === "BUTTON";
        if (e.key === "Escape") {
          e.preventDefault();
          closeErrors();
          return;
        }
        if (e.key === "n" || e.key === "N") {
          e.preventDefault();
          stepErrors(1);
          return;
        }
        if (e.key === "p" || e.key === "P") {
          e.preventDefault();
          stepErrors(-1);
          return;
        }
        if (e.key === "Enter" && (inBar || e.target === document.body)) {
          if (e.shiftKey) {
            e.preventDefault();
            stepErrors(-1);
          } else if (!onButton) {
            e.preventDefault();
            stepErrors(1);
          }
        }
      });
    function errorsLive() {
      if (!ERR.on || navigation.route.v !== "session" || navigation.route.id !== ERR.sid) return null;
      const sid = ERR.sid;
      if (!sid) return null;
      const gen = ERR.gen, was = ERR.count;
      return fetchErrors(sid).then(() => {
        if (!ERR.on || ERR.gen !== gen) return;
        if (ERR.k < 0 && ERR.slots.length) {
          ERR.k = 0;
          showError(true);
          return;
        }
        errLabel(ERR.count !== was);
        markError(false);
      });
    }
    return {
      state: ERR,
      text: errText,
      on: errOn,
      open: openErrors,
      step: stepErrors,
      mark: markError,
      drop: dropErrors,
      close: closeErrors,
      live: errorsLive,
      destroy() {
        releasePending();
        releaseMode();
        scope.destroy();
        errLive.remove();
        ERR.on = false;
        ERR.gen++;
      }
    };
  }

  // src/app/sessionChrome.ts
  function createSessionChrome(host2) {
    const kindText = (s) => s.kind ?? host2.HARNESS[s.harness] ?? s.harness;
    const modelIdOf = (s) => s.model ?? Object.keys(s.tokens_by_model ?? {})[0];
    const viewerBar = createViewerBar();
    function renderTopbar(title, crumb = null, opts = {}) {
      const s = opts.session ?? opts.traceSession;
      const account = host2.ACCOUNT ? {
        account: host2.ACCOUNT,
        compact: false,
        wide: host2.wideMode,
        onWideChange: () => host2.setWideMode(!host2.wideMode)
      } : null;
      const mode = s && errOn(s.id) ? "errors" : s && host2.findOpen ? "find" : "normal";
      const content = viewerBar.update(
        {
          mode,
          name: title,
          session: s?.id,
          state: s?.state,
          stateLabel: s ? host2.STATE[s.state] : void 0,
          stateTip: s ? "Status: " + host2.STATE[s.state] + " \xB7 " + turnsLabel(s) : void 0,
          showState: !!s && !opts.traceSession && !(host2.phone.matches && opts.lineage?.length),
          ancestors: !host2.phone.matches ? opts.lineage?.map((a) => ({
            ...a,
            harnessName: host2.HARNESS[a.harness] ?? a.harness
          })) ?? [] : [],
          crumb: opts.lineage?.length ? void 0 : crumb?.label,
          labels: opts.line2 ?? [],
          trace: !!opts.traceSession,
          analytics: !!opts.analytics,
          days: host2.analyticsRange,
          query: host2.find,
          count: matchText(matchCount()),
          filter: host2.show.messages && host2.show.tools ? "all" : host2.show.messages ? "messages" : "steps",
          failed: s ? host2.countOf(s, "errors") ?? 0 : 0,
          signals: s ? signalCount(s) : 0,
          errorMode: ERR.mode,
          errorText: ERR.notice ?? errText(),
          errorDisabled: ERR.listed && !ERR.slots.length,
          icons: {
            search: host2.I.search,
            back: host2.I.back,
            x: host2.I.x,
            up: host2.I.up,
            more: host2.I.more,
            dn: host2.I.dn
          }
        },
        {
          ancestor: host2.goSession,
          crumb() {
            crumb?.go();
          },
          find() {
            host2.findOpen = true;
            host2.render();
            host2.$("#find")?.focus();
          },
          closeFind() {
            host2.findOpen = false;
            host2.find = "";
            host2.show = { ...host2.SHOW_ALL };
            host2.render();
          },
          query(value) {
            host2.find = value.toLowerCase();
            host2.render();
          },
          filter(key) {
            if (key === "failures" || key === "signals") {
              s && openErrors(s.id, key === "signals" ? "signals" : "errors");
              return;
            }
            host2.show = key === "messages" ? { messages: true, tools: false, thinking: false } : key === "steps" ? { messages: false, tools: true, thinking: false } : { ...host2.SHOW_ALL };
            host2.render();
          },
          menu(trigger, runs) {
            s && openSessionMenu(host2.SESS[s.id] ?? s, trigger, runs ? ".runs" : void 0);
          },
          errors() {
            s && openErrors(s.id);
          },
          closeErrors,
          step: stepErrors,
          range(days) {
            if (host2.analyticsRange === days) return;
            const top = host2.currentScroll();
            host2.analyticsRange = days;
            host2.render();
            host2.restoreScroll(top);
            host2.refreshAnalytics(true);
          }
        }
      );
      const lead = {
        label: opts.traceSession ? "Back to " + s?.name : "Open navigation",
        icon: opts.traceSession ? host2.I.chev : host2.I.menu,
        back: opts.traceSession ? () => s && host2.goSession(
          s.id,
          "turn" in host2.navigation.route ? host2.navigation.route.turn : void 0
        ) : void 0
      };
      host2.shellChrome?.topbar(
        mode === "normal" ? { titleSlot: content.titleSlot, actions: [content.actions], session: !!s, lead, account } : { mode: [content.mode], session: true, account, accountTarget: content.accountTarget }
      );
      if (s && mode === "normal")
        host2.scope.frame(() => {
          const line = host2.$("#topbar .meta-line");
          if (line) measureViewerBar(host2.$("#topbar"));
        });
    }
    const drawSessionBar = () => {
      const s = host2.SESS["id" in host2.navigation.route ? host2.navigation.route.id : ""];
      if (host2.navigation.route.v !== "session" || !s) return;
      renderTopbar(s.name, null, {
        session: s,
        lineage: host2.lineageOf("id" in host2.navigation.route ? host2.navigation.route.id : "").slice(0, -1),
        line2: sessionLine(s)
      });
      setGeometry(document.documentElement, "barHeight", host2.$("#topbar").offsetHeight);
      syncBarLine();
    };
    const keepFocus = (fn) => {
      const id = document.activeElement?.id;
      fn();
      const n = id && document.getElementById(id);
      if (n && n !== document.activeElement) n.focus({ preventScroll: true });
    };
    function centre(node) {
      if (!node.isConnected) return;
      const sc = host2.scroller(), r = (node.querySelector(":scope > button") ?? node).getBoundingClientRect(), bottom = host2.phone.matches ? window.innerHeight : host2.$("#main").getBoundingClientRect().bottom;
      const d = (r.top + r.bottom) / 2 - (host2.edge() + bottom) / 2;
      host2.scrollProgrammatically(() => {
        if (Math.abs(d) >= 1) sc.scrollTop += d;
      });
      syncBarLine();
      host2.syncJump();
      host2.saveHistoryScroll();
    }
    const errorNavigation = createErrorNavigation({
      navigation: host2.navigation,
      TX: host2.TX,
      TXM: host2.TXM,
      TOK: host2.TOK,
      SESS: host2.SESS,
      show: host2.show,
      sidebarOnly: host2.SIDEBAR_ONLY,
      page: () => host2.$("#page"),
      drawSessionBar,
      render: host2.render,
      keepFocus,
      countOf: (session, field) => session ? host2.countOf(session, field) ?? void 0 : void 0,
      capture: () => host2.capture(),
      restore: (saved) => host2.restore(saved),
      opener: (node) => host2.opener(node),
      resetPagerInput: host2.resetPagerInput,
      stopOpeningEndPin: () => host2.stopOpeningEndPin(),
      clearFind() {
        host2.find = "";
      },
      centre,
      fetchTx: host2.fetchTx,
      dropTx: host2.dropTx,
      spread: host2.spread,
      tail: (sid) => host2.tail(sid)
    });
    const {
      state: ERR,
      text: errText,
      on: errOn,
      open: openErrors,
      step: stepErrors,
      mark: markError,
      drop: dropErrors,
      close: closeErrors,
      live: errorsLive
    } = errorNavigation;
    const signalCount = (s) => Object.values(s?.signals ?? {}).reduce((n, x) => n + x, 0);
    const turnsLabel = (s) => {
      const n = (host2.TURNS[s.id] ?? []).filter(host2.hasTurn).length;
      return n + (n === 1 ? " turn" : " turns");
    };
    const sessionLine = (s) => {
      const failed = host2.countOf(s, "errors") ?? 0, runs = host2.descendantsOf(s.id, host2.sessionChildren()), cost2 = runs.length ? host2.costForSessions([s, ...runs]) : host2.costForSession(s.id), labels = [];
      const add = (key, text2, tip, drop, extra = {}) => labels.push({ key, text: text2, tip: tip ?? void 0, drop, ...extra });
      add("state", host2.STATE[s.state], void 0, 0, {
        className: "state " + s.state,
        state: s.state,
        stateLabel: host2.STATE[s.state]
      });
      add("kind", kindText(s), s.kind ? s.kind + " \xB7 " + host2.HARNESS[s.harness] : void 0, 2);
      add(
        "model",
        shortModel(s.model),
        "Model: " + modelIdOf(s) + (s.effort ? ". Reasoning effort: " + s.effort : ""),
        3,
        { className: "meta-model", effort: s.effort }
      );
      if (failed)
        add(
          "errors",
          failed + " failed",
          failed + (failed === 1 ? " failed step" : " failed steps") + ": step through them",
          0,
          { className: "lab-errs", action: "errors" }
        );
      if (runs.length)
        add(
          "runs",
          runs.length + (runs.length === 1 ? " run" : " runs"),
          runs.length + (runs.length === 1 ? " run" : " runs") + " under this session: open the list with their cost",
          1,
          { className: "lab-runs", action: "runs" }
        );
      add(
        "machine",
        host2.MACHINE[s.machine],
        "Machine: " + host2.MACHINE[s.machine] + " \xB7 " + host2.hostOf(s),
        5
      );
      if (s.branch) add("branch", s.branch, "Branch: " + s.branch, 6);
      add(
        "cost",
        host2.costText(cost2),
        "API-equivalent cost" + (runs.length ? ", with " + runs.length + (runs.length === 1 ? " run" : " runs") : "") + ". Details in the session menu.",
        7
      );
      return labels;
    };
    const machineLine = (m) => {
      const here = host2.onMachine(m), w = here.filter((s) => s.state === "work").length, up = host2.MACHINE_UP[m];
      const sessions = here.length + (here.length === 1 ? " session" : " sessions");
      return [
        {
          key: "state",
          text: up ? "Up" : "Not responding",
          drop: 0,
          className: "state " + (up ? "done" : "err"),
          state: up ? w ? "work" : "idle" : "err",
          stateLabel: host2.STATE[up ? w ? "work" : "idle" : "err"]
        },
        {
          key: "activity",
          text: up ? w + " working \xB7 " + sessions : host2.movedOff(m).length ? host2.movedOff(m).length + " moved off" : [
            host2.MACHINE_LAST[m] != null ? "Last seen " + host2.clock(host2.MACHINE_LAST[m]) : null,
            sessions
          ].filter(Boolean).join(" \xB7 "),
          drop: 1
        }
      ];
    };
    const matchCount = () => host2.find || !host2.show.messages || !host2.show.tools || !host2.show.thinking ? host2.$("#page").querySelectorAll(
      ".turns .msg, .turns .bubble, .turns .step:not(.bgend), .turns .event, .turns .child-card"
    ).length : null;
    const matchText = (n) => n == null ? "" : n ? n + (n === 1 ? " match" : " matches") : "No matches";
    let titleObs = null;
    function observeTitle() {
      syncBarLine();
    }
    function syncBarLine() {
      const y = host2.phone.matches ? window.scrollY : host2.$("#main").scrollTop;
      host2.$("#topbar").classList.toggle("scrolled", y > 4);
    }
    if (!host2.SIDEBAR_ONLY) {
      host2.scope.listen(window, "scroll", syncBarLine, { passive: true });
      host2.scope.listen(host2.$("#main"), "scroll", syncBarLine, { passive: true });
    }
    if (!host2.SIDEBAR_ONLY)
      host2.scope.listen(
        window,
        "resize",
        () => {
          const l2 = host2.$("#topbar .meta-line");
          if (l2 && host2.navigation.route.v === "session") measureViewerBar(host2.$("#topbar"));
          host2.syncLayoutPrefs();
          host2.syncJump();
        },
        { passive: true }
      );
    function panel(title, opts = {}) {
      const chrome = createPanelChrome(
        { title, className: opts.cls, label: opts.label, sub: opts.sub },
        {
          opened() {
            host2.viewerEl = chrome.dialog;
            document.documentElement.classList.add("panel-open");
            try {
              history.pushState(
                { ...host2.navigation.route, sheet: 1, scrollTop: host2.currentScroll() },
                ""
              );
            } catch {
            }
          },
          closed() {
            render(null, chrome.body);
            host2.dialogs.delete(chrome.dialog);
            if (host2.disposed) return;
            const d = chrome.dialog;
            document.documentElement.classList.remove("panel-open");
            opts.onClose?.();
            if (host2.viewerEl === d) {
              host2.viewerEl = null;
              if (history.state?.sheet) {
                host2.skipPop = true;
                history.back();
              } else if (host2.pendingSessionOpen) {
                const id = host2.pendingSessionOpen;
                host2.pendingSessionOpen = null;
                host2.goSession(id);
              }
            }
            if (host2.LIVE.pending) host2.refresh();
          }
        }
      );
      host2.dialogs.set(chrome.dialog, chrome);
      return { d: chrome.dialog, body: chrome.body, show: () => chrome.show() };
    }
    const RUNS_CAP = 5;
    const TRADEMARK_NOTICE = "Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.";
    function openSessionMenu(s, anchor = null, scrollTo = void 0) {
      const kids = host2.descendantsOf(s.id, host2.sessionChildren());
      const {
        d,
        body,
        show: open
      } = panel(s.name, {
        cls: "anchored session-menu",
        label: "Session menu for " + s.name,
        sub: [host2.STATE[s.state], kindText(s), shortModel(s.model)].join(" \xB7 "),
        onClose: () => {
          anchor?.setAttribute("aria-expanded", "false");
          anchor?.focus({ focusVisible: false });
        }
      });
      const traceTurn = host2.navigation.route.v === "trace" ? host2.TURN.get(
        ("turn" in host2.navigation.route ? host2.navigation.route.turn : void 0) ?? ""
      ) : (() => {
        const top = host2.$("#topbar").getBoundingClientRect().bottom;
        const nodes = [
          ...document.querySelectorAll("#page .turn[data-turn]")
        ].filter((n) => !n.closest(".cw-body"));
        const visible = nodes.find((n) => n.getBoundingClientRect().bottom > top);
        return host2.TURN.get(visible?.dataset.turn ?? "") ?? (host2.TURNS[s.id] ?? []).at(-1);
      })();
      const actions = [], addAction = (key, text2, icon, className, note, checked, dot) => actions.push({ key, text: text2, icon, className, note, checked, dot });
      if (traceTurn?.out.length) addAction("trace", "Trace this turn", host2.I.trace, "menu-trace");
      const command = s.harness === "codex" ? "codex resume " + s.id : "claude --resume " + (s.sessionId ?? s.id);
      addAction("copy", "Copy resume command", host2.I.copy);
      if (s.harness === "claude") addAction("external", "Open in claude.ai", host2.I.ext);
      if (!host2.phone.matches)
        addAction("wide", "Wide transcript", host2.I.wide, void 0, void 0, host2.wideMode);
      const signals = signalCount(s);
      if (signals)
        addAction("signals", signals + " signals", void 0, "menu-signals", "Step through");
      const errorCount = host2.countOf(s, "errors") ?? 0;
      if (host2.phone.matches) {
        if (errorCount)
          addAction(
            "errors",
            errorCount + " failed",
            void 0,
            "menu-errors",
            "Step through",
            void 0,
            "err"
          );
        if (kids.length) addAction("runs", "Runs \xB7 " + kids.length, host2.I.stack, "menu-runs");
      }
      const machine2 = host2.MACHINE[s.machine] ?? s.machine ?? "Unknown machine", calls = host2.countOf(s, "calls");
      const detailRows = [
        ["Status", host2.STATE[s.state] + " \xB7 " + turnsLabel(s)],
        ...s.kind ? [["Kind", s.kind]] : [],
        ["Harness", host2.HARNESS[s.harness] ?? s.harness, false, host2.harnessSnapshot(s.harness)],
        ["Model", modelIdOf(s), true],
        ...s.effort ? [["Effort", s.effort]] : [],
        [
          "Machine",
          machine2 + (host2.hostOf(s) !== machine2 ? " \xB7 " + host2.hostOf(s) : "") + (s.movedFrom ? " (moved from " + (host2.MACHINE[s.movedFrom] ?? s.movedFrom) + ")" : "")
        ],
        ["Directory", s.cwd ?? s.dir ?? s.directory, true],
        [s.worktree ? "Worktree" : "Branch", host2.branchOf(s), true],
        ["Tool calls", calls == null ? "\u2014" : String(calls)],
        ...errorCount ? [["Errors", String(errorCount)]] : [],
        ["Started", host2.clock(s.start)],
        ["Duration", host2.dur(s.start, s.state === "work" ? null : s.last)],
        ["Process id", s.pid, true],
        ["Session id", s.sessionId ?? s.id, true],
        ...Object.entries(s.signals ?? {}).map(([kind, n]) => [
          "Signals \xB7 " + kind,
          String(n)
        ])
      ];
      const details = detailRows.filter(([, value]) => value != null && value !== "").map(([label, value, mono, harness]) => ({ label, value: String(value), mono, harness }));
      renderSessionMenu(
        body,
        {
          actions,
          command,
          path: host2.phone.matches ? host2.lineageOf(s.id).map((a) => ({
            id: a.id,
            name: a.name,
            harness: a.harness,
            harnessName: host2.HARNESS[a.harness] ?? a.harness
          })) : [],
          status: host2.STATE[s.state] + " \xB7 " + turnsLabel(s),
          state: s.state,
          stateLabel: host2.STATE[s.state],
          details,
          cost: host2.costSnapshot(s, kids),
          notice: TRADEMARK_NOTICE,
          icons: { chevron: host2.I.chev, copy: host2.I.copy }
        },
        {
          wide: () => host2.wideMode,
          session(id) {
            host2.pendingSessionOpen = id;
            d.close();
          },
          action(key) {
            if (key === "trace" && traceTurn) {
              host2.afterPop = () => host2.goTrace(traceTurn.id);
              d.close();
            } else if (key === "wide") host2.setWideMode(!host2.wideMode);
            else if (key === "signals" || key === "errors") {
              d.close();
              s && openErrors(s.id, key === "signals" ? "signals" : "errors");
            } else if (key === "runs")
              body.querySelector(".runs")?.scrollIntoView({ block: "start" });
          }
        }
      );
      anchor?.setAttribute("aria-expanded", "true");
      open();
      if (scrollTo) body.querySelector(scrollTo)?.scrollIntoView({ block: "nearest" });
      return d;
    }
    return {
      errorNavigation,
      viewerBar,
      drawSessionBar,
      syncBarLine,
      dropErrors,
      modelIdOf,
      kindText,
      observeTitle,
      centre,
      panel,
      renderTopbar,
      machineLine,
      sessionLine,
      errOn,
      markError,
      errorsLive
    };
  }

  // src/app/sessionList.ts
  function createSessionList(host2) {
    const laneOf = (sid) => {
      const seen = /* @__PURE__ */ new Set();
      while (host2.parentOf(sid) && !seen.has(sid)) {
        seen.add(sid);
        sid = host2.parentOf(sid);
      }
      return sid;
    };
    function childRuns(sid) {
      const kids = Object.values(host2.SESS).filter(
        (x) => x.id !== sid && laneOf(x.id) === sid && (host2.showApprovalReviews || !host2.isApprovalReview(x))
      ), sub = kids.filter((x) => x.kind === "Subagent").length, cdx = kids.filter((x) => x.kind === "Codex run").length, other = kids.length - sub - cdx;
      return [
        sub ? sub + (sub === 1 ? " subagent" : " subagents") : null,
        cdx ? cdx + (cdx === 1 ? " Codex run" : " Codex runs") : null,
        other ? other + (other === 1 ? " other run" : " other runs") : null
      ].filter(Boolean).join(" \xB7 ");
    }
    function renderSessions(page, focusSearch = false) {
      const all = Object.values(host2.SESS).filter(
        (s) => (host2.showApprovalReviews || !host2.isApprovalReview(s)) && host2.matchesSessionFacets(s)
      );
      const lanes = all.filter((s) => host2.sessMatch(s, host2.query)), order = host2.orderScope("page", host2.pageSig(), host2.navigation.route, host2.pageState());
      let groups;
      if (host2.groupBy === "recent")
        groups = [["", host2.orderList(order, "recent", lanes, host2.byLast), lanes.length]];
      else {
        const keysOf = {
          machine: (s) => host2.MACHINE[s.machine],
          project: (s) => s.repo ?? "No repo (roles)",
          harness: (s) => HARNESS[s.harness]
        };
        const key = keysOf[host2.groupBy] ?? ((s) => s.name);
        const keys = [...new Set(lanes.map(key))].sort(
          (a, b) => Number(a.startsWith("No repo")) - Number(b.startsWith("No repo")) || a.localeCompare(b)
        );
        groups = keys.map((k) => {
          const rows2 = lanes.filter((s) => key(s) === k);
          return [k, host2.orderList(order, "g:" + k, rows2, host2.byLast), rows2.length];
        });
      }
      const rows = groups.filter(([, items, total]) => items.length || !total).map(([title, items, total]) => ({
        title,
        total,
        harness: host2.groupBy === "harness" ? host2.harnessSnapshot(Object.keys(HARNESS).find((k) => HARNESS[k] === title) ?? "") : void 0,
        rows: items.map((s) => {
          const fields = [
            {
              className: "row-duration",
              text: host2.dur(s.start, s.state === "work" || s.state === "wait" ? null : s.last),
              priority: 1,
              tip: "Duration",
              icon: I.duration
            }
          ];
          if (Object.keys(host2.MACHINE).length > 1)
            fields.push({
              className: "row-machine host",
              text: host2.shortHost(s),
              priority: 2,
              tip: "Machine: " + host2.hostOf(s),
              icon: I.machine
            });
          if (s.repo)
            fields.push({
              className: "repo-short",
              text: s.repo,
              priority: 3,
              tip: "Repo: " + s.repo,
              icon: I.repo
            });
          const counts = childRuns(s.id);
          if (counts)
            fields.push({
              className: "row-counts",
              text: counts.replace(/Codex runs/g, "runs"),
              priority: 4
            });
          return {
            id: s.id,
            name: s.name,
            state: s.state,
            stateLabel: STATE[s.state] ?? s.state,
            age: host2.ago(s.last),
            harness: host2.harnessSnapshot(s.harness),
            model: shortModel(s.model ?? s.modelId),
            modelTip: "Model: " + host2.modelIdOf(s),
            delegation: host2.parentOf(s.id) ? I.stack : void 0,
            fields
          };
        })
      }));
      renderSessionsScreen(
        page,
        {
          total: all.length,
          working: all.filter((s) => s.state === "work").length,
          waiting: all.filter((s) => s.state === "wait").length,
          query: host2.query,
          groupBy: host2.groupBy,
          reviews: host2.showApprovalReviews,
          showReviews: Object.values(host2.SESS).some(host2.isApprovalReview) || host2.showApprovalReviews,
          groups: rows,
          matches: lanes.length
        },
        {
          session: host2.goSession,
          committed: host2.observeTitle,
          facets: () => host2.renderFacetFilters(page, () => host2.render()),
          search(value) {
            host2.query = value.trim();
            const address = new URL(location.href);
            if (host2.query) address.searchParams.set("q", host2.query);
            else address.searchParams.delete("q");
            history.replaceState(history.state, "", address);
            renderSessions(page);
            host2.renderLanes();
          },
          group(value) {
            host2.groupBy = value;
            renderSessions(page);
            host2.renderLanes();
          },
          reviews() {
            host2.showApprovalReviews = !host2.showApprovalReviews;
            host2.ORD.delete("page");
            host2.ORD.delete("side");
            host2.render();
          }
        },
        focusSearch
      );
      host2.renderLanes();
    }
    return { renderSessions };
  }

  // src/app/ticker.ts
  function createTicker(host2) {
    const running = (ms) => {
      const x = Math.max(0, Math.floor(ms / 1e3));
      return x < 60 ? x + "s" : Math.floor(x / 60) + "m " + x % 60 + "s";
    };
    function ticker() {
      if (!host2.visible() || Date.now() === host2.transportOwner.fetchedAt) return;
      host2.tick();
      if (host2.navigation.route.v === "session" && host2.navigation.rendered === host2.navigation.route)
        updateSessionClock(
          host2.$("#page"),
          host2.NOW,
          Object.fromEntries(
            Object.values(host2.SESS).filter((s) => s.activity?.[3] != null).map((s) => [s.id, s.activity[3]])
          )
        );
      else if (host2.navigation.route.v === "home" && host2.navigation.rendered === host2.navigation.route)
        host2.renderHome(host2.$("#page"));
    }
    return { ticker, running };
  }

  // src/app/toolLoader.ts
  function createToolLoader(host2) {
    function parseToolDetail(data) {
      const entry = parseEntry({ ...data, k: "tool", name: "", arg: "", ok: true });
      if (entry.k !== "tool") throw new Error("Invalid tool details");
      return entry;
    }
    function fullOf(e) {
      return Promise.all(
        (e.more ?? []).map(async (part) => ({
          part,
          data: object2(
            await host2.api(
              "/api/entry?sid=" + host2.enc(e.sid ?? "") + "&slot=" + e.slot + "&as=" + part
            )
          )
        }))
      ).then((parts) => {
        const full = {}, fullCut = [];
        for (const { part, data } of parts) {
          if (part === "diff") {
            full.diff = parseToolDetail(data).diff;
            full.changes = parseToolDetail(data).changes;
          } else if (part === "out") {
            full.out = optional(data.text, text);
            full.cut = parseToolDetail(data).cut;
          } else if (part === "in") full.in = optional(data.text, text);
          else if (part === "arg") full.arg = optional(data.text, text);
          if (data.truncated === true) fullCut.push(part);
        }
        full.fullCut = fullCut;
        return full;
      });
    }
    return { fullOf };
  }

  // src/app/toolViews.ts
  function createToolViews(host2) {
    let viewerEl = null;
    let skipPop = false;
    function openStepViewer(e, v, ic, inLabel = "Input") {
      if (host2.disposed) return;
      if (e.more?.length && e.slot != null && !e.full) {
        const open2 = (f) => openStepViewer({ ...e, ...f, full: true }, v, ic, inLabel);
        host2.fullOf(e).then(open2, () => open2({ fullFailed: true }));
        return;
      }
      const status2 = e.live ? "Running \xB7 " + e.secs : e.unfinished ? "No result" : e.ok ? "Done \xB7 " + e.secs : e.ok === null ? "Exit unknown \xB7 " + e.secs : "Failed \xB7 " + e.secs;
      const { body, show: open } = host2.panel(v + " " + e.arg, {
        cls: "full",
        sub: e.name + " \xB7 " + status2,
        label: v + " " + e.arg
      });
      body.classList.add("viewer-b");
      renderFullTool(body, e, inLabel, host2.SESS[e.sid ?? ""]?.state === "wait");
      open();
    }
    const THUMB_W = 200, THUMB_H = 160;
    const IMAGE_KIND = {
      "image/png": "PNG",
      "image/jpeg": "JPEG",
      "image/gif": "GIF",
      "image/webp": "WebP"
    };
    const sizeText = (n) => n >= 1048576 ? (n / 1048576).toFixed(1) + " MB" : n >= 1024 ? Math.round(n / 1024) + " KB" : n + " bytes";
    function attachmentSnapshot(e) {
      const images = (e.img ?? []).map((a, i) => {
        const k = a.w != null && a.h != null && a.w > 0 && a.h > 0 ? Math.min(1, THUMB_W / a.w, THUMB_H / a.h) : null;
        return {
          unavailable: !!a.na,
          url: "/api/attachment?sid=" + host2.enc(e.sid ?? "") + "&o=" + host2.enc(a.o) + "&b=" + host2.enc(a.b) + "&v=" + host2.enc(a.v ?? ""),
          label: "Attached image " + (i + 1) + " (" + (IMAGE_KIND[a.type ?? ""] ?? "image") + ", " + sizeText(a.size ?? 0) + ")",
          width: k ? Math.max(1, Math.round(a.w * k)) : void 0,
          height: k ? Math.max(1, Math.round(a.h * k)) : void 0
        };
      });
      return images;
    }
    function openImage(url, label, from) {
      const image2 = createImageViewer(url, label, {
        opened(d) {
          viewerEl = d;
          document.documentElement.classList.add("viewer-open");
          try {
            history.pushState({ ...host2.navigation.route, sheet: 1 }, "");
          } catch {
          }
        },
        closed(d) {
          host2.dialogs.delete(d);
          if (host2.disposed) return;
          document.documentElement.classList.remove("viewer-open");
          if (viewerEl === d) {
            viewerEl = null;
            if (history.state?.sheet) {
              skipPop = true;
              history.back();
            }
          }
          if (host2.LIVE.pending) host2.refresh();
          (from.isConnected ? from : [...document.querySelectorAll("button.attach")].find(
            (x) => x.querySelector("img")?.getAttribute("src") === url
          ))?.focus();
        }
      });
      host2.dialogs.set(image2.dialog, image2);
      image2.show();
    }
    function openScript(e) {
      const done = (fields) => openStepViewer({ ...e, ...fields, full: true }, "View script", "run", "Script");
      host2.api("/api/entry?sid=" + host2.enc(e.sid ?? "") + "&slot=" + e.slot + "&as=script").then((value) => {
        const result = object2(value);
        done({
          scriptText: optional(result.text, text),
          scriptTruncated: result.truncated === true
        });
      }).catch(() => done({ scriptFailed: true }));
    }
    function childSnapshot(h2, c) {
      const calls = host2.countOf(c, "calls"), holder = host2.HOLDS.get(h2.id);
      return {
        kind: "child",
        handoff: h2.id,
        id: c.id,
        turn: host2.STARTS.get(h2.id)?.id,
        name: c.name,
        state: c.state,
        stateLabel: STATE[c.state] ?? c.state,
        meta: [
          host2.kindText(c),
          shortModel(c.model),
          host2.dur(c.start, c.state === "work" ? null : c.last),
          (calls ?? "\u2014") + (calls === 1 ? " step" : " steps"),
          host2.costText(host2.costForSession(c.id, true))
        ].join(" \xB7 "),
        mark: host2.harnessSnapshot(c.harness),
        brief: preview(h2.brief ?? ""),
        result: h2.result || void 0,
        failed: h2.status === "err",
        activity: !h2.result && c.state === "work" && c.activity ? [host2.verbNow(c.activity[0]), c.activity[1]] : void 0,
        trace: holder?.id,
        traceLabel: holder ? "Open run view for " + host2.nameOf(h2.from) : void 0,
        chevron: I.chev
      };
    }
    const callsTip = (s) => {
      const parts = Object.entries(s.tool_calls ?? {}).sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).map(([name, n]) => name + " " + n), failed = host2.countOf(s, "errors");
      if (failed) parts.push(failed + " failed");
      return parts.join(" \xB7 ");
    };
    const timeTip = (s, h2, finished) => "Started " + host2.clock(s.start) + " \xB7 last activity " + host2.clock(s.last) + (finished ? " \xB7 finished " + host2.clock(h2?.done ?? s.last) : "");
    function costTip(cost2) {
      const groups = Object.entries(cost2.by_model ?? {}).map(([id, m]) => {
        const k = m.usd_by_kind ?? {}, kinds2 = [
          ["Input", k.input],
          ["Output", k.output],
          ["Cache read", k.cache_read],
          ["Cache write", (Number(k.cache_write_5m) || 0) + (Number(k.cache_write_1h) || 0)]
        ];
        if (Number(k.web_search) >= 5e-3) kinds2.push(["Web search", k.web_search]);
        return {
          id,
          text: kinds2.map(([label, usd]) => label + " " + host2.asMoney(Number(usd) || 0)).join(" \xB7 ")
        };
      });
      return (groups.length > 1 ? groups.map((g) => g.id + ": " + g.text).join("; ") : groups.map((g) => g.text).join("")) + (groups.length ? ". " : "") + host2.COST_TIP;
    }
    function footerSnapshot(s, h2) {
      const done = s.state === "done" || s.state === "err", finished = h2 ? done || h2.status === "done" || h2.status === "err" : done;
      const state2 = h2 ? finished ? s.state === "err" || h2.status === "err" ? "err" : "done" : "work" : s.state;
      const cost2 = host2.costForSession(s.id), priced2 = cost2.usd != null && !host2.costMissing(cost2).length && cost2.usd >= 5e-3, items = [];
      if (!h2 || !finished)
        items.push({
          kind: "calls",
          text: host2.callsText(host2.countOf(s, "calls")),
          tip: callsTip(s)
        });
      items.push({
        kind: "time",
        text: host2.dur(s.start, state2 === "work" ? null : s.last),
        tip: timeTip(s, h2, finished)
      });
      if (priced2) items.push({ kind: "cost", text: host2.asMoney(cost2.usd ?? 0), tip: costTip(cost2) });
      return {
        state: state2,
        stateLabel: STATE[state2] ?? state2,
        text: h2 && finished ? "Returned to " + host2.nameOf(h2.from) + " \xB7 " + STATE[state2] : STATE[state2],
        items,
        parent: h2 && finished ? { id: h2.from, turn: host2.HOLDS.get(h2.id)?.id, name: host2.nameOf(h2.from) } : void 0
      };
    }
    return {
      get viewerEl() {
        return viewerEl;
      },
      set viewerEl(value) {
        viewerEl = value;
      },
      get skipPop() {
        return skipPop;
      },
      set skipPop(value) {
        skipPop = value;
      },
      openStepViewer,
      openScript,
      openImage,
      attachmentSnapshot,
      childSnapshot,
      footerSnapshot
    };
  }

  // src/app/transcriptRevalidation.ts
  function createTranscriptRevalidation(host2) {
    function shrank(a, b) {
      const [s0, b0] = String(a).split(".").map(Number), [s1, b1] = String(b).split(".").map(Number);
      return s1 < s0 || b1 < b0;
    }
    function revalidate(r) {
      const sid = r.id, m = host2.TXM[sid], moved = m && m.to >= m.total && m.tok != null && host2.transportOwner.TOK[sid] != null && m.tok !== host2.transportOwner.TOK[sid];
      const job = moved ? shrank(m.tok, host2.transportOwner.TOK[sid]) ? host2.reload(sid) : host2.tail(sid) : null, work = job;
      if (work)
        work.then(
          () => {
            if (host2.navigation.route === r && host2.navigation.rendered === r) host2.refresh(null);
          },
          () => {
          }
        );
    }
    return { revalidate, shrank };
  }

  // src/app/transcriptView.ts
  function createTranscriptView(host2) {
    const thoughtText = (e) => String(e.text ?? "").trim();
    const isPendingThought = (e, entries = [], i = 0, sid = "") => !!(e.pending || e.status === "thinking") || Array.isArray(entries) && e.k === "think" && !thoughtText(e) && i === entries.length - 1 && host2.SESS[sid]?.state === "work";
    const signalLabel = (e) => {
      const s = e.signal ?? {}, tag = s.tag ?? "", kind = s.kind;
      if (kind === "compact")
        return "Context compacted" + (s.value != null ? " \xB7 " + compactCount(s.value) + " tokens before" : "");
      if (kind === "interrupt") return "Interrupted by you";
      if (kind === "denial") return (s.tool ?? "Tool call") + " denied";
      if (kind === "hook")
        return "Hook " + (tag.includes("block") ? "blocked" : tag || "ran") + (s.tool ? " " + s.tool : "");
      const name = {
        model: "Model",
        effort: "Effort",
        approval: "Approval policy",
        sandbox: "Sandbox",
        permission: "Permission mode"
      }[kind] ?? "Signal";
      return name + (s.previous ? " " + s.previous + " \u2192 " : " \u2192 ") + tag;
    };
    const isMaskedThought = (e, entries = [], i = 0, sid = "") => e.k === "think" && !isPendingThought(e, entries, i, sid) && !thoughtText(e);
    function thoughtSeconds(e) {
      if (typeof e.secs === "number" && Number.isFinite(e.secs) && e.secs >= 0) return e.secs;
      if (typeof e?.secs === "string") {
        const m = /^(\d+(?:\.\d+)?)s?$/.exec(e.secs.trim());
        if (m) return Number(m[1]);
      }
      return null;
    }
    const thoughtLabel = (secs) => {
      const n = Number.isFinite(secs) ? Math.round(secs ?? 0) : 0;
      return n < 1 ? "Thinking" : "Thinking \xB7 " + (n >= 60 ? Math.floor(n / 60) + "m " + n % 60 + "s" : n + "s");
    };
    function transcriptEntries(entries, sid) {
      const out = [], codex = host2.SESS[sid]?.harness === "codex";
      const turns = codex ? host2.TURNS[sid] ?? [] : [], owners2 = codex ? new Map(turns.flatMap((t) => t.entries.map((e) => [e.key, t.id]))) : /* @__PURE__ */ new Map();
      const turnIds = codex ? new Set(turns.map((t) => t.id)) : /* @__PURE__ */ new Set();
      let chain = null, sawOwnedEntry = false, unresolvedOwner = false;
      for (let i = 0; i < entries.length; i++) {
        const e = entries[i], indexedOwner = codex ? owners2.get(e.key) : null;
        const turn = codex ? indexedOwner ?? (turnIds.has(e.turn) ? e.turn : null) : null;
        if (codex && indexedOwner == null && e.turn != null && !turnIds.has(e.turn)) {
          unresolvedOwner = true;
          chain = null;
        } else if (turn !== null) {
          sawOwnedEntry = true;
          unresolvedOwner = false;
        }
        if (e.k !== "think") {
          out.push(e);
          chain = null;
          continue;
        }
        const pending = isPendingThought(e, entries, i, sid);
        const row = { ...e, ...pending ? { pending: true } : {}, displaySecs: thoughtSeconds(e) };
        const text2 = thoughtText(row);
        if (!codex || pending || !text2 || unresolvedOwner || turn === null && sawOwnedEntry) {
          out.push(row);
          chain = null;
          continue;
        }
        if (chain && chain.turn === turn && (text2 === chain.text || text2.startsWith(chain.text + "\n") || text2.startsWith(chain.text + "\r\n"))) {
          const merged = { ...chain.row, text: text2, displaySecs: row.displaySecs };
          out[chain.index] = merged;
          chain = { index: chain.index, turn: chain.turn, row: merged, text: text2 };
        } else {
          out.push(row);
          chain = { index: out.length - 1, row, text: text2, turn };
        }
      }
      return out;
    }
    const RUN = ["run", "Ran", "ran", "command", "commands"], FIND = ["find", "Searched for", "searched", "time", "times"];
    const TOOLS = {
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
    const toolInfo = (name) => {
      if (TOOLS[name]) return TOOLS[name];
      const m = /^mcp__(.+?)__/.exec(name);
      if (m) {
        const srv = m[1].replace(/^claude_ai_/, "").replace(/_/g, " ");
        return ["ext", "Used " + srv, "used " + srv, "time", "times"];
      }
      return ["run", name, name, "step", "steps"];
    };
    const verb = (name) => toolInfo(name).slice(0, 2);
    const NOW_VERB = {
      Ran: "Running",
      Read: "Reading",
      Edited: "Editing",
      Patched: "Patching",
      Wrote: "Writing",
      "Searched for": "Searching for"
    };
    const verbNow = (name) => NOW_VERB[verb(name)[1]] ?? verb(name)[1];
    const isCmd = (name) => /^(Bash|shell|exec_command|local_shell)$/.test(name);
    const backgroundText = (bg) => {
      if (bg.state === "running") return "running " + bg.secs;
      if (bg.state === "unknown") return "no end recorded";
      const outcome = bg.state === "failed" ? "failed" : bg.state === "killed" ? "stopped" : bg.exit != null ? "exit " + bg.exit : "done";
      return outcome + (bg.secs ? " \xB7 " + bg.secs : "");
    };
    const elapsedMs = (text2) => {
      const m = /^(?:(\d+)m )?(\d+(?:\.\d+)?)s$/.exec(text2 ?? "");
      return m ? (Number(m[1] ?? 0) * 60 + Number(m[2])) * 1e3 : null;
    };
    function transcriptSnapshot(sid, opts = {}) {
      const entries = transcriptEntries(host2.TX[sid] ?? [], sid), blocks2 = [];
      let tx = [];
      const currentTurn = { value: null };
      let loose = 0;
      const hit = (text2) => !host2.find || String(text2 ?? "").toLowerCase().includes(host2.find);
      const filtering = !!host2.find || !host2.show.messages || !host2.show.tools || !host2.show.thinking;
      const entryKey = (e) => e.slot != null ? sid + "#slot:" + e.slot : e.key;
      const keyed = (view, e) => ({
        ...view,
        key: e.key,
        entryKey: e.key ? entryKey(e) : void 0
      });
      const endedCalls = new Set(entries.flatMap((e) => e.k === "bgend" ? [e.call] : []));
      let run = [], masked = false;
      const flush = () => {
        if (!run.length) return;
        const summary = [], ends = run.filter((r) => r.end), counts = /* @__PURE__ */ new Map();
        if (run.length > 1 && !filtering) {
          for (const r of run.filter((r2) => !r2.end)) {
            const [, , p, one, many] = toolInfo(r.k ?? ""), c = counts.get(p) ?? { n: 0, one, many };
            c.n++;
            counts.set(p, c);
          }
          if (ends.length) {
            const outcomes = [
              [ends.filter((r) => r.state === "failed").length, "failed"],
              [ends.filter((r) => r.state === "killed").length, "stopped"]
            ].filter(([n]) => n).map(([n, word]) => n + " " + word);
            summary.push(
              { text: "Finished ", className: "long" },
              { text: ends.length + " background" },
              { text: " command" + (ends.length === 1 ? "" : "s"), className: "long" }
            );
            if (outcomes.length)
              summary.push({ text: " (" + outcomes.join(", ") + ")", className: "tt-counts" });
            for (const [p, c] of counts)
              summary.push(
                { text: ", " },
                { text: p + " ", className: p === "ran" ? "long" : void 0 },
                { text: c.n + " " + (c.n === 1 ? c.one : c.many) }
              );
          } else {
            const text2 = [...counts].map(([p, c]) => p + " " + c.n + " " + (c.n === 1 ? c.one : c.many)).join(", ");
            summary.push({ text: text2[0].toUpperCase() + text2.slice(1) });
          }
        }
        const failed = run.filter(
          (r) => !r.end && r.err && (!r.bg || !endedCalls.has(r.tid ?? ""))
        ).length;
        const live = run.some((r) => r.bg) ? run.filter((r) => r.live).sort((a, b) => (elapsedMs(b.secs) ?? 0) - (elapsedMs(a.secs) ?? 0))[0] : run.find((r) => r.live);
        tx.push({
          kind: "group",
          key: run[0].key ? "g:" + run[0].key : void 0,
          entryKey: run[0].view.entryKey ? "g:" + run[0].view.entryKey : void 0,
          entries: run.map((r) => r.view),
          lone: run.length === 1 && !filtering,
          summary,
          background: !!ends.length,
          failed,
          running: live?.secs,
          stack: host2.I.stack,
          chevron: host2.I.chev
        });
        run = [];
      };
      const firsts = new Map(
        (host2.TURNS[sid] ?? []).filter((t) => t.entries[0]?.key).map((t) => [t.entries[0].key, t])
      );
      const owner = opts.only ? new Map((host2.TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e.key, t.id]))) : null;
      const content = (values) => values.some((v) => v.kind !== "label" || v.className === "harness-note");
      const closeTurn = () => {
        flush();
        if (!currentTurn.value) {
          if (tx.length) blocks2.push({ kind: "loose", key: "loose:" + loose++, entries: tx });
          tx = [];
          return;
        }
        const { t, view } = currentTurn.value, end = host2.turnEnd(t), returned = t.entries.some((e) => e.k === "end" && /^Returned to /.test(e.text ?? ""));
        if (!filtering) {
          if (end?.st === "err" && !returned) view.error = end.text;
          if (t.out.length) view.trace = t.id;
        }
        view.entries = tx;
        if (!filtering || content(tx)) blocks2.push({ kind: "turn", turn: view });
        currentTurn.value = null;
        tx = [];
        masked = false;
      };
      const openTurn = (t) => {
        closeTurn();
        const h2 = t.start, view = {
          id: t.id,
          entries: [],
          traceIcon: host2.I.trace,
          stateLabel: host2.STATE.err
        };
        if (t.u || h2?.kind === "ask")
          view.label = "Your message" + (h2 ? " at " + host2.clock(h2.at) : "");
        else if (h2)
          view.header = {
            parts: host2.sentenceSnapshot(h2, sid, true).parts,
            mark: host2.SESS[h2.from] ? host2.harnessSnapshot(host2.SESS[h2.from].harness) : void 0,
            time: host2.clock(h2.at)
          };
        currentTurn.value = { t, view };
      };
      const toolStep = (e, v, ic, live) => {
        const bg = e.bg, bgRunning = bg?.state === "running", waiting = live && host2.SESS[sid]?.state === "wait";
        const waitingText = host2.SESS[sid]?.waiting_for?.includes("permission") ? "Waiting on permission" : "Waiting for your input";
        const status2 = bg ? null : waiting ? waitingText : live ? e.secs : e.unfinished ? "no result" : e.exit != null ? "exit " + e.exit + " \xB7 " + e.secs : e.ok ? e.secs : e.ok === null ? "exit unknown \xB7 " + e.secs : "failed \xB7 " + e.secs;
        const title = e.title ? String(e.title) : null, command = e.in ?? e.arg;
        const firstLine = typeof command === "string" ? command.split(/\r\n|\n|\r/).find((line) => line.trim()) : null;
        const bgSecs = bgRunning && bg.since != null ? host2.running(host2.NOW - bg.since) : bg?.secs;
        return {
          className: "step" + (bg ? (bg.state === "failed" ? " err" : "") + " background" + (bgRunning ? " background-running" : "") : live ? " live" : e.ok || e.ok === null ? "" : " err"),
          key: e.key,
          entryKey: e.key ? entryKey(e) : void 0,
          tid: e.tid,
          sid: live || bgRunning ? sid : void 0,
          since: live ? e.since : bgRunning ? bg.since : void 0,
          running: live || bgRunning,
          background: !!bg,
          waiting: host2.SESS[sid]?.state === "wait",
          named: !!title,
          prefix: title ? waiting ? waitingText + ": " : live ? "Running: " : e.ok === false ? "Failed: " : v + ": " : void 0,
          verb: waiting ? "Waiting" : live ? verbNow(e.name) : v,
          label: title ?? e.arg,
          tip: title && firstLine != null ? firstLine.slice(0, 200) : void 0,
          status: status2 == null ? null : String(status2),
          backgroundStatus: bg ? backgroundText({ ...bg, secs: bgSecs }) : void 0,
          icon: host2.I[ic],
          chevron: host2.I.chev,
          data: e
        };
      };
      for (const e of entries) {
        if (owner && opts.only && !opts.only.has(owner.get(e.key) ?? "")) continue;
        if (host2.isGap(e)) {
          closeTurn();
          if (!filtering)
            tx.push({
              kind: "label",
              className: "divider",
              text: e.k === "end" ? e.text ?? "" : ""
            });
          closeTurn();
          continue;
        }
        const first = firsts.get(e.key);
        if (first) openTurn(first);
        if (e.k === "think" && (!host2.show.thinking || host2.find)) continue;
        if (e.k === "signal") {
          const label = signalLabel(e);
          if (hit(label))
            tx.push(keyed({ kind: "label", className: "signal-marker", text: label }, e));
          continue;
        }
        if (e.k === "think" && isMaskedThought(e)) {
          if (!masked) {
            masked = true;
            tx.push(
              keyed({ kind: "thought", mode: "masked", label: "Thinking hidden by the harness" }, e)
            );
          }
          continue;
        }
        if (e.k === "bgend") {
          if (!host2.show.tools || !hit(e.label ?? "")) continue;
          const word = e.state === "failed" ? "failed" : e.state === "killed" ? "stopped" : "completed";
          const view = keyed(
            {
              kind: "background",
              failed: e.state === "failed",
              label: "Background command " + word + " \xB7 " + (e.label ?? ""),
              call: e.call,
              loaded: entries.some((entry) => entry.k === "tool" && entry.tid === e.call)
            },
            e
          );
          run.push({ view, end: true, state: e.state, key: e.key });
          continue;
        }
        if (e.k === "tool") {
          if (!host2.show.tools || !hit(
            e.name + " " + (e.title ?? "") + " " + e.arg + " " + (e.in ?? "") + " " + (e.out ?? "")
          ))
            continue;
          const [ic, v] = verb(e.name), view = keyed({ kind: "tool", step: toolStep(e, v, ic, !!e.live && !e.bg) }, e);
          if (e.live && !e.bg) run.push({ view, v, k: e.name, live: true, secs: e.secs, key: e.key });
          else if (e.bg) {
            const live = e.bg.state === "running";
            run.push({
              view,
              v,
              k: e.name,
              err: e.bg.state === "failed",
              bg: true,
              tid: e.tid,
              live,
              secs: live && e.bg.since != null ? host2.running(host2.NOW - e.bg.since) : e.bg.secs ?? e.secs,
              key: e.key
            });
          } else run.push({ view, v, k: e.name, err: e.ok === false, key: e.key });
          continue;
        }
        flush();
        if (e.k === "u" || e.k === "a") {
          if (!host2.show.messages || !hit(e.text)) continue;
          tx.push(
            keyed(
              {
                kind: "message",
                flavor: e.k === "u" ? "user" : "assistant",
                text: e.text,
                images: e.k === "u" ? host2.attachmentSnapshot(e) : void 0
              },
              e
            )
          );
        } else if (e.k === "think")
          tx.push(
            keyed(
              isPendingThought(e) ? { kind: "thought", mode: "pending" } : {
                kind: "thought",
                mode: "readable",
                label: thoughtLabel(e.displaySecs),
                text: thoughtText(e)
              },
              e
            )
          );
        else if (e.k === "harness") {
          if (!host2.show.messages || host2.find) continue;
          tx.push(
            keyed(
              {
                kind: "label",
                className: "harness-note",
                text: "Harness text added before the prompt (" + e.label + ")"
              },
              e
            )
          );
        } else if (e.k === "end") {
          if (!filtering)
            tx.push(keyed({ kind: "label", className: "divider", text: e.text ?? "" }, e));
        } else if (e.k === "h") {
          const h2 = host2.HID.get(e.id);
          if (!h2 || !hit(h2.brief + " " + (h2.result ?? ""))) continue;
          if (h2.kind === "ask") {
            if (!host2.show.messages) continue;
            tx.push(
              keyed(
                {
                  kind: "message",
                  flavor: "user",
                  text: h2.brief ?? "",
                  images: host2.attachmentSnapshot(e)
                },
                e
              )
            );
            if (currentTurn.value?.t.start === h2)
              tx.push({ kind: "label", className: "msg-tm", text: host2.clock(h2.at) });
            continue;
          }
          if (currentTurn.value && currentTurn.value.t.start === h2) {
            if (host2.show.messages)
              tx.push(
                keyed({ kind: "message", flavor: "incoming", text: h2.brief ?? "", handoff: h2.id }, e)
              );
            continue;
          }
          if (h2.kind === "spawn" && h2.from === sid && host2.SESS[h2.to]) {
            if (host2.show.tools) tx.push(keyed(host2.childSnapshot(h2, host2.SESS[h2.to]), e));
            continue;
          }
          if (host2.find && h2.kind === "move" || !host2.show.messages && h2.kind !== "move")
            continue;
          const sentence = host2.sentenceSnapshot(h2, sid, true);
          tx.push(
            keyed(
              {
                kind: "event",
                handoff: h2.id,
                className: "event" + (h2.kind === "toyou" && h2.status === "wait" ? " waiting" : "") + (h2.kind === "move" ? " move" : ""),
                icon: sentence.icon,
                parts: sentence.parts,
                time: host2.clock(h2.at),
                brief: preview(h2.brief ?? ""),
                result: h2.result || void 0,
                resultLabel: h2.kind === "relay" ? "Reply: " : "Returned: ",
                answers: host2.answersOf(h2) ?? void 0,
                waiting: h2.kind === "toyou" && h2.status === "wait",
                stateLabel: host2.STATE.wait
              },
              e
            )
          );
        }
      }
      closeTurn();
      const range = host2.TXM[sid], s = host2.SESS[sid], origin = host2.originHandoff(sid);
      return {
        id: sid,
        name: s.name,
        blocks: blocks2,
        order: (host2.TURNS[sid] ?? []).map((t) => t.id),
        dirty: opts.only,
        before: range?.from > 0 ? host2.pagerSnapshot(sid, "before") : void 0,
        after: range && range.to < range.total ? host2.pagerSnapshot(sid, "after") : void 0,
        started: !range?.from && !filtering ? {
          lead: "Started " + host2.clock(s.start) + " on\xA0",
          machine: host2.MACHINE[s.movedFrom ?? s.machine]
        } : void 0,
        empty: !opts.only && !blocks2.some((b) => content(b.kind === "turn" ? b.turn.entries : b.entries)) ? host2.find ? "Nothing matches \u201C" + host2.find + "\u201D." : "Nothing to show with these filters." : void 0,
        footer: host2.showsFooter(s, origin) ? host2.footerSnapshot(s, origin) : void 0
      };
    }
    return { transcriptEntries, transcriptSnapshot, verb, verbNow };
  }

  // src/app/transport.ts
  function createTransport(host2) {
    let serverNow = 0, fetchedAt = 0;
    let TOK = {};
    const enc = encodeURIComponent;
    const safePath2 = (href) => typeof href === "string" && href.startsWith("/") && !href.startsWith("//") && !href.includes("\\") && !/[\u0000-\u001f\u007f-\u009f]/.test(href) && href.length <= 512;
    const accountOf = parseAccount;
    let embedWarned = false;
    function embeddedAccount() {
      let source;
      try {
        const config = Reflect.get(window, "semonEmbed");
        source = config && typeof config === "object" && "account" in config ? config.account : void 0;
      } catch {
        source = void 0;
      }
      const account = accountOf(source);
      if (source != null && !account && !embedWarned) {
        embedWarned = true;
        console.warn("semon: window.semonEmbed.account was rejected (see the account menu rules)");
      }
      return account;
    }
    async function api(path, signal, unchanged = false) {
      if (host2.disposed) throw new DOMException("Viewer destroyed", "AbortError");
      const controller = host2.scope.request(), release = () => host2.scope.releaseRequest(controller);
      try {
        const response = await requestJson(
          path,
          signal ? AbortSignal.any([signal, controller.signal]) : controller.signal,
          unchanged
        );
        if (host2.disposed) throw new DOMException("Viewer destroyed", "AbortError");
        return response;
      } finally {
        release();
      }
    }
    function tick() {
      host2.NOW = serverNow + (Date.now() - fetchedAt);
      for (const s of Object.values(host2.SESS))
        if (s.activity && s.activity[3] != null)
          s.activity[2] = Math.floor((host2.NOW - s.activity[3]) / 1e3);
    }
    function adopt(value) {
      const m = host2.modelStore.adopt(value);
      host2.domain.invalidate();
      serverNow = m.now;
      fetchedAt = Date.now();
      TOK = m.tx == null ? {} : dictionary(m.tx, text);
      host2.transcripts.marks = TOK;
      const admin = m.admin == null ? null : object2(m.admin);
      host2.ADMIN = admin && typeof admin.href === "string" && safePath2(admin.href) ? { href: admin.href, label: text(admin.label) } : null;
      host2.ACCOUNT = accountOf(m.account) ?? accountOf(host2.viewerHost?.account) ?? embeddedAccount();
      host2.viewerHost?.modelAccount?.(host2.ACCOUNT);
      const nav = m.nav == null ? null : object2(m.nav);
      host2.NAV_MACHINES = host2.viewerHost?.machinesPath ?? (nav && typeof nav.machines === "string" && safePath2(nav.machines) ? nav.machines : null);
      tick();
      return m;
    }
    const spread = (sid) => host2.transcripts.spread(sid);
    const txEntry = (e) => e.k === "end" && e.ret ? {
      k: "end",
      text: "Returned to " + host2.nameOf(e.ret.to) + (e.ret.failed ? " \xB7 failed" : "") + (e.ret.at != null ? " \xB7 " + host2.clock(e.ret.at) : ""),
      turn: e.turn
    } : e;
    const fetchTx = (sid, q = "", where = void 0, signal = void 0, onPage = void 0) => host2.transcripts.fetch(sid, q, where, signal, onPage);
    function load(r, signal = void 0) {
      if (r.v === "analytics")
        return host2.fetchAnalytics().then(() => {
          host2.scheduleAnalytics();
        });
      if (r.v !== "session" || !host2.SESS[r.id]) return null;
      const t = r.turn ? host2.TURN.get(r.turn) : null, deep = t && t.sid === r.id && !t.entries.length;
      if (host2.TX[r.id] && !deep) return null;
      return fetchTx(r.id, deep ? "turn=" + enc(t.id) : "", void 0, signal);
    }
    return {
      api,
      txEntry,
      get TOK() {
        return TOK;
      },
      set TOK(value) {
        TOK = value;
      },
      fetchTx,
      enc,
      adopt,
      load,
      spread,
      tick,
      get fetchedAt() {
        return fetchedAt;
      },
      set fetchedAt(value) {
        fetchedAt = value;
      },
      accountOf
    };
  }

  // src/navigation/scroll.ts
  function createScrollTransactions(host2) {
    let anchor = null, disposed = false;
    const frames = /* @__PURE__ */ new Set();
    const frame = (fn) => {
      const id = requestAnimationFrame(() => {
        frames.delete(id);
        if (!disposed) fn();
      });
      frames.add(id);
    };
    const scroller = () => host2.phone() ? document.scrollingElement ?? document.documentElement : host2.main();
    const ANCHORS = "[data-e], .turn, .hop, .ib, .nrow, .sec-h, .ph, .divider, .analytics-metric, .analytics-panel, .facet-filters, .groupby, .find, .empty";
    const HOSTS = "[data-e], [data-h], [data-id], [data-sid], [data-go], [data-turn], [data-g], [data-m]";
    const FOCUSABLE = "button, input, [tabindex], a[href]";
    const stateKey = (n) => n.dataset.entryKey ?? n.dataset.e;
    const identOf = (n) => {
      const d = n.dataset, keys = [stateKey(n), d.turn, d.h, d.id, d.m, d.sid, d.go, d.g];
      return [
        n.classList[0],
        ...keys,
        keys.some((x) => x != null) ? "" : n.firstChild?.nodeType === 3 ? n.firstChild.textContent : ""
      ].map((x) => x ?? "").join("|");
    };
    function anchors(fn) {
      const seen = /* @__PURE__ */ new Map();
      for (const n of host2.page().querySelectorAll(ANCHORS)) {
        const id = identOf(n), k = seen.get(id) ?? 0;
        seen.set(id, k + 1);
        if (fn(n, id + "#" + k)) return n;
      }
      return null;
    }
    const opener = (n) => n.classList.contains("step") ? n.querySelector(":scope > button") : n.classList.contains("tgroup") ? n.querySelector(":scope > .tsum") : null;
    function capture() {
      const sc = scroller(), line = host2.edge();
      const st = {
        top: sc.scrollTop,
        bottom: sc.scrollHeight - sc.scrollTop - sc.clientHeight <= 80,
        anchor: null,
        open: /* @__PURE__ */ new Set(),
        groups: /* @__PURE__ */ new Set(),
        groupMembers: /* @__PURE__ */ new Set(),
        focus: null,
        drawer: document.body.classList.contains("drawer-open")
      };
      const kept = anchor;
      const still = kept && kept.route === host2.rendered() && Math.abs(sc.scrollTop - kept.top) < 1 ? anchors((n, id) => id === kept.id) : null;
      if (still && kept && Math.abs(still.getBoundingClientRect().top - line - kept.off) <= 1)
        st.anchor = { id: kept.id, off: kept.off };
      else {
        const turnBounds = /* @__PURE__ */ new Map();
        anchors((n, id) => {
          const turn = n.closest(".turn");
          if (turn) {
            let bounds = turnBounds.get(turn);
            if (!bounds) {
              bounds = turn.getBoundingClientRect();
              turnBounds.set(turn, bounds);
            }
            if (bounds.bottom <= line || bounds.top >= (host2.phone() ? innerHeight : sc.getBoundingClientRect().bottom))
              return false;
          }
          if (n.querySelector(ANCHORS)) return false;
          const b = n.getBoundingClientRect();
          if (!b.height || b.bottom <= line) return false;
          st.anchor = { id, off: b.top - line };
          return true;
        });
      }
      for (const n of host2.page().querySelectorAll("[data-e]")) {
        if (n.classList.contains("tgroup")) st.groups.add(stateKey(n));
        if (opener(n)?.getAttribute("aria-expanded") === "true")
          for (const step of n.querySelectorAll(".step[data-e]"))
            st.groupMembers.add(stateKey(step));
        if (opener(n)?.getAttribute("aria-expanded") === "true" || n.classList.contains("event") && n.querySelector(":scope > .ev-text.open"))
          st.open.add(stateKey(n));
      }
      for (const n of host2.page().querySelectorAll(".hop"))
        if (n.querySelector(".brief.open")) st.open.add("hop:" + identOf(n));
      const a = document.activeElement;
      if (a instanceof HTMLElement && a !== document.body && !a.closest("dialog")) {
        const owner = a.id ? null : a.closest(HOSTS), sel = owner && owner !== a ? a.tagName.toLowerCase() + [...a.classList].map((c) => "." + CSS.escape(c)).join("") : null;
        st.focus = {
          id: a.id || null,
          host: owner ? identOf(owner) : null,
          sel,
          i: sel && owner ? [...owner.querySelectorAll(sel)].indexOf(a) : 0,
          range: null,
          label: a.getAttribute("aria-label"),
          at: [...host2.page().querySelectorAll(FOCUSABLE)].indexOf(a),
          of: host2.page().querySelectorAll(FOCUSABLE).length,
          foot: a.closest("#page > .session-foot") ? a.closest("[data-foot]")?.dataset.foot ?? null : null
        };
        try {
          if ((a instanceof HTMLInputElement || a instanceof HTMLTextAreaElement) && typeof a.selectionStart === "number" && a.selectionEnd !== null)
            st.focus.range = [a.selectionStart, a.selectionEnd];
        } catch {
        }
      }
      return st;
    }
    function restore(st, pin = false) {
      const all = (sel) => [...host2.page().querySelectorAll(sel)], r0 = host2.rendered(), revision = host2.revision();
      measureSessionScreen(host2.page());
      measureTraceScreen(host2.page());
      for (const n of all(".tgroup[data-e]")) {
        const want = st.groups.has(stateKey(n)) ? st.open.has(stateKey(n)) : [...n.querySelectorAll(".step[data-e]")].some(
          (x) => st.open.has(stateKey(x)) || st.groupMembers.has(stateKey(x))
        );
        if (want && opener(n).getAttribute("aria-expanded") === "false") opener(n).click();
      }
      for (const n of all(".step[data-e]"))
        if (st.open.has(stateKey(n)) && opener(n)?.getAttribute("aria-expanded") === "false")
          opener(n).click();
      for (const n of all(".event[data-e]"))
        if (st.open.has(stateKey(n)) && !n.querySelector(":scope > .ev-text.open")) {
          const m = n.querySelector(":scope > .ev-more");
          m.click();
        }
      for (const n of all(".hop"))
        if (st.open.has("hop:" + identOf(n)) && !n.querySelector(".brief.open")) {
          const m = n.querySelector(".body > .more");
          m.click();
        }
      if (st.drawer) host2.restoreDrawer();
      if (st.focus) {
        let n = st.focus.id ? document.getElementById(st.focus.id) : null;
        if (!n && st.focus.host) {
          const owner = [...document.querySelectorAll(HOSTS)].find(
            (x) => identOf(x) === st.focus.host
          );
          n = owner && st.focus.sel ? owner.querySelectorAll(st.focus.sel)[st.focus.i] : owner ?? null;
        }
        if (!n && st.focus.label)
          n = [
            ...document.querySelectorAll("#page [aria-label], #topbar [aria-label]")
          ].find((x) => x.getAttribute("aria-label") === st.focus.label) ?? null;
        if (!n && st.focus.foot) {
          const f = host2.page().querySelector(":scope > .session-foot");
          n = f?.querySelector('[data-foot="' + st.focus.foot + '"]') ?? f?.querySelector('[data-foot="time"]') ?? null;
        }
        if (!n && !st.focus.host && st.focus.at >= 0 && host2.page().querySelectorAll(FOCUSABLE).length === st.focus.of)
          n = host2.page().querySelectorAll(FOCUSABLE)[st.focus.at];
        if (n && n !== document.activeElement) {
          n.focus({ preventScroll: true });
          if (st.focus.range)
            try {
              if (n instanceof HTMLInputElement || n instanceof HTMLTextAreaElement)
                n.setSelectionRange(...st.focus.range);
            } catch {
            }
        }
      }
      const pagingTurns = st.paging ? all(".turn") : [];
      if (st.paging) {
        for (const turn of pagingTurns) revealMeasuredTurn(turn, true);
        const heights = pagingTurns.map((turn) => turn.getBoundingClientRect().height);
        pagingTurns.forEach((turn, i) => {
          setGeometry(turn, "intrinsicHeight", Math.ceil(heights[i]));
        });
      }
      const place = (first) => {
        const sc = scroller();
        host2.programmatic(() => {
          if (pin) sc.scrollTop = sc.scrollHeight;
          else {
            const paging = st.paging;
            let found = null;
            if (paging?.anchor)
              found = all(".turns [data-e][data-entry-key]:not(.tgroup)").find(
                (n) => n.dataset.entryKey === paging.anchor.key
              );
            else if (!paging && st.anchor) found = anchors((n, id) => id === st.anchor.id);
            if (found) {
              const line = paging ? host2.phone() ? 0 : sc.getBoundingClientRect().top : host2.edge();
              const d = found.getBoundingClientRect().top - line - (paging ? paging.anchor.off : st.anchor.off), want = sc.scrollTop + d, room = sc.scrollHeight - sc.clientHeight;
              if (want > room + 0.5) {
                const page = host2.page();
                setGeometry(
                  page,
                  "paddingBottom",
                  parseFloat(getComputedStyle(page).paddingBottom) + Math.ceil(want - room)
                );
              }
              if (d) sc.scrollTop = want;
              anchor = paging || !st.anchor ? null : { ...st.anchor, route: r0, top: sc.scrollTop };
            } else if (first)
              sc.scrollTop = st.top + (paging?.before ? sc.scrollHeight - paging.height : 0);
          }
        }, false);
        if (pin) anchor = null;
        host2.sync();
      };
      place(true);
      const placedTop = scroller().scrollTop;
      frame(
        () => frame(() => {
          for (const turn of pagingTurns) revealMeasuredTurn(turn, false);
          if (host2.rendered() === r0 && !host2.sheet() && host2.revision() === revision && Math.abs(scroller().scrollTop - placedTop) < 1)
            place(false);
        })
      );
    }
    return {
      capture,
      restore,
      opener,
      stateKey,
      identOf,
      get anchor() {
        return anchor;
      },
      set anchor(value) {
        anchor = value;
      },
      destroy() {
        if (disposed) return;
        disposed = true;
        for (const id of frames) cancelAnimationFrame(id);
        frames.clear();
        anchor = null;
      }
    };
  }

  // src/app/viewport.ts
  function createViewport(host2) {
    const scroller = () => host2.phone.matches ? document.documentElement : host2.$("#main");
    const edge = () => host2.$("#topbar").getBoundingClientRect().bottom;
    let openingEndUntil = 0;
    let openingEndTimer;
    let openingEndObserver = null;
    function stopOpeningEndPin() {
      const wasPinned = !!openingEndUntil;
      openingEndUntil = 0;
      host2.scope.clearTimeout(openingEndTimer);
      openingEndTimer = void 0;
      openingEndObserver?.disconnect();
      openingEndObserver = null;
      if (wasPinned) host2.queuePagerObservers();
    }
    function pinOpeningEnd() {
      if (host2.navigation.route.v !== "session" || performance.now() >= openingEndUntil) {
        stopOpeningEndPin();
        return;
      }
      const sc = scroller();
      host2.scrollProgrammatically(() => {
        sc.scrollTop = sc.scrollHeight;
      });
      scrollController.anchor = null;
      syncJump();
      host2.saveHistoryScroll();
    }
    function startOpeningEndPin() {
      host2.resetPagerInput();
      stopOpeningEndPin();
      if (host2.navigation.route.v !== "session" || location.hash) return;
      openingEndUntil = performance.now() + 2e3;
      host2.disconnectPagerObservers();
      const turns = host2.$("#page section[aria-label='Transcript'] .turns");
      if (turns) {
        openingEndObserver = new ResizeObserver(pinOpeningEnd);
        openingEndObserver.observe(turns);
      }
      pinOpeningEnd();
      openingEndTimer = host2.scope.timeout(stopOpeningEndPin, 2e3);
    }
    const scrollController = createScrollTransactions({
      page: () => host2.$("#page"),
      main: () => host2.$("#main"),
      phone: () => host2.phone.matches,
      edge,
      rendered: () => host2.navigation.rendered,
      sheet: () => !!host2.viewerEl,
      revision: () => host2.scrollRevision,
      programmatic: host2.scrollProgrammatically,
      sync: host2.syncBarLine,
      restoreDrawer: () => host2.shellChrome?.restoreDrawer()
    });
    const { capture, restore, opener, stateKey, identOf } = scrollController;
    function patchSession(dirty) {
      host2.resetPagerInput();
      host2.holdProgrammaticScroll();
      host2.tick();
      const s = host2.SESS["id" in host2.navigation.route ? host2.navigation.route.id : ""], box = host2.$("#page .turns");
      const keys = () => new Set(
        [
          ...host2.$("#page").querySelectorAll(
            ".turns :is(.msg, .bubble, .step, .event, .child-card, .thought, .think-pending)[data-e]"
          )
        ].map((n2) => n2.dataset.e)
      );
      const before = keys();
      const whole = !dirty || box.querySelector(":scope > p.empty") || [...box.querySelectorAll(":scope > .turn")].some(
        (b) => !host2.TURN.has(b.dataset.turn ?? "")
      );
      host2.renderSession(
        host2.$("#page"),
        "id" in host2.navigation.route ? host2.navigation.route.id : "",
        whole ? {} : { only: dirty }
      );
      host2.drawSessionBar();
      for (const pager of box.querySelectorAll("[data-pager-where]"))
        host2.paintPager(pager);
      host2.renderNav();
      host2.renderLanes();
      host2.ticker();
      let n = 0;
      for (const k of keys()) if (!before.has(k)) n++;
      host2.queuePagerObservers();
      return n;
    }
    function scrollMetrics() {
      if (host2.phone.matches)
        return {
          top: window.scrollY,
          height: document.documentElement.scrollHeight,
          viewport: window.innerHeight,
          gap: Math.max(
            0,
            document.documentElement.scrollHeight - window.innerHeight - window.scrollY
          )
        };
      const m = host2.$("#main");
      return {
        top: m.scrollTop,
        height: m.scrollHeight,
        viewport: m.clientHeight,
        gap: Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop)
      };
    }
    function scrollToEnd(behavior = "smooth") {
      host2.scrollProgrammatically(() => {
        if (host2.phone.matches)
          window.scrollTo({ top: document.documentElement.scrollHeight, behavior });
        else {
          const m = host2.$("#main");
          m.scrollTo({ top: m.scrollHeight, behavior });
        }
      });
    }
    let jumpBusy = false;
    function syncJump() {
      if (host2.navigation.route.v !== "session") {
        host2.LIVE.fresh = 0;
        return;
      }
      const { gap } = scrollMetrics(), newer = host2.TXM["id" in host2.navigation.route ? host2.navigation.route.id : ""]?.newer ?? 0;
      if (gap <= 80) host2.LIVE.fresh = 0;
      updateSessionJump(host2.$("#page"), gap > 80 || !!newer, host2.LIVE.fresh + newer, jumpBusy);
    }
    function clearNewEntries() {
      host2.LIVE.fresh = 0;
      updateSessionJump(host2.$("#page"), false, 0, jumpBusy);
    }
    function jumpToLatest() {
      const sid = "id" in host2.navigation.route ? host2.navigation.route.id : "", r = host2.navigation.route, m = host2.TXM[sid];
      if (m && m.to < m.total) {
        jumpBusy = true;
        syncJump();
        host2.fetchTx(sid, "").then(() => {
          if (host2.navigation.route === r) host2.goSession(sid);
        }).catch(() => {
        }).finally(() => {
          jumpBusy = false;
          syncJump();
        });
      } else scrollToEnd("smooth");
    }
    if (!host2.SIDEBAR_ONLY) {
      host2.scope.listen(window, "scroll", syncJump, { passive: true });
      host2.scope.listen(host2.$("#main"), "scroll", syncJump, { passive: true });
    }
    const cancelOpeningEndPin = () => {
      if (openingEndUntil) stopOpeningEndPin();
    };
    if (!host2.SIDEBAR_ONLY) {
      host2.scope.listen(window, "wheel", cancelOpeningEndPin, { passive: true });
      host2.scope.listen(window, "touchmove", cancelOpeningEndPin, { passive: true });
      host2.scope.listen(window, "pointerdown", cancelOpeningEndPin, { passive: true });
    }
    const transcriptInput = (e) => e.target instanceof Element && !e.target.closest?.("#sidebar, dialog, input, textarea, select, [contenteditable='true']") && (host2.phone.matches || host2.$("#main").contains(e.target));
    if (!host2.SIDEBAR_ONLY) {
      const input = (e) => {
        if (transcriptInput(e)) host2.readerScrollInput();
      };
      host2.scope.listen(window, "wheel", input, { passive: true });
      host2.scope.listen(window, "touchmove", input, { passive: true });
      const scroll = () => {
        if (host2.programmaticScrollPending) host2.holdProgrammaticScroll();
        else if (!openingEndUntil) host2.readerScrollInput();
      };
      host2.scope.listen(
        window,
        "scroll",
        () => {
          if (host2.phone.matches) scroll();
        },
        { passive: true }
      );
      host2.scope.listen(
        host2.$("#main"),
        "scroll",
        () => {
          if (!host2.phone.matches) scroll();
        },
        { passive: true }
      );
    }
    if (!host2.SIDEBAR_ONLY)
      host2.scope.listen(document, "keydown", (e) => {
        if (!e.defaultPrevented && (transcriptInput(e) || e.target === document.body || e.target === document.documentElement) && ["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key))
          host2.readerScrollInput();
      });
    return {
      scrollController,
      stopOpeningEndPin,
      get openingEndUntil() {
        return openingEndUntil;
      },
      scroller,
      capture,
      restore,
      syncJump,
      startOpeningEndPin,
      clearNewEntries,
      edge,
      opener,
      jumpToLatest,
      patchSession
    };
  }

  // src/app/composition.ts
  var ViewerComposition = class {
    scope;
    dialogs;
    disposed;
    application;
    NOW;
    modelStore;
    MACHINE;
    MACHINE_UP;
    MACHINE_LAST;
    ADMIN;
    ACCOUNT;
    viewerHost;
    NATIVE_PAGE;
    NAV_MACHINES;
    SIDEBAR_ONLY;
    SESS;
    H;
    transcripts;
    TX;
    seenResultsOwner;
    HID;
    TURNS;
    TURN;
    STARTS;
    HOLDS;
    TXM;
    domain;
    nameOf = (...args) => this.domain.nameOf(...args);
    hcls = (...args) => this.domain.hcls(...args);
    where = (...args) => this.domain.where(...args);
    hostOf = (...args) => this.domain.hostOf(...args);
    machineLabels = (...args) => this.domain.machineLabels(...args);
    machineLabel = (...args) => this.domain.machineLabel(...args);
    branchOf = (...args) => this.domain.branchOf(...args);
    shortHost = (...args) => this.domain.shortHost(...args);
    parentOf = (...args) => this.domain.parentOf(...args);
    originHandoff = (...args) => this.domain.originHandoff(...args);
    isResult = (...args) => this.domain.isResult(...args);
    inbox = (...args) => this.domain.inbox(...args);
    working = (...args) => this.domain.working(...args);
    answersOf = (...args) => this.domain.answersOf(...args);
    statWord = (...args) => this.domain.statWord(...args);
    hasTurn = (...args) => this.domain.hasTurn(...args);
    oneLine = (...args) => this.domain.oneLine(...args);
    turnEnd = (...args) => this.domain.turnEnd(...args);
    traceRoot = (...args) => this.domain.traceRoot(...args);
    countOf = (...args) => this.domain.countOf(...args);
    callsText = (...args) => this.domain.callsText(...args);
    sessionChildren = (...args) => this.domain.sessionChildren(...args);
    childSessions = (...args) => this.domain.childSessions(...args);
    descendantsOf = (...args) => this.domain.descendantsOf(...args);
    asMoney = (...args) => this.domain.asMoney(...args);
    usageTotal = (...args) => this.domain.usageTotal(...args);
    costForSessions = (...args) => this.domain.costForSessions(...args);
    costForSession = (...args) => this.domain.costForSession(...args);
    costText = (...args) => this.domain.costText(...args);
    costMissing = (...args) => this.domain.costMissing(...args);
    urgentDescendant = (...args) => this.domain.urgentDescendant(...args);
    childParts = (...args) => this.domain.childParts(...args);
    defaultTreeOpen = (...args) => this.domain.defaultTreeOpen(...args);
    kidRank = (...args) => this.domain.kidRank(...args);
    lineageOf = (...args) => this.domain.lineageOf(...args);
    byState = (...args) => this.domain.byState(...args);
    onMachine = (...args) => this.domain.onMachine(...args);
    movedOff = (...args) => this.domain.movedOff(...args);
    movesOf = (...args) => this.domain.movesOf(...args);
    shortMoney = (...args) => this.domain.shortMoney(...args);
    clock;
    ago;
    dur;
    $;
    spaced;
    darkTheme;
    facetLine;
    seenPersistenceOwner;
    markSeenResults = (...args) => this.seenPersistenceOwner.markSeenResults(...args);
    sentencesOwner;
    sentenceSnapshot = (...args) => this.sentencesOwner.sentenceSnapshot(...args);
    isGap;
    transportOwner;
    api = (...args) => this.transportOwner.api(...args);
    txEntry = (...args) => this.transportOwner.txEntry(...args);
    fetchTx = (...args) => this.transportOwner.fetchTx(...args);
    enc = (...args) => this.transportOwner.enc(...args);
    adopt = (...args) => this.transportOwner.adopt(...args);
    load = (...args) => this.transportOwner.load(...args);
    spread = (...args) => this.transportOwner.spread(...args);
    tick = (...args) => this.transportOwner.tick(...args);
    accountOf = (...args) => this.transportOwner.accountOf(...args);
    STALE_BRIEFS;
    TXCACHE;
    cacheTx;
    adoptCached;
    transcriptRevalidationOwner;
    revalidate = (...args) => this.transcriptRevalidationOwner.revalidate(...args);
    shrank = (...args) => this.transcriptRevalidationOwner.shrank(...args);
    pagingOwner;
    resetPagerInput = (...args) => this.pagingOwner.resetPagerInput(...args);
    scrollProgrammatically = (...args) => this.pagingOwner.scrollProgrammatically(...args);
    clearPaging = (...args) => this.pagingOwner.clearPaging(...args);
    dropTx = (...args) => this.pagingOwner.dropTx(...args);
    loadPager = (...args) => this.pagingOwner.loadPager(...args);
    pagerSnapshot = (...args) => this.pagingOwner.pagerSnapshot(...args);
    holdProgrammaticScroll = (...args) => this.pagingOwner.holdProgrammaticScroll(...args);
    queuePagerObservers = (...args) => this.pagingOwner.queuePagerObservers(...args);
    disconnectPagerObservers = (...args) => this.pagingOwner.disconnectPagerObservers(...args);
    paintPager = (...args) => this.pagingOwner.paintPager(...args);
    readerScrollInput = (...args) => this.pagingOwner.readerScrollInput(...args);
    toolLoaderOwner;
    fullOf = (...args) => this.toolLoaderOwner.fullOf(...args);
    bootstrapOwner;
    urlOf = (...args) => this.bootstrapOwner.urlOf(...args);
    routeOf = (...args) => this.bootstrapOwner.routeOf(...args);
    boot = (...args) => this.bootstrapOwner.boot(...args);
    phone;
    navigation;
    layoutOwner;
    setRailMode = (...args) => this.layoutOwner.setRailMode(...args);
    setWideMode = (...args) => this.layoutOwner.setWideMode(...args);
    saveTreePref = (...args) => this.layoutOwner.saveTreePref(...args);
    syncLayoutPrefs = (...args) => this.layoutOwner.syncLayoutPrefs(...args);
    groupBy;
    query;
    focusSessionsSearchOnRender;
    analyticsRange;
    analyticsMeasure;
    showApprovalReviews;
    sessionFilters;
    pendingSessionOpen;
    accountSheet;
    afterPop;
    SHOW_ALL;
    show;
    find;
    findOpen;
    historyScrollOwner;
    saveHistoryScroll = (...args) => this.historyScrollOwner.saveHistoryScroll(...args);
    quietTop = (...args) => this.historyScrollOwner.quietTop(...args);
    restoreScroll = (...args) => this.historyScrollOwner.restoreScroll(...args);
    restoreHostFocus = (...args) => this.historyScrollOwner.restoreHostFocus(...args);
    currentScroll = (...args) => this.historyScrollOwner.currentScroll(...args);
    destination;
    goSession = (...args) => this.destination.goSession(...args);
    go = (...args) => this.destination.go(...args);
    openSender = (...args) => this.destination.openSender(...args);
    revealTurn = (...args) => this.destination.revealTurn(...args);
    revealEntryHash = (...args) => this.destination.revealEntryHash(...args);
    openSessionAtEnd = (...args) => this.destination.openSessionAtEnd(...args);
    goTrace = (...args) => this.destination.goTrace(...args);
    accountControlsOwner;
    closeAccountMenu = (...args) => this.accountControlsOwner.closeAccountMenu(...args);
    renderDrawerAccount = (...args) => this.accountControlsOwner.renderDrawerAccount(...args);
    orderingControlsOwner;
    orderApply = (...args) => this.orderingControlsOwner.orderApply(...args);
    byLast = (...args) => this.orderingControlsOwner.byLast(...args);
    orderList = (...args) => this.orderingControlsOwner.orderList(...args);
    ordState = (...args) => this.orderingControlsOwner.ordState(...args);
    orderScope = (...args) => this.orderingControlsOwner.orderScope(...args);
    ordIdleArm = (...args) => this.orderingControlsOwner.ordIdleArm(...args);
    pageSig = (...args) => this.orderingControlsOwner.pageSig(...args);
    pageState = (...args) => this.orderingControlsOwner.pageState(...args);
    navigationViewOwner;
    renderNav = (...args) => this.navigationViewOwner.renderNav(...args);
    sessMatch = (...args) => this.navigationViewOwner.sessMatch(...args);
    recentNavigation;
    renderLanes = (...args) => this.recentNavigation.renderLanes(...args);
    isApprovalReview = (...args) => this.recentNavigation.isApprovalReview(...args);
    _SessionChrome;
    drawSessionBar = (...args) => this._SessionChrome.drawSessionBar(...args);
    syncBarLine = (...args) => this._SessionChrome.syncBarLine(...args);
    dropErrors = (...args) => this._SessionChrome.dropErrors(...args);
    modelIdOf = (...args) => this._SessionChrome.modelIdOf(...args);
    kindText = (...args) => this._SessionChrome.kindText(...args);
    observeTitle = (...args) => this._SessionChrome.observeTitle(...args);
    centre = (...args) => this._SessionChrome.centre(...args);
    panel = (...args) => this._SessionChrome.panel(...args);
    renderTopbar = (...args) => this._SessionChrome.renderTopbar(...args);
    machineLine = (...args) => this._SessionChrome.machineLine(...args);
    sessionLine = (...args) => this._SessionChrome.sessionLine(...args);
    errOn = (...args) => this._SessionChrome.errOn(...args);
    markError = (...args) => this._SessionChrome.markError(...args);
    errorsLive = (...args) => this._SessionChrome.errorsLive(...args);
    _ScreenViews;
    harnessSnapshot = (...args) => this._ScreenViews.harnessSnapshot(...args);
    costSnapshot = (...args) => this._ScreenViews.costSnapshot(...args);
    showsFooter = (...args) => this._ScreenViews.showsFooter(...args);
    renderHome = (...args) => this._ScreenViews.renderHome(...args);
    renderMachines = (...args) => this._ScreenViews.renderMachines(...args);
    renderMachine = (...args) => this._ScreenViews.renderMachine(...args);
    renderTrace = (...args) => this._ScreenViews.renderTrace(...args);
    renderSession = (...args) => this._ScreenViews.renderSession(...args);
    _TranscriptView;
    transcriptEntries = (...args) => this._TranscriptView.transcriptEntries(...args);
    transcriptSnapshot = (...args) => this._TranscriptView.transcriptSnapshot(...args);
    verb = (...args) => this._TranscriptView.verb(...args);
    verbNow = (...args) => this._TranscriptView.verbNow(...args);
    toolViewsOwner;
    openStepViewer = (...args) => this.toolViewsOwner.openStepViewer(...args);
    openScript = (...args) => this.toolViewsOwner.openScript(...args);
    openImage = (...args) => this.toolViewsOwner.openImage(...args);
    attachmentSnapshot = (...args) => this.toolViewsOwner.attachmentSnapshot(...args);
    childSnapshot = (...args) => this.toolViewsOwner.childSnapshot(...args);
    footerSnapshot = (...args) => this.toolViewsOwner.footerSnapshot(...args);
    documentRendererOwner;
    render = (...args) => this.documentRendererOwner.render(...args);
    slot = (...args) => this.documentRendererOwner.slot(...args);
    _Analytics;
    fetchAnalytics = (...args) => this._Analytics.fetchAnalytics(...args);
    scheduleAnalytics = (...args) => this._Analytics.scheduleAnalytics(...args);
    refreshAnalytics = (...args) => this._Analytics.refreshAnalytics(...args);
    renderAnalytics = (...args) => this._Analytics.renderAnalytics(...args);
    matchesSessionFacets = (...args) => this._Analytics.matchesSessionFacets(...args);
    renderFacetFilters = (...args) => this._Analytics.renderFacetFilters(...args);
    sessionListOwner;
    renderSessions = (...args) => this.sessionListOwner.renderSessions(...args);
    documentEventsOwner;
    closeDrawer = (...args) => this.documentEventsOwner.closeDrawer(...args);
    liveModelOwner;
    remember = (...args) => this.liveModelOwner.remember(...args);
    schedule = (...args) => this.liveModelOwner.schedule(...args);
    visible = (...args) => this.liveModelOwner.visible(...args);
    ended = (...args) => this.liveModelOwner.ended(...args);
    handKey = (...args) => this.liveModelOwner.handKey(...args);
    cardKeys = (...args) => this.liveModelOwner.cardKeys(...args);
    viewed = (...args) => this.liveModelOwner.viewed(...args);
    soft = (...args) => this.liveModelOwner.soft(...args);
    _LiveUpdates;
    reload = (...args) => this._LiveUpdates.reload(...args);
    tail = (...args) => this._LiveUpdates.tail(...args);
    applyModelDelta = (...args) => this._LiveUpdates.applyModelDelta(...args);
    update = (...args) => this._LiveUpdates.update(...args);
    applicationRefreshOwner;
    refresh = (...args) => this.applicationRefreshOwner.refresh(...args);
    viewport;
    stopOpeningEndPin = (...args) => this.viewport.stopOpeningEndPin(...args);
    scroller = (...args) => this.viewport.scroller(...args);
    capture = (...args) => this.viewport.capture(...args);
    restore = (...args) => this.viewport.restore(...args);
    syncJump = (...args) => this.viewport.syncJump(...args);
    startOpeningEndPin = (...args) => this.viewport.startOpeningEndPin(...args);
    clearNewEntries = (...args) => this.viewport.clearNewEntries(...args);
    edge = (...args) => this.viewport.edge(...args);
    opener = (...args) => this.viewport.opener(...args);
    jumpToLatest = (...args) => this.viewport.jumpToLatest(...args);
    patchSession = (...args) => this.viewport.patchSession(...args);
    tickerOwner;
    ticker = (...args) => this.tickerOwner.ticker(...args);
    running = (...args) => this.tickerOwner.running(...args);
    get SEEN_RESULTS() {
      return this.seenResultsOwner.SEEN_RESULTS;
    }
    get SEEN_LIMIT() {
      return this.seenResultsOwner.SEEN_LIMIT;
    }
    get SEEN_KEY() {
      return this.seenResultsOwner.SEEN_KEY;
    }
    get RANK() {
      return this.domain.RANK;
    }
    get TOYOU() {
      return this.domain.TOYOU;
    }
    get TOTAL_TOKEN_KINDS() {
      return this.domain.TOTAL_TOKEN_KINDS;
    }
    get TOKEN_KINDS() {
      return this.domain.TOKEN_KINDS;
    }
    get TREE_RANK() {
      return this.domain.TREE_RANK;
    }
    get sentenceHost() {
      return this.sentencesOwner.sentenceHost;
    }
    get pagerController() {
      return this.pagingOwner.pagerController;
    }
    get routeModel() {
      return this.bootstrapOwner.routeModel;
    }
    get app() {
      return this.layoutOwner.app;
    }
    get shellChrome() {
      return this.accountControlsOwner.shellChrome;
    }
    get accountChrome() {
      return this.accountControlsOwner.accountChrome;
    }
    get ORD() {
      return this.orderingControlsOwner.ORD;
    }
    get ORD_DRAWER_MS() {
      return this.orderingControlsOwner.ORD_DRAWER_MS;
    }
    get recentRenderer() {
      return this.recentNavigation.recentRenderer;
    }
    get COST_TIP() {
      return this.recentNavigation.COST_TIP;
    }
    get errorNavigation() {
      return this._SessionChrome.errorNavigation;
    }
    get viewerBar() {
      return this._SessionChrome.viewerBar;
    }
    get SLOTS() {
      return this.documentRendererOwner.SLOTS;
    }
    get liveController() {
      return this.liveModelOwner.liveController;
    }
    get LIVE() {
      return this.liveModelOwner.LIVE;
    }
    get scrollController() {
      return this.viewport.scrollController;
    }
    get skipPop() {
      return this.toolViewsOwner.skipPop;
    }
    set skipPop(value) {
      this.toolViewsOwner.skipPop = value;
    }
    get viewerEl() {
      return this.toolViewsOwner.viewerEl;
    }
    set viewerEl(value) {
      this.toolViewsOwner.viewerEl = value;
    }
    get STATE() {
      return STATE;
    }
    get treePrefs() {
      return this.layoutOwner.treePrefs;
    }
    set treePrefs(value) {
      this.layoutOwner.treePrefs = value;
    }
    get HARNESSES() {
      return HARNESSES;
    }
    get I() {
      return I;
    }
    get HARNESS() {
      return HARNESS;
    }
    get railMode() {
      return this.layoutOwner.railMode;
    }
    set railMode(value) {
      this.layoutOwner.railMode = value;
    }
    get ordIdle() {
      return this.orderingControlsOwner.ordIdle;
    }
    set ordIdle(value) {
      this.orderingControlsOwner.ordIdle = value;
    }
    get wideMode() {
      return this.layoutOwner.wideMode;
    }
    set wideMode(value) {
      this.layoutOwner.wideMode = value;
    }
    get TOK() {
      return this.transportOwner.TOK;
    }
    set TOK(value) {
      this.transportOwner.TOK = value;
    }
    get HARNESS_SHORT() {
      return HARNESS_SHORT;
    }
    get scrollRevision() {
      return this.pagingOwner.scrollRevision;
    }
    set scrollRevision(value) {
      this.pagingOwner.scrollRevision = value;
    }
    get programmaticScrollPending() {
      return this.pagingOwner.programmaticScrollPending;
    }
    set programmaticScrollPending(value) {
      this.pagingOwner.programmaticScrollPending = value;
    }
    constructor(host2, onDestroyed) {
      const context = this;
      this.scope = new EffectScope();
      this.dialogs = /* @__PURE__ */ new Map();
      this.disposed = false;
      this.application = {
        destroy() {
          if (context.disposed) return;
          context.disposed = true;
          context.scope.destroy();
          context.liveController.destroy();
          context.navigation.destroy();
          context.transcripts.destroy();
          context.stopOpeningEndPin();
          context.pagerController.disconnect();
          context.errorNavigation.destroy();
          context.scrollController.destroy();
          for (const dialog of context.dialogs.values()) dialog.destroy();
          context.dialogs.clear();
          for (const slot of context.SLOTS.values()) slot.ctx.destroy?.();
          context.SLOTS.clear();
          if (!context.SIDEBAR_ONLY) releaseScreen(context.$("#page"));
          context.recentRenderer.destroy();
          context.viewerBar.destroy();
          context.shellChrome?.destroy();
          context.accountChrome.destroy();
          releaseGeometry(context.app);
          releaseGeometry(document.documentElement, ["barHeight"]);
          document.querySelectorAll(".livenote, .livenote-side").forEach((node) => node.remove());
          document.documentElement.classList.remove("viewer-open", "panel-open");
          onDestroyed(context.application);
        }
      };
      this.NOW = Date.now();
      this.modelStore = new ViewerModelStore();
      this.MACHINE = context.modelStore.machines;
      this.MACHINE_UP = context.modelStore.machineUp;
      this.MACHINE_LAST = context.modelStore.machineLast;
      this.ADMIN = null;
      this.ACCOUNT = null;
      this.viewerHost = host2;
      this.NATIVE_PAGE = context.viewerHost?.nativePage;
      this.NAV_MACHINES = context.viewerHost?.machinesPath ?? null;
      this.SIDEBAR_ONLY = document.querySelector(".app")?.dataset.viewer === "sidebar";
      this.SESS = context.modelStore.sessions;
      this.H = context.modelStore.handoffs;
      this.transcripts = new TranscriptStore({
        request: (path, signal) => context.api(path, signal),
        turns: (sid) => context.modelStore.turns[sid] ?? [],
        turn: (id) => context.modelStore.turn.get(id),
        entry: (e) => context.txEntry(e),
        cleared(sid) {
          if (context.navigation.route.v === "session" && ("id" in context.navigation.route ? context.navigation.route.id : "") === sid)
            context.resetPagerInput();
        }
      });
      this.TX = context.transcripts.entries;
      this.seenResultsOwner = createSeenResults({});
      this.HID = context.modelStore.handoff;
      this.TURNS = context.modelStore.turns;
      this.TURN = context.modelStore.turn;
      this.STARTS = context.modelStore.starts;
      this.HOLDS = context.modelStore.holds;
      this.TXM = context.transcripts.meta;
      this.domain = createDomain(
        {
          sessions: context.SESS,
          machines: context.MACHINE,
          handoffs: context.H,
          turns: context.TURNS,
          turn: context.TURN,
          starts: context.STARTS,
          holds: context.HOLDS,
          handoff: context.HID,
          transcriptMeta: context.TXM
        },
        () => context.NOW,
        context.SEEN_RESULTS
      );
      this.clock = (t) => clock(t, context.NOW);
      this.ago = (t) => ago(t, context.NOW);
      this.dur = (a, b) => dur(a, b ?? void 0, context.NOW);
      this.$ = (s, r = document) => r.querySelector(s);
      this.spaced = (t) => String(t).replace(/ · /g, "\u2009 \xB7 \u2009").replace(/^· /, "\xB7\u2009 ");
      this.darkTheme = () => {
        const t = document.documentElement.getAttribute("data-theme");
        return t === "dark" || t !== "light" && !!window.matchMedia?.("(prefers-color-scheme: dark)").matches;
      };
      this.facetLine = (s) => [s.kind ?? HARNESS[s.harness], context.MACHINE[s.machine], context.where(s)].join(" \xB7 ");
      this.seenPersistenceOwner = createSeenPersistence(context);
      this.sentencesOwner = createSentences(context);
      this.isGap = (e) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");
      this.transportOwner = createTransport(context);
      this.STALE_BRIEFS = context.transcripts.staleBriefs;
      this.TXCACHE = context.transcripts.cache;
      this.cacheTx = (sid, entries, meta) => context.transcripts.keep(sid, entries, meta, !!context.originHandoff(sid));
      this.adoptCached = (r) => context.transcripts.adoptCached(r.id, r.turn);
      this.transcriptRevalidationOwner = createTranscriptRevalidation(context);
      this.pagingOwner = createPaging(context);
      this.toolLoaderOwner = createToolLoader(context);
      this.bootstrapOwner = createBootstrap(context);
      this.phone = window.matchMedia("(max-width: 760px)");
      this.navigation = new NavigationController(
        {
          model: context.routeModel,
          loadMachines: host2 ? (signal) => host2.loadMachines(signal) : void 0
        },
        context.viewerHost?.initialMachines ?? null
      );
      this.layoutOwner = createLayout(context);
      this.groupBy = "recent";
      this.query = "";
      this.focusSessionsSearchOnRender = null;
      this.analyticsRange = 7;
      this.analyticsMeasure = "hours";
      this.showApprovalReviews = false;
      this.sessionFilters = { repo: "", machine: "", harness: "", model: "" };
      this.pendingSessionOpen = null;
      this.accountSheet = false;
      this.afterPop = null;
      if (!context.SIDEBAR_ONLY)
        try {
          history.scrollRestoration = "manual";
        } catch {
        }
      this.SHOW_ALL = { messages: true, tools: true, thinking: true };
      this.show = { ...context.SHOW_ALL };
      this.find = "";
      this.findOpen = false;
      this.historyScrollOwner = createHistoryScroll(context);
      this.destination = createDestination(context);
      this.accountControlsOwner = createAccountControls(context);
      this.orderingControlsOwner = createOrderingControls(context);
      this.navigationViewOwner = createNavigationView(context);
      this.recentNavigation = createRecentNavigation(context);
      this._SessionChrome = createSessionChrome(context);
      this._ScreenViews = createScreenViews(context);
      this._TranscriptView = createTranscriptView(context);
      this.toolViewsOwner = createToolViews(context);
      this.documentRendererOwner = createDocumentRenderer(context);
      this._Analytics = createAnalytics(context);
      this.sessionListOwner = createSessionList(context);
      this.documentEventsOwner = createDocumentEvents(context);
      this.liveModelOwner = createLiveModel(context);
      this._LiveUpdates = createLiveUpdates(context);
      this.applicationRefreshOwner = createApplicationRefresh(context);
      this.viewport = createViewport(context);
      this.tickerOwner = createTicker(context);
      if (context.SIDEBAR_ONLY) {
        const nav = context.app.dataset.viewerNav;
        context.navigation.route = context.navigation.historyRoute({ v: nav }, { v: "home" });
      }
      if (context.viewerHost) {
        this.ACCOUNT = context.accountOf(context.viewerHost.account);
        if (context.NATIVE_PAGE)
          context.navigation.route = context.navigation.historyRoute(
            { v: context.NATIVE_PAGE.nav },
            { v: "home" }
          );
        else if (context.navigation.content) context.navigation.route = { v: "machines" };
        if (context.NATIVE_PAGE || context.navigation.content) context.render();
      }
      context.boot();
    }
  };

  // src/app/viewer.ts
  var mounted = null;
  function mountViewerApplication(host2 = getViewerHost()) {
    mounted?.destroy();
    const composition = new ViewerComposition(host2, (owner) => {
      if (mounted === owner) mounted = null;
    });
    mounted = composition.application;
    return composition.application;
  }

  // src/viewer.ts
  queueMicrotask(mountViewerApplication);
})();
