import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { chromium } from 'playwright';

const docsRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const repoRoot = path.resolve(docsRoot, '..');
const args = new Map();

for (let index = 2; index < process.argv.length; index += 1) {
  const name = process.argv[index];
  if (!name.startsWith('--')) throw new Error(`Unexpected argument: ${name}`);
  const value = process.argv[index + 1];
  if (!value || value.startsWith('--')) throw new Error(`Missing value for ${name}`);
  args.set(name.slice(2), value);
  index += 1;
}

const referenceBase = new URL(args.get('reference') ?? 'https://worktrunk.dev/');
const candidateBase = new URL(args.get('candidate') ?? 'http://127.0.0.1:4321/');
const timestamp = new Date().toISOString().replaceAll(':', '-').replaceAll('.', '-');
const outputDir = path.resolve(args.get('output') ?? path.join(repoRoot, '.tmp', 'site-parity', timestamp));

const routes = [
  { path: '/', label: 'Home' },
  { path: '/switch/', label: 'Switch' },
  { path: '/extending/', label: 'Extending' },
  { path: '/config/', label: 'Config' },
];
const viewports = [
  { name: 'desktop', width: 1440, height: 1000 },
  { name: 'mobile', width: 390, height: 844 },
];
const themes = ['light', 'dark'];

await mkdir(outputDir, { recursive: true });

let browser;
try {
  browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_BROWSER_CHANNEL ?? 'chrome' });
} catch (error) {
  if (process.env.PLAYWRIGHT_BROWSER_CHANNEL) throw error;
  browser = await chromium.launch();
}

const report = {
  generatedAt: new Date().toISOString(),
  reference: referenceBase.href,
  candidate: candidateBase.href,
  outputDir,
  cases: [],
};

