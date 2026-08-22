import { unified } from '@astrojs/markdown-remark';
import sitemap from '@astrojs/sitemap';
import starlight from '@astrojs/starlight';
import { defineConfig } from 'astro/config';

import { rehypePagefindCommandReferences } from './src/plugins/pagefind-command-references.mjs';
import { rehypeResponsiveTables } from './src/plugins/responsive-tables.mjs';
import { rehypeStableHeadingIds } from './src/plugins/stable-heading-ids.mjs';
import { sidebar } from './src/site-navigation.mjs';
import { sitemapCompatibility } from './src/plugins/sitemap-compatibility.mjs';
import { pluginWorktrunkTerminal } from './src/plugins/worktrunk-terminal.mjs';

const repository = 'https://github.com/max-sixty/worktrunk';
const site = 'https://worktrunk.dev';

export default defineConfig({
  site,
  devToolbar: {
    enabled: false,
  },
  markdown: {
    processor: unified({
      rehypePlugins: [
        rehypeStableHeadingIds,
        rehypePagefindCommandReferences,
        rehypeResponsiveTables,
      ],
    }),
  },
  integrations: [
    starlight({
      title: 'Worktrunk',
      description: 'Git worktree management for parallel AI agent workflows.',
      disable404Route: true,
      favicon: '/favicon.png',
      logo: {
        src: './public/logo.png',
        alt: 'Worktrunk',
      },
      social: [
        { icon: 'github', label: 'GitHub', href: repository },
      ],
      tableOfContents: {
        minHeadingLevel: 1,
        maxHeadingLevel: 2,
      },
      editLink: {
        baseUrl: `${repository}/edit/main/docs/`,
      },
      customCss: ['./src/styles/custom.css'],
      components: {
        Footer: './src/components/Footer.astro',
        Head: './src/components/Head.astro',
        PageTitle: './src/components/PageTitle.astro',
        SocialIcons: './src/components/SocialIcons.astro',
        ThemeSelect: './src/components/ThemeSelect.astro',
      },
      expressiveCode: {
        plugins: [pluginWorktrunkTerminal()],
        styleOverrides: {
          borderRadius: '0',
          codeFontFamily: 'var(--sl-font-mono)',
          frames: {
            frameBoxShadowCssValue: 'none',
          },
        },
      },
      sidebar,
    }),
    sitemap({
      filter: (page) => ![`${site}/404/`, `${site}/worktrunk/`].includes(page),
    }),
    sitemapCompatibility(),
  ],
});
