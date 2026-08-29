import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ReactElement } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsProvider } from '../lib/settings';
import type {
  AppSettings,
  Hotkey,
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
  damagedPlacements: [],
  canMoveToDisplay: true,
};

const HISTORY: WindowHistoryStatus = { canUndo: true, canRedo: true };

const WINDOW_HOTKEYS: Hotkey[] = [
  {
    id: 'left',
    label: 'Left',
    accelerator: 'Ctrl+Alt+Left',
    action: { type: 'snapWindow', value: 'leftHalf' },
    enabled: true,
  },
  {
    id: 'right',
    label: 'Right',
    accelerator: 'Ctrl+Alt+Right',
    action: { type: 'snapWindow', value: 'rightHalf' },
    enabled: true,
  },
  {
    id: 'maximize',
    label: 'Maximize',
    accelerator: 'Ctrl+Alt+Up',
    action: { type: 'snapWindow', value: 'maximize' },
    enabled: true,
  },
  {
    id: 'recall',
    label: 'Restore',
    accelerator: 'Ctrl+Alt+Down',
    action: { type: 'recallWindowPlacement' },
    enabled: true,
  },
  {
    id: 'move-recall',
    label: 'Move and restore',
    accelerator: 'Ctrl+Alt+Shift+Right',
    action: { type: 'moveWindowToDisplayAndRecall', value: 'next' },
    enabled: true,
  },
  {
    id: 'undo',
    label: 'Undo',
    accelerator: 'Ctrl+Alt+Z',
    action: { type: 'undoWindow' },
    enabled: true,
  },
  {
    id: 'redo',
    label: 'Redo',
    accelerator: 'Ctrl+Alt+Shift+Z',
    action: { type: 'redoWindow' },
    enabled: true,
  },
];

function renderView(ui: ReactElement) {
  return render(<SettingsProvider>{ui}</SettingsProvider>);
}

