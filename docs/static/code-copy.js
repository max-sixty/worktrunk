// Copy-to-clipboard for code blocks
// Strips leading `$ ` from bash/shell blocks (detected via data-lang or syntect classes)
//
// Split into two phases to prevent layout jump on page load:
// 1. Wrapper script (inline, sync) - wraps <pre> elements before first paint
// 2. Button script (deferred) - adds copy buttons after DOM ready

document.addEventListener('DOMContentLoaded', function() {
  // Find all code blocks that were wrapped by the inline script
  const wrappers = document.querySelectorAll('.code-block-wrapper');

  wrappers.forEach(function(wrapper) {
    const block = wrapper.querySelector('pre');
    if (!block) return;

    const button = document.createElement('button');
    button.className = 'code-copy-btn';
    button.setAttribute('aria-label', 'Copy code');
    button.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1"/></svg>';

    button.addEventListener('click', function() {
      // Check if this is a bash/shell code block
      const codeEl = block.querySelector('code');
      const isBash = codeEl && (
        codeEl.dataset.lang === 'bash' ||
        codeEl.dataset.lang === 'console' ||
        /z-shell|z-bash/.test(codeEl.innerHTML)
      );
      const isTerminal = block.classList.contains('terminal');
      let text = block.textContent;

      if (isBash || isTerminal) {
        // Strip leading `$ ` from each line
        text = text.split('\n').map(function(line) {
          return line.replace(/^\$ /, '');
        }).join('\n');
      }

      navigator.clipboard.writeText(text.trim()).then(function() {
        button.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="20 6 9 17 4 12"/></svg>';
        button.classList.add('copied');
        setTimeout(function() {
          button.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1"/></svg>';
          button.classList.remove('copied');
        }, 2000);
      });
    });

    wrapper.appendChild(button);
  });
});
