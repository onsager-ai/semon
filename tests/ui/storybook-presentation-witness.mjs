import { createHash } from 'node:crypto';

// Both sides use this schema. No text/order/source/open-state/geometry rewrites.
// Runtime-only DOM owner IDs and URL origins are not presentation fields; native
// source-qualified data-id/turn/entry identities remain byte-for-byte evidence.
export async function presentationWitness(page, selector = null) {
  return page.evaluate((selector) => {
    const roots = selector ? [selector] : ['#sidebar', '#topbar', '#page', 'dialog[open]'];
    const describe = (root) => {
      const elements = [root, ...root.querySelectorAll('*')];
      if (elements.length > 10000) throw Error('Presentation witness exceeds bounded fixture size');
      return elements.filter((node) => node.getClientRects().length && getComputedStyle(node).visibility !== 'hidden')
        .map((node) => {
          const box = node.getBoundingClientRect(), style = getComputedStyle(node);
          return {
            tag: node.tagName, class: node.getAttribute('class'),
            attributes: Object.fromEntries(['role', 'aria-label', 'aria-expanded', 'aria-current',
              'data-id', 'data-session-row', 'data-turn', 'data-entry-key', 'data-drop', 'alt', 'open']
              .map((name) => [name, node.getAttribute(name)])),
            text: [...node.childNodes].filter((child) => child.nodeType === Node.TEXT_NODE).map((child) => child.textContent).join(''),
            box: [box.x, box.y, box.width, box.height],
            style: Object.fromEntries(['display', 'font', 'color', 'backgroundColor', 'padding', 'gap', 'overflow', 'position']
              .map((key) => [key, style[key]])),
            vector: node.tagName.toLowerCase() === 'path' ? node.getAttribute('d') : null,
          };
        });
    };
    const main = document.querySelector('#main');
    return {
      version: 1,
      viewport: [innerWidth, innerHeight, devicePixelRatio],
      scroll: { window: scrollY, main: main?.scrollTop ?? 0 },
      accountCount: document.querySelectorAll('.account-trigger').length,
      preferences: Object.fromEntries(['semon.seen', 'semon.tree'].map((key) => [key, localStorage.getItem(key)])),
      roots: Object.fromEntries(roots.map((key) => [key, [...document.querySelectorAll(key)].map(describe)])),
    };
  }, selector);
}

export async function nativePresentationWitness(page, bytes, selector = null) {
  return { sourceRevision: process.env.GITHUB_SHA ?? null,
    imageSha256: createHash('sha256').update(bytes).digest('hex'),
    presentation: await presentationWitness(page, selector) };
}

export function witnessDifferences(native, story, path = '') {
  if (Object.is(native, story)) return [];
  if (native === null || story === null || typeof native !== 'object' || typeof story !== 'object') return [path];
  if (Array.isArray(native) !== Array.isArray(story)) return [path];
  const nativeKeys = Object.keys(native), storyKeys = Object.keys(story);
  if (JSON.stringify(nativeKeys) !== JSON.stringify(storyKeys)) return [path + ': fields/order'];
  return nativeKeys.flatMap((key) => witnessDifferences(native[key], story[key], path + '/' + key));
}
