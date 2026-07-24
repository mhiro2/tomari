import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Hotkey } from '../lib/types';
import { SetupView } from './SetupView';

// Mock the Tauri command bridge so the real `api` wrappers run against it.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

const noop = () => {};

// The seeded left-half snap binding the "try it" hint looks up.
const SNAP_LEFT: Hotkey = {
  id: 'hk-left',
  label: 'Snap Left',
  accelerator: 'Ctrl+Alt+Left',
  action: { type: 'snapWindow', value: 'leftHalf' },
  enabled: true,
};

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const value = overrides[cmd];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    switch (cmd) {
      case 'list_hotkeys':
        return Promise.resolve([SNAP_LEFT]);
      case 'request_accessibility':
      case 'request_input_monitoring':
        return Promise.resolve(true);
      default:
        return Promise.resolve(null);
    }
  });
}

function renderView(overrides: Partial<Parameters<typeof SetupView>[0]> = {}) {
  return render(
    <SetupView
      permissions={{ accessibility: false, inputMonitoring: false }}
      updateRegrant={false}
      onGranted={noop}
      onDismiss={noop}
      onDone={noop}
      {...overrides}
    />,
  );
}

describe('SetupView', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
  });

  it('lists both permissions with a grant button while they are missing', () => {
    renderView();

    expect(screen.getByText('Accessibility')).toBeInTheDocument();
    expect(screen.getByText('Input Monitoring')).toBeInTheDocument();
    expect(screen.getAllByText('Grant Access')).toHaveLength(2);
    // The admin password is per-use, not a grantable permission — a note only.
    expect(screen.getByText(/administrator password/)).toBeInTheDocument();
  });

  it('shows a granted permission as an announced chip, not a button', () => {
    renderView({ permissions: { accessibility: true, inputMonitoring: false } });

    // Every row keeps an (initially empty) live region mounted so the later
    // flip to "Granted" lands *inside* an existing region and gets announced.
    const regions = screen.getAllByRole('status');
    expect(regions).toHaveLength(2);
    expect(screen.getByText('Granted')).toBeInTheDocument();
    expect(screen.getAllByText('Grant Access')).toHaveLength(1);
  });

  it('names each grant button after its permission for assistive tech', () => {
    renderView();

    expect(screen.getByLabelText('Grant Access for Accessibility')).toBeInTheDocument();
    expect(screen.getByLabelText('Grant Access for Input Monitoring')).toBeInTheDocument();
  });

  it('moves focus to the heading on mount', () => {
    renderView();

    expect(screen.getByText('Set up Tomari')).toHaveFocus();
  });

  it('requests the permission and reports an immediate grant upward', async () => {
    const onGranted = vi.fn();
    renderView({
      permissions: { accessibility: true, inputMonitoring: false },
      onGranted,
    });

    fireEvent.click(screen.getByText('Grant Access'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('request_input_monitoring');
      expect(onGranted).toHaveBeenCalledWith({ inputMonitoring: true });
    });
  });

  it('does not report a grant when the request comes back denied', async () => {
    mockCommands({ request_accessibility: false });
    const onGranted = vi.fn();
    renderView({
      permissions: { accessibility: false, inputMonitoring: true },
      onGranted,
    });

    fireEvent.click(screen.getByText('Grant Access'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('request_accessibility');
    });
    expect(onGranted).not.toHaveBeenCalled();
  });

  it('offers "Set up later" while something is missing', () => {
    const onDismiss = vi.fn();
    renderView({ onDismiss });

    fireEvent.click(screen.getByText('Set up later'));
    expect(onDismiss).toHaveBeenCalled();
    expect(screen.queryByText('Done')).not.toBeInTheDocument();
  });

  it('swaps to the all-set hint and a Done button once everything is granted', () => {
    const onDone = vi.fn();
    renderView({
      permissions: { accessibility: true, inputMonitoring: true },
      onDone,
    });

    expect(screen.getByText(/All set/)).toBeInTheDocument();
    expect(screen.queryByText('Set up later')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText('Done'));
    expect(onDone).toHaveBeenCalled();
  });

  it('shows the try-it hint with the currently bound accelerator once all is granted', async () => {
    renderView({ permissions: { accessibility: true, inputMonitoring: true } });

    expect(await screen.findByText(/Try it: press/)).toBeInTheDocument();
    // Ctrl+Alt+Left rendered as native keycap glyphs.
    for (const glyph of ['⌃', '⌥', '←']) {
      expect(screen.getByText(glyph)).toBeInTheDocument();
    }
  });

  it('shows the rebound accelerator, not the seeded default, in the try-it hint', async () => {
    mockCommands({
      list_hotkeys: [{ ...SNAP_LEFT, accelerator: 'Cmd+Shift+L' }],
    });
    renderView({ permissions: { accessibility: true, inputMonitoring: true } });

    expect(await screen.findByText(/Try it: press/)).toBeInTheDocument();
    for (const glyph of ['⌘', '⇧', 'L']) {
      expect(screen.getByText(glyph)).toBeInTheDocument();
    }
  });

  it('drops the try-it hint when no enabled left-half binding exists', async () => {
    mockCommands({ list_hotkeys: [{ ...SNAP_LEFT, enabled: false }] });
    renderView({ permissions: { accessibility: true, inputMonitoring: true } });

    // The all-set line arriving means the async hotkey lookup has settled too.
    expect(await screen.findByText(/All set/)).toBeInTheDocument();
    expect(screen.queryByText(/Try it/)).not.toBeInTheDocument();
  });

  it('drops the try-it hint when the hotkey list cannot be read', async () => {
    mockCommands({
      list_hotkeys: Object.assign(new Error('list failed'), { code: 'unknown' }),
    });
    renderView({ permissions: { accessibility: true, inputMonitoring: true } });

    expect(await screen.findByText(/All set/)).toBeInTheDocument();
    expect(screen.queryByText(/Try it/)).not.toBeInTheDocument();
  });

  it('explains the update-caused regrant when flagged', () => {
    renderView({ updateRegrant: true });

    expect(screen.getByText(/went missing after the update/)).toBeInTheDocument();
  });

  it('surfaces a failed permission request as an error', async () => {
    mockCommands({
      request_accessibility: Object.assign(new Error('request failed'), { code: 'unknown' }),
    });
    renderView({ permissions: { accessibility: false, inputMonitoring: true } });

    fireEvent.click(screen.getByText('Grant Access'));

    expect(await screen.findByRole('alert')).toHaveTextContent('request failed');
  });
});
