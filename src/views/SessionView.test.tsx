import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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

const OFF: KeepAwakeStatus = {
  active: false,
  lidClose: 'off',
  phase: 'off',
  options: { durationSecs: null, endsAtMs: null, acOnly: false, lowBatteryAction: 'warn' },
  notice: null,
  revision: 1,
};
// Revisions order the fixtures the way the backend would emit them: ENABLING is
// stamped before the ON it settles into, so a late ENABLING is a stale snapshot.
const ENABLING: KeepAwakeStatus = {
  ...OFF,
  active: true,
  lidClose: 'pending',
  phase: 'enabling',
  revision: 2,
};
const ON: KeepAwakeStatus = {
  ...OFF,
  active: true,
  lidClose: 'engaged',
  phase: 'on',
  revision: 3,
};

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
    const toggle = await screen.findByRole('switch', { name: 'Keep this Mac awake' });
    expect(toggle).not.toBeDisabled();

    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toBeDisabled());
    fireEvent.click(toggle);
    fireEvent.click(toggle);

    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'set_keep_awake')).toHaveLength(1);

    // The administrator prompt keeps every surface locked well past the command
    // response, so the pending phase — not the in-flight call — holds it shut.
    resolveSet(ENABLING);
    keepAwakeChanged?.(ENABLING);
    await waitFor(() => expect(toggle).toBeDisabled());
    fireEvent.click(toggle);
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'set_keep_awake')).toHaveLength(1);

    keepAwakeChanged?.(ON);
    await waitFor(() => expect(toggle).not.toBeDisabled());
  });

  it('keeps a settled event when the command that started it responds late', async () => {
    let resolveSet!: (v: KeepAwakeStatus) => void;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_keep_awake') return Promise.resolve(OFF);
      if (cmd === 'set_keep_awake') return new Promise((resolve) => (resolveSet = resolve));
      return Promise.resolve(null);
    });

    render(<SessionView />);
    const toggle = await screen.findByRole('switch', { name: 'Keep this Mac awake' });
    fireEvent.click(toggle);

    // The background worker can settle before the response to the call that
    // spawned it gets back, so that response carries the older snapshot.
    keepAwakeChanged?.(ON);
    await waitFor(() => expect(toggle).toHaveAttribute('aria-checked', 'true'));
    resolveSet(ENABLING);

    // Applying it would strand the panel in a transition that already finished.
    await waitFor(() => expect(toggle).not.toBeDisabled());
    expect(toggle).toHaveAttribute('aria-checked', 'true');
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
  });

  it('subscribes to the change event before the first read', async () => {
    const order: string[] = [];
    mockListen.mockImplementation((event) => {
      order.push(`listen:${event}`);
      return Promise.resolve(() => {});
    });
    mockInvoke.mockImplementation((cmd: string) => {
      order.push(`invoke:${cmd}`);
      return Promise.resolve(cmd === 'get_keep_awake' ? OFF : null);
    });

    render(<SessionView />);
    await waitFor(() => expect(order).toContain('invoke:get_keep_awake'));

    // A transition settling in the gap would emit an event nobody is listening
    // for yet, and the read's pending snapshot would become the final state.
    expect(order.indexOf('listen:tomari:keep-awake-changed')).toBeLessThan(
      order.indexOf('invoke:get_keep_awake'),
    );
  });

  it('ignores an event whose revision is older than one already applied', async () => {
    render(<SessionView />);
    const toggle = await screen.findByRole('switch', { name: 'Keep this Mac awake' });

    keepAwakeChanged?.(ON);
    await waitFor(() => expect(toggle).toHaveAttribute('aria-checked', 'true'));

    // Stamped before ON but delivered after it: every emitting backend thread
    // snapshots under the state lock and emits outside it, so two of them can
    // reach the webview in the opposite order. `act` flushes the render, so the
    // assertions below see the applied state rather than a pending update.
    await act(async () => keepAwakeChanged?.(ENABLING));

    expect(screen.getByText('Sleep is being prevented')).toBeInTheDocument();
    expect(toggle).not.toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
  });

  it('shows an error and re-syncs from the backend when set_keep_awake rejects', async () => {
    mockCommands({
      set_keep_awake: new Rejection({ code: 'permissionRequired', message: 'denied' }),
    });

    render(<SessionView />);
    const toggle = await screen.findByRole('switch', { name: 'Keep this Mac awake' });
    fireEvent.click(toggle);

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Accessibility permission is required.',
    );
    // One fetch on mount, one re-sync after the failed toggle.
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_keep_awake')).toHaveLength(2),
    );
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    expect(toggle).not.toBeDisabled();
  });

  it('offers a retry when the initial getKeepAwake rejects', async () => {
    let attempts = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_keep_awake') {
        attempts += 1;
        return attempts === 1 ? Promise.reject(new Error('backend gone')) : Promise.resolve(OFF);
      }
      return Promise.resolve(null);
    });

    render(<SessionView />);

    expect(await screen.findByRole('alert')).toHaveTextContent('backend gone');
    // A failed initial read must not expose a clickable switch backed by a
    // guessed off state.
    const toggle = screen.getByRole('switch', { name: 'Keep this Mac awake' });
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    expect(toggle).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(toggle).not.toBeDisabled());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(attempts).toBe(2);
  });

  it('updates from the tomari:keep-awake-changed event', async () => {
    render(<SessionView />);
    const toggle = await screen.findByRole('switch', { name: 'Keep this Mac awake' });
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
    const toggle = await screen.findByRole('switch', { name: 'Keep this Mac awake' });
    fireEvent.click(toggle);

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
    // The toggle finishes (not stuck busy) even though the re-sync failed.
    await waitFor(() => expect(toggle).not.toBeDisabled());
    expect(toggle).toHaveAttribute('aria-checked', 'false');
  });

  it('cancels an administrator prompt without issuing another normal toggle', async () => {
    mockCommands({ get_keep_awake: ENABLING, cancel_keep_awake_transition: OFF });

    render(<SessionView />);

    const toggle = await screen.findByRole('switch', { name: 'Working…' });
    expect(toggle).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.filter(([cmd]) => cmd === 'cancel_keep_awake_transition'),
      ).toHaveLength(1),
    );
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'set_keep_awake')).toHaveLength(0);
  });

  it('stores a 30-minute preset that the backend arms when enabled', async () => {
    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'get_keep_awake') return Promise.resolve(OFF);
      if (cmd === 'configure_keep_awake') {
        return Promise.resolve({
          ...OFF,
          options: (args as { options: KeepAwakeStatus['options'] }).options,
        });
      }
      return Promise.resolve(null);
    });

    render(<SessionView />);
    const timer = await screen.findByRole('combobox', { name: 'Turn off automatically' });
    fireEvent.change(timer, { target: { value: '30m' } });

    await waitFor(() => {
      const call = mockInvoke.mock.calls.find(([cmd]) => cmd === 'configure_keep_awake');
      expect(call?.[1]).toEqual({
        options: {
          durationSecs: 30 * 60,
          endsAtMs: null,
          acOnly: false,
          lowBatteryAction: 'warn',
        },
      });
    });
  });

  it('drops an end time the backend reports as spent instead of re-sending it', async () => {
    const spent = Date.now() - 60_000;
    mockCommands({
      get_keep_awake: { ...OFF, options: { ...OFF.options, endsAtMs: spent } },
    });

    render(<SessionView />);
    const timer = await screen.findByRole('combobox', { name: 'Turn off automatically' });
    await waitFor(() => expect(timer).toHaveValue('time'));

    // Engaging clears an end time that is already in the past, so the panel must
    // follow the backend rather than resurrect the deadline on the next toggle —
    // which would refuse the session it was just asked to start.
    keepAwakeChanged?.(OFF);
    await waitFor(() => expect(timer).toHaveValue('never'));

    fireEvent.click(screen.getByRole('switch', { name: 'Keep this Mac awake' }));
    await waitFor(() => {
      const call = mockInvoke.mock.calls.find(([cmd]) => cmd === 'set_keep_awake');
      expect(call?.[1]).toEqual({ enabled: true, options: OFF.options });
    });
  });

  it('clears the stored deadline when the end-time field is emptied', async () => {
    const deadline = Date.now() + 3_600_000;
    mockCommands({
      get_keep_awake: { ...OFF, options: { ...OFF.options, endsAtMs: deadline } },
    });

    render(<SessionView />);
    const endTime = await screen.findByLabelText('End time');
    fireEvent.change(endTime, { target: { value: '' } });

    // Without this the backend would keep enforcing an end time the panel no
    // longer shows, and the next event would restore it into the field.
    await waitFor(() => {
      const call = mockInvoke.mock.calls.findLast(([cmd]) => cmd === 'configure_keep_awake');
      expect(call?.[1]).toEqual({
        options: { durationSecs: null, endsAtMs: null, acOnly: false, lowBatteryAction: 'warn' },
      });
    });
  });

  it('renders the backend deadline as a live countdown', async () => {
    const now = 1_800_000_000_000;
    vi.spyOn(Date, 'now').mockReturnValue(now);
    mockCommands({
      get_keep_awake: { ...ON, options: { ...ON.options, endsAtMs: now + 65_000 } },
    });

    render(<SessionView />);

    expect(await screen.findByLabelText('Time remaining')).toHaveTextContent('1:05');
  });
});
