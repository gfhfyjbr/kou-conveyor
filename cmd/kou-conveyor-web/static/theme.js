// Applies a stored theme choice before first paint; without one the page
// follows the system preference.
try {
  const theme = JSON.parse(localStorage.getItem('kou-conveyor.theme'));
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
} catch { /* no stored preference */ }
