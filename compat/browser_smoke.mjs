import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { promises as fs } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '../tmp/rust-browser/node_modules/playwright/index.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'kou-browser-'));
let server;
let browser;
try {
  const fixtures = path.join(root, 'harness/sessionstore/localfile/testdata');
  for (const name of ['golden-session', 'golden-fork']) {
    await fs.copyFile(path.join(fixtures, `${name}.session.jsonl`), path.join(directory, `${name}.session.jsonl`));
  }
  const original = await fs.readFile(path.join(directory, 'golden-session.session.jsonl'), 'utf8');
  await fs.writeFile(path.join(directory, 'evil.session.jsonl'), original.replace('"ID":"golden-session"', '"ID":"evil"').replace('"message":"hello"', '"message":"<img src=x onerror=alert(1)>"'), { mode: 0o600 });
  server = spawn(path.join(root, 'tmp/rust-rewrite-target/debug/kou-conveyor-web-rs'), [
    '--session-dir', directory, '--listen', '127.0.0.1:0', '--assets', path.join(root, 'tmp/rust-web-assets'),
  ], { cwd: root, stdio: ['ignore', 'ignore', 'pipe'] });
  const url = await new Promise((resolve, reject) => {
    let stderr = '';
    const timer = setTimeout(() => reject(new Error(`server startup timed out: ${stderr}`)), 30000);
    server.once('error', reject);
    server.once('exit', code => { clearTimeout(timer); reject(new Error(`server exited: ${code}`)); });
    server.stderr.on('data', chunk => {
      stderr += chunk.toString();
      const match = stderr.match(/http:\/\/127\.0\.0\.1:\d+\b/);
      if (match) { clearTimeout(timer); resolve(match[0]); }
    });
  });
  browser = await chromium.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true });
  const errors = [];
  for (const [name, width, height, colorScheme] of [['desktop', 1280, 800, 'dark'], ['mobile', 375, 812, 'light']]) {
    const page = await browser.newPage({ viewport: { width, height }, colorScheme });
    page.on('pageerror', error => errors.push(error.message));
    page.on('dialog', async dialog => { errors.push(`unexpected dialog: ${dialog.type()}`); await dialog.dismiss(); });
    await page.goto(url);
    await page.locator('#session option[value="golden-session"]').waitFor({ state: 'attached' });
    await page.selectOption('#session', 'golden-session');
    await page.getByRole('heading', { name: '4 records', exact: true }).waitFor();
    assert.equal(await page.locator('.record').count(), 4);
    await page.locator('.record').filter({ hasText: 'tool_call_status' }).click();
    await page.getByRole('heading', { name: 'Record 4', exact: true }).waitFor();
    await page.waitForFunction(() => document.querySelector('pre')?.textContent.includes('"Status": "ready"'));
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
    assert.equal(overflow, false, `${name}: horizontal overflow`);
    await page.screenshot({ path: path.join(root, `rust-rewrite-${name}.png`), fullPage: true });
    await page.getByRole('button', { name: 'Close record', exact: true }).click();
    assert.equal(await page.locator('pre').count(), 0);
    await page.selectOption('#session', 'golden-fork');
    await page.selectOption('#session', 'golden-session');
    await page.getByRole('heading', { name: '4 records', exact: true }).waitFor();
    await page.selectOption('#session', 'evil');
    await page.getByRole('heading', { name: '4 records', exact: true }).waitFor();
    await page.locator('.record-preview').filter({ hasText: '<img src=x onerror=alert(1)>' }).waitFor();
    assert.equal(await page.locator('img').count(), 0);
    await page.keyboard.press('Tab');
    assert.equal(await page.evaluate(() => document.activeElement?.tagName === 'BODY'), false);
    await page.close();
    console.log(`PASS: ${name}, real WASM load, selection, lazy record, stale-request switching, XSS text, keyboard, layout`);
  }
  assert.deepEqual(errors, []);
} finally {
  if (browser) await browser.close();
  if (server && server.exitCode === null) {
    server.kill('SIGINT');
    await new Promise(resolve => server.once('exit', resolve));
  }
  await fs.rm(directory, { recursive: true, force: true });
}
