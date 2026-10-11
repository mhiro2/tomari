import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { DisplaySnapshot } from '../lib/displays';
import { DisplayView } from './DisplayView';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);
const { listen } = await import('@tauri-apps/api/event');
const mockListen = vi.mocked(listen);

const MODE = {
  id: 1,
  width: 1920,
  height: 1080,
  pixelWidth: 3840,
  pixelHeight: 2160,
  refreshRate: 60,
  hiDpi: true,
};
const SECOND_MODE = {
  ...MODE,
  id: 2,
  width: 2560,
  height: 1440,
  pixelWidth: 5120,
  pixelHeight: 2880,
};
function inventory(): DisplaySnapshot {
  return {
    displays: [
      {
        id: 'screen-a',
        connectionId: 1,
        name: 'Studio Display',
        builtIn: false,
        main: true,
        mirrored: false,
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        currentMode: MODE,
        modes: [MODE, SECOND_MODE],
        brightness: 0.5,
        softwareDimming: 0,
      },
    ],
    pending: null,
    notice: null,
    revision: 0,
  };
}

function firstDisplay(snapshot: DisplaySnapshot) {
  const display = snapshot.displays[0];
  if (!display) throw new Error('fixture has no display');
  return display;
}

beforeEach(() => {
  mockInvoke.mockReset();
  mockListen.mockClear();
  mockInvoke.mockResolvedValue(inventory());
});

