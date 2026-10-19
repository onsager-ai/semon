import { mountDesignGallery } from './design-gallery';

const app = document.querySelector<HTMLElement>('.app');
if (!app) throw new Error('Missing component gallery app root');
const cleanup = mountDesignGallery(app);
window.addEventListener('pagehide', cleanup, { once: true });
