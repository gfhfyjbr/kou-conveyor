import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';

const [browser, base, out, home] = process.argv.slice(2);
const url = base + '/fixture/compaction';
const terminal = process.env.KOU_COMPACTION_TERMINAL_ATTEMPT === '1'
  ? spawnSync('terminal-browser', ['new-tab', url], { encoding: 'utf8', timeout: 15_000 })
  : { status: null, stderr: 'Not repeated: the initial Ghostty failure is preserved in compaction-ui-browser.log.' };
console.log('terminal-browser own-fixture capability:', terminal.status, terminal.error?.message || terminal.stderr.trim());
const profile = await mkdtemp(path.join(home, 'compaction-browser-'));
// Run only this fixture's main in the existing generic Electron runtime, not terminal-browser's app or daemon.
await mkdir(path.join(profile, 'tmp'), { recursive: true });
const browserEnv = { ...process.env, TMPDIR: path.join(profile, 'tmp') };
delete browserEnv.ELECTRON_RUN_AS_NODE;
const chrome = spawn(browser, [path.resolve('testdata/compaction-electron.cjs'), profile], { env: browserEnv, stdio: ['pipe', 'pipe', 'pipe'] });
let stderr = '';
let client;
let failureEvaluate;
let stage = 'owned runtime startup';
let nextNative = 0;
const nativePending = new Map();
const nativeEvents = [];
const keyReceipts = [];
const errors = [];
const consoleMessages = [];
const pageRequests = [];
const guard = setTimeout(() => { console.error('Owned browser overall guard:', stage, [...(client?.pending.entries() || [])].map(([id, item]) => ({ id, method: item.method }))); chrome.kill('SIGTERM'); }, 90_000);

class CDP {
  constructor(ws) {
    this.ws = ws;
    this.next = 0;
    this.pending = new Map();
    this.listeners = [];
    ws.addEventListener('message', ({ data }) => {
      const message = JSON.parse(data);
      if (message.id) {
        const pending = this.pending.get(message.id);
        this.pending.delete(message.id);
        clearTimeout(pending?.timer);
        if (message.error) pending?.reject(new Error(JSON.stringify(message.error)));
        else pending?.resolve(message.result);
      } else {
        for (const listener of this.listeners) listener(message);
      }
    });
    ws.addEventListener('close', () => {
      for (const pending of this.pending.values()) {
        clearTimeout(pending.timer);
        pending.reject(new Error(`owned browser connection closed during ${pending.method}`));
      }
      this.pending.clear();
    });
  }
  call(method, params = {}, sessionId) {
    const id = ++this.next;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`CDP stage timed out: ${method} #${id}`)); }, 10_000);
      this.pending.set(id, { resolve, reject, method, timer });
      this.ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }
  once(method, sessionId) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.listeners = this.listeners.filter((item) => item !== listener); reject(new Error(`browser event timed out: ${method}`)); }, 10_000);
      const listener = (message) => {
        if (message.method !== method || message.sessionId !== sessionId) return;
        clearTimeout(timer);
        this.listeners = this.listeners.filter((item) => item !== listener);
        resolve(message.params);
      };
      this.listeners.push(listener);
    });
  }
}

