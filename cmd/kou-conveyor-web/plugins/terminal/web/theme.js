// The terminal's colours: a theme of the cockpit's own, dark and light,
// drawn from its tokens — the background, the text, the accent — with an
// ANSI palette that sits beside them. Colour 208, the xterm palette's
// orange, is the cockpit's accent, which the kou-conveyor prompt draws its
// mark and arrow in.

const PALETTES = {
  dark: {
    black: '#1f1f1c', red: '#ff5468', green: '#9ccf83', yellow: '#efb443', blue: '#7aa7d9', magenta: '#d28bc4', cyan: '#6fc2b8', white: '#b3b1a9',
    brightBlack: '#5f5d57', brightRed: '#ff7a8a', brightGreen: '#b8e0a4', brightYellow: '#f6cd78', brightBlue: '#9dc0e8', brightMagenta: '#e2a9d6', brightCyan: '#95d8cf', brightWhite: '#ecebe6',
    selection: '#4a2a1b',
  },
  light: {
    black: '#151513', red: '#cc1f3d', green: '#2e7a34', yellow: '#9a6200', blue: '#2d5f9a', magenta: '#9c3d87', cyan: '#1f7a72', white: '#6f6c64',
    brightBlack: '#8f8c83', brightRed: '#e0485f', brightGreen: '#3f9446', brightYellow: '#b87a0a', brightBlue: '#3f75b8', brightMagenta: '#b5559f', brightCyan: '#2a948a', brightWhite: '#44423d',
    selection: '#f2cbb8',
  },
};

// token reads a colour token of the cockpit's theme, as a hex colour.
function token(name, fallback) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return /^#[0-9a-f]{6}$/i.test(value) ? value : fallback;
}

// mode is the cockpit's theme: light or dark.
export function mode() {
  const chosen = document.documentElement.dataset.theme;
  if (chosen === 'light' || chosen === 'dark') return chosen;
  return matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}

// colours are what the terminal and its panes are drawn with.
export function colours() {
  const dark = mode() === 'dark';
  const p = PALETTES[dark ? 'dark' : 'light'];
  return {
    background: token('--bg', dark ? '#0c0c0b' : '#f4f3ef'),
    foreground: token('--fg', dark ? '#ecebe6' : '#151513'),
    accent: token('--accent', dark ? '#ff5b1f' : '#e4470c'),
    line: token('--line', dark ? '#262623' : '#dcdad2'),
    panel: token('--bg-2', dark ? '#121211' : '#fbfaf7'),
    muted: token('--fg-3', dark ? '#7f7d76' : '#77756c'),
    ...p,
  };
}

// ghostty is the theme in Ghostty's format, which restty parses.
export function ghostty() {
  const c = colours();
  const palette = [c.black, c.red, c.green, c.yellow, c.blue, c.magenta, c.cyan, c.white,
    c.brightBlack, c.brightRed, c.brightGreen, c.brightYellow, c.brightBlue, c.brightMagenta, c.brightCyan, c.brightWhite];
  const lines = [
    `background = ${c.background}`,
    `foreground = ${c.foreground}`,
    `cursor-color = ${c.accent}`,
    `cursor-text = ${c.background}`,
    `selection-background = ${c.selection}`,
    `selection-foreground = ${c.foreground}`,
    ...palette.map((colour, i) => `palette = ${i}=${colour}`),
    `palette = 208=${c.accent}`,
  ];
  return lines.join('\n');
}
