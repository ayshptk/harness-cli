import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: 'harness',
    },
    links: [
      {
        text: 'GitHub',
        url: 'https://github.com/ayshptk/harness-cli',
      },
    ],
  };
}
