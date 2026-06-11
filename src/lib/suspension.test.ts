import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./api', () => ({ setHotkeysSuspended: vi.fn() }));

// The lease state lives at module scope, so each test re-imports a fresh copy
// of the module. The mocked api instance survives resetModules, so its call
// history is cleared here instead.
async function load() {
  vi.resetModules();
  const api = await import('./api');
  const suspension = await import('./suspension');
  const setSuspended = vi.mocked(api.setHotkeysSuspended);
  setSuspended.mockReset();
  setSuspended.mockResolvedValue(undefined);
  return { ...suspension, setSuspended };
}

// Let the serialized suspendQueue drain, including chained continuations.
function flush(): Promise<void> {
  let drained = Promise.resolve();
  for (let i = 0; i < 10; i += 1) drained = drained.then(() => undefined);
  return drained;
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('suspension lease', () => {
  it('suspends on acquire and resumes on release', async () => {
    const { acquireSuspension, releaseSuspension, setSuspended } = await load();

    await acquireSuspension();
    expect(setSuspended.mock.calls).toEqual([[true]]);

    releaseSuspension();
    await flush();
    expect(setSuspended.mock.calls).toEqual([[true], [false]]);
  });

  it('keeps shortcuts suspended while any holder remains', async () => {
    const { acquireSuspension, releaseSuspension, setSuspended } = await load();

    await acquireSuspension();
    await acquireSuspension();
    // Already suspended: the second acquire must not re-send.
    expect(setSuspended.mock.calls).toEqual([[true]]);

    releaseSuspension();
    await flush();
    // One holder left: still suspended.
    expect(setSuspended.mock.calls).toEqual([[true]]);

    releaseSuspension();
    await flush();
    expect(setSuspended.mock.calls).toEqual([[true], [false]]);
  });

  it('skips the IPC entirely when released before the queue runs', async () => {
    const { acquireSuspension, releaseSuspension, setSuspended } = await load();

    // The release lands before the queued sync evaluates the count, so the
    // backend never sees a suspend it would then have to undo.
    const acquired = acquireSuspension();
    releaseSuspension();
    await acquired;
    await flush();

    expect(setSuspended).not.toHaveBeenCalled();
  });

  it('serializes a release behind an in-flight suspend', async () => {
    const { acquireSuspension, releaseSuspension, setSuspended } = await load();

    let resolveSuspend!: () => void;
    setSuspended.mockImplementationOnce(
      () => new Promise<void>((resolve) => (resolveSuspend = resolve)),
    );

    const acquired = acquireSuspension();
    await flush();
    expect(setSuspended.mock.calls).toEqual([[true]]);

    // Released while the suspend IPC is still pending: the resume must queue
    // behind it, never racing ahead of the unfinished suspend.
    releaseSuspension();
    await flush();
    expect(setSuspended.mock.calls).toEqual([[true]]);

    resolveSuspend();
    await acquired;
    await flush();
    expect(setSuspended.mock.calls).toEqual([[true], [false]]);
  });

  it('takes no lease when the suspend IPC fails', async () => {
    const { acquireSuspension, releaseSuspension, setSuspended } = await load();

    setSuspended.mockRejectedValueOnce(new Error('ipc down'));
    await expect(acquireSuspension()).rejects.toThrow('ipc down');

    // The failed acquire dropped its count: a balanced acquire/release from
    // another recorder must still resume at the end.
    await acquireSuspension();
    releaseSuspension();
    await flush();
    expect(setSuspended.mock.calls).toEqual([[true], [true], [false]]);
  });

  it('retries once after a failed resume', async () => {
    const { acquireSuspension, releaseSuspension, setSuspended } = await load();

    await acquireSuspension();
    setSuspended.mockRejectedValueOnce(new Error('ipc down'));
    releaseSuspension();
    await flush();
    expect(setSuspended.mock.calls).toEqual([[true], [false]]);

    await vi.advanceTimersByTimeAsync(500);
    expect(setSuspended.mock.calls).toEqual([[true], [false], [false]]);
  });

  it('ignores a release without a matching acquire', async () => {
    const { acquireSuspension, releaseSuspension, setSuspended } = await load();

    releaseSuspension();
    await flush();
    expect(setSuspended).not.toHaveBeenCalled();

    // The stray release must not have gone negative and eaten a real lease.
    await acquireSuspension();
    expect(setSuspended.mock.calls).toEqual([[true]]);
  });
});
