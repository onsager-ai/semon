import { mountViewerApplication, mountNativeShell } from '../../ui/src/application';
mountNativeShell({ chrome: false });
mountViewerApplication({
  machinesPath: '/machines',
  catalogSources: true,
  nativePage: { title: 'Devices', nav: 'machines' },
});
