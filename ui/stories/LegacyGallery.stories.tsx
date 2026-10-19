import { useEffect, useRef } from 'preact/hooks';
import type { Meta, StoryObj } from '@storybook/preact-vite';
import { mountDesignGallery } from '../tests/fixtures/design-gallery';
import '../../tests/ui/gallery.css';

function GalleryOwner() {
  const app = useRef<HTMLDivElement>(null);
  useEffect(() => app.current ? mountDesignGallery(app.current) : undefined, []);
  return <div class="app" ref={app} data-storybook-route="gallery" />;
}
const meta = { title: 'Semon/Legacy gallery', parameters: { layout: 'fullscreen', routeComposition: true } } satisfies Meta;
export default meta;
export const Gallery: StoryObj<typeof meta> = { render: () => <GalleryOwner /> };