describe('display management', () => {
  it('keeps system display settings reachable when the inventory cannot load', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_displays') throw new Error('inventory unavailable');
    });
    render(<DisplayView />);
    await screen.findByRole('alert');
    fireEvent.click(screen.getByRole('button', { name: 'Open macOS Display settings…' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('open_system_display_settings'));
    expect(mockInvoke).not.toHaveBeenCalledWith('preview_display_configuration', expect.anything());
  });

  it('finishes initial loading when an operation supersedes the first inventory request', async () => {
    let finishInitial: ((value: DisplaySnapshot) => void) | undefined;
    mockInvoke.mockImplementationOnce(
      () =>
        new Promise<DisplaySnapshot>((resolve) => {
          finishInitial = resolve;
        }),
    );
    render(<DisplayView />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('get_displays'));
    expect(screen.getByText('Reading displays…')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Open macOS Display settings…' }));
    await screen.findByRole('combobox', { name: 'Resolution' });
    expect(screen.queryByText('Reading displays…')).not.toBeInTheDocument();
    await act(async () => {
      finishInitial?.(inventory());
    });
    expect(screen.queryByText('Reading displays…')).not.toBeInTheDocument();
  });

  it('reads the inventory and disables a no-op resolution change', async () => {
    render(<DisplayView />);
    expect(await screen.findByRole('combobox', { name: 'Resolution' })).toHaveValue(
      '1920x1080:3840x2160',
    );
    expect(screen.getByRole('button', { name: 'Try resolution' })).toBeDisabled();
    expect(mockInvoke).toHaveBeenCalledWith('get_displays');
  });

  it('explains an incomplete configuration without disabling independent brightness controls', async () => {
    const data = inventory();
    data.displays.push({
      ...firstDisplay(data),
      id: 'unreadable',
      connectionId: 2,
      main: false,
      currentMode: null,
    });
    mockInvoke.mockResolvedValue(data);
    render(<DisplayView />);
    await screen.findByText(/macOS could not report the current resolution for every display/);
    expect(screen.getByRole('combobox', { name: 'Resolution' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Identify' })).toBeEnabled();
    fireEvent.click(screen.getByRole('tab', { name: 'Arrangement' }));
    expect(screen.getByRole('button', { name: 'Try arrangement' })).toBeDisabled();
    fireEvent.click(screen.getByRole('tab', { name: 'Brightness' }));
    expect(screen.getByRole('slider', { name: 'Brightness' })).toBeEnabled();
    expect(screen.getByRole('slider', { name: 'Extra dimming' })).toBeEnabled();
  });

  it('keeps rollback reachable but prevents confirming an unreadable current configuration', async () => {
    const data = inventory();
    firstDisplay(data).currentMode = null;
    data.pending = { token: 1, displayId: 'screen-a', remainingMs: 15000 };
    mockInvoke.mockResolvedValue(data);
    render(<DisplayView />);
    expect(await screen.findByRole('button', { name: 'Keep changes' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Revert now' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Refresh' })).toBeEnabled();
  });

  it('previews a selected mode and only persists after confirmation', async () => {
    const initial = inventory();
    const preview: DisplaySnapshot = {
      ...initial,
      revision: 1,
      pending: { token: 1, displayId: 'screen-a', remainingMs: 20000 },
      displays: [{ ...firstDisplay(initial), currentMode: SECOND_MODE }],
    };
    mockInvoke.mockImplementation(async (command) =>
      command === 'preview_display_configuration' ? preview : initial,
    );
    render(<DisplayView />);
    fireEvent.change(await screen.findByRole('combobox', { name: 'Resolution' }), {
      target: { value: '2560x1440:5120x2880' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Try resolution' }));
    const confirm = await screen.findByRole('button', { name: 'Keep changes' });
    expect(mockInvoke).toHaveBeenCalledWith('preview_display_configuration', {
      request: { kind: 'mode', displayId: 'screen-a', modeId: 2 },
    });
    expect(screen.getByRole('combobox', { name: 'Resolution' })).toBeDisabled();
    expect(mockInvoke).not.toHaveBeenCalledWith('confirm_display_configuration', expect.anything());
    fireEvent.click(confirm);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('confirm_display_configuration', { token: 1 }),
    );
  });

  it('exposes a restore action when the backend reports a failed rollback', async () => {
    mockInvoke.mockResolvedValue({
      ...inventory(),
      notice: 'restoreFailed',
      pending: { token: 8, displayId: 'screen-a', remainingMs: 15000 },
    });
    render(<DisplayView />);
    expect(await screen.findByRole('button', { name: 'Keep changes' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Revert now' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('revert_display_configuration', { token: 8 }),
    );
  });

  it('does not offer hardware brightness when it is unavailable', async () => {
    const data = inventory();
    firstDisplay(data).brightness = null;
    mockInvoke.mockResolvedValue(data);
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Brightness' }));
    await screen.findByText(/Hardware brightness is unavailable/);
    expect(screen.queryByRole('slider', { name: 'Brightness' })).not.toBeInTheDocument();
  });

  it('sends normalized brightness only after Apply', async () => {
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Brightness' }));
    fireEvent.change(await screen.findByRole('slider', { name: 'Brightness' }), {
      target: { value: '37' },
    });
    expect(mockInvoke).not.toHaveBeenCalledWith('set_display_brightness', expect.anything());
    fireEvent.click(screen.getByRole('button', { name: 'Apply brightness' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('set_display_brightness', {
        displayId: 'screen-a',
        value: 0.37,
      }),
    );
  });

  it('shows a retryable inventory failure', async () => {
    mockInvoke.mockRejectedValueOnce(new Error('offline'));
    render(<DisplayView />);
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not read your displays');
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(await screen.findByRole('combobox', { name: 'Resolution' })).toBeEnabled();
  });

  it('does not allow mirrored modes to change independently', async () => {
    const data = inventory();
    firstDisplay(data).mirrored = true;
    mockInvoke.mockResolvedValue(data);
    render(<DisplayView />);
    await screen.findByText(/Change mirrored displays together/);
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  });

  it('explains configuration restrictions when another screen is mirrored', async () => {
    const data = inventory();
    data.displays.push({
      ...firstDisplay(data),
      id: 'mirrored-screen',
      connectionId: 2,
      main: false,
      mirrored: true,
    });
    mockInvoke.mockResolvedValue(data);
    render(<DisplayView />);
    await screen.findByText(/Change mirrored displays together/);
    expect(screen.getByRole('button', { name: 'Try resolution' })).toBeDisabled();
    fireEvent.click(screen.getByRole('tab', { name: 'Arrangement' }));
    expect(screen.getByText(/Change mirrored displays together/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Try arrangement' })).toBeDisabled();
  });

  it('ignores a stale inventory response after a display mutation', async () => {
    let resolveOld: ((value: DisplaySnapshot) => void) | undefined;
    let calls = 0;
    const after = {
      ...inventory(),
      revision: 1,
      pending: { token: 1, displayId: 'screen-a', remainingMs: 20000 },
    };
    mockInvoke.mockImplementation((command) => {
      if (command === 'preview_display_configuration') return Promise.resolve(after);
      if (++calls === 2)
        return new Promise<DisplaySnapshot>((resolve) => {
          resolveOld = resolve;
        });
      return Promise.resolve(inventory());
    });
    render(<DisplayView />);
    fireEvent.change(await screen.findByRole('combobox', { name: 'Resolution' }), {
      target: { value: '2560x1440:5120x2880' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    fireEvent.click(screen.getByRole('button', { name: 'Try resolution' }));
    await screen.findByRole('button', { name: 'Keep changes' });
    await act(async () => {
      resolveOld?.(inventory());
    });
    expect(screen.getByRole('button', { name: 'Keep changes' })).toBeInTheDocument();
  });
  it('previews arrangement and main-display changes for the selected screen', async () => {
    const data = inventory();
    data.displays.push({
      ...firstDisplay(data),
      id: 'screen-b',
      connectionId: 2,
      name: 'External screen',
      main: false,
      x: 1920,
    });
    mockInvoke.mockResolvedValue(data);
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Arrangement' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Select 2. External screen' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Position' }), {
      target: { value: 'above' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Try arrangement' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('preview_display_configuration', {
        request: {
          kind: 'position',
          displayId: 'screen-b',
          relativeTo: 'screen-a',
          placement: 'above',
        },
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Use as main display' })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Use as main display' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('preview_display_configuration', {
        request: { kind: 'main', displayId: 'screen-b' },
      }),
    );
  });

  it('focuses confirmation and allows Escape to revert', async () => {
    mockInvoke.mockResolvedValue({
      ...inventory(),
      pending: { token: 3, displayId: 'screen-a', remainingMs: 10000 },
    });
    render(<DisplayView />);
    const title = await screen.findByRole('heading', { name: 'Keep this display setting?' });
    await waitFor(() => expect(title).toHaveFocus());
    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('revert_display_configuration', { token: 3 }),
    );
  });

  it('does not erase a failed change when a read-only refresh succeeds', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'preview_display_configuration') throw { code: 'modeUnavailable' };
      return inventory();
    });
    render(<DisplayView />);
    fireEvent.change(await screen.findByRole('combobox', { name: 'Resolution' }), {
      target: { value: '2560x1440:5120x2880' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Try resolution' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'This resolution is no longer available',
    );
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh' })).toBeEnabled());
    const reads = mockInvoke.mock.calls.filter(([command]) => command === 'get_displays').length;
    fireEvent.focus(window);
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([command]) => command === 'get_displays')).toHaveLength(
        reads + 1,
      ),
    );
    expect(screen.getByRole('alert')).toHaveTextContent('This resolution is no longer available');
  });
  it('coalesces refreshes while a slow inventory read is in flight', async () => {
    let resolveFirst: ((value: DisplaySnapshot) => void) | undefined;
    mockInvoke.mockImplementationOnce(
      () =>
        new Promise<DisplaySnapshot>((resolve) => {
          resolveFirst = resolve;
        }),
    );
    render(<DisplayView />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledTimes(1));
    fireEvent.focus(window);
    fireEvent.focus(window);
    fireEvent.focus(window);
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    await act(async () => {
      resolveFirst?.(inventory());
    });
    await screen.findByRole('combobox', { name: 'Resolution' });
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledTimes(2));
  });

  it('refreshes once after connection notifications arrive during a display mutation', async () => {
    const initial = inventory();
    let latest = initial;
    let finish: ((snapshot: DisplaySnapshot) => void) | undefined;
    mockInvoke.mockImplementation((command) => {
      if (command === 'preview_display_configuration')
        return new Promise<DisplaySnapshot>((resolve) => {
          finish = resolve;
        });
      return Promise.resolve(latest);
    });
    render(<DisplayView />);
    fireEvent.change(await screen.findByRole('combobox', { name: 'Resolution' }), {
      target: { value: '2560x1440:5120x2880' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Try resolution' }));
    const onChanged = mockListen.mock.calls.find(
      ([event]) => event === 'tomari:displays-changed',
    )?.[1];
    if (!onChanged || !finish) throw new Error('missing display subscription or mutation');
    latest = {
      ...initial,
      displays: [
        ...initial.displays,
        { ...firstDisplay(initial), id: 'new', connectionId: 2, name: 'New monitor', main: false },
      ],
    };
    act(() => {
      onChanged({ event: 'tomari:displays-changed', id: 1, payload: null });
      onChanged({ event: 'tomari:displays-changed', id: 2, payload: null });
      fireEvent.focus(window);
    });
    expect(mockInvoke.mock.calls.filter(([command]) => command === 'get_displays')).toHaveLength(1);
    await act(async () => {
      finish?.({ ...initial, pending: { token: 1, displayId: 'screen-a', remainingMs: 20000 } });
    });
    expect(await screen.findByText('2. New monitor')).toBeInTheDocument();
    expect(mockInvoke.mock.calls.filter(([command]) => command === 'get_displays')).toHaveLength(2);
    expect(screen.queryByRole('button', { name: 'Keep changes' })).not.toBeInTheDocument();
  });

  it('offers software dimming when native brightness is unavailable', async () => {
    const data = inventory();
    firstDisplay(data).brightness = null;
    mockInvoke.mockResolvedValue(data);
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Brightness' }));
    fireEvent.change(screen.getByRole('slider', { name: 'Extra dimming' }), {
      target: { value: '35' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Apply dimming' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('set_display_dimming', {
        displayId: 'screen-a',
        amount: 0.35,
      }),
    );
    expect(screen.queryByRole('slider', { name: 'Brightness' })).not.toBeInTheDocument();
  });

  it('keeps the all-display dimming reset available during a configuration preview', async () => {
    const data = inventory();
    firstDisplay(data).softwareDimming = 0.4;
    data.pending = { token: 1, displayId: 'screen-a', remainingMs: 10000 };
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'reset_display_dimming') firstDisplay(data).softwareDimming = 0;
      return data;
    });
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('button', { name: 'Reset all dimming' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('reset_display_dimming'));
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'Reset all dimming' })).not.toBeInTheDocument(),
    );
  });
  it('recovers a live preview after the write response loses its status snapshot', async () => {
    const data = inventory();
    let changed = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'preview_display_configuration') {
        changed = true;
        throw { code: 'refreshRequired' };
      }
      return changed
        ? { ...data, pending: { token: 7, displayId: 'screen-a', remainingMs: 16000 } }
        : data;
    });
    render(<DisplayView />);
    fireEvent.change(await screen.findByRole('combobox', { name: 'Resolution' }), {
      target: { value: '2560x1440:5120x2880' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Try resolution' }));
    expect(await screen.findByRole('button', { name: 'Revert now' })).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('The change was sent');
  });
});

function savedProfile() {
  return {
    id: 'work-profile',
    name: 'Work',
    displays: [
      {
        displayId: 'screen-a',
        x: 0,
        y: 0,
        mode: {
          width: MODE.width,
          height: MODE.height,
          pixelWidth: MODE.pixelWidth,
          pixelHeight: MODE.pixelHeight,
          refreshRate: MODE.refreshRate,
        },
      },
    ],
  };
}

describe('saved display configurations', () => {
  it('renames a disconnected saved profile without recapturing the live configuration', async () => {
    let profile = savedProfile();
    const saved = profile.displays[0];
    if (!saved) throw new Error('missing display fixture');
    saved.displayId = 'disconnected';
    mockInvoke.mockImplementation(async (command, payload) => {
      if (command === 'get_display_profiles') return [profile];
      if (command === 'rename_display_profile') {
        profile = { ...profile, name: (payload as { name: string }).name.trim() };
        return;
      }
      return inventory();
    });
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Profiles' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Rename profile' }));
    const input = screen.getByRole('textbox', { name: 'New profile name' });
    expect(input).toHaveFocus();
    expect(input).toHaveValue('Work');
    expect(screen.getByRole('button', { name: 'Save name' })).toBeDisabled();
    fireEvent.change(input, { target: { value: 'Desk' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save name' }));
    await screen.findByRole('option', { name: 'Desk' });
    expect(mockInvoke).toHaveBeenCalledWith('rename_display_profile', {
      id: 'work-profile',
      name: 'Desk',
    });
    expect(screen.getByRole('combobox', { name: 'Saved configurations' })).toHaveValue(
      'work-profile',
    );
    expect(screen.queryByRole('form', { name: 'Rename profile' })).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith('save_display_profile', expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith('preview_display_profile', expect.anything());
  });

  it('keeps a failed rename draft available for retry or cancellation', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_display_profiles') return [savedProfile()];
      if (command === 'rename_display_profile') throw { code: 'profileStorage' };
      return inventory();
    });
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Profiles' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Rename profile' }));
    const input = screen.getByRole('textbox', { name: 'New profile name' });
    fireEvent.change(input, { target: { value: 'Desk' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save name' }));
    await screen.findByRole('alert');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save name' })).toBeEnabled());
    expect(input).toHaveValue('Desk');
    expect(screen.getByRole('option', { name: 'Work' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('form', { name: 'Rename profile' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Rename profile' }));
    expect(screen.getByRole('textbox', { name: 'New profile name' })).toHaveValue('Work');
  });

  it('shows saved modes and blocks recall when the connected screen no longer offers them', async () => {
    const profile = savedProfile();
    const saved = profile.displays[0];
    if (!saved) throw new Error('missing display fixture');
    saved.mode.refreshRate = 59.94;
    mockInvoke.mockImplementation(async (command) =>
      command === 'get_display_profiles' ? [profile] : inventory(),
    );
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Profiles' }));
    const summary = await screen.findByRole('list', { name: 'Saved display settings' });
    expect(summary).toHaveTextContent('Studio Display · Main display');
    expect(summary).toHaveTextContent('1920 × 1080 · HiDPI · 59.94 Hz');
    expect(screen.getByRole('button', { name: 'Preview configuration' })).toBeDisabled();
    expect(mockInvoke).not.toHaveBeenCalledWith('preview_display_profile', expect.anything());
  });

  it('saves the current native configuration by name rather than trusting client geometry', async () => {
    const previous = { ...savedProfile(), id: 'older-profile', name: 'Older setup' };
    let profiles: ReturnType<typeof savedProfile>[] = [previous];
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_display_profiles') return profiles;
      if (command === 'save_display_profile') {
        profiles = [previous, savedProfile()];
        return 'work-profile';
      }
      return inventory();
    });
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Profiles' }));
    const name = await screen.findByRole('textbox', { name: 'Profile name' });
    await waitFor(() => expect(name).toBeEnabled());
    fireEvent.change(name, { target: { value: 'Work' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save current configuration' }));
    await screen.findByRole('option', { name: 'Work' });
    expect(mockInvoke).toHaveBeenCalledWith('save_display_profile', { name: 'Work' });
    expect(name).toHaveValue('');
    expect(screen.getByRole('combobox', { name: 'Saved configurations' })).toHaveValue(
      'work-profile',
    );
  });

  it('uses the reversible preview for a saved profile and does not confirm automatically', async () => {
    const data = inventory();
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_display_profiles') return [savedProfile()];
      if (command === 'preview_display_profile')
        return { ...data, pending: { token: 9, displayId: 'screen-a', remainingMs: 20000 } };
      return data;
    });
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Profiles' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Preview configuration' }));
    await screen.findByRole('button', { name: 'Keep changes' });
    expect(mockInvoke).toHaveBeenCalledWith('preview_display_profile', { id: 'work-profile' });
    expect(mockInvoke).not.toHaveBeenCalledWith('confirm_display_configuration', expect.anything());
    expect(screen.getByRole('button', { name: 'Preview configuration' })).toBeDisabled();
    expect(screen.getByRole('textbox', { name: 'Profile name' })).toBeDisabled();
  });

  it('does not apply a profile saved for a different display', async () => {
    const profile = savedProfile();
    const saved = profile.displays[0];
    if (!saved) throw new Error('fixture has no saved display');
    saved.displayId = 'disconnected';
    mockInvoke.mockImplementation(async (command) =>
      command === 'get_display_profiles' ? [profile] : inventory(),
    );
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Profiles' }));
    expect(await screen.findByRole('button', { name: 'Preview configuration' })).toBeDisabled();
    expect(
      screen.getByText('Connect exactly the displays used by this saved configuration.'),
    ).toBeInTheDocument();
  });

  it('deletes a saved profile only after the explicit removal step', async () => {
    let profiles = [savedProfile()];
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_display_profiles') return profiles;
      if (command === 'delete_display_profile') {
        profiles = [];
        return;
      }
      return inventory();
    });
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Profiles' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Delete profile' }));
    expect(mockInvoke).not.toHaveBeenCalledWith('delete_display_profile', expect.anything());
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await screen.findByText('No saved configurations yet.');
    expect(mockInvoke).toHaveBeenCalledWith('delete_display_profile', { id: 'work-profile' });
  });

  it('offers a confirmed reset for damaged profile data without changing the live display', async () => {
    let damaged = true;
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_display_profiles') {
        if (damaged) throw new Error('damaged');
        return [];
      }
      if (command === 'clear_display_profiles') {
        damaged = false;
        return;
      }
      return inventory();
    });
    render(<DisplayView />);
    fireEvent.click(await screen.findByRole('tab', { name: 'Profiles' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Reset saved profiles' }));
    expect(mockInvoke).not.toHaveBeenCalledWith('clear_display_profiles');
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await screen.findByText('No saved configurations yet.');
    expect(mockInvoke).toHaveBeenCalledWith('clear_display_profiles');
    expect(mockInvoke).not.toHaveBeenCalledWith('preview_display_configuration', expect.anything());
  });
});

