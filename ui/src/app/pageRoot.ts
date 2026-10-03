import { ownsScreen, releaseScreen, screenKind } from '../lib/screens';
export interface PageRoot {
  viewer(root: HTMLElement, kind: string): void;
  native(root: HTMLElement): void;
  destroy(root: HTMLElement): void;
}
/** Preact exclusively owns viewer pages. Native descendants belong to the host. */
export function createPageRoot(): PageRoot {
  return {
    viewer(root, kind) {
      if (ownsScreen(root) && screenKind(root) === kind) return;
      releaseScreen(root);
      // Rust fallback and previously detached native content are outside Preact.
      root.replaceChildren();
    },
    native(root) {
      releaseScreen(root);
    },
    destroy(root) {
      releaseScreen(root);
    },
  };
}
