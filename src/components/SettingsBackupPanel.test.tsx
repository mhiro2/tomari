import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsProvider } from '../lib/settings';
import type { AppSettings, ConfigurationBackupPreview } from '../lib/types';
import { SettingsBackupPanel } from './SettingsBackupPanel';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);
const SETTINGS: AppSettings = {
  launchAtLogin: false,
  language: 'en',
  keyboardEnabled: true,
  windowManagementEnabled: true,
  externalWindowActionsEnabled: true,
  commandImeSwitchEnabled: true,
  showInMenuBar: true,
  dragToSnapEnabled: false,
  dragToMoveEnabled: false,
  menuBarTidyEnabled: false,
  menuBarAutoCollapseSecs: 0,
};
const PREVIEW: ConfigurationBackupPreview = {
  token: 17,
  kind: 'import',
  current: { hotkeys: 3, modifierRules: 1, windowPlacements: 0, displayProfiles: 0 },
  incoming: { hotkeys: 2, modifierRules: 4, windowPlacements: 5, displayProfiles: 2 },
  currentSettings: SETTINGS,
  settings: { ...SETTINGS, launchAtLogin: true, showInMenuBar: false },
};

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((command) => {
    if (command in overrides) {
      const value = overrides[command];
      return Promise.resolve(typeof value === 'function' ? value() : value);
    }
    if (command === 'get_settings') return Promise.resolve(SETTINGS);
    if (command === 'get_configuration_backup_status')
      return Promise.resolve({ canRestore: false, lastApplied: null });
    if (command === 'preview_configuration_import') return Promise.resolve(PREVIEW);
    if (command === 'apply_configuration_backup')
      return Promise.resolve({ settings: PREVIEW.settings, applyWarnings: [] });
    return Promise.resolve(null);
  });
}

async function renderPanel() {
  const result = render(
    <SettingsProvider>
      <SettingsBackupPanel />
    </SettingsProvider>,
  );
  await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('get_settings'));
  await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('get_configuration_backup_status'));
  return result;
}

function selectFile(text = '{"formatVersion":1}') {
  const file = new File([text], 'settings.json', { type: 'application/json' });
  Object.defineProperty(file, 'text', { value: vi.fn().mockResolvedValue(text) });
  fireEvent.change(screen.getByLabelText('Choose Backup…'), { target: { files: [file] } });
  return file;
}

function calls(command: string) {
  return mockInvoke.mock.calls.filter(([name]) => name === command);
}

