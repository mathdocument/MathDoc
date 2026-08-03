import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://mathdocument.github.io',
  base: '/MathDoc',
  integrations: [
    starlight({
      title: 'MathDoc',
      description: 'Build connected mathematical knowledge with plain text and native proof tools.',
      logo: {
        src: './src/assets/mdc-logo.svg',
        alt: 'MathDoc',
      },
      favicon: '/mdc-logo.svg',
      customCss: [
        '@fontsource-variable/manrope',
        '@fontsource-variable/newsreader',
        './src/styles/custom.css',
      ],
      social: [
        {
          icon: 'github',
          label: 'MathDoc on GitHub',
          href: 'https://github.com/mathdocument/MathDoc',
        },
      ],
      editLink: {
        baseUrl: 'https://github.com/mathdocument/MathDoc/edit/main/docs/',
      },
      head: [
        {
          tag: 'meta',
          attrs: {
            property: 'og:image',
            content: 'https://mathdocument.github.io/MathDoc/mdc-logo.png',
          },
        },
        {
          tag: 'meta',
          attrs: {
            name: 'twitter:image',
            content: 'https://mathdocument.github.io/MathDoc/mdc-logo.png',
          },
        },
      ],
      lastUpdated: true,
      expressiveCode: {
        styleOverrides: {
          borderRadius: '0.75rem',
        },
      },
      sidebar: [
        {
          label: 'Start Here',
          items: [
            { label: 'Overview', slug: 'index' },
            { label: 'Installation', slug: 'getting-started/installation' },
            { label: 'Quick Start', slug: 'getting-started/quick-start' },
          ],
        },
        {
          label: 'Core Concepts',
          items: [
            { label: 'The .mdoc Format', slug: 'concepts/mdoc-format' },
            { label: 'Workspaces & References', slug: 'concepts/workspaces' },
            { label: 'Dependency Graph', slug: 'concepts/dependency-graph' },
            { label: 'Source Workflow', slug: 'concepts/source-workflow' },
            { label: 'Web Interface', slug: 'concepts/web-interface' },
          ],
        },
        {
          label: 'CLI Reference',
          items: [
            { label: 'Workspace Commands', slug: 'reference/workspace-commands' },
            { label: 'Dependency Commands', slug: 'reference/dependency-commands' },
            { label: 'Graph & Metrics', slug: 'reference/graph-and-metrics' },
            { label: 'Work, Back & Compilers', slug: 'reference/work-and-compilers' },
            { label: 'Configuration & Profiling', slug: 'reference/configuration' },
          ],
        },
        {
          label: 'Development',
          collapsed: true,
          items: [
            { label: 'Development Setup', slug: 'development/setup' },
            { label: 'Architecture', slug: 'development/architecture' },
            { label: 'Index & Cache', slug: 'development/index-cache' },
            { label: 'Safe Mutations', slug: 'development/safe-mutations' },
            { label: 'Web Frontend', slug: 'development/web-frontend' },
            { label: 'Compiler Internals', slug: 'development/compiler-internals' },
            { label: 'Editor & Release', slug: 'development/editor-release' },
          ],
        },
      ],
    }),
  ],
});
