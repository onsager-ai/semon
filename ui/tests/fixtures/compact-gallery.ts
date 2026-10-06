import { mountNativeShell } from '../../src/application';
let owner = mountNativeShell({ chrome: false });
document.querySelector('#remount')?.addEventListener('click', () => {
  owner.destroy();
  owner = mountNativeShell({ chrome: false });
});
