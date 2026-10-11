import { listen } from '@tauri-apps/api/event';
import { useEffect } from 'react';

import { acknowledgeDisplayNavigation, getDisplayNavigation } from './displays';

export function useDisplayNavigation(open: () => void) {
  useEffect(() => {
    let active = true;
    let handled: number | null = null;
    let generation = 0;
    const pull = async () => {
      if (!active) return;
      const request = ++generation;
      try {
        const revision = await getDisplayNavigation();
        if (!active || request !== generation || revision === null || revision === handled) return;
        handled = revision;
        open();
        await acknowledgeDisplayNavigation(revision);
      } catch {
        // The request stays pending if the window cannot acknowledge it.
      }
    };
    const unlisten = listen('tomari:open-displays', () => {
      void pull();
    });
    void unlisten.then(pull).catch(pull);
    return () => {
      active = false;
      void unlisten.then((stop) => stop()).catch(() => {});
    };
  }, [open]);
}
