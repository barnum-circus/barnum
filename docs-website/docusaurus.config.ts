import type * as Preset from '@docusaurus/preset-classic';
import type { Config } from '@docusaurus/types';
import { themes as prismThemes } from 'prism-react-renderer';

const lightTheme = prismThemes.github;
lightTheme.plain.backgroundColor = 'rgba(0, 0, 0, 0.02)';

const config: Config = {
  title: 'GSD',
  tagline: 'Don\'t just /loop it. The missing workflow engine for agents.',
  favicon: 'img/favicon.svg',

  url: 'https://gsd-now.github.io/',
  baseUrl: '/',
  trailingSlash: true,

  organizationName: 'gsd-now',
  projectName: 'gsd-now.github.io',
  deploymentBranch: 'main',

  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'throw',

  staticDirectories: ['static'],

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    metadata: [
      {
        name: 'keywords',
        content: 'GSD, LLM, agents, task queue, state machine, Rust, CLI',
      },
    ],

    navbar: {
      title: '⚡ GSD',
      items: [
        {
          type: 'docSidebar',
          sidebarId: 'documentationSidebar',
          position: 'left',
          label: 'Documentation',
        },
        {
          href: 'https://discord.gg/eBjM5XX6nk',
          label: 'Discord',
          position: 'right',
        },
        {
          href: 'https://github.com/gsd-now/gsd',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Docs',
          items: [
            {
              label: 'Introduction',
              to: '/docs/',
            },
            {
              label: 'Quick Start',
              to: '/docs/quickstart',
            },
          ],
        },
        {
          title: 'Community',
          items: [
            {
              label: 'Discord',
              href: 'https://discord.gg/eBjM5XX6nk',
            },
            {
              label: 'GitHub',
              href: 'https://github.com/gsd-now/gsd',
            },
          ],
        },
      ],
      copyright: undefined,
    },
    prism: {
      theme: lightTheme,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ['bash', 'json'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
