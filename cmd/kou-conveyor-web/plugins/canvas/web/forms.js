// A form made from a JSON schema: the configurations of nodes, as presets
// and sources declare them — an object of strings (with enum, a choice),
// numbers, integers, booleans and arrays of strings, each with its title,
// description, default and pattern, and the required ones.
//
//   form(h, schema, values, { onChange(key, value), compact }) → { node, values(), valid() }
//
// onChange runs as a field is left or picked, with the value it holds:
// a number for a number, null for a field emptied.

export function form(h, schema, values = {}, { onChange, idPrefix = 'cv-f' } = {}) {
  const properties = schema?.properties || {};
  const required = new Set(schema?.required || []);
  const fields = [];
  const node = h('div', { class: 'cv-form' });
  for (const [key, property] of Object.entries(properties)) {
    const id = `${idPrefix}-${key}`;
    const title = property.title || key;
    const current = values?.[key] ?? property.default ?? '';
    let input;
    let read;
    const changed = () => {
      field.dataset.invalid = check() ? '' : 'true';
      if (!check()) return;
      onChange?.(key, read());
    };
    if (Array.isArray(property.enum)) {
      input = h('select', { id, onchange: changed },
        !required.has(key) && property.default === undefined ? h('option', { value: '', text: '—' }) : null,
        ...property.enum.map((choice) => h('option', { value: String(choice), text: String(choice), selected: String(choice) === String(current) ? true : null })));
      read = () => (input.value === '' ? null : property.type === 'integer' || property.type === 'number' ? Number(input.value) : input.value);
    } else if (property.type === 'boolean') {
      input = h('input', { id, type: 'checkbox', onchange: changed });
      input.checked = !!current;
      read = () => input.checked;
    } else if (property.type === 'number' || property.type === 'integer') {
      input = h('input', { id, type: 'number', step: property.type === 'integer' ? '1' : 'any', value: current === '' ? '' : String(current), onchange: changed });
      read = () => (input.value.trim() === '' ? null : Number(input.value));
    } else if (property.type === 'array') {
      input = h('textarea', { id, rows: '3', spellcheck: 'false', placeholder: 'One per line', onchange: changed });
      input.value = Array.isArray(current) ? current.join('\n') : '';
      read = () => {
        const list = input.value.split('\n').map((line) => line.trim()).filter(Boolean);
        return list.length ? list : null;
      };
    } else {
      const long = /instructions|prompt|role|text|template|body/i.test(key);
      input = long
        ? h('textarea', { id, rows: '3', spellcheck: 'false', onchange: changed })
        : h('input', { id, type: 'text', spellcheck: 'false', autocomplete: 'off', onchange: changed });
      input.value = current === null ? '' : String(current);
      if (property.pattern && !long) input.setAttribute('pattern', property.pattern);
      read = () => (input.value === '' ? null : input.value);
    }
    const check = () => {
      const value = read();
      if (value === null || value === '') return !required.has(key);
      if (property.pattern && typeof value === 'string') {
        try {
          if (!new RegExp(property.pattern).test(value)) return false;
        } catch {
          return true;
        }
      }
      if ((property.type === 'number' || property.type === 'integer') && !Number.isFinite(value)) return false;
      return true;
    };
    const field = h('label', { class: 'cv-field', for: id, data: { type: property.type || 'string' } },
      h('span', { class: 'cv-field-title' }, title, required.has(key) ? h('i', { text: ' *', 'aria-label': 'required' }) : null),
      input,
      property.description ? h('small', { text: property.description }) : null);
    fields.push({ key, read, check, input });
    node.append(field);
  }
  if (!fields.length) node.append(h('p', { class: 'none', text: 'Nothing to set.' }));
  return {
    node,
    values() {
      const out = {};
      for (const f of fields) {
        const value = f.read();
        if (value !== null && value !== '') out[f.key] = value;
      }
      return out;
    },
    valid: () => fields.every((f) => f.check()),
    focus: () => fields[0]?.input.focus(),
  };
}

// defaults are the values a schema gives when nothing is set.
export function defaults(schema) {
  const out = {};
  for (const [key, property] of Object.entries(schema?.properties || {})) {
    if (property.default !== undefined) out[key] = property.default;
  }
  return out;
}

// missing names the required fields a configuration lacks.
export function missing(schema, values = {}) {
  return (schema?.required || []).filter((key) => values[key] === undefined || values[key] === null || values[key] === '');
}
