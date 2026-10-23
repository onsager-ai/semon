/** Typed document lifecycle for hosts that mount and replace a viewer explicitly. */
export { mountViewerApplication, type ViewerApplication } from './app/viewer';
export type { ViewerHost, ViewerContent, ViewerPage, ViewerPages } from './viewer-host';

export { mountNativeShell, type NativeShellOptions } from './app/native-shell';