describe('SettingsBackupPanel', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
  });

  it('exports a saved file and opens its backup folder', async () => {
    mockCommands({
      export_configuration_backup: { path: '/app/backups/settings.json', generatedAtMs: 1 },
    });
    await renderPanel();
    fireEvent.click(screen.getByRole('button', { name: 'Export Settings' }));
    expect(await screen.findByText('/app/backups/settings.json')).toBeInTheDocument();
    expect(calls('export_configuration_backup')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Open Backup Folder' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('reveal_configuration_backups'));
  });

  it('reviews exact counts and setting changes, then applies only the confirmed token', async () => {
    await renderPanel();
    selectFile();
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByRole('row', { name: 'Shortcuts 3 2' })).toBeInTheDocument();
    expect(within(dialog).getByRole('row', { name: 'Modifier rules 1 4' })).toBeInTheDocument();
    expect(within(dialog).getByRole('row', { name: 'Window positions 0 5' })).toBeInTheDocument();
    expect(within(dialog).getByText(/Spotlight/)).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Cancel' })).toHaveFocus();
    expect(calls('apply_configuration_backup')).toHaveLength(0);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Replace All Settings' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(mockInvoke).toHaveBeenCalledWith('apply_configuration_backup', { token: 17 });
    expect(
      await screen.findByText(
        'Settings replaced. Use “Restore Previous Settings” to undo this import.',
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Choose Backup…' })).toHaveFocus();
  });

  it('cancels without replacing settings and can review the same file again', async () => {
    await renderPanel();
    const file = selectFile();
    fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }),
    );
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Choose Backup…' })).toHaveFocus();
    expect(calls('apply_configuration_backup')).toHaveLength(0);
    fireEvent.change(screen.getByLabelText('Choose Backup…'), { target: { files: [file] } });
    await screen.findByRole('dialog');
    expect(calls('preview_configuration_import')).toHaveLength(2);
  });

  it('rejects an oversized file before reading it or sending a preview request', async () => {
    await renderPanel();
    const file = new File([], 'huge.json');
    const read = vi.fn();
    Object.defineProperties(file, { size: { value: 2 * 1024 * 1024 + 1 }, text: { value: read } });
    fireEvent.change(screen.getByLabelText('Choose Backup…'), { target: { files: [file] } });
    expect(await screen.findByRole('alert')).toHaveTextContent('no larger than 2 MiB');
    expect(read).not.toHaveBeenCalled();
    expect(calls('preview_configuration_import')).toHaveLength(0);
  });

  it('localizes an invalid backup and leaves settings untouched', async () => {
    mockCommands({
      preview_configuration_import: () =>
        Promise.reject({ code: 'backupInvalid', message: 'private parsing detail' }),
    });
    await renderPanel();
    selectFile();
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'not a supported, valid Tomari settings backup',
    );
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(calls('apply_configuration_backup')).toHaveLength(0);
  });

  it('requires a fresh preview after the current settings have changed', async () => {
    let previews = 0;
    let applies = 0;
    mockCommands({
      preview_configuration_import: () => ({ ...PREVIEW, token: ++previews === 1 ? 17 : 18 }),
      apply_configuration_backup: () =>
        ++applies === 1
          ? Promise.reject({ code: 'backupPreviewExpired', message: 'stale' })
          : { settings: PREVIEW.settings, applyWarnings: [] },
    });
    await renderPanel();
    selectFile();
    fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', {
        name: 'Replace All Settings',
      }),
    );
    const review = await screen.findByRole('button', { name: 'Review Again' });
    expect(screen.queryByRole('button', { name: 'Replace All Settings' })).not.toBeInTheDocument();
    fireEvent.click(review);
    fireEvent.click(await screen.findByRole('button', { name: 'Replace All Settings' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(calls('apply_configuration_backup').map(([, args]) => args)).toEqual([
      { token: 17 },
      { token: 18 },
    ]);
  });

  it('keeps persisted restore available after a failed apply and after reopening the view', async () => {
    mockCommands({
      get_configuration_backup_status: { canRestore: true, lastApplied: null },
      preview_configuration_restore: { ...PREVIEW, kind: 'restore' },
      apply_configuration_backup: () => Promise.reject(new Error('connection lost')),
    });
    const view = await renderPanel();
    fireEvent.click(await screen.findByRole('button', { name: 'Restore Previous Settings…' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText(/changes made since/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Replace All Settings' }));
    expect(await screen.findByText(/The result could not be confirmed/)).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Restore Previous Settings…' })).toBeEnabled(),
    );
    view.rerender(
      <SettingsProvider>
        <span>Another view</span>
      </SettingsProvider>,
    );
    view.rerender(
      <SettingsProvider>
        <SettingsBackupPanel />
      </SettingsProvider>,
    );
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Restore Previous Settings…' })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Restore Previous Settings…' }));
    await screen.findByRole('dialog');
    expect(calls('preview_configuration_restore')).toHaveLength(2);
  });

  it('does not start a preview when file reading completes after leaving the view', async () => {
    const view = await renderPanel();
    const file = new File([], 'settings.json');
    let resolveRead: ((contents: string) => void) | undefined;
    Object.defineProperty(file, 'text', {
      value: () =>
        new Promise<string>((resolve) => {
          resolveRead = resolve;
        }),
    });
    fireEvent.change(screen.getByLabelText('Choose Backup…'), { target: { files: [file] } });
    view.unmount();
    await act(async () => resolveRead?.('{}'));
    expect(calls('preview_configuration_import')).toHaveLength(0);
  });

  it('locks duplicate apply and cancellation until replacement is complete', async () => {
    let finish: ((result: unknown) => void) | undefined;
    mockCommands({
      apply_configuration_backup: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    });
    await renderPanel();
    selectFile();
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Replace All Settings' }));
    await waitFor(() => expect(calls('apply_configuration_backup')).toHaveLength(1));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Replacing Settings…' }));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    fireEvent(dialog, new Event('cancel', { bubbles: true, cancelable: true }));
    expect(dialog).toBeInTheDocument();
    expect(calls('apply_configuration_backup')).toHaveLength(1);
    await act(async () => finish?.({ settings: PREVIEW.settings, applyWarnings: [] }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
