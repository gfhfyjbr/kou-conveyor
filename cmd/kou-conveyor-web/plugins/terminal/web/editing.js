// The keys of macOS's text editing, as Ghostty sends them to the shell:
// restty sends ⌥⌫ as ⌫, ⌘⌫ as nothing, and ⌥← ⌥→ as sequences shells do
// not bind. editingKeys reads them off a terminal's keydown, in the capture
// phase, and sends what a shell, readline or a TUI takes for them.

export const mac = /Mac|iPhone|iPad/.test(navigator.platform || '') || navigator.userAgentData?.platform === 'macOS';

const OPTION = {
  Backspace: '\x1b\x7f', // the word before the cursor
  Delete: '\x1bd', // the word after it
  ArrowLeft: '\x1bb', // a word back
  ArrowRight: '\x1bf', // a word on
};

const COMMAND = {
  Backspace: '\x15', // the line before the cursor
  ArrowLeft: '\x01', // the line's start
  ArrowRight: '\x05', // its end
};

// sequenceOf is what a key pressed sends in place of restty's, '' for
// restty's own.
export function sequenceOf(event) {
  if (!mac || event.isComposing || event.ctrlKey || event.shiftKey) return '';
  if (event.altKey && !event.metaKey) return OPTION[event.key] || '';
  if (event.metaKey && !event.altKey) return COMMAND[event.key] || '';
  return '';
}

// editingKeys has the keys of element's terminal send what Ghostty would;
// send(text) types it into the shell in focus. It returns the undoing.
export function editingKeys(element, send) {
  const keydown = (event) => {
    const sequence = sequenceOf(event);
    if (!sequence || !send(sequence)) return;
    event.preventDefault();
    event.stopPropagation();
  };
  element.addEventListener('keydown', keydown, true);
  return () => element.removeEventListener('keydown', keydown, true);
}
