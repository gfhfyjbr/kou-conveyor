const { app, BrowserWindow } = require('electron');
const path = require('node:path');
const fs = require('node:fs');

const profile = process.argv[2];
if (!profile) throw new Error('fixture profile directory is required');
app.setPath('userData', profile);
app.setPath('sessionData', profile);
for (const name of ['logs', 'crashDumps']) {
  const dir = path.join(profile, name);
  fs.mkdirSync(dir, { recursive: true });
  app.setPath(name, dir);
}
app.commandLine.appendSwitch('remote-debugging-port', '0');
app.commandLine.appendSwitch('disable-background-networking');
app.commandLine.appendSwitch('disk-cache-dir', path.join(profile, 'cache'));
app.commandLine.appendSwitch('disable-component-update');
app.commandLine.appendSwitch('disable-sync');
app.disableHardwareAcceleration();
app.whenReady().then(async () => {
  const window = new BrowserWindow({
    show: true, width: 640, height: 850,
    webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false, backgroundThrottling: false },
  });
  const windowState = () => ({ visible: window.isVisible(), focused: window.isFocused() });
  const reportWindow = () => process.stderr.write(`OWN_FIXTURE_WINDOW ${JSON.stringify(windowState())}\n`);
  for (const event of ['show', 'hide', 'focus', 'blur']) window.on(event, reportWindow);
  await window.loadURL('about:blank');
  window.show();
  window.focus();
  if (!window.isFocused()) {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`owned window focus timed out: ${JSON.stringify(windowState())}`)), 5000);
      window.once('focus', () => { clearTimeout(timer); resolve(); });
    });
  }
  reportWindow();
  require('node:readline').createInterface({ input: process.stdin }).on('line', (line) => {
    let id;
    try {
      const command = JSON.parse(line);
      id = command.id;
      if (!['down', 'up', 'return'].includes(command.keyCode)) throw new Error('unsupported fixture key');
      if (!window.isVisible() || !window.isFocused()) throw new Error('owned fixture window is not focused');
      // Match the installed Electron sender's rawKeyDown/char/keyUp API, never global keys.
      const events = [{ type: 'rawKeyDown', keyCode: command.keyCode }];
      if (command.keyCode === 'return') events.push({ type: 'char', keyCode: '\r' });
      events.push({ type: 'keyUp', keyCode: command.keyCode });
      for (const event of events) window.webContents.sendInputEvent(event);
      process.stderr.write(`OWN_FIXTURE_INPUT ${JSON.stringify({ id, events, window: windowState() })}\n`);
    } catch (error) {
      process.stderr.write(`OWN_FIXTURE_INPUT ${JSON.stringify({ id, error: error.message, window: windowState() })}\n`);
    }
  });
  process.stderr.write(`OWN_FIXTURE_READY electron=${process.versions.electron} chrome=${process.versions.chrome} window=${JSON.stringify(windowState())}\n`);
}).catch((error) => {
  process.stderr.write(`OWN_FIXTURE_ERROR ${error.stack}\n`);
  app.exit(1);
});
app.on('window-all-closed', () => app.quit());
