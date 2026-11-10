import init from './kou_web.js';

try {
  await init();
} catch {
  const message = document.createElement('p');
  message.setAttribute('role', 'alert');
  message.textContent = 'The Rust session reader could not start. Reload the page or rebuild its WASM assets.';
  document.body.append(message);
}
