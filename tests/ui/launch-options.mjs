import { fileURLToPath } from 'node:url';

export const launchOptions = ({ baselineRaster = false } = {}) => ({
  args: [
    '--disable-gpu',
    '--font-render-hinting=none',
    // Deterministic rendering stages, while Playwright keeps normal frame
    // scheduling (enable-begin-frame-control requires a separate CDP driver).
    ...(baselineRaster
      ? [
          '--disable-skia-runtime-opts',
          '--run-all-compositor-stages-before-draw',
          '--disable-new-content-rendering-timeout',
          '--disable-threaded-animation',
          '--disable-threaded-scrolling',
          '--disable-checker-imaging',
          '--disable-image-animation-resync',
          '--disable-partial-raster',
        ]
      : []),
  ],
  ...(process.platform === 'linux'
    ? {
        env: {
          ...process.env,
          FONTCONFIG_FILE: fileURLToPath(new URL('./fontconfig.conf', import.meta.url)),
        },
      }
    : {}),
});
