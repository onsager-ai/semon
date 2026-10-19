const uiRoot = decodeURIComponent(new URL('../', import.meta.url).pathname).replace(/\/$/, '');
const sessionSource = decodeURIComponent(
  new URL('../../crates/semon-sessions/src/', import.meta.url).pathname,
).replace(/\/$/, '');

export default {
  stories: ['../stories/**/*.stories.@(js|jsx|mjs|ts|tsx)'],
  framework: {
    name: '@storybook/preact-vite',
    options: {},
  },
  viteFinal: async (config) => {
    config.resolve ??= {};
    config.resolve.dedupe = [
      ...new Set([...(config.resolve.dedupe ?? []), 'preact', 'preact/hooks', 'preact/jsx-runtime']),
    ];
    config.server ??= {};
    config.server.fs ??= {};
    config.server.fs.allow = [...new Set([...(config.server.fs.allow ?? []), uiRoot, sessionSource])];
    return config;
  },
};
