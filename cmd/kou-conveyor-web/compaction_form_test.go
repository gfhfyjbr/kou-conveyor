package main

import (
	"net/http"
	"net/http/httptest"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func TestCompactionFormPreservesConnection(t *testing.T) {
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("no node")
	}
	module, err := filepath.Abs("plugins/connection/web/compaction.js")
	if err != nil {
		t.Fatal(err)
	}
	dom, err := filepath.Abs("static/kernel/dom.js")
	if err != nil {
		t.Fatal(err)
	}
	for _, saved := range []cockpit.Settings{
		{AutoCompactPercent: 50},
		{API: cockpit.APIGateway, Model: "automatic-test", AutoCompactPercent: 50},
		{API: cockpit.APIGateway, Protocol: cockpit.APIResponses, Model: "gpt-test", AutoCompactPercent: 50},
		{API: cockpit.APIGateway, Protocol: cockpit.APIMessages, Model: "claude-test", AutoCompactPercent: 50},
		{API: cockpit.APIResponses, BaseURL: "https://responses.example.invalid/v1", APIKey: "private-responses-secret", Model: "gpt-test", AutoCompactPercent: 50},
		{API: cockpit.APIMessages, BaseURL: "https://messages.example.invalid", APIKey: "private-messages-secret", Model: "claude-test", AutoCompactPercent: 50},
	} {
		t.Run(string(saved.API), func(t *testing.T) {
			h := newHarness(t)
			path := h.server.opt.SettingsFile
			if err := cockpit.SaveSettings(path, saved); err != nil {
				t.Fatal(err)
			}
			// The fixture changes only its own saved connection between load and submit.
			mux := http.NewServeMux()
			mux.Handle("/api/", h.server.handler())
			mux.HandleFunc("GET /fixture/check-connection", func(w http.ResponseWriter, r *http.Request) {
				got, err := cockpit.LoadSettings(path)
				got.AutoCompactPercent = saved.AutoCompactPercent
				if err != nil || got != saved {
					t.Errorf("percentage form changed the connection or saved key: %+v, %v; want %+v", got, err, saved)
					w.WriteHeader(http.StatusConflict)
					return
				}
				w.WriteHeader(http.StatusNoContent)
			})
			mux.HandleFunc("POST /fixture/switch", func(w http.ResponseWriter, r *http.Request) {
				next := cockpit.Settings{API: cockpit.APIGateway, Protocol: cockpit.APIResponses, Model: "changed-model", AutoCompactPercent: 62.5}
				if err := cockpit.SaveSettings(path, next); err != nil {
					t.Error(err)
					w.WriteHeader(http.StatusInternalServerError)
				}
			})
			fixture := httptest.NewServer(mux)
			defer fixture.Close()
			cmd := exec.Command(node, "--input-type=module", "-e", compactionFormScript, "file://"+module, "file://"+dom, fixture.URL)
			out, err := cmd.CombinedOutput()
			if err != nil || strings.TrimSpace(string(out)) != "ok" {
				t.Fatalf("production form: %v\n%s", err, out)
			}
			got, err := cockpit.LoadSettings(path)
			want := cockpit.Settings{API: cockpit.APIGateway, Protocol: cockpit.APIResponses, Model: "changed-model", AutoCompactPercent: 75}
			if err != nil || got != want {
				t.Fatalf("after stale-form save: %+v, %v; want %+v", got, err, want)
			}
		})
	}
}

