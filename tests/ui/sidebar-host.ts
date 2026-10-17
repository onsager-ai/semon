import { mountViewerApplication, mountNativeShell } from '../../ui/src/application';
mountNativeShell({ chrome: false });
mountViewerApplication({
  machinesPath: '/machines',
  catalogSources: true,
  nativePage: { title: 'Devices', nav: 'machines' },
});
// This receipt is emitted only after the real native shell and viewer mounts return.
Object.assign(window, { __nativeSidebarHostMounted: true });
