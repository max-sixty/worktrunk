import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  keepCurrentTocLinkVisible,
  syncTocCurrentToHash,
} from '../src/components/toc-scroll.mjs';

const docsRoot = fileURLToPath(new URL('..', import.meta.url));
const dist = path.join(docsRoot, 'dist');
const publicRoutes = [
  '/',
  '/claude-code/',
  '/code-signing/',
  '/config/',
  '/extending/',
  '/faq/',
  '/hook/',
  '/list/',
  '/llm-commits/',
  '/merge/',
  '/remove/',
  '/step/',
  '/switch/',
  '/tips-patterns/',
];

async function htmlFiles(directory = dist) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const file = path.join(directory, entry.name);
    return entry.isDirectory() ? htmlFiles(file) : entry.name.endsWith('.html') ? [file] : [];
  }));
  return nested.flat();
}

async function allFiles(directory = dist) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const file = path.join(directory, entry.name);
    return entry.isDirectory() ? allFiles(file) : [file];
  }));
  return nested.flat();
}

function routeFile(pathname) {
  if (pathname === '/') return path.join(dist, 'index.html');
  if (pathname === '/404/') return path.join(dist, '404.html');
  if (pathname.endsWith('/')) return path.join(dist, pathname, 'index.html');
  if (pathname.endsWith('.html')) return path.join(dist, pathname);
}

const renderedPages = [
  '/404/',
  ...publicRoutes,
  '/worktrunk/',
].map(routeFile);

function fileRoute(file) {
  const relative = path.relative(dist, file);
  if (relative === 'index.html') return '/';
  if (relative === '404.html') return '/404/';
  return `/${path.dirname(relative)}/`;
}

function tableSummaries(html) {
  const text = (value) => value
    .replace(/<[^>]*>/g, '')
    .replaceAll('&#x26;', '&')
    .replaceAll('&amp;', '&')
    .replace(/\s+/g, ' ')
    .trim();
  return [...html.matchAll(/<table\b([^>]*)>([\s\S]*?)<\/table>/g)].map((match) => {
    const body = match[2].match(/<tbody>([\s\S]*?)<\/tbody>/)?.[1] ?? '';
    const rows = [...body.matchAll(/<tr\b[^>]*>([\s\S]*?)<\/tr>/g)];
    return {
      attributes: match[1],
      headers: [...match[2].matchAll(/<th\b[^>]*>([\s\S]*?)<\/th>/g)]
        .map((header) => text(header[1])),
      rowLabels: rows.map((row) => (
        [...row[1].matchAll(/<span\b([^>]*)>([\s\S]*?)<\/span>/g)]
          .filter((label) => (
            /\baria-hidden="true"/.test(label[1])
            && /\bclass="[^"]*\bwt-responsive-record-label\b[^"]*"/.test(label[1])
          ))
          .map((label) => text(label[2]))
      )),
    };
  });
}

