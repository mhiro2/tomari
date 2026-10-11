import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { I18nProvider } from '../lib/i18n';
import { SettingsProvider, useSettings } from '../lib/settings';
import type { AppSettings } from '../lib/types';
import { ApplyWarningPanel } from './ApplyWarningPanel';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);
const SETTINGS: AppSettings = {
  launchAtLogin: false,
  language: 'en',
  keyboardEnabled: true,
  windowManagementEnabled: true,
  externalWindowActionsEnabled: false,
  commandImeSwitchEnabled: false,
  showInMenuBar: true,
  dragToSnapEnabled: false,
  dragToMoveEnabled: false,
  menuBarTidyEnabled: false,
  menuBarAutoCollapseSecs: 0,
};
let warnings: string[];
const GRANTED_PERMISSIONS = { accessibility: true, inputMonitoring: true };

function Harness({
  permissions,
  section,
}: {
  permissions: typeof GRANTED_PERMISSIONS | null;
  section?: string;
}) {
  const { settings, update, configurationBusy } = useSettings();
  return (
    <>
      <button
        type="button"
        disabled={!settings}
        onClick={() => update({ launchAtLogin: !settings?.launchAtLogin })}
      >
        Edit
      </button>
      <output data-testid="busy">{String(configurationBusy)}</output>
      <ApplyWarningPanel
        section={section}
        permissions={permissions}
        onOpenPermissions={vi.fn()}
        onOpenDiagnostics={vi.fn()}
      />
    </>
  );
}

function renderPanel(
  lang: 'en' | 'ja' = 'en',
  permissions: typeof GRANTED_PERMISSIONS | null = GRANTED_PERMISSIONS,
  section?: string,
) {
  return render(
    <SettingsProvider>
      <I18nProvider lang={lang}>
        <Harness permissions={permissions} section={section} />
      </I18nProvider>
    </SettingsProvider>,
  );
}

function calls(command: string) {
  return mockInvoke.mock.calls.filter(([cmd]) => cmd === command);
}

