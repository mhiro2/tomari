// One module-scoped lease over the global-shortcut suspension, shared by every
// recorder instance (hotkey rows + the add form): the IPC calls are serialized
// and refcounted so overlapping start/stop never resume shortcuts while
// another recorder is still capturing.

import * as api from './api';

let suspendCount = 0;
let suspendApplied = false;
let suspendQueue: Promise<void> = Promise.resolve();

/** Converge the backend onto `suspendCount > 0`. Rejects if the IPC fails. */
function syncSuspension(): Promise<void> {
  const prior = suspendQueue;
  const run = (async () => {
    await prior;
    const want = suspendCount > 0;
    if (want !== suspendApplied) {
      await api.setHotkeysSuspended(want);
      suspendApplied = want;
    }
  })();
  suspendQueue = run.catch(() => undefined);
  return run;
}

/** Suspend global shortcuts; throws (and takes no lease) if that fails. */
export async function acquireSuspension(): Promise<void> {
  suspendCount += 1;
  try {
    await syncSuspension();
  } catch (e) {
    suspendCount -= 1;
    throw e;
  }
}

export function releaseSuspension(): void {
  suspendCount = Math.max(0, suspendCount - 1);
  void syncSuspension().catch(() => {
    // A failed resume would leave every shortcut dead, so try once more; any
    // later hotkey save also re-registers everything.
    setTimeout(() => void syncSuspension().catch(() => undefined), 500);
  });
}
