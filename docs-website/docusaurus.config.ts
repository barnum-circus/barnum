import type * as Preset from '@docusaurus/preset-classic';
import type { Config } from '@docusaurus/types';
import { themes as prismThemes } from 'prism-react-renderer';

const lightTheme = prismThemes.github;
lightTheme.plain.backgroundColor = 'rgba(0, 0, 0, 0.02)';

const config: Config = {
  title: 'Barnum',
  tagline: 'Don\'t just /loop it. The ringmaster for your agents.',
  favicon: 'img/favicon.svg',

  url: 'https://barnum-circus.github.io/',
  baseUrl: '/',
  trailingSlash: true,

  organizationName: 'barnum-circus',
  projectName: 'barnum-circus.github.io',
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
    image: 'img/og-image.png',
    metadata: [
      {
        name: 'keywords',
        content: 'Barnum, LLM, agents, task queue, state machine, Rust, CLI',
      },
      {
        name: 'twitter:card',
        content: 'summary_large_image',
      },
      {
        name: 'twitter:title',
        content: 'Barnum - The ringmaster for your agents',
      },
      {
        name: 'twitter:description',
        content: "Don't just /loop it. The missing workflow engine for AI agents.",
      },
      {
        name: 'twitter:image',
        content: 'https://barnum-circus.github.io/img/og-image.png',
      },
      {
        property: 'og:type',
        content: 'website',
      },
    ],

    navbar: {
      title: '🎪 Barnum',
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
          href: 'https://github.com/barnum-circus/barnum',
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
              label: 'Quickstart',
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
              href: 'https://github.com/barnum-circus/barnum',
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