try {
  for (const viewport of viewports) {
    for (const theme of themes) {
      for (const route of routes) {
        const id = `${route.path === '/' ? 'home' : route.path.split('/').filter(Boolean).join('-')}-${viewport.name}-${theme}`;
        const entry = { id, route: route.path, routeLabel: route.label, viewport, theme };

        for (const target of [
          { name: 'reference', base: referenceBase },
          { name: 'candidate', base: candidateBase },
        ]) {
          const context = await browser.newContext({
            viewport: { width: viewport.width, height: viewport.height },
            colorScheme: theme,
            reducedMotion: 'reduce',
          });
          await context.addInitScript((selectedTheme) => {
            localStorage.setItem('starlight-theme', selectedTheme);
          }, theme);
          const page = await context.newPage();
          const consoleErrors = [];
          const failedRequests = [];
          page.on('console', (message) => {
            if (message.type() === 'error') consoleErrors.push(message.text());
          });
          page.on('requestfailed', (request) => {
            failedRequests.push({ url: request.url(), error: request.failure()?.errorText ?? 'unknown' });
          });

          const url = new URL(route.path, target.base);
          const response = await page.goto(url.href, { waitUntil: 'domcontentloaded', timeout: 30_000 });
          await page.evaluate(async () => {
            if (document.fonts?.ready) await document.fonts.ready;
          });

          const screenshot = `${id}-${target.name}.png`;
          await page.screenshot({
            path: path.join(outputDir, screenshot),
            fullPage: true,
            animations: 'disabled',
          });

          const observation = await page.evaluate(() => {
            const visible = (element) => {
              if (!element) return false;
              const style = getComputedStyle(element);
              const rect = element.getBoundingClientRect();
              return style.display !== 'none' && style.visibility !== 'hidden' && rect.width > 0 && rect.height > 0;
            };
            const rect = (element) => {
              if (!element) return null;
              const value = element.getBoundingClientRect();
              return {
                x: Math.round(value.x * 100) / 100,
                y: Math.round(value.y * 100) / 100,
                width: Math.round(value.width * 100) / 100,
                height: Math.round(value.height * 100) / 100,
              };
            };
            const textElement = (selector, text) =>
              [...document.querySelectorAll(selector)].find((element) => element.textContent?.trim() === text);
            const comparison = [...document.querySelectorAll('table')].find((table) =>
              /Worktrunk/.test(table.textContent ?? '') && /Plain Git/i.test(table.textContent ?? '')
            );
            const prose = document.querySelector('[data-contract="home-content-rail"] > p')
              ?? document.querySelector('main .content > p, .sl-markdown-content > p, main p');
            const demo = document.querySelector('figure.demo');
            const railRects = [prose, demo, comparison].map(rect).filter(Boolean);
            const lefts = railRects.map((value) => value.x);
            const starLink = document.querySelector('a[aria-label="GitHub stars"]')
              ?? [...document.querySelectorAll('a[href="https://github.com/max-sixty/worktrunk"]')]
                .find((anchor) => /stars/i.test(anchor.textContent ?? ''));
            const starImage = starLink?.querySelector('img[src*="img.shields.io/github/stars"]') ?? null;
            const mobileMenu = textElement('button', 'Menu') ?? document.querySelector('button[aria-label*="menu" i]');
            const hrefs = [...document.querySelectorAll('a[href]')].map((anchor) => anchor.href);

            return {
              title: document.title,
              document: {
                width: document.documentElement.scrollWidth,
                clientWidth: document.documentElement.clientWidth,
                height: document.documentElement.scrollHeight,
              },
              geometry: {
                header: rect(document.querySelector('header')),
                hero: rect(document.querySelector('[data-contract="home-hero"], .hero')),
                heroTitle: rect(textElement('h1', 'Worktrunk')),
                heroLogo: rect(document.querySelector('[data-contract="hero-logo"] img, .hero-image, .hero > img')),
                prose: rect(prose),
                demo: rect(demo),
                comparison: rect(comparison),
                contentRailLeftSpread: lefts.length > 1 ? Math.max(...lefts) - Math.min(...lefts) : null,
              },
              capabilities: {
                star: {
                  linkFound: Boolean(starLink),
                  href: starLink?.href ?? null,
                  visibleText: starLink?.textContent?.trim() ?? null,
                  liveBadge: Boolean(starImage),
                  imageSrc: starImage?.src ?? null,
                  imageComplete: starImage?.complete ?? false,
                  naturalWidth: starImage?.naturalWidth ?? 0,
                  naturalHeight: starImage?.naturalHeight ?? 0,
                },
                mobileMenu: {
                  present: Boolean(mobileMenu),
                  visible: visible(mobileMenu),
                },
                destinations: {
                  github: hrefs.some((href) => href === 'https://github.com/max-sixty/worktrunk'),
                  crates: hrefs.some((href) => href.startsWith('https://crates.io/crates/worktrunk')),
                  share: hrefs.some((href) => href.startsWith('https://twitter.com/intent/tweet')),
                  tips: hrefs.some((href) => new URL(href).pathname === '/tips-patterns/'),
                },
              },
            };
          });

          entry[target.name] = {
            url: url.href,
            status: response?.status() ?? null,
            screenshot,
            consoleErrors,
            failedRequests,
            ...observation,
          };
          await context.close();
        }
        report.cases.push(entry);
      }
    }
  }
} finally {
  await browser.close();
}

const escapeHtml = (value) => String(value)
  .replaceAll('&', '&amp;')
  .replaceAll('<', '&lt;')
  .replaceAll('>', '&gt;')
  .replaceAll('"', '&quot;');

