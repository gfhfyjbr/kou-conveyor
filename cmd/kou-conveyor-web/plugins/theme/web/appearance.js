// The appearance: the accent colour, the shade of the greys and how round
// corners are, which the terminal cockpit shares through the preferences.
// The presets and the arithmetic are cmd/internal/cockpit/appearance.go's,
// and the iPhone app's: OKLCH, which keeps lightness as the eye sees it
// while hue and chroma change.
//
// An appearance comes to the page as custom properties on its root —
// --kou-dark-* and --kou-light-*, the colours in either theme, and
// --kou-radius — which theme.css's tokens read; unset, the tokens are the
// cockpit's own orange, graphite and square corners.

export const MAX_RADIUS = 16;
export const DEFAULT = Object.freeze({ accent: 'orange', shade: 'graphite', radius: 0 });

// The accent presets, in dark and in light: the light one darker, to read
// on a light background.
export const ACCENTS = Object.freeze([
  { name: 'orange', label: 'Orange', dark: '#ff5b1f', light: '#e4470c' },
  { name: 'amber', label: 'Amber', dark: '#f5a524', light: '#a16207' },
  { name: 'green', label: 'Green', dark: '#3ecf72', light: '#18853f' },
  { name: 'teal', label: 'Teal', dark: '#22c3b0', light: '#0b7f72' },
  { name: 'blue', label: 'Blue', dark: '#4c8dff', light: '#2160e0' },
  { name: 'violet', label: 'Violet', dark: '#9d7bff', light: '#6d44e0' },
  { name: 'pink', label: 'Pink', dark: '#ff5c9d', light: '#d0246c' },
  { name: 'mono', label: 'Mono', dark: '#ecebe6', light: '#151513' },
]);

// The shade presets tint graphite's greys: each keeps its lightness, and
// takes the hue and graphite's chroma times dark or light.
export const SHADES = Object.freeze([
  { name: 'graphite', label: 'Graphite', hue: 0, dark: 1, light: 1 },
  { name: 'neutral', label: 'Neutral', hue: 0, dark: 0, light: 0 },
  { name: 'slate', label: 'Slate', hue: 255, dark: 2.2, light: 1 },
  { name: 'midnight', label: 'Midnight', hue: 264, dark: 5, light: 1.8 },
  { name: 'sage', label: 'Sage', hue: 165, dark: 2, light: 0.9 },
  { name: 'mauve', label: 'Mauve', hue: 305, dark: 2.2, light: 1 },
  { name: 'mocha', label: 'Mocha', hue: 55, dark: 3, light: 1.2 },
]);

// Corners with names; the slider has every radius between.
export const CORNERS = Object.freeze([
  { name: 'sharp', label: 'Sharp', radius: 0 },
  { name: 'soft', label: 'Soft', radius: 4 },
  { name: 'rounded', label: 'Rounded', radius: 8 },
  { name: 'round', label: 'Round', radius: 12 },
]);

// Graphite's greys, which a shade tints: theme.css's tokens of these names.
const GREYS = {
  dark: {
    'bg': '#0c0c0b', 'bg-2': '#121211', 'bg-3': '#191917', 'bg-4': '#22221f', 'line': '#262623', 'line-2': '#373632',
    'fg': '#ecebe6', 'fg-2': '#b3b1a9', 'fg-3': '#7f7d76', 'fg-4': '#5a5852', 'code': '#0f0f0e',
    'syn-fg': '#e3e2dc', 'syn-comment': '#6d6b64', 'syn-punct': '#8c8a83', 'syn-op': '#b3b1a9', 'syn-prop': '#cfcdc5',
  },
  light: {
    'bg': '#f4f3ef', 'bg-2': '#fbfaf7', 'bg-3': '#ecebe5', 'bg-4': '#e2e0d8', 'line': '#dcdad2', 'line-2': '#c6c3b9',
    'fg': '#151513', 'fg-2': '#46453f', 'fg-3': '#77756c', 'fg-4': '#a19e94', 'code': '#efeee8',
    'syn-fg': '#1c1c19', 'syn-comment': '#8f8c83', 'syn-punct': '#6f6c64', 'syn-op': '#55534c', 'syn-prop': '#3d3c38',
  },
};
// Tinted text reads as coloured sooner than a tinted ground: it takes less.
const TEXT = new Set(['fg', 'fg-2', 'fg-3', 'fg-4', 'syn-fg', 'syn-comment', 'syn-punct', 'syn-op', 'syn-prop']);

// normalAccent is an accent as the preferences keep it — a preset's name, or
// #rrggbb in lower case — or '' for none.
export function normalAccent(value) {
  const accent = String(value ?? '').trim().toLowerCase();
  if (ACCENTS.some((a) => a.name === accent)) return accent;
  return /^#[0-9a-f]{6}$/.test(accent) ? accent : '';
}

// normalize is an appearance with what no cockpit understands left at the
// defaults.
export function normalize(a) {
  const accent = normalAccent(a?.accent) || DEFAULT.accent;
  const shade = SHADES.some((s) => s.name === a?.shade) ? a.shade : DEFAULT.shade;
  const radius = Math.round(Number(a?.radius));
  return { accent, shade, radius: radius >= 0 && radius <= MAX_RADIUS ? radius : DEFAULT.radius };
}

export const same = (a, b) => a.accent === b.accent && a.shade === b.shade && a.radius === b.radius;

