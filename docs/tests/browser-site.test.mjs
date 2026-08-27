import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import path from 'node:path';
import { after, before, test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { webkit } from 'playwright';

const docsRoot = fileURLToPath(new URL('..', import.meta.url));
const dist = path.join(docsRoot, 'dist');
const mobileWidths = [320, 393];
const mimeTypes = new Map([
  ['.css', 'text/css'],
  ['.gif', 'image/gif'],
  ['.html', 'text/html'],
  ['.js', 'text/javascript'],
  ['.json', 'application/json'],
  ['.svg', 'image/svg+xml'],
  ['.webp', 'image/webp'],
]);

let server;
let baseUrl;

before(async () => {
  server = createServer(async (request, response) => {
    const url = new URL(request.url, 'http://127.0.0.1');
    const relative = decodeURIComponent(url.pathname).replace(/^\/+/, '');
    const file = path.join(dist, relative, url.pathname.endsWith('/') ? 'index.html' : '');
    try {
      const body = await readFile(file);
      response.writeHead(200, { 'content-type': mimeTypes.get(path.extname(file)) ?? 'application/octet-stream' });
      response.end(body);
    } catch {
      response.writeHead(404);
      response.end('Not found');
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  baseUrl = `http://127.0.0.1:${server.address().port}`;
});

after(async () => {
  await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
});

async function sitemapRoutes() {
  const sitemap = await readFile(path.join(dist, 'sitemap-0.xml'), 'utf8');
  return [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/gu)]
    .map((match) => new URL(match[1]).pathname);
}

test('mobile pages stay viewport-bound while code remains readable', { timeout: 120_000 }, async () => {
  const browser = await webkit.launch();
  const publicRoutes = await sitemapRoutes();
  let sawScrollableTerminal = false;
  try {
    for (const theme of ['light', 'dark']) {
      for (const width of mobileWidths) {
        const page = await browser.newPage({ viewport: { width, height: 844 }, colorScheme: theme });
        for (const route of publicRoutes) {
          await page.goto(`${baseUrl}${route}`, { waitUntil: 'domcontentloaded' });
          await page.evaluate((selectedTheme) => {
            document.documentElement.dataset.theme = selectedTheme;
          }, theme);

          const layout = await page.evaluate(() => {
            const rgb = (value) => {
              const channels = value.match(/[\d.]+/gu).slice(0, 3).map(Number);
              return value.startsWith('color(srgb')
                ? channels.map((channel) => channel * 255)
                : channels;
            };
            const luminance = (color) => {
              const linear = color.map((channel) => {
                const normalized = channel / 255;
                return normalized <= 0.04045
                  ? normalized / 12.92
                  : ((normalized + 0.055) / 1.055) ** 2.4;
              });
              return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
            };
            const contrast = (foreground, background) => {
              const lighter = Math.max(luminance(foreground), luminance(background));
              const darker = Math.min(luminance(foreground), luminance(background));
              return (lighter + 0.05) / (darker + 0.05);
            };
            const dimContrasts = [...document.querySelectorAll('.wt-terminal-dim')].map((span) => {
              const style = getComputedStyle(span);
              const foreground = rgb(style.color);
              const background = rgb(getComputedStyle(span.closest('pre')).backgroundColor);
              const opacity = Number(style.opacity);
              const effective = foreground.map((channel, index) => (
                opacity * channel + (1 - opacity) * background[index]
              ));
              return contrast(effective, background);
            });
            const fixedTerminal = [...document.querySelectorAll('.frame.is-terminal')]
              .filter((frame) => frame.querySelector('.wt-output'))
              .map((frame) => frame.querySelector('pre'))
              .find((pre) => pre.scrollWidth > pre.clientWidth + 1);
            if (fixedTerminal) fixedTerminal.scrollLeft = fixedTerminal.scrollWidth;
            return {
              viewportWidth: document.documentElement.clientWidth,
              documentWidth: document.documentElement.scrollWidth,
              dimContrasts,
              fixedTerminalScrollLeft: fixedTerminal?.scrollLeft ?? 0,
              frames: [...document.querySelectorAll('.expressive-code .frame')].map((frame) => {
                const pre = frame.querySelector('pre');
                return {
                  className: frame.className,
                  managedTerminal: Boolean(
                    frame.querySelector('.wt-command, .wt-output, .wt-copyable'),
                  ),
                  hasOutput: Boolean(frame.querySelector('.wt-output')),
                  frameLeft: frame.getBoundingClientRect().left,
                  frameRight: frame.getBoundingClientRect().right,
                  preClientWidth: pre?.clientWidth ?? 0,
                  preScrollWidth: pre?.scrollWidth ?? 0,
                };
              }),
            };
          });

          assert.equal(
            layout.documentWidth,
            layout.viewportWidth,
            `${theme} ${width}px ${route} widens the document`,
          );
          for (const ratio of layout.dimContrasts) {
            assert.ok(
              ratio >= 4.5,
              `${theme} ${width}px ${route} dim text contrast is ${ratio.toFixed(2)}:1`,
            );
          }
          for (const frame of layout.frames) {
            assert.ok(frame.frameLeft >= -1, `${theme} ${width}px ${route} code starts offscreen`);
            assert.ok(
              frame.frameRight <= layout.viewportWidth + 1,
              `${theme} ${width}px ${route} code ends offscreen`,
            );
            const terminal = frame.className.includes('is-terminal');
            const commandsOnly = frame.className.includes('wt-commands-only');
            if (terminal) {
              assert.equal(
                commandsOnly,
                !frame.hasOutput,
                `${theme} ${width}px ${route} misclassifies a terminal block`,
              );
            }
            const wraps = !terminal || commandsOnly;
            if (wraps) {
              assert.ok(
                frame.preScrollWidth <= frame.preClientWidth + 1,
                `${theme} ${width}px ${route} wrappable code still scrolls horizontally`,
              );
            }
          }

          if (layout.fixedTerminalScrollLeft > 0) sawScrollableTerminal = true;

          await page.evaluate(() => window.scrollTo({ left: 100, top: 0 }));
          assert.equal(await page.evaluate(() => window.scrollX), 0, `${theme} ${width}px ${route} pans sideways`);
        }
        await page.close();
      }
    }
    assert.ok(sawScrollableTerminal, 'expected a fixed terminal table to retain internal scrolling');
  } finally {
    await browser.close();
  }
});
