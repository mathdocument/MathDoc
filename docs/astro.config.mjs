import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://mathdocument.github.io',
  base: '/MathDoc',
  integrations: [
    starlight({
      title: 'MathDoc',
      description: 'Collaborative mathematical graphs whose Lean proofs are checked by LeanGround.',
      logo: {
        src: './src/assets/mdc-logo.svg',
        alt: 'MathDoc',
      },
      favicon: '/mdc-logo.png',
      components: { Header: './src/components/Header.astro' },
      customCss: ['./src/styles/custom.css'],
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
          tag: 'link',
          attrs: {
            rel: 'apple-touch-icon',
            sizes: '400x400',
            href: '/MathDoc/mdc-logo.png?v=2',
          },
        },
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
          borderRadius: 'var(--mdc-radius-md)',
          borderColor: 'var(--mdc-border)',
          codeBackground: 'var(--mdc-code-bg)',
          codeFontFamily: 'var(--mdc-mono)',
          codeFontSize: '0.875rem',
          uiFontFamily: 'var(--mdc-font)',
          frames: {
            terminalBackground: 'var(--mdc-code-bg)',
            terminalTitlebarBackground: 'var(--mdc-panel-raised)',
            editorActiveTabBackground: 'var(--mdc-panel-raised)',
            editorActiveTabForeground: 'var(--mdc-fg-soft)',
            editorTabBarBackground: 'var(--mdc-panel)',
            editorActiveTabIndicatorTopColor: 'var(--mdc-accent)',
          },
        },
      },
      sidebar: [
        {
          label: 'Start Here',
          items: [
            { label: 'Overview', slug: 'index' },
            { label: 'Installation', slug: 'getting-started/installation' },
            { label: 'Server Deployment', slug: 'getting-started/server-deployment' },
            { label: 'Quick Start', slug: 'getting-started/quick-start' },
          ],
        },
        {
          label: 'Core Concepts',
          items: [
            { label: 'Databases & Workspaces', slug: 'concepts/workspaces' },
            { label: 'Collaboration', slug: 'concepts/collaboration' },
            { label: 'Dependency Graph', slug: 'concepts/dependency-graph' },
            { label: 'Source Workflow', slug: 'concepts/source-workflow' },
            { label: 'Web Interface', slug: 'concepts/web-interface' },
            { label: 'Export & Restore', slug: 'concepts/import-export' },
            { label: 'LaTeX & References', slug: 'concepts/latex' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'CLI Commands', slug: 'reference/workspace-commands' },
            { label: 'Dependency Commands', slug: 'reference/dependency-commands' },
            { label: 'Graph & Metrics', slug: 'reference/graph-and-metrics' },
            { label: 'Proofs with LeanGround', slug: 'reference/proofs' },
            { label: 'Agent Interface', slug: 'reference/agents' },
            { label: 'Configuration', slug: 'reference/configuration' },
            { label: 'HTTP API', slug: 'reference/http-api' },
          ],
        },
        {
          label: 'Development',
          collapsed: true,
          items: [
            { label: 'Development Setup', slug: 'development/setup' },
            { label: 'Architecture', slug: 'development/architecture' },
            { label: 'Versioned Mutations', slug: 'development/safe-mutations' },
            { label: 'Web Frontend', slug: 'development/web-frontend' },
            { label: 'Performance Measurements', slug: 'development/performance' },
            { label: 'Release Checks', slug: 'development/editor-release' },
          ],
        },
      ],
    }),
  ],
});