// accentColour is an accent in dark or in light. A colour of the user's is
// made lighter in dark and darker in light until it reads on the
// background; presets read as they are.
export function accentColour(accent, dark) {
  const name = normalAccent(accent) || DEFAULT.accent;
  const preset = ACCENTS.find((a) => a.name === name);
  if (preset) return dark ? preset.dark : preset.light;
  return dark ? fitContrast(name, GREYS.dark.bg, 4.5, true) : fitContrast(name, GREYS.light.bg, 3, false);
}

// greys are a shade's colours in dark or in light, by token.
export function greys(shade, dark) {
  const base = GREYS[dark ? 'dark' : 'light'];
  const preset = SHADES.find((s) => s.name === shade) || SHADES[0];
  if (preset.name === DEFAULT.shade) return { ...base };
  const out = {};
  for (const [name, hex] of Object.entries(base)) {
    let k = dark ? preset.dark : preset.light;
    if (TEXT.has(name)) k = Math.min(k, 1 + (k - 1) * 0.4);
    const c = toOklch(hex);
    out[name] = fromOklch(c.l, c.c * k, preset.hue);
  }
  return out;
}

// variables are the custom properties that show an appearance, as the
// root's style takes them: none for the cockpit's own look.
export function variables(a) {
  const { accent, shade, radius } = normalize(a);
  const vars = {};
  for (const dark of [true, false]) {
    const mode = dark ? 'dark' : 'light';
    const g = greys(shade, dark);
    if (shade !== DEFAULT.shade) {
      for (const [name, hex] of Object.entries(g)) vars[`--kou-${mode}-${name}`] = hex;
    }
    if (accent !== DEFAULT.accent) {
      const colour = accentColour(accent, dark);
      vars[`--kou-${mode}-accent`] = colour;
      // The selection where it cannot be laid over the text, as in a
      // terminal: the cockpit's own orange has one of its own.
      vars[`--kou-${mode}-selection-solid`] = mix(colour, g.bg, dark ? 0.3 : 0.25);
    }
  }
  if (radius) vars['--kou-radius'] = `${radius}px`;
  return vars;
}

// ---------------------------------------------------------------- colour

const toLinear = (c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const fromLinear = (c) => {
  const v = Math.min(1, Math.max(0, c));
  return v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055;
};
const rgbOf = (hex) => {
  const v = parseInt(hex.slice(1), 16);
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
};
const hexOf = (rgb) => `#${rgb.map((c) => Math.round(Math.min(1, Math.max(0, c)) * 255).toString(16).padStart(2, '0')).join('')}`;

export function toOklch(hex) {
  const [r, g, b] = rgbOf(hex).map(toLinear);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
  return { l: L, c: Math.hypot(A, B), h: ((Math.atan2(B, A) * 180) / Math.PI + 360) % 360 };
}

function linear(l, c, h) {
  const A = c * Math.cos((h * Math.PI) / 180);
  const B = c * Math.sin((h * Math.PI) / 180);
  const l1 = (l + 0.3963377774 * A + 0.2158037573 * B) ** 3;
  const m1 = (l - 0.1055613458 * A - 0.0638541728 * B) ** 3;
  const s1 = (l - 0.0894841775 * A - 1.2914855480 * B) ** 3;
  return [
    4.0767416621 * l1 - 3.3077115913 * m1 + 0.2309699292 * s1,
    -1.2684380046 * l1 + 2.6097574011 * m1 - 0.3413193965 * s1,
    -0.0041960863 * l1 - 0.7034186147 * m1 + 1.7076147010 * s1,
  ];
}

const inGamut = (l, c, h) => linear(l, c, h).every((v) => v >= -1e-4 && v <= 1 + 1e-4);

// fromOklch is a colour as #rrggbb, its chroma reduced to what sRGB shows.
export function fromOklch(l, c, h) {
  let chroma = c;
  if (!inGamut(l, chroma, h)) {
    let lo = 0;
    let hi = chroma;
    for (let i = 0; i < 30; i++) {
      const mid = (lo + hi) / 2;
      if (inGamut(l, mid, h)) lo = mid;
      else hi = mid;
    }
    chroma = lo;
  }
  return hexOf(linear(l, chroma, h).map(fromLinear));
}

const luminance = (hex) => {
  const [r, g, b] = rgbOf(hex).map(toLinear);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};

// contrast is WCAG's contrast between two colours, 1 to 21.
export function contrast(a, b) {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

// fitContrast makes a colour lighter, or darker, keeping its hue and
// chroma, until it has the contrast needed against a background.
function fitContrast(hex, bg, need, lighter) {
  if (contrast(hex, bg) >= need) return hex;
  const c = toOklch(hex);
  let lo = lighter ? c.l : 0;
  let hi = lighter ? 1 : c.l;
  for (let i = 0; i < 40; i++) {
    const mid = (lo + hi) / 2;
    const ok = contrast(fromOklch(mid, c.c, c.h), bg) >= need;
    if (ok === lighter) hi = mid;
    else lo = mid;
  }
  return fromOklch(lighter ? hi : lo, c.c, c.h);
}

// mix is t of a over the rest of b.
export function mix(a, b, t) {
  const x = rgbOf(a);
  const y = rgbOf(b);
  return hexOf(x.map((v, i) => v * t + y[i] * (1 - t)));
}
