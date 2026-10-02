import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsProvider, useSettings } from './settings';
import type { AppSettings } from './types';

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
const IMPORTED: AppSettings = { ...SETTINGS, language: 'ja', keyboardEnabled: false };

function Consumer() {
  const {
    settings,
    configurationBusy,
    update,
    applyConfigurationBackup,
    applyWarnings,
    loadError,
  } = useSettings();
  const [failed, setFailed] = useState(false);
  return (
    <>
      <output data-testid="settings">{JSON.stringify(settings)}</output>
      <output data-testid="busy">{String(configurationBusy)}</output>
      <output data-testid="warnings">{applyWarnings.join(',')}</output>
      <output data-testid="load-error">{String(loadError !== null)}</output>
      <output data-testid="failed">{String(failed)}</output>
      <button type="button" onClick={() => update({ launchAtLogin: !settings?.launchAtLogin })}>
        Edit
      </button>
      <button
        type="button"
        onClick={() => void applyConfigurationBackup(17).catch(() => setFailed(true))}
      >
        Import
      </button>
    </>
  );
}

function commandCalls(name: string) {
  return mockInvoke.mock.calls.filter(([command]) => command === name);
}

describe('settings backup write barrier', () => {
  beforeEach(() => mockInvoke.mockReset());

  it('drains every queued save before importing and blocks edits until the new snapshot is adopted', async () => {
    const saves: Array<() => void> = [];
    mockInvoke.mockImplementation((command) => {
      if (command === 'get_settings') return Promise.resolve(SETTINGS);
      if (command === 'save_settings')
        return new Promise((resolve) => saves.push(() => resolve({ applyWarnings: [] })));
      if (command === 'apply_configuration_backup')
        return Promise.resolve({ settings: IMPORTED, applyWarnings: ['capsLockRemap'] });
      return Promise.resolve(null);
    });
    render(
      <SettingsProvider>
        <Consumer />
      </SettingsProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('settings')).toHaveTextContent('"language":"en"'),
    );
    fireEvent.click(screen.getByText('Edit'));
    fireEvent.click(screen.getByText('Edit'));
    expect(commandCalls('save_settings')).toHaveLength(1);
    fireEvent.click(screen.getByText('Import'));
    expect(screen.getByTestId('busy')).toHaveTextContent('true');
    fireEvent.click(screen.getByText('Edit'));
    expect(screen.getByTestId('settings')).toHaveTextContent('"launchAtLogin":false');
    expect(commandCalls('apply_configuration_backup')).toHaveLength(0);
    await act(async () => saves[0]?.());
    expect(commandCalls('save_settings')).toHaveLength(2);
    expect(commandCalls('apply_configuration_backup')).toHaveLength(0);
    await act(async () => saves[1]?.());
    await waitFor(() =>
      expect(screen.getByTestId('settings')).toHaveTextContent('"language":"ja"'),
    );
    expect(screen.getByTestId('warnings')).toHaveTextContent('capsLockRemap');
    expect(screen.getByTestId('busy')).toHaveTextContent('false');
    fireEvent.click(screen.getByText('Edit'));
    expect(commandCalls('save_settings')[2]?.[1]).toEqual({
      settings: { ...IMPORTED, launchAtLogin: true },
    });
    await act(async () => saves[2]?.());
  });

  it('recovers a successful import and its apply warnings from the matching result token', async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === 'get_settings') return Promise.resolve(SETTINGS);
      if (command === 'apply_configuration_backup')
        return Promise.reject(new Error('acknowledgement lost'));
      if (command === 'get_configuration_backup_status')
        return Promise.resolve({
          canRestore: true,
          lastApplied: { token: 17, settings: IMPORTED, applyWarnings: ['globalShortcuts'] },
        });
      return Promise.resolve(null);
    });
    render(
      <SettingsProvider>
        <Consumer />
      </SettingsProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('settings')).toHaveTextContent('"language":"en"'),
    );
    fireEvent.click(screen.getByText('Import'));
    await waitFor(() =>
      expect(screen.getByTestId('settings')).toHaveTextContent('"language":"ja"'),
    );
    expect(screen.getByTestId('failed')).toHaveTextContent('false');
    expect(screen.getByTestId('warnings')).toHaveTextContent('globalShortcuts');
    expect(commandCalls('apply_configuration_backup')).toHaveLength(1);
  });

  it('resynchronizes an unconfirmed result before allowing another ordinary edit', async () => {
    let reads = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === 'get_settings') return Promise.resolve(++reads === 1 ? SETTINGS : IMPORTED);
      if (command === 'apply_configuration_backup')
        return Promise.reject(new Error('IPC disconnected'));
      if (command === 'get_configuration_backup_status')
        return Promise.resolve({
          canRestore: true,
          lastApplied: { token: 99, settings: SETTINGS, applyWarnings: [] },
        });
      if (command === 'get_apply_warnings')
        return Promise.resolve({ warnings: ['capsLockRemap'], unprobed: [] });
      if (command === 'save_settings') return Promise.resolve({ applyWarnings: [] });
      return Promise.resolve(null);
    });
    render(
      <SettingsProvider>
        <Consumer />
      </SettingsProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('settings')).toHaveTextContent('"language":"en"'),
    );
    fireEvent.click(screen.getByText('Import'));
    await waitFor(() => expect(screen.getByTestId('failed')).toHaveTextContent('true'));
    expect(screen.getByTestId('settings')).toHaveTextContent('"language":"ja"');
    expect(screen.getByTestId('warnings')).toHaveTextContent('capsLockRemap');
    fireEvent.click(screen.getByText('Edit'));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: { ...IMPORTED, launchAtLogin: true },
      }),
    );
  });

  it('blocks ordinary edits when an unconfirmed result cannot be read back', async () => {
    let reads = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === 'get_settings')
        return ++reads === 1 ? Promise.resolve(SETTINGS) : Promise.reject(new Error('read failed'));
      if (command === 'apply_configuration_backup')
        return Promise.reject(new Error('IPC disconnected'));
      if (command === 'get_configuration_backup_status')
        return Promise.resolve({ canRestore: true, lastApplied: null });
      return Promise.resolve(null);
    });
    render(
      <SettingsProvider>
        <Consumer />
      </SettingsProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('settings')).toHaveTextContent('"language":"en"'),
    );
    fireEvent.click(screen.getByText('Import'));
    await waitFor(() => expect(screen.getByTestId('failed')).toHaveTextContent('true'));
    expect(screen.getByTestId('settings')).toHaveTextContent('null');
    expect(screen.getByTestId('load-error')).toHaveTextContent('true');
    fireEvent.click(screen.getByText('Edit'));
    expect(commandCalls('save_settings')).toHaveLength(0);
  });
});