const compactionFormScript = `
import assert from 'node:assert/strict';
class Element {
  constructor(tag) { this.tagName = tag; this.children = []; this.attrs = {}; this.dataset = {}; this.listeners = {}; this.value = ''; this.hidden = false; this.disabled = false; this.idle = []; }
  setAttribute(name, value) {
    this.attrs[name] = String(value);
    if (['id', 'name', 'type', 'value', 'role'].includes(name)) this[name] = String(value);
    if (['hidden', 'disabled'].includes(name)) this[name] = true;
    if (name === 'aria-busy' && value === 'false') for (const resolve of this.idle.splice(0)) resolve();
  }
  removeAttribute(name) { delete this.attrs[name]; }
  addEventListener(name, callback) { (this.listeners[name] ??= []).push(callback); }
  append(...nodes) { this.children.push(...nodes); if (this.tagName === 'select' && this.children.length === nodes.length) this.value = nodes[0]?.value || ''; }
  focus() { document.activeElement = this; }
  async fire(name) { for (const callback of this.listeners[name] || []) await callback({ currentTarget: this, preventDefault() {} }); }
  ready() { return this.attrs['aria-busy'] === 'false' ? Promise.resolve() : new Promise((resolve) => this.idle.push(resolve)); }
}
globalThis.Node = Element;
globalThis.document = { createElement: (tag) => new Element(tag), activeElement: null };
const { h } = await import(process.argv[2]);
const { compactionSettings, compactionPayload } = await import(process.argv[1]);
const base = process.argv[3];
const requests = [];
let failLoad = false, failSave = false, heldSave = null;
async function api(path, options = {}) {
  if (!options.method && failLoad) throw new Error('fixture load failed');
  if (options.method === 'PUT') {
    requests.push(options.body);
    if (failSave) throw new Error('fixture save failed');
    if (heldSave) await heldSave.promise;
  }
  const response = await fetch(base + path, { method: options.method || 'GET', headers: { 'Content-Type': 'application/json' }, body: options.body ? JSON.stringify(options.body) : undefined });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || String(response.status));
  return data;
}
const walk = (node) => node instanceof Element ? [node, ...node.children.flatMap(walk)] : [];
const controls = (form) => {
  const nodes = walk(form);
  return { mode: nodes.find((n) => n.name === 'auto_compact_mode'), percent: nodes.find((n) => n.name === 'auto_compact_percent'), save: nodes.find((n) => n.type === 'submit'), retry: nodes.find((n) => n.tagName === 'button' && n.type === 'button'), status: nodes.find((n) => n.role === 'status') };
};
const initial = await api('/api/settings');
assert.ok(Object.hasOwn(initial, 'protocol'), 'GET must expose the exact saved gateway protocol');
const { default: activate } = await import(new URL('./connection.js', process.argv[1]));
const sections = [];
const registry = { register() {} };
activate({
  h, api, svg: () => h('svg'), use: () => ({ state: {} }), has: () => false,
  ui: { mount() {} }, on() {}, listen: (node, type, callback) => node.addEventListener(type, callback),
  provide() {}, contribute: (point, value) => { if (point === 'settings.section') sections.push(value); },
  commands: registry, keys: registry, palette: registry, render() {},
});
assert.equal(sections.length, 1, 'the production connection plugin must register the settings section');
assert.equal(sections[0].id, 'compaction');
assert.equal(sections[0].title, 'Automatic compaction');
const form = sections[0].render();
const c = controls(form);
assert.equal(c.mode.disabled, true);
await form.ready();
assert.equal(c.mode.value, 'custom');
assert.equal(c.percent.value, '50');
assert.equal(c.mode.disabled, false);
assert.ok(walk(form).some((n) => n.attrs.for === c.mode.id));
assert.ok(walk(form).some((n) => n.attrs.for === c.percent.id));
assert.equal(c.status.attrs['aria-live'], 'polite');

for (const value of ['', '0', '-1', '100.01', 'NaN', 'Infinity', '1e999']) {
  c.percent.value = value;
  await form.fire('submit');
  assert.equal(requests.length, 0, 'invalid value must not reach PUT: ' + value);
  assert.equal(c.percent.attrs['aria-invalid'], 'true');
  assert.equal(document.activeElement, c.percent);
  assert.match(c.status.textContent, /greater than 0 and at most 100/);
}
c.percent.value = '62.5';
await c.percent.fire('input');
assert.equal(c.percent.attrs['aria-invalid'], undefined);
let release;
heldSave = { promise: new Promise((resolve) => { release = resolve; }) };
const saving = form.fire('submit');
// Observe the real submit handler reaching its asynchronous API boundary without a timer.
await new Promise((resolve) => {
  const old = form.setAttribute.bind(form);
  if (form.attrs['aria-busy'] === 'true') return resolve();
  form.setAttribute = (name, value) => { old(name, value); if (name === 'aria-busy' && value === 'true') resolve(); };
});
assert.equal(c.save.disabled, true);
assert.equal(c.mode.disabled, true);
assert.equal(c.save.textContent, 'Saving…');
release();
await saving;
heldSave = null;
const first = requests[0];
assert.deepEqual(Object.keys(first).sort(), ['auto_compact_percent', 'base_url', 'model', 'protocol', 'provider_type']);
for (const field of ['provider_type', 'base_url', 'model', 'protocol']) assert.equal(first[field], initial[field]);
assert.equal(Object.hasOwn(first, 'api_key'), false);
assert.equal(first.auto_compact_percent, 62.5);
assert.match(c.status.textContent, /Saved 62.5%/);
const reloaded = compactionSettings({ h, api });
await reloaded.ready();
assert.equal(controls(reloaded).percent.value, '62.5');
assert.equal((await api('/api/settings')).api_key_set, initial.api_key_set);
assert.equal((await fetch(base + '/fixture/check-connection')).status, 204, 'saved API key and every connection field must survive the production PUT');

c.mode.value = 'default';
await c.mode.fire('change');
assert.equal(c.percent.disabled, true);
assert.ok(walk(form).some((n) => String(n.textContent).includes('not an Off switch')));
await form.fire('submit');
assert.equal(requests.at(-1).auto_compact_percent, 0);
assert.equal((await api('/api/settings')).auto_compact_percent, 0);
assert.equal((await fetch(base + '/fixture/check-connection')).status, 204, 'reset must retain the saved key and connection');
assert.match(c.status.textContent, /Default policy saved/);

c.mode.value = 'custom';
await c.mode.fire('change');
assert.equal(document.activeElement, c.percent);
c.percent.value = '75';
failSave = true;
await form.fire('submit');
assert.equal(c.percent.value, '75', 'failed save must retain the draft');
assert.equal(c.save.disabled, false);
assert.match(c.status.textContent, /not saved: fixture save failed/);
failSave = false;
failLoad = true;
const attempts = requests.length;
await form.fire('submit');
assert.equal(requests.length, attempts, 'failed fresh GET must not send a replacement connection');
assert.equal(c.percent.value, '75');
assert.equal(c.save.disabled, false);
assert.match(c.status.textContent, /not saved: fixture load failed/);
failLoad = false;
await fetch(base + '/fixture/switch', { method: 'POST' });
await form.fire('submit');
assert.equal(requests.at(-1).provider_type, 'gateway');
assert.equal(requests.at(-1).protocol, 'responses');
assert.equal(requests.at(-1).model, 'changed-model', 'stale loaded connection must not overwrite the current connection');

failLoad = true;
const failed = compactionSettings({ h, api });
await failed.ready();
const f = controls(failed);
assert.equal(f.save.disabled, true);
assert.equal(f.retry.hidden, false);
assert.match(f.status.textContent, /Could not load/);
failLoad = false;
await f.retry.fire('click');
assert.equal(f.save.disabled, false);
assert.equal(f.retry.hidden, true);
for (const data of [{ available: false }, { available: true, error: 'broken' }]) {
  const blocked = compactionSettings({ h, api: async () => data });
  await blocked.ready();
  assert.equal(controls(blocked).save.disabled, true);
  assert.equal(controls(blocked).retry.hidden, false);
}
let resolveOld;
const old = compactionSettings({ h, api: () => new Promise((resolve) => { resolveOld = resolve; }) });
const fresh = compactionSettings({ h, api });
await fresh.ready();
assert.equal(controls(fresh).percent.value, '75');
resolveOld({ ...initial, auto_compact_percent: 25 });
await old.ready();
assert.equal(controls(old).percent.value, '25');
assert.equal(controls(fresh).percent.value, '75', 'late response to an old section must not overwrite the reopened section');
assert.notEqual(controls(old).percent.id, controls(fresh).percent.id);
assert.throws(() => compactionPayload({ available: true, error: 'broken' }, 'custom', '80'), /Fix the connection settings/);
console.log('ok');
`
