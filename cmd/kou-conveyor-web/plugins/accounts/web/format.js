// Amounts of money, as the Accounts view shows them: US dollars, as API
// prices are, or the credits of the user's own unit ({ name, usd }: what
// one is worth in dollars).

export const DOLLARS = Object.freeze({ name: 'USD', usd: 1 });

const formats = new Map();
// number groups a number's thousands: with digits decimals, or up to most.
function number(value, digits, most = digits) {
  const key = `${digits}:${most}`;
  let f = formats.get(key);
  if (!f) {
    f = new Intl.NumberFormat('en-US', { minimumFractionDigits: digits, maximumFractionDigits: most });
    formats.set(key, f);
  }
  return f.format(value);
}

export const isDollars = (unit) => !unit || unit.name === 'USD' || !(unit.usd > 0);

// inUnit is an amount of dollars in the unit.
export const inUnit = (usd, unit) => (isDollars(unit) ? usd : usd / unit.usd);

// money reads an amount of dollars in the unit: "$1,234.56", "$0.042",
// "12,346 credits".
export function money(usd, unit = DOLLARS) {
  if (!Number.isFinite(usd)) return '—';
  const v = inUnit(usd, unit);
  const abs = Math.abs(v);
  const sign = v < 0 ? '−' : '';
  if (isDollars(unit)) {
    if (abs > 0 && abs < 0.001) return `${sign}<$0.001`;
    const text = abs === 0 ? '0' : number(abs, abs < 0.1 ? 3 : abs < 10000 ? 2 : 0);
    return `${sign}$${text}`;
  }
  const text = abs === 0 ? '0' : abs < 1 ? String(+abs.toPrecision(2)) : number(abs, 0, abs < 100 ? 1 : 0);
  return `${sign}${text} ${unit.name}`;
}

// compact reads a number in a few characters: "1.2K", "15", "0.42".
function compact(abs) {
  for (const [div, suffix] of [[1e9, 'B'], [1e6, 'M'], [1e3, 'K']]) {
    if (abs >= div * 0.9995) {
      const x = abs / div;
      return `${x >= 9.95 ? Math.round(x) : +x.toFixed(1)}${suffix}`;
    }
  }
  if (abs >= 9.95) return String(Math.round(abs));
  if (abs >= 1) return String(+abs.toFixed(1));
  if (abs >= 0.01) return String(+abs.toFixed(2));
  return abs === 0 ? '0' : String(+abs.toPrecision(1));
}

// moneyShort reads an amount briefly, for axes and badges: "$1.2K", "$0.5".
export function moneyShort(usd, unit = DOLLARS) {
  if (!Number.isFinite(usd)) return '—';
  const v = inUnit(usd, unit);
  const sign = v < 0 ? '−' : '';
  return isDollars(unit) ? `${sign}$${compact(Math.abs(v))}` : `${sign}${compact(Math.abs(v))}`;
}

// count reads a number of requests: "2,880", "10,963", "1.2M".
export function count(n) {
  if (!Number.isFinite(n)) return '—';
  return Math.abs(n) < 100000 ? number(n, 0) : compact(Math.abs(n));
}

// tokens reads a number of tokens as the cockpit does, billions too:
// "48.2M", "1.06B".
export function tokens(n, fallback) {
  if (!Number.isFinite(n)) return '—';
  if (Math.abs(n) >= 1e9) return `${(n / 1e9).toFixed(Math.abs(n) < 1e10 ? 2 : 1)}B`;
  return fallback(n);
}

// rate reads a price per million tokens: "$5", "$0.125".
export function rate(usdPerMillion, unit = DOLLARS) {
  if (!Number.isFinite(usdPerMillion)) return '—';
  const v = inUnit(usdPerMillion, unit);
  const text = v === 0 ? '0' : v >= 100 ? number(Math.round(v), 0) : String(+v.toFixed(v >= 1 ? 2 : v >= 0.01 ? 3 : 4));
  return isDollars(unit) ? `$${text}` : `${text} ${unit.name}`;
}

// unitName says what amounts are in, for labels: "USD", "credits".
export const unitName = (unit) => (isDollars(unit) ? 'USD' : unit.name);