function renderedText(html) {
  return html
    .replace(/<[^>]*>/g, '')
    .replace(/&#x([\da-f]+);/giu, (_, value) => String.fromCodePoint(Number.parseInt(value, 16)))
    .replace(/&#(\d+);/gu, (_, value) => String.fromCodePoint(Number(value)))
    .replaceAll('&quot;', '"')
    .replaceAll('&gt;', '>')
    .replaceAll('&lt;', '<')
    .replaceAll('&amp;', '&');
}

test('build preserves the public route contract', async () => {
  const files = new Set(await htmlFiles());
  const buildFiles = new Set(await allFiles());
  const expected = [
    path.join(dist, '404.html'),
    ...publicRoutes.map(routeFile),
    routeFile('/worktrunk/'),
  ];
  for (const file of expected) assert.ok(files.has(file), `missing ${file}`);

  const sitemap = await readFile(path.join(dist, 'sitemap-0.xml'), 'utf8');
  for (const route of publicRoutes) {
    assert.match(sitemap, new RegExp(`<loc>https://worktrunk\\.dev${route}</loc>`));
  }
  assert.doesNotMatch(sitemap, /worktrunk\.dev\/(?:404|worktrunk)\//);

  const robots = await readFile(path.join(dist, 'robots.txt'), 'utf8');
  for (const match of robots.matchAll(/^Sitemap:\s+(\S+)$/gm)) {
    const url = new URL(match[1]);
    assert.equal(url.origin, 'https://worktrunk.dev');
    assert.ok(buildFiles.has(path.join(dist, url.pathname)), `robots.txt references missing ${url.pathname}`);
  }

  const compatibilityPage = await readFile(routeFile('/worktrunk/'), 'utf8');
  assert.match(compatibilityPage, /<link rel="canonical" href="https:\/\/worktrunk\.dev\/"/);
  assert.match(compatibilityPage, /<meta name="robots" content="noindex"/);
  assert.match(compatibilityPage, /<meta property="og:url" content="https:\/\/worktrunk\.dev\/"/);
  assert.doesNotMatch(compatibilityPage, /<meta property="og:url" content="https:\/\/worktrunk\.dev\/worktrunk\/"/);

  const homepage = await readFile(routeFile('/'), 'utf8');
  assert.match(homepage, /<title>Worktrunk — Git worktree management for parallel AI agent workflows<\/title>/);
  assert.doesNotMatch(homepage, /data-contract="home-hero"/);
  assert.doesNotMatch(homepage, /data-has-hero/);
  assert.doesNotMatch(homepage, />\s*Learn More\s*</);
  assert.match(
    homepage,
    /<a\b[^>]*data-contract="github-stars"[^>]*href="https:\/\/github\.com\/max-sixty\/worktrunk"[^>]*>[\s\S]*?<img\b[^>]*src="https:\/\/img\.shields\.io\/github\/stars\/max-sixty\/worktrunk\?style=social"[^>]*>[\s\S]*?<\/a>/,
  );
  assert.doesNotMatch(homepage, />\s*GitHub stars\s*</);
  assert.match(
    homepage,
    /<div\b[^>]*class="wt-home"[^>]*>[\s\S]*?<p>Worktrunk is a CLI for Git worktree management, designed for <strong>parallel AI agent\s+workflows<\/strong>\.<\/p>/,
  );
  assert.match(homepage, /<button\b[^>]*aria-label="Menu"[^>]*aria-controls="starlight__sidebar"/);
  assert.match(homepage, /<div id="starlight__sidebar" class="sidebar-pane\b/);
  assert.match(homepage, /<a\b[^>]*href="\/switch\/"[^>]*><span\b[^>]*>wt switch<\/span><\/a>/);
  assert.match(homepage, /<a\b[^>]*href="\/faq\/"[^>]*><span\b[^>]*>FAQ<\/span><\/a>/);
  assert.doesNotMatch(homepage, /data-has-toc/);
  assert.doesNotMatch(homepage, /<nav aria-labelledby="starlight__on-this-page">/);
  assert.doesNotMatch(homepage, /<starlight-theme-select>/);
  assert.match(homepage, /<div\b[^>]*class="wt-home"[^>]*data-contract="home-content-rail"/);
  assert.match(homepage, /<h1 id="_top"[^>]*><span id="worktrunk"[^>]*>Worktrunk<\/span><\/h1>/);
  assert.match(
    homepage,
    /<source srcset="\/assets\/docs\/dark\/wt-core-mobile\.gif"[^>]*data-demo-theme="dark"[^>]*data-demo-media="\(max-width: 42rem\)"/,
  );
  assert.match(
    homepage,
    /<source srcset="\/assets\/docs\/light\/wt-core-mobile\.gif"[^>]*data-demo-theme="light"[^>]*data-demo-media="\(max-width: 42rem\)"/,
  );
  assert.match(homepage, /<img src="\/assets\/docs\/light\/wt-core\.gif"/);
  assert.equal(homepage.match(/<h1\b/g)?.length, 1);
  assert.match(homepage, /<pre><code>git worktree add -b feat \.\.\/repo\.feat &#x26;&#x26; \\\ncd \.\.\/repo\.feat &#x26;&#x26; \\\nclaude<\/code><\/pre>/);
  const comparison = homepage.match(/<table class="cmd-compare">([\s\S]*?)<\/table>/)?.[1];
  assert.ok(comparison, 'homepage is missing the command comparison table');
  assert.equal(comparison.match(/<td data-label="Worktrunk">/g)?.length, 4);
  assert.equal(comparison.match(/<td data-label="Plain git">/g)?.length, 4);
  assert.match(homepage, /class="wt-positive"/);
  assert.match(homepage, /class="wt-negative"/);
  const switchPage = await readFile(routeFile('/switch/'), 'utf8');
  assert.match(switchPage, /<nav aria-labelledby="starlight__on-this-page">/);
  assert.match(
    switchPage,
    /<a\b[^>]*data-contract="github-stars"[^>]*href="https:\/\/github\.com\/max-sixty\/worktrunk"[^>]*>[\s\S]*?<img\b[^>]*src="https:\/\/img\.shields\.io\/github\/stars\/max-sixty\/worktrunk\?style=social"[^>]*>[\s\S]*?<\/a>/,
  );
  assert.doesNotMatch(switchPage, /<starlight-theme-select>/);

  const listPage = await readFile(routeFile('/list/'), 'utf8');
  assert.match(listPage, /href="https:\/\/github\.com\/max-sixty\/worktrunk\/edit\/main\/docs\/src\/content\/docs\/list\.md"/);
  assert.doesNotMatch(listPage, /docs\/src\/content\/docs\/src\/content\/docs/);

  const configPage = await readFile(routeFile('/config/'), 'utf8');
  const configToc = configPage.match(
    /<nav aria-labelledby="starlight__on-this-page">([\s\S]*?)<\/nav>/,
  )?.[1];
  assert.ok(configToc, 'config page is missing its desktop table of contents');
  assert.equal(
    [...configToc.matchAll(/<a href="#[^"]+"/g)].length,
    32,
    'config table of contents should expose overview and structural section headings only',
  );
  for (const [id, title] of [
    ['user-configuration', 'User Configuration'],
    ['project-configuration', 'Project Configuration'],
    ['shell-integration', 'Shell Integration'],
    ['other', 'Other'],
    ['subcommands', 'Subcommands'],
  ]) {
    assert.match(
      configToc,
      new RegExp(`<a href="#${id}"[^>]*>[\\s\\S]*?<span[^>]*>${title}</span></a>`),
    );
  }
  assert.doesNotMatch(configToc, /href="#claude-code"/);
  assert.doesNotMatch(configToc, /href="#command-reference-1"/);

  const structuralSections = new Map([
    ['/hook/', ['hook-types', 'security', 'configuration', 'running-hooks-manually', 'recipes']],
    ['/list/', ['subcommands']],
    ['/step/', ['subcommands']],
  ]);
  for (const [route, ids] of structuralSections) {
    const page = await readFile(routeFile(route), 'utf8');
    const toc = page.match(/<nav aria-labelledby="starlight__on-this-page">([\s\S]*?)<\/nav>/)?.[1];
    assert.ok(toc, `${route} is missing its desktop table of contents`);
    for (const id of ids) assert.match(toc, new RegExp(`<a href="#${id}"`));
  }
});

test('built pages omit low-value footer navigation', async () => {
  for (const page of renderedPages) {
    const html = await readFile(page, 'utf8');
    assert.doesNotMatch(
      html,
      /class="[^"]*\bpagination-links\b/,
      `${page} renders previous/next pagination`,
    );
    assert.doesNotMatch(
      html,
      /data-contract="project-links"/,
      `${page} renders duplicate project links`,
    );
  }
});

test('nested command references expose unique search-only fragment titles', async () => {
  const stepPage = await readFile(routeFile('/step/'), 'utf8');
  assert.match(stepPage, /<h2 id="command-reference">Command reference<\/h2>/);

  const references = [...stepPage.matchAll(
    /<h3 id="command-reference-\d+"><span class="wt-pagefind-fragment-title" hidden aria-hidden="true">(wt step [^<]+) — <\/span>Command reference<\/h3>/g,
  )].map((match) => match[1]);
  assert.deepEqual(references, [
    'wt step commit',
    'wt step squash',
    'wt step rebase',
    'wt step push',
    'wt step diff',
    'wt step copy-ignored',
    'wt step eval',
    'wt step for-each',
    'wt step promote',
    'wt step prune',
    'wt step relocate',
    'wt step tether',
  ]);

  const listPage = await readFile(routeFile('/list/'), 'utf8');
  assert.match(
    listPage,
    /<h3 id="command-reference-1"><span class="wt-pagefind-fragment-title" hidden aria-hidden="true">wt list statusline — <\/span>Command reference<\/h3>/,
  );
});

test('built pages only reference emitted local assets', async () => {
  const files = new Set(await allFiles());

  for (const page of renderedPages) {
    const html = await readFile(page, 'utf8');
    const assertAsset = (reference) => {
      const url = new URL(reference.replaceAll('&amp;', '&'), `https://worktrunk.dev${fileRoute(page)}`);
      if (url.origin !== 'https://worktrunk.dev' || url.pathname.endsWith('/')) return;
      const target = path.join(dist, decodeURIComponent(url.pathname));
      assert.ok(files.has(target), `${page} references missing asset ${url.pathname}`);
    };
    for (const match of html.matchAll(/\s(href|src|srcset)="([^"]+)"/g)) {
      const references = match[1] === 'srcset'
        ? match[2].split(',').map((candidate) => candidate.trim().split(/\s+/, 1)[0])
        : [match[2]];
      for (const reference of references) assertAsset(reference);
    }
    for (const match of html.matchAll(/<meta (?:property="og:image"|name="twitter:image") content="([^"]+)"/g)) {
      assertAsset(match[1]);
    }
  }
});

test('output-only console blocks do not expose copy controls', async () => {
  let outputOnlyBlocks = 0;
  for (const page of renderedPages) {
    const html = await readFile(page, 'utf8');
    for (const match of html.matchAll(/<figure class="frame is-terminal[^"]*">([\s\S]*?)<\/figure>/g)) {
      const frame = match[1];
      if (!/class="ec-line wt-output"/.test(frame)) continue;
      if (/class="ec-line wt-(?:command|copyable)"/.test(frame)) continue;
      outputOnlyBlocks += 1;
      assert.doesNotMatch(frame, /class="copy"/, `${page} exposes a copy control for captured output`);
    }
  }
  assert.equal(outputOnlyBlocks, 1, 'expected the FAQ approval transcript to exercise this case');
});

test('command-bearing console blocks emit command-only copy payloads', async () => {
  let commandBearingBlocks = 0;
  for (const page of renderedPages) {
    const html = await readFile(page, 'utf8');
    for (const match of html.matchAll(/<figure class="frame is-terminal[^"]*">([\s\S]*?)<\/figure>/g)) {
      const frame = match[1];
      const lines = [...frame.matchAll(
        /<div class="ec-line wt-(command|copyable|output)"><div class="code">([\s\S]*?)<\/div><\/div>/g,
      )];
      const expected = lines
        .filter((line) => line[1] !== 'output')
        .map((line) => renderedText(line[2]).replace(/\n$/u, ''));
      if (expected.length === 0) continue;

      commandBearingBlocks += 1;
      const encodedPayload = frame.match(/<button\b[^>]*\bdata-code="([^"]*)"/)?.[1];
      assert.notEqual(encodedPayload, undefined, `${page} is missing a terminal copy payload`);
      assert.equal(renderedText(encodedPayload), expected.join('\u007f'), `${page} copies captured output`);
    }
  }
  assert.ok(commandBearingBlocks > 0, 'expected command-bearing console blocks');
});

test('titleless code frames do not render decorative headers', async () => {
  for (const page of renderedPages) {
    const html = await readFile(page, 'utf8');
    assert.doesNotMatch(
      html,
      /<figcaption class="header"><span class="title"><\/span>/,
      `${page} renders chrome for an untitled code block`,
    );
  }
});

test('long desktop TOCs keep the current link inside their own scrollport', () => {
  const scrollport = {
    clientHeight: 1_000,
    scrollHeight: 1_961,
    scrollTop: 0,
    getBoundingClientRect: () => ({ top: 0, bottom: 1_000 }),
  };
  const visibleToc = {
    closest: () => scrollport,
    getClientRects: () => [{}],
  };
  const current = {
    closest: () => visibleToc,
    getBoundingClientRect: () => ({ top: 1_386.84375, bottom: 1_411.09375 }),
  };

  assert.equal(keepCurrentTocLinkVisible(current), true);
  assert.equal(scrollport.scrollTop, 459.09375);

  current.getBoundingClientRect = () => ({ top: 500, bottom: 525 });
  assert.equal(keepCurrentTocLinkVisible(current), false);
  assert.equal(scrollport.scrollTop, 459.09375);

  current.getBoundingClientRect = () => ({ top: 20, bottom: 45 });
  assert.equal(keepCurrentTocLinkVisible(current), true);
  assert.equal(scrollport.scrollTop, 431.09375);

  visibleToc.getClientRects = () => [];
  current.getBoundingClientRect = () => ({ top: 1_386.84375, bottom: 1_411.09375 });
  assert.equal(keepCurrentTocLinkVisible(current), false);
  assert.equal(scrollport.scrollTop, 431.09375);

  visibleToc.getClientRects = () => [{}];
  scrollport.scrollHeight = scrollport.clientHeight;
  assert.equal(keepCurrentTocLinkVisible(current), false);
  assert.equal(scrollport.scrollTop, 431.09375);
});

test('hash navigation selects the requested section in every rendered TOC', () => {
  const link = (hash, current = false) => ({
    hash,
    current,
    getAttribute(name) {
      return name === 'aria-current' && this.current ? 'true' : null;
    },
    setAttribute(name, value) {
      if (name === 'aria-current') this.current = value === 'true';
    },
    removeAttribute(name) {
      if (name === 'aria-current') this.current = false;
    },
    textContent: hash.slice(1).replaceAll('-', ' '),
  });
  const root = (mobile = false) => {
    const overview = link('#_top', true);
    const requested = link('#reporting-a-problem');
    const display = mobile ? { textContent: '' } : null;
    return {
      display,
      links: [overview, requested],
      querySelector(selector) {
        return selector === '.display-current' ? display : null;
      },
      querySelectorAll(selector) {
        if (selector === 'a') return this.links;
        if (selector === 'a[aria-current="true"]') {
          return this.links.filter((candidate) => candidate.current);
        }
        return [];
      },
    };
  };
  const desktop = root();
  const mobile = root(true);
  const doc = {
    querySelectorAll: () => [desktop, mobile],
  };

  assert.equal(syncTocCurrentToHash(doc, '#reporting-a-problem'), true);
  for (const toc of [desktop, mobile]) {
    assert.equal(toc.links[0].current, false);
    assert.equal(toc.links[1].current, true);
  }
  assert.equal(mobile.display.textContent, 'reporting a problem');
  assert.equal(syncTocCurrentToHash(doc, '#missing'), false);
});

test('every structural content heading appears in the page outline', async () => {
  for (const route of publicRoutes.filter((candidate) => candidate !== '/')) {
    const page = await readFile(routeFile(route), 'utf8');
    const headings = [...page.matchAll(/<h[12]\b[^>]*\bid="([^"]+)"/g)]
      .map((match) => match[1])
      .filter((id) => id !== 'starlight__on-this-page');
    const toc = page.match(/<nav aria-labelledby="starlight__on-this-page">([\s\S]*?)<\/nav>/)?.[1];
    assert.ok(toc, `${route} is missing its desktop table of contents`);
    const links = [...toc.matchAll(/<a href="#([^"]+)"/g)].map((match) => match[1]);
    assert.deepEqual(links, headings, `${route} outline does not match its structural headings`);
  }
});

test('short wide tables become labeled records without capturing dense tables', async () => {
  const pages = new Map(await Promise.all(publicRoutes.map(async (route) => (
    [route, await readFile(routeFile(route), 'utf8')]
  ))));
  const tables = new Map([...pages].map(([route, html]) => [route, tableSummaries(html)]));
  const findTable = (route, headers) => {
    const table = tables.get(route).find((candidate) => (
      candidate.headers.join('|') === headers.join('|')
    ));
    assert.ok(table, `${route} is missing table ${headers.join(' | ')}`);
    return table;
  };

  const configFiles = findTable(
    '/config/',
    ['File', 'Location', 'Contains', 'Committed & shared'],
  );
  assert.match(configFiles.attributes, /class="wt-responsive-records"/);

  for (const pageTables of tables.values()) {
    for (const table of pageTables) {
      if (!/\bwt-responsive-records\b/.test(table.attributes)) continue;
      assert.ok(table.headers.length >= 3);
      assert.ok(table.rowLabels.length >= 1 && table.rowLabels.length <= 3);
      assert.deepEqual(
        table.rowLabels,
        Array.from({ length: table.rowLabels.length }, () => table.headers),
      );
    }
  }

  assert.doesNotMatch(
    findTable('/extending/', ['', 'Hooks', 'Aliases', 'Custom subcommands']).attributes,
    /\bwt-responsive-records\b/,
  );
  assert.doesNotMatch(
    findTable('/claude-code/', ['Capability', 'Claude Code', 'Codex', 'OpenCode', 'Gemini CLI']).attributes,
    /\bwt-responsive-records\b/,
  );
  assert.doesNotMatch(
    findTable('/hook/', ['Kind', 'Variable', 'Description']).attributes,
    /\bwt-responsive-records\b/,
  );
  assert.doesNotMatch(
    findTable('/list/', ['Column', 'Shows']).attributes,
    /\bwt-responsive-records\b/,
  );
  assert.doesNotMatch(
    findTable('/', ['Task', 'Worktrunk', 'Plain git']).attributes,
    /\bwt-responsive-records\b/,
  );
});

test('mobile menu control is hidden until its script is available', async () => {
  const faqPage = await readFile(routeFile('/faq/'), 'utf8');
  assert.match(faqPage, /<starlight-menu-button\b/);
  assert.match(
    faqPage,
    /<style>\s*starlight-menu-button:not\(:defined\)\s*\{\s*display:\s*none;\s*\}\s*<\/style>/,
  );
});

test('built pages have unique IDs and valid internal page links', async () => {
  const pages = await htmlFiles();
  const contents = new Map(await Promise.all(pages.map(async (file) => [file, await readFile(file, 'utf8')])));

  for (const [source, html] of contents) {
    const ids = [...html.matchAll(/\sid="([^"]+)"/g)].map((match) => match[1]);
    assert.equal(new Set(ids).size, ids.length, `duplicate ID in ${source}`);

    for (const match of html.matchAll(/\shref="([^"]+)"/g)) {
      const href = match[1].replaceAll('&amp;', '&');
      const url = new URL(href, `https://worktrunk.dev${fileRoute(source)}`);
      if (url.origin !== 'https://worktrunk.dev') continue;
      const targetFile = routeFile(decodeURIComponent(url.pathname));
      if (!targetFile) continue;
      const target = contents.get(targetFile);
      assert.ok(target, `${source} links to missing page ${url.pathname}`);
      if (!url.hash) continue;
      const id = decodeURIComponent(url.hash.slice(1));
      assert.match(target, new RegExp(`\\sid="${id.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}"`), `${source} links to missing ${url.hash}`);
    }
  }
});
