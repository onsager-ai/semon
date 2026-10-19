declare module '*.css';

declare module '*.svg?url' {
  const assetUrl: string;
  export default assetUrl;
}

// Preserve same-origin asset URLs accepted by the production Recent owner.
declare module '*.svg?url&no-inline' {
  const assetUrl: string;
  export default assetUrl;
}
