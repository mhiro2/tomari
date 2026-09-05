import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { StrictMode, type ComponentProps } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { I18nProvider } from '../lib/i18n';
import type { DiagnosticsSnapshot } from '../lib/types';
import { HealthView } from './HealthView';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

const HEALTHY: DiagnosticsSnapshot = {
  generatedAtMs: 1,
  app: { version: '1.2.3', os: 'macos', architecture: 'aarch64' },
  permissions: { accessibility: true, inputMonitoring: true },
  taps: [
    {
      kind: 'keyboard',
      enabled: true,
      state: 'healthy',
      restartCount: 2,
      disableCount: 1,
      recoveryCount: 1,
    },
    {
      kind: 'dragToSnap',
      enabled: false,
      state: 'stopped',
      restartCount: 0,
      disableCount: 0,
      recoveryCount: 0,
    },
    {
      kind: 'dragToMove',
      enabled: false,
      state: 'stopped',
      restartCount: 0,
      disableCount: 0,
      recoveryCount: 0,
    },
  ],
  capsLock: { ownership: 'held', mappingActive: true, reconciled: true },
  shortcuts: {
    enabled: true,
    registrationIncomplete: false,
    registeredCount: 4,
    invalidCount: 0,
  },
  menuBar: {
    enabled: true,
    supported: true,
    permissionGranted: true,
    dividerAvailable: true,
  },
  keepAwake: {
    active: false,
    phase: 'off',
    markerPresent: false,
    kernelSleepDisabled: false,
    ownsLidClose: false,
  },
  database: { integrityOk: true, schemaVersion: 3, latestSchemaVersion: 3 },
  updater: { signatureConfigured: true },
  privacy: {
    rawInputIncluded: false,
    accessibilityLabelsIncluded: false,
    processDetailsIncluded: false,
    filesystemPathsIncluded: false,
  },
};

function renderView(props: ComponentProps<typeof HealthView> = {}) {
  return render(
    <I18nProvider lang="en">
      <HealthView {...props} />
    </I18nProvider>,
  );
}

// The row that carries `title`; its lead dot names the row's state.
function row(title: string): HTMLElement {
  const element = screen.getByText(title).closest('.settings-row');
  if (!(element instanceof HTMLElement)) throw new Error(`row "${title}" is missing`);
  return element;
}

