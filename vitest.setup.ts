import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';

// Tauri's event API touches internals that don't exist under jsdom. Components
// only subscribe for live updates, so a no-op listen/emit is enough for tests.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emit: vi.fn(() => Promise.resolve()),
}));

// jsdom does not implement native dialog focus trapping. Keep its open/close
// state in tests; supported macOS WebKit supplies the native methods.
HTMLDialogElement.prototype.showModal = function () {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close = function () {
  this.removeAttribute('open');
};

afterEach(() => {
  cleanup();
});
