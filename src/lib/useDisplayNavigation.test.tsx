import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useDisplayNavigation } from './useDisplayNavigation';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const { listen } = await import('@tauri-apps/api/event');
const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

beforeEach(() => {
  mockInvoke.mockReset();
  mockListen.mockReset();
  mockListen.mockResolvedValue(() => {});
});

describe('display settings navigation', () => {
  it('subscribes before pulling a request retained during startup', async () => {
    let ready: ((stop: () => void) => void) | undefined;
    mockListen.mockReturnValue(
      new Promise((resolve) => {
        ready = resolve;
      }),
    );
    mockInvoke.mockResolvedValue(7);
    const open = vi.fn();
    renderHook(() => useDisplayNavigation(open));
    expect(mockInvoke).not.toHaveBeenCalled();
    await act(async () => {
      ready?.(() => {});
    });
    await waitFor(() => expect(open).toHaveBeenCalledOnce());
    expect(mockInvoke).toHaveBeenCalledWith('acknowledge_display_navigation', { revision: 7 });
  });

  it('does not navigate again when an older pull arrives after a newer request', async () => {
    let trigger: (() => void) | undefined;
    mockListen.mockImplementation((_event, callback) => {
      trigger = () => callback({ event: 'tomari:open-displays', id: 1, payload: undefined });
      return Promise.resolve(() => {});
    });
    let finishFirst: ((value: number) => void) | undefined;
    let reads = 0;
    mockInvoke.mockImplementation(async (command) => {
      if (command !== 'get_display_navigation') return;
      reads += 1;
      if (reads === 1)
        return new Promise<number>((resolve) => {
          finishFirst = resolve;
        });
      return 2;
    });
    const open = vi.fn();
    renderHook(() => useDisplayNavigation(open));
    await waitFor(() => expect(reads).toBe(1));
    await act(async () => {
      trigger?.();
    });
    await waitFor(() => expect(open).toHaveBeenCalledOnce());
    await act(async () => {
      finishFirst?.(1);
    });
    expect(open).toHaveBeenCalledOnce();
    expect(mockInvoke).not.toHaveBeenCalledWith('acknowledge_display_navigation', { revision: 1 });
    expect(mockInvoke).toHaveBeenCalledWith('acknowledge_display_navigation', { revision: 2 });
  });

  it('leaves the request unacknowledged when the settings shell unmounts during its read', async () => {
    let finish: ((value: number) => void) | undefined;
    mockInvoke.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const open = vi.fn();
    const view = renderHook(() => useDisplayNavigation(open));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('get_display_navigation'));
    view.unmount();
    await act(async () => {
      finish?.(3);
    });
    expect(open).not.toHaveBeenCalled();
    expect(mockInvoke).not.toHaveBeenCalledWith(
      'acknowledge_display_navigation',
      expect.anything(),
    );
  });
});
