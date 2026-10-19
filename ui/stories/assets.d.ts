declare module '*.css';

declare module '*.svg?url' {
  const assetUrl: string;
  export default assetUrl;
}
