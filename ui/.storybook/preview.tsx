import type { Preview } from '@storybook/preact-vite';
import '../../crates/semon-sessions/src/viewer.css';
import '../../crates/semon-sessions/src/select.css';
import '../../crates/semon-sessions/src/shell.css';
import './preview.css';

const preview: Preview = {
  globalTypes: {
    theme: {
      description: 'Semon color scheme',
      toolbar: {
        title: 'Theme',
        icon: 'paintbrush',
        items: [
          { value: 'light', title: 'Light' },
          { value: 'dark', title: 'Dark' },
        ],
        dynamicTitle: true,
      },
    },
  },
  initialGlobals: { theme: 'light' },
  decorators: [
    (Story, context) => {
      const theme = context.globals.theme === 'dark' ? 'dark' : 'light';
      document.documentElement.dataset.theme = theme;
      return (
        <div class="storybook-stage" data-theme={theme}>
          <Story />
        </div>
      );
    },
  ],
};

export default preview;
