let drawings = 0;

export function compactionPayload(settings, mode, value) {
  if (!settings.available) throw new Error('Settings are unavailable. Set KOU_CONVEYOR_CONFIG on the server.');
  if (settings.error) throw new Error('Fix the connection settings before changing automatic compaction.');
  const percent = mode === 'default' ? 0 : String(value).trim() === '' ? NaN : Number(value);
  if ((mode !== 'default' && mode !== 'custom') || !Number.isFinite(percent) || percent < 0 || percent > 100 || (mode === 'custom' && percent === 0)) {
    throw new Error('Enter a used-window percentage greater than 0 and at most 100. Decimals are allowed.');
  }
  // SettingsUpdate replaces the connection; omit its key to keep the saved secret.
  return {
    provider_type: settings.provider_type || '',
    base_url: settings.base_url || '',
    model: settings.model || '',
    protocol: settings.protocol || '',
    auto_compact_percent: percent,
  };
}

export function compactionSettings(cockpit) {
  const { h } = cockpit;
  const id = `compaction-${++drawings}`;
  let available = false;
  let saving = false;
  let loading = true;
  const mode = h('select', { id: `${id}-mode`, name: 'auto_compact_mode', disabled: true, 'aria-describedby': `${id}-help`, onchange: edit },
    h('option', { value: 'default', text: 'Default runner policy' }),
    h('option', { value: 'custom', text: 'Custom percentage' }));
  const percent = h('input', {
    id: `${id}-percent`, name: 'auto_compact_percent', type: 'number', inputmode: 'decimal', min: '0', max: '100', step: 'any',
    disabled: true, 'aria-describedby': `${id}-help ${id}-status`, oninput: edit,
  });
  const percentField = h('label', { class: 'field', for: `${id}-percent`, hidden: true }, h('span', { class: 'label', text: 'Used context window (%)' }), percent);
  const help = h('p', { class: 'set-note', id: `${id}-help` });
  const status = h('p', { class: 'settings-status', id: `${id}-status`, role: 'status', 'aria-live': 'polite', 'aria-atomic': 'true', text: 'Loading compaction settings…' });
  const save = h('button', { class: 'primary', type: 'submit', disabled: true }, 'Save compaction');
  const retry = h('button', { class: 'act', type: 'button', hidden: true, onclick: load }, 'Retry loading');
  const form = h('form', { class: 'compaction-form', novalidate: true, 'aria-busy': 'true', onsubmit: submit },
    h('label', { class: 'field', for: `${id}-mode` }, h('span', { class: 'label', text: 'Automatic compaction policy' }), mode),
    percentField, help,
    h('p', { class: 'set-note', text: 'Applies when a run starts. Active runs keep their current policy.' }),
    h('div', { class: 'set-inline' }, save, retry), status);

  function say(text, kind = '') {
    status.textContent = text;
    status.dataset.kind = kind;
  }

  function draw() {
    const custom = mode.value === 'custom';
    percentField.hidden = !custom;
    mode.disabled = save.disabled = !available || saving || loading;
    percent.disabled = !custom || mode.disabled;
    save.textContent = saving ? 'Saving…' : 'Save compaction';
    form.setAttribute('aria-busy', String(saving || loading));
    help.textContent = custom
      ? 'Compact after this percentage of the model context window is used. Enter a value greater than 0 and at most 100; decimals are allowed.'
      : 'Default keeps the existing runner policy, including any environment override. It is not an Off switch.';
  }

  function fill(data) {
    available = data.available && !data.error;
    const value = data.auto_compact_percent || 0;
    mode.value = value > 0 ? 'custom' : 'default';
    percent.value = value > 0 ? String(value) : '';
    percent.removeAttribute('aria-invalid');
    retry.hidden = available;
    if (data.error) say('Fix the connection settings, then retry loading automatic compaction.', 'error');
    else if (!data.available) say('Settings are unavailable. Set KOU_CONVEYOR_CONFIG on the server.', 'error');
  }

  function edit(event) {
    percent.removeAttribute('aria-invalid');
    say('');
    draw();
    if (event.currentTarget === mode && mode.value === 'custom') percent.focus();
  }

  async function load() {
    loading = true;
    retry.hidden = true;
    say('Loading compaction settings…');
    draw();
    try {
      const data = await cockpit.api('/api/settings');
      fill(data);
      if (available) say('');
    } catch (error) {
      available = false;
      retry.hidden = false;
      say(`Could not load compaction settings: ${error.message}`, 'error');
    } finally {
      loading = false;
      draw();
    }
  }

  async function submit(event) {
    event.preventDefault();
    if (!available || saving || loading) return;
    let value;
    try {
      value = compactionPayload({ available: true }, mode.value, percent.value).auto_compact_percent;
    } catch (error) {
      percent.setAttribute('aria-invalid', 'true');
      say(error.message, 'error');
      percent.focus();
      return;
    }
    saving = true;
    say('Saving…');
    draw();
    try {
      // Reload the connection so an Accounts change cannot be replaced by an old form snapshot.
      const current = await cockpit.api('/api/settings');
      const body = compactionPayload(current, value === 0 ? 'default' : 'custom', value);
      const data = await cockpit.api('/api/settings', { method: 'PUT', body });
      fill(data);
      if (available) say(value === 0 ? 'Default policy saved. Applies to new runs.' : `Saved ${data.auto_compact_percent}% used context. Applies to new runs.`, 'ok');
    } catch (error) {
      say(`Compaction settings were not saved: ${error.message}`, 'error');
    } finally {
      saving = false;
      draw();
    }
  }

  load();
  return form;
}
