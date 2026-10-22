// Applies a stored theme choice before first paint; without one the page
// follows the system preference.
try {
  const theme = JSON.parse(localStorage.getItem('kou-conveyor.theme'));
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
} catch { /* no stored preference */ }

// And the appearance last shown — the accent, the shade, the corners — as
// the theme plugin left it (plugins/theme/web/appearance.js): the colours
// as it worked them out, which the tokens of theme.css read.
try {
  const look = JSON.parse(localStorage.getItem('kou-conveyor.appearance'));
  const root = document.documentElement;
  for (const [name, value] of Object.entries(look?.vars || {})) {
    if (/^--kou-[a-z0-9-]+$/.test(name) && /^(#[0-9a-f]{6}|\d{1,2}px)$/.test(value)) root.style.setProperty(name, value);
  }
  if (look?.radius > 0) root.dataset.corners = 'round';
} catch { /* no stored appearance: the cockpit's own */ }

// And whether the rail is compact: one panel where round corners would set
// its parts apart.
try {
  if (JSON.parse(localStorage.getItem('kou-conveyor.rail-compact')) === true) document.documentElement.dataset.railCompact = '';
} catch { /* the rail's parts apart */ }