describe('display identification', () => {
  it('numbers the cards and sends the same display order to native identification', async () => {
    const data = inventory();
    data.displays.push({
      ...firstDisplay(data),
      id: 'screen-b',
      connectionId: 2,
      main: false,
      x: 1920,
    });
    mockInvoke.mockImplementation(async (command) =>
      command === 'identify_displays' ? undefined : data,
    );
    render(<DisplayView />);
    expect(await screen.findByText('1. Studio Display')).toBeInTheDocument();
    expect(screen.getByText('2. Studio Display')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Identify' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('identify_displays', {
        displayIds: ['screen-a', 'screen-b'],
      }),
    );
    expect(mockInvoke).not.toHaveBeenCalledWith('preview_display_configuration', expect.anything());
  });

  it('does not independently identify screens sharing a mirrored desktop', async () => {
    const data = inventory();
    firstDisplay(data).mirrored = true;
    mockInvoke.mockResolvedValue(data);
    render(<DisplayView />);
    await screen.findByText('1. Studio Display');
    expect(screen.getByRole('button', { name: 'Identify' })).toBeDisabled();
  });
});

it('keeps ambiguous displays read-only while allowing brightness on a unique screen', async () => {
  const data = inventory();
  firstDisplay(data).softwareDimming = 0.2;
  data.displays.push({ ...firstDisplay(data), connectionId: 2, main: false, x: 1920 });
  data.displays.push({
    ...firstDisplay(data),
    id: 'unique',
    connectionId: 3,
    name: 'Unique display',
    main: false,
    x: 3840,
  });
  mockInvoke.mockResolvedValue(data);
  render(<DisplayView />);
  await screen.findByText(/macOS reports the same identity/);
  expect(screen.getByRole('combobox', { name: 'Resolution' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Identify' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Reset all dimming' })).toBeEnabled();
  expect(screen.getByRole('button', { name: /^1\. Studio Display/ })).toBeDisabled();
  expect(screen.getByRole('button', { name: /^2\. Studio Display/ })).toBeDisabled();
  fireEvent.click(screen.getByRole('tab', { name: 'Brightness' }));
  expect(screen.getByRole('slider', { name: 'Brightness' })).toBeEnabled();
  fireEvent.change(screen.getByRole('slider', { name: 'Brightness' }), { target: { value: '40' } });
  fireEvent.click(screen.getByRole('button', { name: 'Apply brightness' }));
  await waitFor(() =>
    expect(mockInvoke).toHaveBeenCalledWith('set_display_brightness', {
      displayId: 'unique',
      value: 0.4,
    }),
  );
});

it('does not expose brightness writes when every connected identity is ambiguous', async () => {
  const data = inventory();
  data.displays.push({ ...firstDisplay(data), connectionId: 2, main: false, x: 1920 });
  mockInvoke.mockResolvedValue(data);
  render(<DisplayView />);
  fireEvent.click(await screen.findByRole('tab', { name: 'Brightness' }));
  expect(screen.getByRole('slider', { name: 'Brightness' })).toBeDisabled();
  expect(screen.getByRole('slider', { name: 'Extra dimming' })).toBeDisabled();
});
