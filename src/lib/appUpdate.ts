import { useSyncExternalStore } from 'react';

import * as api from './api';
import { cmdErrorMessage } from './errors';
import type { UpdateInfo } from './types';

export type UpdateState =
  | { phase: 'idle' }
  | { phase: 'checking' }
  | { phase: 'upToDate' }
  | { phase: 'available'; update: UpdateInfo; error?: string }
  | { phase: 'installing'; update: UpdateInfo }
  | { phase: 'error'; message: string };

// The updater's progress lives at module scope rather than in a component:
// a check or install outlasts the General view (navigating away unmounts it,
// and the tray's "Check for Updates" can arrive at any time), and a remounted
// view must come back to "Installing…" instead of offering a second run.
let state: UpdateState = { phase: 'idle' };
const listeners = new Set<() => void>();

function setState(next: UpdateState) {
  state = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

const getSnapshot = () => state;

export function useUpdateState(): UpdateState {
  return useSyncExternalStore(subscribe, getSnapshot);
}

// One check at a time, and none while an install runs: the backend refuses a
// check during an install, and its result would replace the progress shown.
export async function checkForUpdate(): Promise<void> {
  if (state.phase === 'checking' || state.phase === 'installing') return;
  setState({ phase: 'checking' });
  try {
    const found = await api.checkForUpdate();
    setState(found ? { phase: 'available', update: found } : { phase: 'upToDate' });
  } catch (e) {
    // Update-check failures are always uncategorized (updater/network), so
    // the message is shown directly.
    setState({ phase: 'error', message: cmdErrorMessage(e) });
  }
}

// Installs the update the last check found. A no-op unless one is on offer,
// which also makes a repeated request during an install do nothing.
export async function installUpdate(): Promise<void> {
  if (state.phase !== 'available') return;
  const info = state.update;
  setState({ phase: 'installing', update: info });
  try {
    // On success the app relaunches, so this never resolves.
    await api.installUpdate();
  } catch (e) {
    // The backend keeps the pending update, so offer the install again.
    setState({ phase: 'available', update: info, error: cmdErrorMessage(e) });
  }
}

export function resetUpdateStateForTest() {
  setState({ phase: 'idle' });
}
