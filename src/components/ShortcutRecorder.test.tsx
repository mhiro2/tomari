import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ShortcutRecorder } from './ShortcutRecorder';

// Mock the Tauri command bridge so the real `api` wrappers run against it.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const value = overrides[cmd];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    switch (cmd) {
      case 'validate_accelerator':
        return Promise.resolve({ valid: true, normalized: 'Shift+Cmd+L', error: null });
      default:
        return Promise.resolve(null);
    }
  });
}

function suspendCalls(): boolean[] {
  return mockInvoke.mock.calls
    .filter(([cmd]) => cmd === 'set_hotkeys_suspended')
    .map(([, args]) => (args as { suspended: boolean }).suspended);
}

describe('ShortcutRecorder', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
  });

  it('suspends shortcuts while recording and resumes on Escape', async () => {
    render(<ShortcutRecorder onCapture={() => {}} ariaLabel="Record shortcut" />);

    const button = screen.getByRole('button', { name: 'Record shortcut' });
    fireEvent.click(button);
    await screen.findByText('Type shortcut…');
    expect(suspendCalls()).toEqual([true]);

    fireEvent.keyDown(button, { code: 'Escape' });
    await screen.findByText('Click to record');
    await waitFor(() => expect(suspendCalls()).toEqual([true, false]));
  });

  it('focuses the field on start so keydown reaches it', async () => {
    // macOS WebKit does not focus a <button> on click, so without an explicit
    // focus the recorder would never see the chord or Escape on device.
    render(<ShortcutRecorder onCapture={() => {}} ariaLabel="Record shortcut" />);

    const button = screen.getByRole('button', { name: 'Record shortcut' });
    fireEvent.click(button);
    await screen.findByText('Type shortcut…');
    expect(document.activeElement).toBe(button);
  });

  it('commits the normalized chord and resumes', async () => {
    const onCapture = vi.fn();
    render(<ShortcutRecorder onCapture={onCapture} ariaLabel="Record shortcut" />);

    const button = screen.getByRole('button', { name: 'Record shortcut' });
    fireEvent.click(button);
    await screen.findByText('Type shortcut…');

    fireEvent.keyDown(button, { code: 'KeyL', metaKey: true, shiftKey: true });
    await waitFor(() => expect(onCapture).toHaveBeenCalledWith('Shift+Cmd+L'));
    await waitFor(() => expect(suspendCalls()).toEqual([true, false]));
  });

  it('releases the lease when unmounted mid-recording', async () => {
    const { unmount } = render(
      <ShortcutRecorder onCapture={() => {}} ariaLabel="Record shortcut" />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Record shortcut' }));
    await screen.findByText('Type shortcut…');
    expect(suspendCalls()).toEqual([true]);

    unmount();
    await waitFor(() => expect(suspendCalls()).toEqual([true, false]));
  });

  // Make the first suspend IPC hang until the test resolves it, so the test
  // can act while the acquire is still in flight.
  function deferFirstSuspend(): { resolve: () => void } {
    const deferred = { resolve: () => {} };
    let suspends = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'set_hotkeys_suspended' && (suspends += 1) === 1) {
        return new Promise<void>((resolve) => (deferred.resolve = resolve));
      }
      return Promise.resolve(null);
    });
    return deferred;
  }

  it('hands the lease back when unmounted while the suspend IPC is pending', async () => {
    const suspend = deferFirstSuspend();
    const { unmount } = render(
      <ShortcutRecorder onCapture={() => {}} ariaLabel="Record shortcut" />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Record shortcut' }));
    await waitFor(() => expect(suspendCalls()).toEqual([true]));

    // The unmount cleanup runs before the suspend settles, so it cannot see
    // the lease yet — start() itself must give it back once acquired.
    unmount();
    suspend.resolve();
    await waitFor(() => expect(suspendCalls()).toEqual([true, false]));
  });

  it('takes a single lease when clicked twice before the suspend settles', async () => {
    const suspend = deferFirstSuspend();
    render(<ShortcutRecorder onCapture={() => {}} ariaLabel="Record shortcut" />);

    const button = screen.getByRole('button', { name: 'Record shortcut' });
    fireEvent.click(button);
    fireEvent.click(button);
    await waitFor(() => expect(suspendCalls()).toEqual([true]));
    suspend.resolve();
    await screen.findByText('Type shortcut…');

    // One stop must fully resume: a double-counted lease would stay suspended.
    fireEvent.keyDown(button, { code: 'Escape' });
    await waitFor(() => expect(suspendCalls()).toEqual([true, false]));
  });

  it('keeps shortcuts suspended until the last recorder stops', async () => {
    render(
      <>
        <ShortcutRecorder onCapture={() => {}} ariaLabel="First" />
        <ShortcutRecorder onCapture={() => {}} ariaLabel="Second" />
      </>,
    );
    const first = screen.getByRole('button', { name: 'First' });
    const second = screen.getByRole('button', { name: 'Second' });

    fireEvent.click(first);
    fireEvent.click(second);
    await waitFor(() => expect(screen.getAllByText('Type shortcut…')).toHaveLength(2));
    expect(suspendCalls()).toEqual([true]);

    fireEvent.keyDown(first, { code: 'Escape' });
    await waitFor(() => expect(screen.getAllByText('Type shortcut…')).toHaveLength(1));
    expect(suspendCalls()).toEqual([true]);

    fireEvent.keyDown(second, { code: 'Escape' });
    await waitFor(() => expect(suspendCalls()).toEqual([true, false]));
  });

  it('does not start recording when suspension fails', async () => {
    mockCommands({ set_hotkeys_suspended: new Error('ipc down') });
    render(<ShortcutRecorder onCapture={() => {}} ariaLabel="Record shortcut" />);

    const button = screen.getByRole('button', { name: 'Record shortcut' });
    fireEvent.click(button);

    await screen.findByText('Could not start recording');
    expect(screen.queryByText('Type shortcut…')).not.toBeInTheDocument();
    // The failed attempt took no lease, so nothing tries to resume afterwards.
    expect(suspendCalls()).toEqual([true]);
  });
});
