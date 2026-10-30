import { defineConfig } from 'vitepress'

import { frost, night, stavLanguage } from './syntax'

const repo = 'https://github.com/QaidVoid/laft'
const base = process.env.DOCS_BASE ?? '/'

export default defineConfig({
  title: 'Laft',
  description:
    'A Linux distribution with a content addressed store, immutable generations that switch atomically, a declarative configuration in Stav, runit services and tooling written in Rust.',
  lang: 'en-US',
  base,
  cleanUrls: true,
  lastUpdated: true,
  appearance: true,

  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: `${base}favicon.svg` }],
    ['link', { rel: 'icon', type: 'image/png', sizes: '32x32', href: `${base}favicon-32.png` }],
    ['link', { rel: 'apple-touch-icon', sizes: '180x180', href: `${base}apple-touch-icon.png` }],
    ['meta', { name: 'theme-color', content: '#f4f7f6', media: '(prefers-color-scheme: light)' }],
    ['meta', { name: 'theme-color', content: '#0c1317', media: '(prefers-color-scheme: dark)' }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:title', content: 'Laft' }],
    [
      'meta',
      {
        property: 'og:description',
        content: 'A Linux system built from one declarative file, kept as immutable generations, switched with one rename.',
      },
    ],
  ],

  markdown: {
    languages: [stavLanguage],
    theme: { light: frost, dark: night },
  },

  themeConfig: {
    logo: { light: '/logo.svg', dark: '/logo-dark.svg', alt: '' },

    nav: [
      { text: 'Guide', link: '/guide/introduction', activeMatch: '^/guide/(?!try-it)' },
      { text: 'Reference', link: '/reference/laft', activeMatch: '/reference/' },
      { text: 'Try it', link: '/guide/try-it', activeMatch: '^/guide/try-it' },
      {
        text: '0.1.0',
        items: [
          { text: 'Status and roadmap', link: '/guide/status' },
          { text: 'Stav, the configuration language', link: 'https://github.com/QaidVoid/stav' },
        ],
      },
    ],

    sidebar: {
      '/': [
        {
          text: 'Guide',
          items: [
            { text: 'What Laft is', link: '/guide/introduction' },
            { text: 'Core concepts', link: '/guide/concepts' },
            { text: 'Try it in QEMU', link: '/guide/try-it' },
            { text: 'Configuring a system', link: '/guide/configuration' },
            { text: 'Everyday use', link: '/guide/everyday' },
            { text: 'Writing recipes', link: '/guide/recipes' },
            { text: 'Building packages', link: '/guide/building' },
            { text: 'Status and roadmap', link: '/guide/status' },
          ],
        },
        {
          text: 'Reference',
          items: [
            { text: 'The laft command', link: '/reference/laft' },
            { text: 'The laft-build command', link: '/reference/laft-build' },
            { text: 'System configuration', link: '/reference/system' },
            { text: 'Recipes and stages', link: '/reference/recipe' },
            { text: 'Diagnostic codes', link: '/reference/codes' },
          ],
        },
      ],
    },

    socialLinks: [{ icon: 'github', link: repo }],

    editLink: {
      pattern: `${repo}/edit/main/docs/:path`,
      text: 'Edit this page',
    },

    search: { provider: 'local' },

    outline: { level: [2, 3], label: 'On this page' },

    footer: {
      message: 'Released under the MIT or Apache 2.0 license, at your option.',
    },
  },
})