describe('ApplyWarningPanel', () => {
  beforeEach(() => {
    warnings = ['keyboardTap'];
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_apply_warnings') return Promise.resolve({ warnings, unprobed: [] });
      if (cmd === 'save_settings') return Promise.resolve({ applyWarnings: warnings });
      return Promise.resolve(null);
    });
  });

  it.each([
    ['keyboard', ['Caps Lock', 'Keyboard customization'], ['Drag to snap', 'launch at login']],
    ['window', ['Drag to snap'], ['Caps Lock', 'launch at login']],
    ['general', ['launch at login'], ['Caps Lock', 'Drag to snap']],
    ['session', [], ['Caps Lock', 'Drag to snap', 'launch at login']],
    ['health', ['Caps Lock', 'Drag to snap', 'launch at login'], []],
  ])(
    'limits warnings to %s and keeps all recovery details in diagnostics',
    async (section, visible, hidden) => {
      warnings = ['capsLockRemap', 'keyboardTap', 'dragToSnapTap', 'launchAtLogin'];
      renderPanel('en', GRANTED_PERMISSIONS, section);
      await waitFor(() => expect(screen.getByText('Edit')).toBeEnabled());
      await waitFor(() => {
        for (const text of visible) expect(screen.getByText(new RegExp(text))).toBeInTheDocument();
      });
      for (const text of hidden)
        expect(screen.queryByText(new RegExp(text))).not.toBeInTheDocument();
    },
  );

  it('waits for queued autosaves before restarting and blocks duplicate requests', async () => {
    const base = mockInvoke.getMockImplementation();
    if (!base) throw new Error('Command mock missing');
    const finishSaves: Array<() => void> = [];
    let finishRestart!: () => void;
    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'save_settings')
        return new Promise((resolve) =>
          finishSaves.push(() => resolve({ applyWarnings: warnings })),
        );
      if (cmd === 'restart_app')
        return new Promise<void>((resolve) => {
          finishRestart = resolve;
        });
      return base(cmd, args);
    });
    renderPanel();
    const restart = await screen.findByRole('button', { name: 'Restart Tomari' });
    fireEvent.click(screen.getByText('Edit'));
    fireEvent.click(screen.getByText('Edit'));
    fireEvent.click(restart);
    fireEvent.click(restart);
    expect(calls('restart_app')).toHaveLength(0);
    expect(screen.getByTestId('busy')).toHaveTextContent('true');
    expect(screen.getByRole('button', { name: 'Open Diagnostics' })).toBeDisabled();
    await act(async () => finishSaves[0]?.());
    expect(calls('save_settings')).toHaveLength(2);
    expect(calls('restart_app')).toHaveLength(0);
    await act(async () => finishSaves[1]?.());
    await waitFor(() => expect(calls('restart_app')).toHaveLength(1));
    expect(screen.getByRole('button', { name: 'Restarting Tomari…' })).toBeDisabled();
    await act(async () => finishRestart());
    expect(screen.getByRole('button', { name: 'Restarting Tomari…' })).toBeDisabled();
    expect(calls('restart_app')).toHaveLength(1);
  });

  it('reports a rejected restart and only retries after another click', async () => {
    const base = mockInvoke.getMockImplementation();
    if (!base) throw new Error('Command mock missing');
    mockInvoke.mockImplementation((cmd, args) =>
      cmd === 'restart_app' ? Promise.reject(new Error('cleanup unavailable')) : base(cmd, args),
    );
    renderPanel();
    fireEvent.click(await screen.findByRole('button', { name: 'Restart Tomari' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Tomari could not restart: cleanup unavailable',
    );
    expect(screen.getByRole('button', { name: 'Restart Tomari' })).toBeEnabled();
    expect(calls('restart_app')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Restart Tomari' }));
    await waitFor(() => expect(calls('restart_app')).toHaveLength(2));
  });

  it('does not restart after a pending settings save fails', async () => {
    const base = mockInvoke.getMockImplementation();
    if (!base) throw new Error('Command mock missing');
    let failSave!: () => void;
    mockInvoke.mockImplementation((cmd, args) =>
      cmd === 'save_settings'
        ? new Promise((_, reject) => {
            failSave = () => reject(new Error('disk full'));
          })
        : base(cmd, args),
    );
    renderPanel();
    const restart = await screen.findByRole('button', { name: 'Restart Tomari' });
    fireEvent.click(screen.getByText('Edit'));
    fireEvent.click(restart);
    await act(async () => failSave());

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Tomari could not restart: disk full',
    );
    expect(calls('restart_app')).toHaveLength(0);
    expect(screen.getByRole('button', { name: 'Restart Tomari' })).toBeEnabled();
  });

  it('describes shortcut conflicts and unknown failures without claiming a restart will fix them', async () => {
    warnings = ['globalShortcuts', 'futureWarning'];
    renderPanel();
    expect(await screen.findByText(/Some shortcuts are unavailable/)).toBeInTheDocument();
    expect(screen.getByText(/Tomari could not start one of its features/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Open Diagnostics' })).toHaveClass('btn--primary');
    expect(screen.queryByRole('button', { name: 'Restart Tomari' })).not.toBeInTheDocument();
  });

  it('localizes concrete failures and restart actions in Japanese', async () => {
    warnings = ['capsLockRemap'];
    renderPanel('ja');
    expect(await screen.findByText(/Caps Lock の変更を適用できません/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Tomari を再起動' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '診断を開く' })).toBeInTheDocument();
  });

  it.each(['globalShortcuts', 'commandImeRules', 'capsLockRemap'])(
    'does not route %s failures to unrelated missing permissions',
    async (code) => {
      warnings = [code];
      renderPanel('en', { accessibility: false, inputMonitoring: false });

      const diagnostics = await screen.findByRole('button', { name: 'Open Diagnostics' });
      expect(diagnostics).toBeEnabled();
      expect(
        screen.queryByRole('button', { name: 'Open Permission Settings' }),
      ).not.toBeInTheDocument();
      if (code === 'globalShortcuts') {
        expect(diagnostics).toHaveClass('btn--primary');
        expect(screen.queryByRole('button', { name: 'Restart Tomari' })).not.toBeInTheDocument();
      }
    },
  );

  it.each([
    ['dragToSnapTap', { accessibility: false, inputMonitoring: true }, false],
    ['dragToSnapTap', { accessibility: true, inputMonitoring: false }, true],
    ['keyboardTap', { accessibility: false, inputMonitoring: true }, true],
    ['keyboardTap', { accessibility: true, inputMonitoring: false }, true],
    ['dragToMoveTap', { accessibility: false, inputMonitoring: true }, true],
    ['keyboardTap', null, false],
  ] as const)(
    'routes %s only to its known missing grants: %o',
    async (code, permissions, expected) => {
      warnings = [code];
      renderPanel('en', permissions);
      await screen.findByRole('button', { name: 'Open Diagnostics' });

      const permissionAction = screen.queryByRole('button', { name: 'Open Permission Settings' });
      if (expected) {
        expect(permissionAction).toHaveClass('btn--primary');
        expect(screen.queryByRole('button', { name: 'Restart Tomari' })).not.toBeInTheDocument();
      } else {
        expect(permissionAction).not.toBeInTheDocument();
        expect(screen.getByRole('button', { name: 'Restart Tomari' })).toBeEnabled();
      }
    },
  );
});