async function openTab(name: string) {
  fireEvent.click(await screen.findByRole('tab', { name }));
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
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
      case 'save_hotkey':
        return Promise.resolve((args as { hotkey: Hotkey }).hotkey);
      case 'delete_hotkey':
        return Promise.resolve(undefined);
      case 'capture_window_placement':
      case 'forget_window_placement':
        return Promise.resolve({ changed: true, undoable: true });
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
  // Every `tomari:panel-shown` listener under the view — the view's own and
  // the settings provider's — fires together, as the real event would.
  let panelShownHandlers: Array<() => void> = [];
  const panelShown = () => panelShownHandlers.forEach((fire) => fire());

  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
    panelShownHandlers = [];
    mockListen.mockReset();
    mockListen.mockImplementation((event, handler) => {
      if (event === 'tomari:panel-shown') {
        panelShownHandlers.push(() =>
          (handler as (value: { event: string; id: number; payload: unknown }) => void)({
            event,
            id: 0,
            payload: null,
          }),
        );
      }
      return Promise.resolve(() => {});
    });
  });

  it('targets the window represented by the panel and names the restored home', async () => {
    renderView(<WindowView />);

    expect(await screen.findByText('Editor')).toBeInTheDocument();
    expect(screen.queryByText('com.example.Editor')).not.toBeInTheDocument();
    fireEvent.click(screen.getByText('Restore saved position'));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('recall_window_placement', {
        target: CONTEXT.target,
      }),
    );
    expect(await screen.findByRole('status')).toHaveTextContent('Restored Editor to Position A');
    const homeCard = screen
      .getAllByText('Position A')
      .find((element) => element.closest('article'));
    expect(homeCard?.closest('article')).toHaveAttribute('aria-current', 'true');
  });

  it('offers to replace or forget a damaged saved position instead of showing it empty', async () => {
    mockCommands({
      get_placement_context: {
        ...CONTEXT,
        placements: [],
        damagedPlacements: ['primary'],
      },
    });
    renderView(<WindowView />);

    expect(
      await screen.findByText(
        'The saved position is damaged and cannot be used. Replace it or forget it.',
      ),
    ).toBeInTheDocument();
    // The damaged slot keeps its Forget action so the row can be cleared.
    fireEvent.click(screen.getByRole('button', { name: 'Forget Position A' }));
    fireEvent.click(screen.getByRole('button', { name: 'Forget Position A?' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        'forget_window_placement',
        expect.objectContaining({ slot: 'primary' }),
      ),
    );
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
    panelShown();

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
    panelShown();
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_placement_context')).toHaveLength(
      1,
    );

    resolveContext?.(CONTEXT);
    expect(await screen.findByText('Editor')).toBeInTheDocument();
  });

  it('pulls fresh context after a mutation and discards a pull that started before it', async () => {
    // 1st pull: the initial load. 2nd: a focus refresh that hangs. 3rd: the
    // post-mutation pull, which must be a new request rather than a reuse of
    // the hanging one, and whose answer must not be overwritten when the
    // hanging pre-mutation request finally lands with the old world.
    let resolveStale: ((context: PlacementContext) => void) | undefined;
    const browser: PlacementContext = {
      ...CONTEXT,
      target: { bundleId: 'com.example.Browser', windowId: '0000000000000099' },
      application: { bundleId: 'com.example.Browser', name: 'Browser' },
      placements: [],
    };
    mockCommands();
    const base = mockInvoke.getMockImplementation();
    let contextPulls = 0;
    mockInvoke.mockImplementation((cmd: string, args?: Parameters<typeof mockInvoke>[1]) => {
      if (cmd !== 'get_placement_context') return base?.(cmd, args) ?? Promise.resolve(null);
      contextPulls += 1;
      if (contextPulls === 1) return Promise.resolve(CONTEXT);
      if (contextPulls === 2)
        return new Promise<PlacementContext>((resolve) => {
          resolveStale = resolve;
        });
      return Promise.resolve(browser);
    });
    renderView(<WindowView />);
    expect(await screen.findByText('Editor')).toBeInTheDocument();

    window.dispatchEvent(new Event('focus'));
    await waitFor(() => expect(contextPulls).toBe(2));

    fireEvent.click(screen.getByText('Replace position'));
    await waitFor(() => expect(contextPulls).toBe(3));
    expect(await screen.findByText('Browser')).toBeInTheDocument();

    // The pre-mutation pull answers late with the old world: ignored. Settle
    // its continuation inside `act` so the assertion runs after it would have
    // applied, not before.
    await act(async () => {
      resolveStale?.(CONTEXT);
    });
    expect(screen.queryByText('Editor')).not.toBeInTheDocument();
    expect(screen.getByText('Browser')).toBeInTheDocument();
  });

  it('discards a pre-mutation pull that fails late without disturbing the fresh context', async () => {
    let rejectStale: ((error: unknown) => void) | undefined;
    const browser: PlacementContext = {
      ...CONTEXT,
      target: { bundleId: 'com.example.Browser', windowId: '0000000000000099' },
      application: { bundleId: 'com.example.Browser', name: 'Browser' },
      placements: [],
    };
    mockCommands();
    const base = mockInvoke.getMockImplementation();
    let contextPulls = 0;
    mockInvoke.mockImplementation((cmd: string, args?: Parameters<typeof mockInvoke>[1]) => {
      if (cmd !== 'get_placement_context') return base?.(cmd, args) ?? Promise.resolve(null);
      contextPulls += 1;
      if (contextPulls === 1) return Promise.resolve(CONTEXT);
      if (contextPulls === 2)
        return new Promise<PlacementContext>((_resolve, reject) => {
          rejectStale = reject;
        });
      return Promise.resolve(browser);
    });
    renderView(<WindowView />);
    expect(await screen.findByText('Editor')).toBeInTheDocument();

    window.dispatchEvent(new Event('focus'));
    await waitFor(() => expect(contextPulls).toBe(2));
    fireEvent.click(screen.getByText('Replace position'));
    expect(await screen.findByText('Browser')).toBeInTheDocument();

    // The stale pull fails late: neither the context nor an error may change.
    await act(async () => {
      rejectStale?.(new Error('stale failure'));
    });
    expect(screen.getByText('Browser')).toBeInTheDocument();
    expect(screen.queryByText('stale failure')).not.toBeInTheDocument();
  });

  it('discards a history status that started before an undo and lands after it', async () => {
    // Initial history: undo available. A focus refresh's history pull hangs;
    // the user undoes (history becomes empty, per the post-mutation pull); the
    // hanging pre-undo pull then answers "undo available" — and must lose.
    let resolveStale: ((status: WindowHistoryStatus) => void) | undefined;
    mockCommands({ undo_window: 'applied' });
    const base = mockInvoke.getMockImplementation();
    let historyPulls = 0;
    mockInvoke.mockImplementation((cmd: string, args?: Parameters<typeof mockInvoke>[1]) => {
      if (cmd !== 'get_window_history_status') return base?.(cmd, args) ?? Promise.resolve(null);
      historyPulls += 1;
      if (historyPulls === 1) return Promise.resolve(HISTORY);
      if (historyPulls === 2)
        return new Promise<WindowHistoryStatus>((resolve) => {
          resolveStale = resolve;
        });
      return Promise.resolve({ canUndo: false, canRedo: true });
    });
    renderView(<WindowView />);
    const undo = await screen.findByRole('button', { name: 'Undo' });
    await waitFor(() => expect(undo).not.toBeDisabled());

    window.dispatchEvent(new Event('focus'));
    await waitFor(() => expect(historyPulls).toBe(2));

    fireEvent.click(undo);
    await waitFor(() => expect(historyPulls).toBe(3));
    await waitFor(() => expect(undo).toBeDisabled());

    await act(async () => {
      resolveStale?.(HISTORY);
    });
    expect(undo).toBeDisabled();
  });

  it('makes remembered-position replacement recoverable from the toast', async () => {
    renderView(<WindowView />);
    fireEvent.click(await screen.findByText('Replace position'));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('capture_window_placement', {
        target: CONTEXT.target,
        slot: 'primary',
      }),
    );
    const toast = await screen.findByRole('status');
    expect(toast).toHaveTextContent('Remembered Position A');
    const undoButtons = screen.getAllByText('Undo');
    const placementUndo = undoButtons.at(-1);
    if (!placementUndo) throw new Error('Remembered-position undo was not rendered');
    fireEvent.click(placementUndo);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('undo_window_placement_edit'));
  });

  it('does not offer placement undo when the backend could not create a recovery entry', async () => {
    mockCommands({ capture_window_placement: { changed: true, undoable: false } });
    renderView(<WindowView />);
    fireEvent.click(await screen.findByText('Replace position'));

    const toast = await screen.findByRole('status');
    expect(toast).toHaveTextContent('Remembered Position A');
    expect(within(toast).queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument();
  });

  it('requires confirmation before forgetting a remembered position', async () => {
    renderView(<WindowView />);

    fireEvent.click(await screen.findByLabelText('Forget Position A'));
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'forget_window_placement')).toHaveLength(
      0,
    );
    fireEvent.click(await screen.findByLabelText('Forget Position A?'));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('forget_window_placement', {
        target: CONTEXT.target,
        slot: 'primary',
      }),
    );
    expect(await screen.findByRole('status')).toHaveTextContent('Forgot Position A');
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
    await openTab('Shortcuts');
    fireEvent.click(await screen.findByText('Open Keyboard'));
    expect(onOpenKeyboard).toHaveBeenCalledOnce();
    expect(screen.queryByText('Tap Caps Lock to restore position')).not.toBeInTheDocument();
  });

  it('offers every window preset and both ordinary display moves as shortcuts', async () => {
    renderView(<WindowView />);
    await openTab('Shortcuts');
    fireEvent.click(await screen.findByText('Add Shortcut'));
    const action = await screen.findByLabelText('Action');

    expect(action).toHaveTextContent('Snap: Top Left');
    expect(action).toHaveTextContent('Snap: Bottom Half');
    expect(action).toHaveTextContent('Snap: Right ⅔');
    expect(action).toHaveTextContent('Move to Previous Display');
    expect(action).toHaveTextContent('Move to Next Display');
  });

  it('opens Add Shortcut as a modal, focuses its form, and closes on Escape', async () => {
    const showModalDescriptor = Object.getOwnPropertyDescriptor(
      HTMLDialogElement.prototype,
      'showModal',
    );
    const closeDescriptor = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, 'close');
    const showModal = vi.fn(function (this: HTMLDialogElement) {
      this.setAttribute('open', '');
    });
    const close = vi.fn(function (this: HTMLDialogElement) {
      this.removeAttribute('open');
    });
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', {
      configurable: true,
      value: showModal,
    });
    Object.defineProperty(HTMLDialogElement.prototype, 'close', {
      configurable: true,
      value: close,
    });

    try {
      renderView(<WindowView />);
      await openTab('Shortcuts');
      fireEvent.click(await screen.findByText('Add Shortcut'));

      const dialog = await screen.findByRole('dialog', { name: 'Add Shortcut' });
      expect(showModal).toHaveBeenCalledOnce();
      expect(screen.getByLabelText('Shortcut label')).toHaveFocus();

      fireEvent(dialog, new Event('cancel', { cancelable: true }));
      expect(screen.queryByRole('dialog', { name: 'Add Shortcut' })).not.toBeInTheDocument();
      expect(close).toHaveBeenCalledOnce();
    } finally {
      if (showModalDescriptor) {
        Object.defineProperty(HTMLDialogElement.prototype, 'showModal', showModalDescriptor);
      } else {
        Reflect.deleteProperty(HTMLDialogElement.prototype, 'showModal');
      }
      if (closeDescriptor) {
        Object.defineProperty(HTMLDialogElement.prototype, 'close', closeDescriptor);
      } else {
        Reflect.deleteProperty(HTMLDialogElement.prototype, 'close');
      }
    }
  });

  it('keeps user labels visible so duplicate window actions remain distinguishable', async () => {
    const left = WINDOW_HOTKEYS[0];
    if (!left) throw new Error('Left shortcut fixture is missing');
    mockCommands({
      list_hotkeys: [
        { ...left, id: 'editor-left', label: 'Editor left' },
        { ...left, id: 'reference-left', label: 'Reference left', accelerator: 'Ctrl+Alt+1' },
      ],
    });
    renderView(<WindowView />);
    await openTab('Shortcuts');

    expect(await screen.findByText('Editor left — Snap: Left Half')).toBeInTheDocument();
    expect(screen.getByText('Reference left — Snap: Left Half')).toBeInTheDocument();
    expect(
      screen.getByRole('button', {
        name: 'Change shortcut for Reference left — Snap: Left Half',
      }),
    ).toBeInTheDocument();
  });

  it('sanitizes quarantined window-shortcut text in visible and accessible labels', async () => {
    const left = WINDOW_HOTKEYS[0];
    if (!left) throw new Error('Left shortcut fixture is missing');
    mockCommands({
      list_hotkeys: [
        {
          ...left,
          label: `Safe\u0000\u202e${'界'.repeat(120)}`,
          accelerator: 'Cmd+\u202eK\u0000',
        },
      ],
    });
    const { container } = renderView(<WindowView />);
    await openTab('Shortcuts');

    const title = await screen.findByText(/^Safe 界+… — Snap: Left Half$/u);
    expect(container.textContent).not.toMatch(/[\p{Cc}\p{Cf}]/u);
    expect(Array.from(title.textContent?.split(' — ')[0] ?? '')).toHaveLength(97);
    expect(
      screen.getByRole('button', { name: /^Change shortcut for Safe 界+… — Snap: Left Half$/u }),
    ).toHaveTextContent('⌘K');
  });

  it('uses the canonical hotkey returned by save for the next window-shortcut mutation', async () => {
    const left = WINDOW_HOTKEYS[0];
    if (!left) throw new Error('Left shortcut fixture is missing');
    const raw = { ...left, id: ' left ', accelerator: 'command+plus', enabled: false };
    const savedHotkeys: Hotkey[] = [];
    mockCommands();
    const base = mockInvoke.getMockImplementation();
    mockInvoke.mockImplementation((cmd: string, args?: Parameters<typeof mockInvoke>[1]) => {
      if (cmd === 'list_hotkeys') return Promise.resolve([raw]);
      if (cmd === 'save_hotkey') {
        const submitted = (args as { hotkey: Hotkey }).hotkey;
        savedHotkeys.push(submitted);
        return Promise.resolve({
          ...submitted,
          id: 'left',
          accelerator: 'Shift+Cmd+Equal',
        });
      }
      return base?.(cmd, args) ?? Promise.resolve(null);
    });
    renderView(<WindowView />);
    await openTab('Shortcuts');

    const label = 'Enable Left — Snap: Left Half';
    fireEvent.click(await screen.findByRole('switch', { name: label }));
    await waitFor(() => expect(screen.getByRole('switch', { name: label })).not.toBeDisabled());
    fireEvent.click(screen.getByRole('switch', { name: label }));

    await waitFor(() => expect(savedHotkeys).toHaveLength(2));
    expect(savedHotkeys[0]).toMatchObject({ id: ' left ', accelerator: 'command+plus' });
    expect(savedHotkeys[1]).toMatchObject({ id: 'left', accelerator: 'Shift+Cmd+Equal' });
  });

  it('disables window shortcut editing while Keyboard customization is off', async () => {
    const onOpenKeyboard = vi.fn();
    mockCommands({
      get_settings: { ...SETTINGS, keyboardEnabled: false },
      list_hotkeys: [WINDOW_HOTKEYS[0]],
    });
    renderView(<WindowView onOpenKeyboard={onOpenKeyboard} />);
    await openTab('Shortcuts');

    const title = 'Left — Snap: Left Half';
    expect(await screen.findByText(title)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add Shortcut' })).toBeDisabled();
    expect(screen.getByRole('button', { name: `Change shortcut for ${title}` })).toBeDisabled();
    expect(screen.getByRole('button', { name: `Delete ${title}` })).toBeDisabled();
    expect(screen.getByRole('switch', { name: `Enable ${title}` })).toBeDisabled();
    expect(
      screen.getByText(
        'Turn on Keyboard customization to use these shortcuts or assign Restore to a modifier-key tap.',
      ),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Open Keyboard' }));
    expect(onOpenKeyboard).toHaveBeenCalledOnce();
  });

  it('separates saved positions, shortcuts, and mouse controls into focused tabs', async () => {
    renderView(<WindowView />);

    expect(await screen.findByRole('heading', { name: 'Current window' })).toBeInTheDocument();
    expect(
      screen.getByRole('heading', { name: 'Saved positions for this app' }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Common shortcuts' })).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Mouse' })).not.toBeInTheDocument();

    await openTab('Shortcuts');
    expect(screen.getByRole('heading', { name: 'Common shortcuts' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Current window' })).not.toBeInTheDocument();

    await openTab('Mouse');
    expect(screen.getByRole('heading', { name: 'Mouse' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Common shortcuts' })).not.toBeInTheDocument();
  });

  it('shows common shortcuts first and keeps the remaining actions folded', async () => {
    mockCommands({ list_hotkeys: WINDOW_HOTKEYS });
    renderView(<WindowView />);
    await openTab('Shortcuts');

    expect(await screen.findByText('Left — Snap: Left Half')).toBeInTheDocument();
    expect(
      screen.getByText('Move and restore — Move & Restore on Next Display'),
    ).toBeInTheDocument();
    expect(screen.queryByText('Undo — Undo Window Change')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Other layouts (2)' }));
    expect(screen.getByText('Undo — Undo Window Change')).toBeInTheDocument();
    expect(screen.getByText('Redo — Redo Window Change')).toBeInTheDocument();
  });

  it('enables drag-to-snap and persists the toggle', async () => {
    renderView(<WindowView />);
    await openTab('Mouse');
    fireEvent.click(await screen.findByLabelText('Enable Drag to Snap'));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ dragToSnapEnabled: true }),
      }),
    );
  });

  it('keeps the selected settings visible but disabled while window management is off', async () => {
    mockCommands({
      get_settings: { ...SETTINGS, windowManagementEnabled: false },
    });
    renderView(<WindowView />);

    expect(await screen.findByText('Editor')).toBeInTheDocument();
    expect(screen.getByText('Restore saved position')).toBeDisabled();
    expect(screen.getByRole('switch', { name: 'Enable Window placement' })).not.toBeDisabled();

    await openTab('Mouse');
    expect(screen.getByLabelText('Enable Drag to Snap')).toBeDisabled();
  });

  it('surfaces an initial context failure without crashing', async () => {
    mockCommands({
      get_placement_context: Object.assign(new Error('context unavailable'), { code: 'unknown' }),
    });
    renderView(<WindowView />);
    expect(await screen.findByRole('alert')).toHaveTextContent('context unavailable');
  });

  it('clears a context-load error after a successful manual refresh', async () => {
    mockCommands({
      get_placement_context: Object.assign(new Error('context unavailable'), { code: 'unknown' }),
    });
    renderView(<WindowView />);
    expect(await screen.findByRole('alert')).toHaveTextContent('context unavailable');

    mockCommands();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));

    expect(await screen.findByText('Editor')).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  });

  it('shows a persistent fixed error toast with a distinct tone', async () => {
    mockCommands({
      recall_window_placement: Object.assign(new Error('restore failed'), { code: 'unknown' }),
    });
    renderView(<WindowView />);
    fireEvent.click(await screen.findByText('Restore saved position'));

    const status = await screen.findByRole('alert');
    expect(status).toHaveTextContent('restore failed');
    expect(status).toHaveClass('window-toast', 'window-toast--err');

    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_placement_context')).toHaveLength(
        2,
      ),
    );
    expect(screen.getByRole('alert')).toHaveTextContent('restore failed');
  });

  it('auto-clears a success status', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      renderView(<WindowView />);
      fireEvent.click(await screen.findByText('Restore saved position'));
      expect(await screen.findByRole('status')).toHaveTextContent('Restored Editor to Position A');

      await act(() => vi.advanceTimersByTimeAsync(4000));
      expect(screen.queryByRole('status')).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
