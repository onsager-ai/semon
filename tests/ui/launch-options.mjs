import { fileURLToPath } from 'node:url';

export const launchOptions = () => ({
  args: ['--disable-gpu', '--font-render-hinting=none'],
  ...(process.platform === 'linux'
    ? {
        env: {
          ...process.env,
          FONTCONFIG_FILE: fileURLToPath(new URL('./fontconfig.conf', import.meta.url)),
        },
      }
    : {}),
});
