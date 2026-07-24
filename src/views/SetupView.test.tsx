import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SetupView } from './SetupView';

// Mock the Tauri command bridge so the real `api` wrappers run against it.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

const noop = () => {};

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
    mockInvoke.mockResolvedValue(true);
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

    // role="status" so the flip to "Granted" is announced when it happens.
    expect(screen.getByRole('status')).toHaveTextContent('Granted');
    expect(screen.getAllByText('Grant Access')).toHaveLength(1);
  });

  it('names each grant button after its permission for assistive tech', () => {
    renderView();

    expect(screen.getByLabelText('Grant Accessibility access')).toBeInTheDocument();
    expect(screen.getByLabelText('Grant Input Monitoring access')).toBeInTheDocument();
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
    mockInvoke.mockResolvedValue(false);
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

  it('explains the update-caused regrant when flagged', () => {
    renderView({ updateRegrant: true });

    expect(screen.getByText(/The update reset these permissions/)).toBeInTheDocument();
  });

  it('surfaces a failed permission request as an error', async () => {
    mockInvoke.mockRejectedValue(Object.assign(new Error('request failed'), { code: 'unknown' }));
    renderView({ permissions: { accessibility: false, inputMonitoring: true } });

    fireEvent.click(screen.getByText('Grant Access'));

    expect(await screen.findByRole('alert')).toHaveTextContent('request failed');
  });
});
