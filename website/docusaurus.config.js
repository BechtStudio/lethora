// The docs site: the developer docs (docs/dev/) and the user manual
// (docs/manual/), read in place so the Markdown stays readable on GitHub.
// The two folders are siblings: Docusaurus can't nest docs instances.

const repo = 'https://github.com/BechtStudio/lethora';

/** @type {import('@docusaurus/types').Config} */
export default {
  title: 'Lethora',
  tagline: 'A simple, distraction-free writing editor',
  favicon: 'img/favicon.png',

  // GitHub Pages project site.
  url: 'https://bechtstudio.github.io',
  baseUrl: '/lethora/',
  organizationName: 'BechtStudio',
  projectName: 'lethora',
  // GitHub Pages redirects to a trailing slash unless this is set.
  trailingSlash: false,

  onBrokenLinks: 'throw',
  onBrokenAnchors: 'throw',

  markdown: {
    // .md as CommonMark, not MDX: the docs use HTML comments and plain
    // braces that MDX rejects.
    format: 'detect',
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
  },

  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        // The two docs instances are plugins below.
        docs: false,
        blog: false,
      }),
    ],
  ],

  plugins: [
    [
      '@docusaurus/plugin-content-docs',
      {
        id: 'dev',
        path: '../docs/dev',
        routeBasePath: 'dev',
        // The path is outside website/, so the edit link is built by hand.
        editUrl: ({docPath}) => `${repo}/edit/main/docs/dev/${docPath}`,
      },
    ],
    [
      '@docusaurus/plugin-content-docs',
      {
        id: 'manual',
        path: '../docs/manual',
        routeBasePath: 'manual',
        editUrl: ({docPath}) => `${repo}/edit/main/docs/manual/${docPath}`,
      },
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      // Like the app's theme setting: follow the system.
      colorMode: {respectPrefersColorScheme: true},
      navbar: {
        title: 'Lethora',
        logo: {alt: 'Lethora', src: 'img/logo.png'},
        items: [
          // Doc links rather than paths, so they follow versions later.
          {type: 'doc', docsPluginId: 'manual', docId: 'index', label: 'Manual'},
          {type: 'doc', docsPluginId: 'dev', docId: 'index', label: 'Developer docs'},
          {href: repo, label: 'GitHub', position: 'right'},
        ],
      },
      footer: {
        style: 'dark',
        copyright: `Copyright © ${new Date().getFullYear()} Maxim Becht`,
      },
    }),
};
