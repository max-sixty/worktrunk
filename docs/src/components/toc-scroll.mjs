const desktopTocSelector = 'nav[aria-labelledby="starlight__on-this-page"]';
const tocRootSelector = 'starlight-toc, mobile-starlight-toc';
const scrollportSelector = '.right-sidebar';
const desktopBreakpoint = '(min-width: 72rem)';
const tocInset = 48;

function currentSetter(element) {
  for (let prototype = Object.getPrototypeOf(element); prototype; prototype = Object.getPrototypeOf(prototype)) {
    const setter = Object.getOwnPropertyDescriptor(prototype, 'current')?.set;
    if (setter) return setter;
  }
}

function syncTocRootToHash(toc, hash) {
  const link = [...toc.querySelectorAll('a')].find((candidate) => candidate.hash === hash);
  if (!link) return false;

  const setter = currentSetter(toc);
  if (setter) {
    setter.call(toc, link);
  } else {
    for (const current of toc.querySelectorAll('a[aria-current="true"]')) {
      current.removeAttribute('aria-current');
    }
    link.setAttribute('aria-current', 'true');
  }

  const display = toc.querySelector('.display-current');
  if (display) display.textContent = link.textContent;
  return true;
}

export function syncTocCurrentToHash(doc = document, hash = window.location.hash) {
  if (!hash) return false;
  return [...doc.querySelectorAll(tocRootSelector)]
    .map((toc) => syncTocRootToHash(toc, hash))
    .some(Boolean);
}

export function tocScrollDelta(scrollportRect, currentRect, inset = tocInset) {
  const availableInset = Math.min(inset, (scrollportRect.bottom - scrollportRect.top) / 4);
  const top = scrollportRect.top + availableInset;
  const bottom = scrollportRect.bottom - availableInset;
  if (currentRect.top < top) return currentRect.top - top;
  if (currentRect.bottom > bottom) return currentRect.bottom - bottom;
  return 0;
}

export function keepCurrentTocLinkVisible(current) {
  const toc = current.closest(desktopTocSelector);
  if (!toc || toc.getClientRects().length === 0) return false;

  const scrollport = toc.closest(scrollportSelector);
  if (!scrollport || scrollport.scrollHeight <= scrollport.clientHeight) return false;

  const delta = tocScrollDelta(
    scrollport.getBoundingClientRect(),
    current.getBoundingClientRect(),
  );
  if (delta === 0) return false;
  scrollport.scrollTop += delta;
  return true;
}

// Follow Starlight's own current-section state; never infer it from document scroll.
export function installTocFollower(doc = document, view = window) {
  let pendingHash = view.location.hash;
  const initializedTocs = new WeakSet();
  const correctedTocs = new Set();
  const applyPendingHash = () => {
    const tocs = [...doc.querySelectorAll(tocRootSelector)];
    if (!pendingHash || tocs.length === 0 || !tocs.every((toc) => initializedTocs.has(toc))) {
      return false;
    }
    const matched = syncTocCurrentToHash(doc, pendingHash);
    if (matched) pendingHash = '';
    return matched;
  };
  const followCurrent = () => {
    const current = doc.querySelector(`${desktopTocSelector} a[aria-current="true"]`);
    if (current) keepCurrentTocLinkVisible(current);
  };

  const observer = new MutationObserver((mutations) => {
    for (const { target } of mutations) {
      if (target.getAttribute('aria-current') !== 'true') continue;
      const toc = target.closest(tocRootSelector);
      if (toc) initializedTocs.add(toc);
      if (pendingHash && toc && !correctedTocs.has(toc)) {
        syncTocRootToHash(toc, pendingHash);
        correctedTocs.add(toc);
        const tocs = [...doc.querySelectorAll(tocRootSelector)];
        if (tocs.length > 0 && tocs.every((candidate) => correctedTocs.has(candidate))) {
          pendingHash = '';
        }
        break;
      }
      if (keepCurrentTocLinkVisible(target)) break;
    }
  });
  observer.observe(doc.documentElement, {
    attributes: true,
    attributeFilter: ['aria-current'],
    subtree: true,
  });

  const desktop = view.matchMedia(desktopBreakpoint);
  const followOnDesktop = ({ matches }) => {
    if (matches) followCurrent();
  };
  desktop.addEventListener('change', followOnDesktop);
  doc.addEventListener('astro:page-load', followCurrent);
  followCurrent();

  let syncFrame;
  const scheduleHashSync = (hash = view.location.hash) => {
    if (!hash) return;
    pendingHash = hash;
    correctedTocs.clear();
    view.cancelAnimationFrame(syncFrame);
    syncFrame = view.requestAnimationFrame(() => {
      syncFrame = view.requestAnimationFrame(() => {
        applyPendingHash();
      });
    });
  };
  const syncLocationHash = () => scheduleHashSync();
  const syncClickedHash = ({ target }) => {
    const link = target?.closest?.(`${tocRootSelector} a[href^="#"]`);
    if (link) scheduleHashSync(link.hash);
  };
  doc.addEventListener('click', syncClickedHash);
  doc.addEventListener('astro:page-load', syncLocationHash);
  view.addEventListener('hashchange', syncLocationHash);

  return () => {
    observer.disconnect();
    view.cancelAnimationFrame(syncFrame);
    desktop.removeEventListener('change', followOnDesktop);
    doc.removeEventListener('astro:page-load', followCurrent);
    doc.removeEventListener('click', syncClickedHash);
    doc.removeEventListener('astro:page-load', syncLocationHash);
    view.removeEventListener('hashchange', syncLocationHash);
  };
}