describe('HealthView', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((command) => {
      if (command === 'get_diagnostics') return Promise.resolve(HEALTHY);
      if (command === 'export_support_bundle') {
        return Promise.resolve({
          path: '/private/support/tomari-support-1.json',
          generatedAtMs: 1,
        });
      }
      return Promise.resolve(null);
    });
  });

  it('renders every check as a row with one state dot and plain-language detail', async () => {
    renderView();

    expect(await screen.findByText('No health issues detected')).toBeInTheDocument();
    expect(screen.getByText('Tomari 1.2.3 · aarch64 · checked just now')).toBeInTheDocument();
    // Summary dot plus one per row: permissions, three taps, Caps Lock,
    // shortcuts, Menu Bar, Prevent Sleep, database, updater.
    expect(document.querySelectorAll('.health-dot')).toHaveLength(11);

    const keyboard = row('Keyboard monitoring');
    expect(within(keyboard).getByText('Ready')).toBeInTheDocument();
    expect(within(keyboard).getByText('Running.')).toBeInTheDocument();
    expect(
      within(keyboard).getByText('Restarted 2 times · paused by macOS 1 times · recovered 1 times'),
    ).toBeInTheDocument();

    // Counters that are all zero add nothing to a row.
    const dragToSnap = row('Drag-to-snap monitoring');
    expect(within(dragToSnap).getByText('Off')).toBeInTheDocument();
    expect(within(dragToSnap).getByText('Turned off in its settings.')).toBeInTheDocument();
    expect(within(dragToSnap).queryByText(/Restarted/)).not.toBeInTheDocument();

    expect(screen.getByText('Your saved data is up to date and intact.')).toBeInTheDocument();
    expect(screen.getByText(/Updates are verified as genuine/)).toBeInTheDocument();
    expect(within(row('Prevent Sleep')).getByText('Off')).toBeInTheDocument();
    // Nothing is actionable on a healthy report except the report itself.
    expect(screen.getAllByRole('button').map((b) => b.textContent)).toEqual([
      'Refresh',
      'Export Support Bundle',
    ]);
  });

  it('is fully localized in Japanese', async () => {
    render(
      <I18nProvider lang="ja">
        <HealthView />
      </I18nProvider>,
    );

    expect(await screen.findByText('問題は検出されませんでした')).toBeInTheDocument();
    expect(screen.getByText('キーボードの監視')).toBeInTheDocument();
    expect(screen.getAllByText('オフ').length).toBeGreaterThan(0);
  });

  it('marks a Prevent Sleep transition as in progress rather than actionable', async () => {
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({
            ...HEALTHY,
            keepAwake: {
              active: true,
              phase: 'disabling',
              markerPresent: true,
              kernelSleepDisabled: true,
              ownsLidClose: true,
            },
          })
        : Promise.resolve(null),
    );
    renderView();

    expect(await screen.findByText('1 health check is in progress')).toBeInTheDocument();
    const sleep = row('Prevent Sleep');
    expect(within(sleep).getByText('In progress')).toBeInTheDocument();
    expect(within(sleep).queryByRole('button')).not.toBeInTheDocument();
  });

  it('accepts a foreign kernel sleep override without claiming ownership', async () => {
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({
            ...HEALTHY,
            keepAwake: {
              active: true,
              phase: 'on',
              markerPresent: false,
              kernelSleepDisabled: true,
              ownsLidClose: false,
            },
          })
        : Promise.resolve(null),
    );
    renderView();

    expect(await screen.findByText('No health issues detected')).toBeInTheDocument();
    expect(within(row('Prevent Sleep')).getByText('Ready')).toBeInTheDocument();
  });

  it('ignores a foreign kernel sleep override while Prevent Sleep is off', async () => {
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({
            ...HEALTHY,
            keepAwake: { ...HEALTHY.keepAwake, kernelSleepDisabled: true },
          })
        : Promise.resolve(null),
    );
    renderView();

    expect(await screen.findByText('No health issues detected')).toBeInTheDocument();
    expect(screen.queryByText(/process remains/)).not.toBeInTheDocument();
  });

  it('explains an outdated database schema instead of describing it as healthy', async () => {
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({
            ...HEALTHY,
            database: { integrityOk: true, schemaVersion: 2, latestSchemaVersion: 3 },
          })
        : Promise.resolve(null),
    );
    renderView();

    expect(await screen.findByText('1 item needs action')).toBeInTheDocument();
    expect(
      screen.getByText(
        'Your saved data is in an older format than this version expects. Export a support bundle.',
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/integrity check passed/)).not.toBeInTheDocument();
    // The export lives in its own row; the database row does not repeat it.
    expect(screen.getAllByRole('button', { name: 'Export Support Bundle' })).toHaveLength(1);
  });

  it('distinguishes a failed database health read from an integrity failure', async () => {
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({ ...HEALTHY, database: null })
        : Promise.resolve(null),
    );
    renderView();

    expect(
      await screen.findByText(
        'Your saved data could not be checked. Export a support bundle for troubleshooting.',
      ),
    ).toBeInTheDocument();
  });

  it('does not let an older Strict Mode read strand the view as busy', async () => {
    let resolveOlder: ((snapshot: DiagnosticsSnapshot) => void) | undefined;
    let resolveNewest: ((snapshot: DiagnosticsSnapshot) => void) | undefined;
    mockInvoke
      .mockImplementationOnce(
        () =>
          new Promise<DiagnosticsSnapshot>((resolve) => {
            resolveOlder = resolve;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise<DiagnosticsSnapshot>((resolve) => {
            resolveNewest = resolve;
          }),
      );

    render(
      <StrictMode>
        <I18nProvider lang="en">
          <HealthView />
        </I18nProvider>
      </StrictMode>,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledTimes(2));
    if (resolveNewest === undefined || resolveOlder === undefined) {
      throw new Error('Strict Mode did not start both diagnostics reads');
    }
    resolveNewest(HEALTHY);
    expect(await screen.findByText('No health issues detected')).toBeInTheDocument();
    resolveOlder(HEALTHY);

    await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh' })).not.toBeDisabled());
  });

  it('counts each actionable row once and never counts a disabled feature', async () => {
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({
            ...HEALTHY,
            permissions: { accessibility: false, inputMonitoring: true },
            shortcuts: { ...HEALTHY.shortcuts, registrationIncomplete: true },
            updater: { signatureConfigured: false },
          })
        : Promise.resolve(null),
    );
    renderView();

    expect(await screen.findByText('3 items need action')).toBeInTheDocument();
    expect(screen.getAllByText('Action needed')).toHaveLength(4);
    expect(screen.getAllByText('Off').length).toBeGreaterThan(0);
    expect(screen.getByText(/cannot verify that updates are genuine/)).toBeInTheDocument();
  });

  it('opens Setup for a missing macOS permission', async () => {
    const onOpenPermissions = vi.fn();
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({
            ...HEALTHY,
            permissions: { accessibility: false, inputMonitoring: true },
          })
        : Promise.resolve(null),
    );
    renderView({ onOpenPermissions });

    fireEvent.click(await screen.findByRole('button', { name: 'Open Setup' }));

    expect(onOpenPermissions).toHaveBeenCalledOnce();
    expect(
      screen.getByText('Accessibility is not granted. Open Setup to grant it in System Settings.'),
    ).toBeInTheDocument();
  });

  it('sends a permission-denied tap to Setup and other tap failures to their settings', async () => {
    const onNavigate = vi.fn();
    const onOpenPermissions = vi.fn();
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({
            ...HEALTHY,
            taps: [
              { ...HEALTHY.taps[0], state: 'permissionDenied' },
              { ...HEALTHY.taps[1], enabled: true, state: 'failed' },
              HEALTHY.taps[2],
            ],
          })
        : Promise.resolve(null),
    );
    renderView({ onNavigate, onOpenPermissions });

    fireEvent.click(await screen.findByRole('button', { name: 'Open Setup' }));
    expect(onOpenPermissions).toHaveBeenCalledOnce();

    fireEvent.click(screen.getByRole('button', { name: 'Open Mouse Settings' }));
    expect(onNavigate).toHaveBeenCalledWith({ section: 'window', tab: 'mouse' });
  });

  it('targets the exact settings tab for an actionable shortcut failure', async () => {
    const onNavigate = vi.fn();
    mockInvoke.mockImplementation((command) =>
      command === 'get_diagnostics'
        ? Promise.resolve({
            ...HEALTHY,
            shortcuts: { ...HEALTHY.shortcuts, registrationIncomplete: true },
          })
        : Promise.resolve(null),
    );
    renderView({ onNavigate });

    fireEvent.click(await screen.findByRole('button', { name: 'Open Shortcuts' }));

    expect(onNavigate).toHaveBeenCalledWith({ section: 'keyboard', tab: 'shortcuts' });
  });

  it('exports a sanitized bundle and announces its saved path', async () => {
    renderView();
    fireEvent.click(await screen.findByRole('button', { name: 'Export Support Bundle' }));

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('export_support_bundle'));
    expect(await screen.findByText('Support bundle saved')).toBeInTheDocument();
    expect(screen.getByText('/private/support/tomari-support-1.json')).toBeInTheDocument();
    expect(screen.getByText(/does not include what you typed, your shortcuts/)).toBeInTheDocument();
  });

  it('keeps the previous snapshot visible when a refresh fails', async () => {
    let reads = 0;
    mockInvoke.mockImplementation((command) => {
      if (command !== 'get_diagnostics') return Promise.resolve(null);
      reads += 1;
      return reads === 1 ? Promise.resolve(HEALTHY) : Promise.reject(new Error('probe busy'));
    });
    renderView();
    await screen.findByText('No health issues detected');

    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));

    expect(
      await screen.findByText(/showing the previous snapshot: probe busy/),
    ).toBeInTheDocument();
    expect(screen.getByText('No health issues detected')).toBeInTheDocument();
  });
});
