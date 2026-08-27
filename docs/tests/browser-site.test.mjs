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

function rgbChannels(value) {
  const channels = value.match(/[\d.]+/gu).slice(0, 3).map(Number);
  return value.startsWith('color(srgb')
    ? channels.map((channel) => channel * 255)
    : channels;
}

function relativeLuminance(value) {
  const linear = rgbChannels(value).map((channel) => {
    const normalized = channel / 255;
    return normalized <= 0.04045
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}

function contrastRatio(foreground, background) {
  const foregroundLuminance = relativeLuminance(foreground);
  const backgroundLuminance = relativeLuminance(background);
  return (Math.max(foregroundLuminance, backgroundLuminance) + 0.05)
    / (Math.min(foregroundLuminance, backgroundLuminance) + 0.05);
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

test('code artifacts keep their visual hierarchy in both themes', async () => {
  const browser = await webkit.launch();
  try {
    for (const theme of ['light', 'dark']) {
      const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, colorScheme: theme });

      await page.goto(`${baseUrl}/claude-code/`, { waitUntil: 'domcontentloaded' });
      await page.evaluate((selectedTheme) => {
        document.documentElement.dataset.theme = selectedTheme;
      }, theme);
      const commandStyles = await page.evaluate(() => {
        const frame = [...document.querySelectorAll('.expressive-code .frame')]
          .find((candidate) => candidate.textContent.includes(
            'codex plugin marketplace add max-sixty/worktrunk',
          ));
        return {
          background: getComputedStyle(frame.querySelector('pre')).backgroundColor,
          tokens: [...frame.querySelectorAll('.code span')]
            .filter((span) => span.textContent.trim())
            .map((span) => ({
              text: span.textContent,
              color: getComputedStyle(span).color,
            })),
        };
      });
      assert.notEqual(
        commandStyles.tokens.find(({ text }) => text === 'codex').color,
        commandStyles.tokens.find(({ text }) => text === 'plugin').color,
        `${theme} shell command flattens the executable and arguments to one color`,
      );
      for (const { text, color } of commandStyles.tokens) {
        const ratio = contrastRatio(color, commandStyles.background);
        assert.ok(ratio >= 4.5, `${theme} shell token ${text} contrast is ${ratio.toFixed(2)}:1`);
      }

      await page.goto(`${baseUrl}/list/`, { waitUntil: 'domcontentloaded' });
      await page.evaluate((selectedTheme) => {
        document.documentElement.dataset.theme = selectedTheme;
      }, theme);
      const listStyles = await page.evaluate(() => {
        const frame = [...document.querySelectorAll('.frame.is-terminal')]
          .find((candidate) => candidate.textContent.includes('feature-api'));
        const normal = frame.querySelector('.wt-output .code');
        const dim = frame.querySelector('.wt-terminal-dim:not(.wt-terminal-red)');
        return {
          normal: getComputedStyle(normal).color,
          dim: getComputedStyle(dim).color,
          dimOpacity: getComputedStyle(dim).opacity,
          commandColors: new Set(
            [...frame.querySelectorAll('.wt-command .code span')]
              .filter((span) => span.textContent.trim())
              .map((span) => getComputedStyle(span).color),
          ).size,
        };
      });
      assert.notEqual(listStyles.normal, listStyles.dim, `${theme} wt list dim text is not distinct`);
      assert.equal(listStyles.dimOpacity, '1', `${theme} wt list dim text relies on opacity`);
      assert.ok(listStyles.commandColors >= 2, `${theme} console command loses Bash syntax colors`);

      const referenceStyles = await page.evaluate(() => {
        const frame = document.querySelector('.frame.wt-command-reference');
        const roles = ['heading', 'command', 'option', 'value', 'meta'];
        return {
          present: Boolean(frame),
          background: getComputedStyle(frame.querySelector('pre')).backgroundColor,
          colors: roles.map((role) => getComputedStyle(
            frame.querySelector(`.wt-help-${role}`),
          ).color),
        };
      });
      assert.equal(referenceStyles.present, true, `${theme} command reference is not classified`);
      assert.equal(
        new Set(referenceStyles.colors).size,
        referenceStyles.colors.length,
        `${theme} command reference syntax roles collapse to the same color`,
      );
      for (const color of referenceStyles.colors) {
        const ratio = contrastRatio(color, referenceStyles.background);
        assert.ok(ratio >= 4.5, `${theme} help-role contrast is ${ratio.toFixed(2)}:1`);
      }

      await page.goto(`${baseUrl}/config/`, { waitUntil: 'domcontentloaded' });
      await page.evaluate((selectedTheme) => {
        document.documentElement.dataset.theme = selectedTheme;
      }, theme);
      const fileFrame = await page.evaluate(() => {
        const frame = [...document.querySelectorAll('.frame.has-title')]
          .find((candidate) => candidate.textContent.includes('~/.config/worktrunk/config.toml'));
        const title = frame.querySelector('.title');
        const pre = frame.querySelector('pre');
        return {
          title: title.textContent,
          titleBottom: title.getBoundingClientRect().bottom,
          codeTop: pre.getBoundingClientRect().top,
          titleFont: getComputedStyle(title).fontFamily,
          titleColor: getComputedStyle(title).color,
          titleBackground: getComputedStyle(title).backgroundColor,
          codeBackground: getComputedStyle(pre).backgroundColor,
          accentWidth: getComputedStyle(pre).borderInlineStartWidth,
        };
      });
      assert.equal(fileFrame.title, '~/.config/worktrunk/config.toml');
      assert.ok(fileFrame.titleFont.includes('JetBrains Mono'), `${theme} file label is not monospaced`);
      assert.ok(
        Math.abs(fileFrame.titleBottom - fileFrame.codeTop) < 1,
        `${theme} file label is detached from its code`,
      );
      assert.notEqual(
        fileFrame.titleBackground,
        fileFrame.codeBackground,
        `${theme} file label does not read separately from its code`,
      );
      const titleContrast = contrastRatio(fileFrame.titleColor, fileFrame.titleBackground);
      assert.ok(titleContrast >= 4.5, `${theme} file-label contrast is ${titleContrast.toFixed(2)}:1`);
      assert.equal(fileFrame.accentWidth, '3px');
      await page.close();
    }
  } finally {
    await browser.close();
  }
});