const homeCases = report.cases.filter((entry) => entry.route === '/');
const parityRows = homeCases.map((entry) => {
  const referenceCapabilities = entry.reference.capabilities;
  const candidateCapabilities = entry.candidate.capabilities;
  const starClass = candidateCapabilities.star.liveBadge && candidateCapabilities.star.naturalWidth > 0 ? 'pass' : 'fail';
  return `<tr>
    <td>${escapeHtml(`${entry.viewport.name} / ${entry.theme}`)}</td>
    <td>${referenceCapabilities.star.liveBadge ? `${referenceCapabilities.star.naturalWidth}×${referenceCapabilities.star.naturalHeight}` : 'missing'}</td>
    <td class="${starClass}">${candidateCapabilities.star.liveBadge ? `${candidateCapabilities.star.naturalWidth}×${candidateCapabilities.star.naturalHeight}` : `missing; text=${escapeHtml(candidateCapabilities.star.visibleText)}`}</td>
    <td>${entry.reference.geometry.contentRailLeftSpread ?? 'n/a'}px</td>
    <td>${entry.candidate.geometry.contentRailLeftSpread ?? 'n/a'}px</td>
  </tr>`;
}).join('\n');

const casesHtml = report.cases.map((entry) => `<section>
  <h2>${escapeHtml(`${entry.routeLabel} · ${entry.viewport.name} · ${entry.theme}`)}</h2>
  <div class="pair">
    <figure><figcaption>Reference</figcaption><a href="${entry.reference.screenshot}"><img src="${entry.reference.screenshot}" alt="Reference ${escapeHtml(entry.routeLabel)}"></a></figure>
    <figure><figcaption>Candidate</figcaption><a href="${entry.candidate.screenshot}"><img src="${entry.candidate.screenshot}" alt="Candidate ${escapeHtml(entry.routeLabel)}"></a></figure>
  </div>
</section>`).join('\n');

const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Worktrunk site parity report</title>
<style>
  :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
  body { margin: 0 auto; max-width: 110rem; padding: 2rem; background: #eeece7; color: #2d2926; }
  h1, h2 { letter-spacing: -.025em; } h2 { margin-top: 4rem; }
  table { border-collapse: collapse; background: #fff; } th, td { padding: .6rem .8rem; border: 1px solid #d8d3cb; text-align: left; }
  .pass { color: #28650b; } .fail { color: #a61b1b; font-weight: 700; }
  .pair { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; align-items: start; }
  figure { margin: 0; } figcaption { margin-bottom: .5rem; font: 600 .78rem ui-monospace, monospace; text-transform: uppercase; letter-spacing: .08em; }
  img { display: block; width: 100%; height: auto; background: white; border: 1px solid #d8d3cb; }
  @media (max-width: 60rem) { .pair { grid-template-columns: 1fr; } body { padding: 1rem; } }
</style></head><body>
<h1>Worktrunk site parity report</h1>
<p><strong>Reference:</strong> ${escapeHtml(referenceBase.href)}<br><strong>Candidate:</strong> ${escapeHtml(candidateBase.href)}<br><strong>Generated:</strong> ${escapeHtml(report.generatedAt)}</p>
<p>This report exposes visual and capability differences; it does not decide whether an intentional change is better.</p>
<h2>Homepage capability floor</h2>
<table><thead><tr><th>Case</th><th>Reference stars</th><th>Candidate stars</th><th>Reference rail spread</th><th>Candidate rail spread</th></tr></thead><tbody>${parityRows}</tbody></table>
${casesHtml}
</body></html>`;

await Promise.all([
  writeFile(path.join(outputDir, 'report.json'), `${JSON.stringify(report, null, 2)}\n`),
  writeFile(path.join(outputDir, 'index.html'), html),
]);

const missingCandidateStars = homeCases.filter((entry) => !entry.candidate.capabilities.star.liveBadge || entry.candidate.capabilities.star.naturalWidth === 0);
console.log(`Parity report: ${path.join(outputDir, 'index.html')}`);
console.log(`Machine data:  ${path.join(outputDir, 'report.json')}`);
console.log(`Captured ${report.cases.length} cases (${report.cases.length * 2} screenshots).`);
if (missingCandidateStars.length > 0) {
  console.log(`Capability mismatch: live GitHub star badge missing in ${missingCandidateStars.length}/${homeCases.length} candidate homepage cases.`);
}
