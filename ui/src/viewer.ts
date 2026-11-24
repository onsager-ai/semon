import { receiverHost } from './app/manage';
import { mountViewerApplication } from './app/viewer';

queueMicrotask(() => {
  if (document.querySelector('meta[name="semon-receiver"]')) mountViewerApplication(receiverHost);
  else mountViewerApplication();
});
