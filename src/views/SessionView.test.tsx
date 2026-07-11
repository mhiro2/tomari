import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { KeepAwakeStatus } from '../lib/types';
import { SessionView } from './SessionView';

// Mock the Tauri command bridge so the real `api` wrappers run against it.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

// vitest.setup.ts stubs `listen` as a permanent no-op; capture the callback
// here so tests can drive the "tomari:keep-awake-changed" event directly.
const { listen } = await import('@tauri-apps/api/event');
const mockListen = vi.mocked(listen);

const OFF: KeepAwakeStatus = { active: false, lidClose: 'off' };
const ON: KeepAwakeStatus = { active: true, lidClose: 'engaged' };

// Marks an override value as a command rejection rather than a resolved value.
class Rejection {
  constructor(readonly reason: unknown) {}
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const result = overrides[cmd];
      return result instanceof Rejection ? Promise.reject(result.reason) : Promise.resolve(result);
    }
    switch (cmd) {
      case 'get_keep_awake':
        return Promise.resolve(OFF);
      case 'set_keep_awake':
        return Promise.resolve(ON);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('SessionView', () => {
  let keepAwakeChanged: ((payload: KeepAwakeStatus) => void) | undefined;

  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
    keepAwakeChanged = undefined;
    mockListen.mockReset();
    mockListen.mockImplementation((event, handler) => {
      if (event === 'tomari:keep-awake-changed') {
        keepAwakeChanged = (payload) =>
          (handler as (e: { event: string; id: number; payload: unknown }) => void)({
            event,
            id: 0,
            payload,
          });
      }
      return Promise.resolve(() => {});
    });
  });

  it('ignores a second click while a toggle is in flight', async () => {
    let resolveSet!: (v: KeepAwakeStatus) => void;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_keep_awake') return Promise.resolve(OFF);
      if (cmd === 'set_keep_awake') return new Promise((resolve) => (resolveSet = resolve));
      return Promise.resolve(null);
    });

    render(<SessionView />);
    const toggle = await screen.findByRole('switch');
    expect(toggle).not.toBeDisabled();

    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toBeDisabled());
    fireEvent.click(toggle);
    fireEvent.click(toggle);

    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'set_keep_awake')).toHaveLength(1);

    resolveSet(ON);
    await waitFor(() => expect(toggle).not.toBeDisabled());
  });

  it('shows an error and re-syncs from the backend when set_keep_awake rejects', async () => {
    mockCommands({
      set_keep_awake: new Rejection({ code: 'permissionRequired', message: 'denied' }),
    });

    render(<SessionView />);
    const toggle = await screen.findByRole('switch');
    fireEvent.click(toggle);

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Accessibility permission is required.',
    );
    await waitFor(() => expect(toggle).toHaveAttribute('aria-checked', 'false'));
    expect(toggle).not.toBeDisabled();
  });

  it('updates from the tomari:keep-awake-changed event', async () => {
    render(<SessionView />);
    const toggle = await screen.findByRole('switch');
    expect(toggle).toHaveAttribute('aria-checked', 'false');

    keepAwakeChanged?.(ON);

    await waitFor(() => expect(toggle).toHaveAttribute('aria-checked', 'true'));
    expect(await screen.findByText('Active')).toBeInTheDocument();
  });

  it('does not throw an unhandled rejection when the re-sync getKeepAwake also rejects', async () => {
    let getCalls = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_keep_awake') {
        getCalls += 1;
        // First call resolves for the initial fetch; the re-sync call rejects.
        return getCalls === 1 ? Promise.resolve(OFF) : Promise.reject(new Error('offline'));
      }
      if (cmd === 'set_keep_awake') return Promise.reject(new Error('boom'));
      return Promise.resolve(null);
    });

    render(<SessionView />);
    const toggle = await screen.findByRole('switch');
    fireEvent.click(toggle);

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
    // The toggle finishes (not stuck busy) even though the re-sync failed.
    await waitFor(() => expect(toggle).not.toBeDisabled());
    expect(toggle).toHaveAttribute('aria-checked', 'false');
  });
});