try {
  const endpoint = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`owned runtime startup timed out: ${stderr}`)), 15_000);
    chrome.stderr.on('data', (data) => {
      stderr += data.toString();
      for (const match of stderr.matchAll(/OWN_FIXTURE_INPUT (\{[^\n]+\})\n/g)) {
        const reply = JSON.parse(match[1]);
        const pending = nativePending.get(reply.id);
        if (!pending) continue;
        nativePending.delete(reply.id);
        clearTimeout(pending.timer);
        nativeEvents.push(reply);
        if (reply.error) pending.reject(new Error(reply.error));
        else pending.resolve(reply);
      }
      const found = stderr.match(/DevTools listening on (ws:\/\/[^\s]+)/);
      if (found && stderr.includes('OWN_FIXTURE_READY')) { clearTimeout(timer); resolve(found[1]); }
    });
    chrome.once('error', (error) => { clearTimeout(timer); reject(error); });
    chrome.once('exit', (code) => { clearTimeout(timer); reject(new Error(`owned runtime exited ${code}: ${stderr}`)); });
  });
  const devtools = 'http://' + new URL(endpoint).host;
  const schema = await (await fetch(devtools + '/json/protocol')).json();
  const methods = new Set(schema.domains.flatMap((domain) => (domain.commands || []).map((command) => `${domain.domain}.${command.name}`)));
  for (const method of ['Runtime.evaluate', 'Page.navigate', 'Page.captureScreenshot', 'Input.dispatchMouseEvent', 'Input.dispatchKeyEvent', 'Input.insertText', 'Emulation.setDeviceMetricsOverride', 'Fetch.enable']) assert.ok(methods.has(method), method);
  const versionData = await (await fetch(devtools + '/json/version')).json();
  const version = { product: versionData.Browser };
  const targets = await (await fetch(devtools + '/json/list')).json();
  const target = targets.find((item) => item.type === 'page' && item.url === 'about:blank');
  assert.ok(target, 'owned fixture page');
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }); });
  client = new CDP(ws);
  const sessionId = undefined;
  const call = (method, params) => client.call(method, params);
  client.listeners.push((message) => {
    if (message.sessionId !== sessionId) return;
    if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails.text + ': ' + (message.params.exceptionDetails.exception?.description || ''));
    if (message.method === 'Runtime.consoleAPICalled') {
      const text = message.params.args.map((arg) => arg.value || arg.description).join(' ');
      if (text.startsWith('OWN_FIXTURE_RECEIPT ')) keyReceipts.push(JSON.parse(text.slice('OWN_FIXTURE_RECEIPT '.length)));
      else consoleMessages.push({ type: message.params.type, text });
    }
    if (message.method === 'Fetch.requestPaused') {
      const { requestId, request } = message.params;
      pageRequests.push(request.url);
      call(request.url.startsWith(base + '/') || request.url.startsWith('data:') ? 'Fetch.continueRequest' : 'Fetch.failRequest', request.url.startsWith(base + '/') || request.url.startsWith('data:') ? { requestId } : { requestId, errorReason: 'BlockedByClient' }).catch((error) => errors.push(error.message));
    }
  });
  await call('Page.enable');
  await call('Runtime.enable');
  assert.ok(methods.has('Page.addScriptToEvaluateOnNewDocument'));
  await call('Page.addScriptToEvaluateOnNewDocument', { source: `
    for (const type of ['keydown', 'keypress', 'keyup', 'input', 'change']) {
      document.addEventListener(type, (event) => {
        queueMicrotask(() => console.debug('OWN_FIXTURE_RECEIPT ' + JSON.stringify({
          type, key: event.key, code: event.code, isTrusted: event.isTrusted,
          defaultPrevented: event.defaultPrevented, target: event.target.name || event.target.tagName,
          value: event.target.value, mode: document.querySelector('select')?.value,
          focused: document.hasFocus(), visibility: document.visibilityState,
        })));
      }, true);
    }
  ` });
  await call('Fetch.enable', { patterns: [{ urlPattern: '*' }] });
  async function evaluate(expression) {
    const answer = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (answer.exceptionDetails) throw new Error(answer.exceptionDetails.exception?.description || answer.exceptionDetails.text);
    return answer.result.value;
  }
  failureEvaluate = evaluate;
  async function observe(condition) {
    stage = condition;
    console.log('observe:', condition);
    return evaluate(`new Promise((resolve, reject) => {
      let observer, timer;
      const cleanup = () => { observer?.disconnect(); clearTimeout(timer); document.removeEventListener('input', test); document.removeEventListener('change', test); };
      const test = () => { if (${condition}) { cleanup(); resolve(true); } };
      if (${condition}) return resolve(true);
      observer = new MutationObserver(test);
      observer.observe(document.documentElement, { childList: true, subtree: true, attributes: true, characterData: true });
      document.addEventListener('input', test); document.addEventListener('change', test);
      timer = setTimeout(() => { cleanup(); reject(new Error('condition timed out: ' + ${JSON.stringify(condition)} + ' state=' + JSON.stringify({ ready:document.readyState, visibility:document.visibilityState, documentFocused:document.hasFocus(), mode:document.querySelector('select')?.value, active:document.activeElement?.outerHTML, busy:document.querySelector('form')?.getAttribute('aria-busy'), status:document.querySelector('[role=status]')?.textContent, value:document.querySelector('input')?.value }))); }, 5000);
    })`);
  }
  async function navigate() {
    stage = 'own fixture document load';
    console.log('navigate own fixture:', url);
    const loaded = client.once('Page.loadEventFired', sessionId);
    await Promise.all([call('Page.navigate', { url }), loaded]);
    console.log('document loaded:', await evaluate('JSON.stringify({ready:document.readyState,visibility:document.visibilityState,documentFocused:document.hasFocus(),body:document.body.innerText})'));
    if (errors.length) throw new Error(errors.join('\n'));
    await observe(`document.querySelector('.compaction-form')?.getAttribute('aria-busy') === 'false'`);
  }
  async function click(selector) {
    const rect = await evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); if (!node) throw Error('missing click target'); node.scrollIntoView({ block: 'center' }); const r = node.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2, width: r.width, height: r.height }; })()`);
    assert.ok(rect.width > 0 && rect.height > 0, selector);
    await call('Input.dispatchMouseEvent', { type: 'mousePressed', x: rect.x, y: rect.y, button: 'left', clickCount: 1 });
    await call('Input.dispatchMouseEvent', { type: 'mouseReleased', x: rect.x, y: rect.y, button: 'left', clickCount: 1 });
  }
  async function key(key, code, windowsVirtualKeyCode, modifiers = 0) {
    await call('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code, windowsVirtualKeyCode, modifiers });
    await call('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode, modifiers });
  }
  function nativeKey(keyCode) {
    stage = `owned Electron sendInputEvent ${keyCode}`;
    const id = ++nextNative;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { nativePending.delete(id); reject(new Error(`owned native input timed out: ${keyCode} #${id}`)); }, 10_000);
      nativePending.set(id, { resolve, reject, timer, keyCode });
      chrome.stdin.write(JSON.stringify({ id, keyCode }) + '\n', (error) => {
        if (!error) return;
        clearTimeout(timer);
        nativePending.delete(id);
        reject(error);
      });
    });
  }
  async function selectOption(direction, keyCode) {
    assert.equal(keyCode, direction === 'ArrowDown' ? 40 : 38);
    await nativeKey(direction === 'ArrowDown' ? 'down' : 'up');
    if (await evaluate('document.activeElement.tagName === "SELECT"')) await nativeKey('return');
  }
  async function number(value) {
    await click('input[name="auto_compact_percent"]');
    await key('a', 'KeyA', 65, 4);
    await call('Input.insertText', { text: value });
    assert.equal(await evaluate('document.querySelector("input").value'), value);
  }
  async function capture(name) {
    await mkdir(out, { recursive: true });
    const shot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    await writeFile(path.join(out, name + '.png'), Buffer.from(shot.data, 'base64'));
  }

  await navigate();
  assert.equal(await evaluate('document.visibilityState'), 'visible', 'owned fixture visibility');
  assert.equal(await evaluate('document.hasFocus()'), true, 'owned fixture document focus');
  assert.equal(await evaluate('document.querySelector("select").value'), 'default');
  assert.match(await evaluate('document.querySelector(".compaction-form").innerText'), /not an Off switch/);
  await capture('compaction-ui-default-dark-square');
  await key('Tab', 'Tab', 9);
  assert.equal(await evaluate('document.activeElement.tagName'), 'SELECT');
  await selectOption('ArrowDown', 40);
  await observe('document.querySelector("select").value === "custom"');
  assert.equal(await evaluate('document.activeElement.name'), 'auto_compact_percent');
  await number('0');
  await click('button[type="submit"]');
  await observe('document.querySelector("input").getAttribute("aria-invalid") === "true"');
  assert.match(await evaluate('document.querySelector("[role=status]").textContent'), /greater than 0 and at most 100/);
  assert.equal(await evaluate('document.activeElement.name'), 'auto_compact_percent');
  await capture('compaction-ui-invalid-dark-square');
  await number('62.5');
  await click('button[type="submit"]');
  await observe('document.querySelector("[role=status]").textContent.startsWith("Saved 62.5%")');
  await navigate();
  assert.equal(await evaluate('document.querySelector("input").value'), '62.5');

  await evaluate('fixtureFailures.save = true');
  await number('75');
  await click('button[type="submit"]');
  await observe('document.querySelector("[role=status]").textContent.includes("not saved")');
  assert.equal(await evaluate('document.querySelector("input").value'), '75');
  assert.equal(await evaluate('document.querySelector("button[type=submit]").disabled'), false);
  await capture('compaction-ui-save-error-dark-square');
  await evaluate('fixtureFailures.save = false');
  await number('62.5');
  await click('button[type="submit"]');
  await observe('document.querySelector("[role=status]").textContent.startsWith("Saved 62.5%")');

  // Keyboard reset uses the native select and submits the actual form.
  await evaluate('document.querySelector("select").focus()');
  await selectOption('ArrowUp', 38);
  await observe('document.querySelector("select").value === "default"');
  await click('button[type="submit"]');
  await observe('document.querySelector("[role=status]").textContent.startsWith("Default policy saved")');
  assert.equal((await (await fetch(base + '/api/settings')).json()).auto_compact_percent, 0);
  await evaluate('document.querySelector("select").focus()');
  await selectOption('ArrowDown', 40);
  await number('62.5');
  await click('button[type="submit"]');
  await observe('document.querySelector("[role=status]").textContent.startsWith("Saved 62.5%")');

  await evaluate('fixtureFailures.load = true; fixtureReload()');
  await observe('document.querySelector("[role=status]").textContent.includes("Could not load")');
  assert.equal(await evaluate('document.querySelector("button[type=submit]").disabled'), true);
  await evaluate('fixtureFailures.load = false');
  await click('button[type="button"]');
  await observe('document.querySelector(".compaction-form").getAttribute("aria-busy") === "false" && !document.querySelector("button[type=submit]").disabled');
  assert.equal(await evaluate('document.querySelector("input").value'), '62.5');

  const cases = [
    { name: 'desktop-dark-square', width: 640, height: 850, theme: 'dark', round: false, mobile: false },
    { name: 'desktop-light-round', width: 640, height: 850, theme: 'light', round: true, mobile: false },
    { name: 'mobile-dark-round', width: 375, height: 812, theme: 'dark', round: true, mobile: true },
    { name: 'mobile-light-square', width: 375, height: 812, theme: 'light', round: false, mobile: true },
  ];
  const metrics = [];
  for (const c of cases) {
    await call('Emulation.setDeviceMetricsOverride', { width: c.width, height: c.height, deviceScaleFactor: 1, mobile: c.mobile });
    await call('Emulation.setTouchEmulationEnabled', { enabled: c.mobile });
    await evaluate(`document.documentElement.dataset.theme = ${JSON.stringify(c.theme)}; document.documentElement.dataset.corners = ${JSON.stringify(c.round ? 'round' : 'square')}; document.documentElement.style.setProperty('--kou-radius', ${JSON.stringify(c.round ? '8px' : '0px')}); document.querySelector('select').focus();`);
    await key('Tab', 'Tab', 9);
    const geometry = await evaluate(`(() => { const form = document.querySelector('.compaction-form'); const input = form.querySelector('input'); const note = form.querySelector('.set-note'); const style = getComputedStyle(input); return { overflow: document.documentElement.scrollWidth > innerWidth, targets: [...form.querySelectorAll('input,select,button:not([hidden])')].filter(n => !n.disabled).map(n => ({ tag:n.tagName, height:n.getBoundingClientRect().height })), radius:style.borderRadius, outline:style.outlineStyle, outlineWidth:style.outlineWidth, noteColor:getComputedStyle(note).color, background:getComputedStyle(document.querySelector('section')).backgroundColor, inputFont:style.fontSize, width:innerWidth }; })()`);
    const luminance = (rgb) => rgb.match(/[\d.]+/g).slice(0, 3).map(Number).map((value) => value / 255).map((value) => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4).reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0);
    const contrast = [luminance(geometry.noteColor), luminance(geometry.background)].sort((a, b) => a - b);
    geometry.noteContrast = (contrast[1] + 0.05) / (contrast[0] + 0.05);
    assert.ok(geometry.noteContrast >= 4.5, c.name + ' explanatory text contrast');
    assert.equal(geometry.overflow, false, c.name);
    assert.ok(geometry.targets.every((target) => target.height >= 44), c.name + ' tap targets');
    assert.equal(geometry.radius, c.round ? '8px' : '0px', c.name);
    assert.equal(geometry.outline, 'solid', c.name + ' visible focus');
    assert.equal(geometry.outlineWidth, '2px', c.name);
    await capture('compaction-ui-' + c.name);
    metrics.push({ ...c, ...geometry });
  }
  assert.deepEqual(errors, [], 'browser exceptions');
  assert.deepEqual(consoleMessages.filter((message) => message.type === 'error'), [], 'browser console errors');
  assert.ok(pageRequests.every((request) => request.startsWith(base + '/') || request.startsWith('data:')), 'fixture must not request external URLs');
  await writeFile(path.join(out, 'compaction-ui-browser-evidence.log'), JSON.stringify({ runtime: 'existing generic Electron with fixture-only main', executable: browser, readiness: stderr.match(/OWN_FIXTURE_READY[^\n]*/)?.[0], browser: version.product, platform: process.platform, fixture: url, pageRequests, terminalBrowser: { status: terminal.status, error: terminal.error?.message || terminal.stderr.trim() }, checks: ['real keyboard default/custom selection', 'real mouse submit and typed fractional input', 'invalid zero and focused validation', 'save/reload 62.5%', 'saved 0 default reset', 'save error retains draft', 'load error and retry', 'gateway protocol/model retained', 'dark/light square/round', '375px no overflow and 44px targets', 'visible keyboard focus', 'no browser exceptions, console errors or external requests'], metrics, nativeEvents, keyReceipts, consoleMessages, errors }, null, 2));
  console.log('isolated Electron/Chromium fixture PASS:', version.product, metrics.map((c) => c.name).join(', '));
} catch (error) {
  let state;
  try {
    state = await failureEvaluate?.(`({ready:document.readyState, visibility:document.visibilityState, documentFocused:document.hasFocus(), mode:document.querySelector('select')?.value, active:document.activeElement?.outerHTML, busy:document.querySelector('form')?.getAttribute('aria-busy'), status:document.querySelector('[role=status]')?.textContent, value:document.querySelector('input')?.value})`);
  } catch (snapshotError) { state = { snapshotError: snapshotError.message }; }
  const ownWindow = [...stderr.matchAll(/OWN_FIXTURE_WINDOW (\{[^\n]+\})/g)].map((match) => JSON.parse(match[1])).at(-1);
  const failure = { stage, error: error.stack, state, ownWindow, pending: [...(client?.pending.entries() || [])].map(([id, item]) => ({ id, method: item.method })), nativePending: [...nativePending.entries()].map(([id, item]) => ({ id, method: 'webContents.sendInputEvent', keyCode: item.keyCode })), nativeEvents, keyReceipts, consoleMessages, errors, pageRequests, stderr };
  await mkdir(out, { recursive: true });
  const file = path.join(out, `compaction-ui-browser-failure-${process.env.KOU_COMPACTION_BROWSER_RUN || Date.now()}.log`);
  await writeFile(file, JSON.stringify(failure, null, 2));
  console.error('Owned fixture failed before cleanup:', stage, error.message, JSON.stringify(state), 'evidence:', file);
  throw error;
} finally {
  clearTimeout(guard);
  client?.ws.close();
  if (chrome.exitCode === null) {
    const exited = new Promise((resolve) => chrome.once('exit', resolve));
    chrome.kill('SIGTERM');
    await exited;
  }
  await rm(profile, { recursive: true, force: true });
}
