import { options } from 'preact';
import { safePath } from './account';
import { externalUrl } from './richtext';
let installed = false;
/** Application VNodes cannot reach the pinned runtime's HTML or style sinks. */
export function installPropGuard() {
  if (installed) return;
  installed = true;
  const previous = options.vnode;
  options.vnode = (vnode) => {
    previous?.(vnode);
    if (typeof vnode.type === 'string') {
      for (const key of Object.keys(vnode.props)) {
        if (['dangerouslySetInnerHTML', 'style', 'title'].includes(key))
          throw new Error(`Forbidden DOM prop: ${key}`);
        const value = (vnode.props as Record<string, unknown>)[key];
        const externalLink =
          key === 'href' &&
          vnode.type === 'a' &&
          externalUrl(value) &&
          (vnode.props as Record<string, unknown>).target === '_blank' &&
          (vnode.props as Record<string, unknown>).rel === 'noopener noreferrer';
        if (
          ['href', 'src', 'action'].includes(key) &&
          value != null &&
          !safePath(value) &&
          !externalLink
        )
          throw new Error(`Unsafe DOM path: ${key}`);
        if (/^on/i.test(key) && typeof value === 'string')
          throw new Error(`Inline DOM handler: ${key}`);
      }
    }
  };
}
