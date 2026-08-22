import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsProvider } from '../lib/settings';
import type {
  AppSettings,
  PermissionsChanged,
  PlacementContext,
  WindowHistoryStatus,
  WindowPlacement,
} from '../lib/types';
import { WindowView } from './WindowView';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

const { listen } = await import('@tauri-apps/api/event');
const mockListen = vi.mocked(listen);

const SETTINGS: AppSettings = {
  launchAtLogin: false,
  language: 'system',
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

const PRIMARY: WindowPlacement = {
  application: { bundleId: 'com.example.Editor', name: 'Editor' },
  slot: 'primary',
  frame: { x: 0, y: 0, width: 0.6, height: 1 },
};

const CONTEXT: PlacementContext = {
  target: { bundleId: PRIMARY.application.bundleId, windowId: '0000000000000042' },
  application: PRIMARY.application,
  currentFrame: { x: 0.4, y: 0.1, width: 0.5, height: 0.8 },
  placements: [PRIMARY],
  canMoveToDisplay: true,
};

const HISTORY: WindowHistoryStatus = { canUndo: true, canRedo: true };

function renderView(ui: ReactElement) {
  return render(<SettingsProvider>{ui}</SettingsProvider>);
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const value = overrides[cmd];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    switch (cmd) {
      case 'accessibility_status':
        return Promise.resolve(true);
      case 'get_placement_context':
        return Promise.resolve(CONTEXT);
      case 'get_window_history_status':
        return Promise.resolve(HISTORY);
      case 'list_hotkeys':
        return Promise.resolve([]);
      case 'capture_window_placement':
      case 'forget_window_placement':
        return Promise.resolve({ changed: true });
      case 'undo_window_placement_edit':
      case 'undo_window':
      case 'redo_window':
        return Promise.resolve('applied');
      case 'recall_window_placement':
        return Promise.resolve('primary');
      case 'move_window_to_display_and_recall':
        return Promise.resolve({ status: 'moved', slot: 'primary' });
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('WindowView', () => {
  let permissionsChanged: ((payload: PermissionsChanged) => void) | undefined;
  let panelShown: (() => void) | undefined;

  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
    permissionsChanged = undefined;
    panelShown = undefined;
    mockListen.mockReset();
    mockListen.mockImplementation((event, handler) => {
      if (event === 'tomari:permissions-changed') {
        permissionsChanged = (payload) =>
          (handler as (value: { event: string; id: number; payload: unknown }) => void)({
            event,
            id: 0,
            payload,
          });
      }
      if (event === 'tomari:panel-shown') {
        panelShown = () =>
          (handler as (value: { event: string; id: number; payload: unknown }) => void)({
            event,
            id: 0,
            payload: null,
          });
      }
      return Promise.resolve(() => {});
    });
  });

  it('targets the window represented by the panel and names the restored home', async () => {
    renderView(<WindowView />);

    expect(await screen.findByText('Editor')).toBeInTheDocument();
    expect(screen.queryByText('com.example.Editor')).not.toBeInTheDocument();
    fireEvent.click(screen.getByText('Restore position'));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('recall_window_placement', {
        target: CONTEXT.target,
      }),
    );
    expect(await screen.findByRole('status')).toHaveTextContent('Restored Editor to Home 1');
    const homeCard = screen.getAllByText('Home 1').find((element) => element.closest('article'));
    expect(homeCard?.closest('article')).toHaveAttribute('aria-current', 'true');
  });

  it('refreshes the focused application whenever the panel is shown', async () => {
    renderView(<WindowView />);
    expect(await screen.findByText('Editor')).toBeInTheDocument();

    const browser = {
      ...CONTEXT,
      target: { bundleId: 'com.example.Browser', windowId: '0000000000000099' },
      application: { bundleId: 'com.example.Browser', name: 'Browser' },
      placements: [],
    };
    mockCommands({ get_placement_context: browser });
    panelShown?.();

    expect(await screen.findByText('Browser')).toBeInTheDocument();
    expect(screen.queryByText('Editor')).not.toBeInTheDocument();
  });

  it('coalesces overlapping focus refreshes into one Accessibility request', async () => {
    let resolveContext: ((context: PlacementContext) => void) | undefined;
    const contextRequest = new Promise<PlacementContext>((resolve) => {
      resolveContext = resolve;
    });
    mockCommands({ get_placement_context: contextRequest });
    renderView(<WindowView />);

    window.dispatchEvent(new Event('focus'));
    panelShown?.();
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_placement_context')).toHaveLength(
      1,
    );

    resolveContext?.(CONTEXT);
    expect(await screen.findByText('Editor')).toBeInTheDocument();
  });

  it('makes remembered-position replacement recoverable from the toast', async () => {
    renderView(<WindowView />);
    fireEvent.click(await screen.findByText('Replace'));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('capture_window_placement', {
        target: CONTEXT.target,
        slot: 'primary',
      }),
    );
    const toast = await screen.findByRole('status');
    expect(toast).toHaveTextContent('Remembered Home 1');
    const undoButtons = screen.getAllByText('Undo');
    const placementUndo = undoButtons.at(-1);
    if (!placementUndo) throw new Error('Remembered-position undo was not rendered');
    fireEvent.click(placementUndo);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('undo_window_placement_edit'));
  });

  it('requires confirmation before forgetting a remembered position', async () => {
    renderView(<WindowView />);

    fireEvent.click(await screen.findByLabelText('Forget Home 1'));
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'forget_window_placement')).toHaveLength(
      0,
    );
    fireEvent.click(await screen.findByLabelText('Forget Home 1?'));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('forget_window_placement', {
        target: CONTEXT.target,
        slot: 'primary',
      }),
    );
    expect(await screen.findByRole('status')).toHaveTextContent('Forgot Home 1');
  });

  it('uses independent history state and reports the actual undo result', async () => {
    renderView(<WindowView />);
    const [undoButton] = await screen.findAllByText('Undo');
    if (!undoButton) throw new Error('Window undo was not rendered');
    fireEvent.click(undoButton);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('undo_window'));
    expect(await screen.findByRole('status')).toHaveTextContent('Window change undone');

    fireEvent.click(screen.getByText('Redo'));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('redo_window'));
  });

  it('disables display restore when there is no other display', async () => {
    mockCommands({ get_placement_context: { ...CONTEXT, canMoveToDisplay: false } });
    renderView(<WindowView />);

    expect(await screen.findByText('Next display & restore')).toBeDisabled();
  });

  it('does not claim a move when the backend finds no adjacent display', async () => {
    mockCommands({
      move_window_to_display_and_recall: { status: 'noAdjacentDisplay' },
    });
    renderView(<WindowView />);
    fireEvent.click(await screen.findByText('Next display & restore'));

    expect(await screen.findByRole('status')).toHaveTextContent('No other display is available');
  });

  it('links modifier tap accelerators to Keyboard instead of dedicating Caps Lock', async () => {
    const onOpenKeyboard = vi.fn();
    renderView(<WindowView onOpenKeyboard={onOpenKeyboard} />);
    fireEvent.click(await screen.findByText('Open Keyboard'));
    expect(onOpenKeyboard).toHaveBeenCalledOnce();
    expect(screen.queryByText('Tap Caps Lock to restore position')).not.toBeInTheDocument();
  });

  it('offers every window preset and both ordinary display moves as shortcuts', async () => {
    renderView(<WindowView />);
    const action = await screen.findByLabelText('Action');

    expect(action).toHaveTextContent('Snap: Top Left');
    expect(action).toHaveTextContent('Snap: Bottom Half');
    expect(action).toHaveTextContent('Snap: Right ⅔');
    expect(action).toHaveTextContent('Move to Previous Display');
    expect(action).toHaveTextContent('Move to Next Display');
  });

  it('puts direct mouse controls before remembered positions and shortcuts', async () => {
    renderView(<WindowView />);

    const mouse = await screen.findByRole('heading', { name: 'Mouse' });
    const remembered = screen.getByRole('heading', { name: 'Remembered positions' });
    const controls = screen.getByRole('heading', { name: 'Controls' });

    expect(mouse.compareDocumentPosition(remembered) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    );
    expect(
      remembered.compareDocumentPosition(controls) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).not.toBe(0);
  });

  it('shows the permission banner and follows permission changes', async () => {
    mockCommands({ accessibility_status: false });
    renderView(<WindowView />);
    expect(await screen.findByText('Accessibility access needed')).toBeInTheDocument();

    permissionsChanged?.({ accessibility: true, inputMonitoring: true });
    await waitFor(() =>
      expect(screen.queryByText('Accessibility access needed')).not.toBeInTheDocument(),
    );
  });

  it('enables drag-to-snap and persists the toggle', async () => {
    renderView(<WindowView />);
    fireEvent.click(await screen.findByLabelText('Enable Drag to Snap'));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ dragToSnapEnabled: true }),
      }),
    );
  });

  it('surfaces an initial context failure without crashing', async () => {
    mockCommands({
      get_placement_context: Object.assign(new Error('context unavailable'), { code: 'unknown' }),
    });
    renderView(<WindowView />);
    expect(await screen.findByRole('alert')).toHaveTextContent('context unavailable');
  });

  it('shows a persistent fixed error toast with a distinct tone', async () => {
    mockCommands({
      recall_window_placement: Object.assign(new Error('restore failed'), { code: 'unknown' }),
    });
    renderView(<WindowView />);
    fireEvent.click(await screen.findByText('Restore position'));

    const status = await screen.findByRole('alert');
    expect(status).toHaveTextContent('restore failed');
    expect(status).toHaveClass('window-toast', 'window-toast--err');
  });

  it('auto-clears a success status', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      renderView(<WindowView />);
      fireEvent.click(await screen.findByText('Restore position'));
      expect(await screen.findByRole('status')).toHaveTextContent('Restored Editor to Home 1');

      await act(() => vi.advanceTimersByTimeAsync(4000));
      expect(screen.queryByRole('status')).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
