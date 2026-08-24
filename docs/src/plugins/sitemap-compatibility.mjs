import { copyFile } from 'node:fs/promises';

/** Keeps the sitemap URL published by the previous site generator working. */
export function sitemapCompatibility() {
  return {
    name: 'worktrunk-sitemap-compatibility',
    hooks: {
      'astro:build:done': async ({ dir }) => {
        await copyFile(
          new URL('sitemap-index.xml', dir),
          new URL('sitemap.xml', dir),
        );
      },
    },
  };
}
