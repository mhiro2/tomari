import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

import * as api from './api';
import type { MenuBarStatus } from './types';

// Whether this system still supports menu bar tidying, or null while the
// backend has not answered yet. macOS 27 withdrew the mechanism the feature is
// built on, so a surface that offers its controls — a switch, an action to
// bind a key to — has to wait for the answer rather than assume the usual one.
export function useMenuBarSupported(): boolean | null {
  const [supported, setSupported] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    let eventApplied = false;
    const apply = (status: MenuBarStatus) => {
      if (!cancelled) setSupported(status.supported);
    };
    const unlisten = listen<MenuBarStatus>('tomari:menu-bar-changed', (event) => {
      eventApplied = true;
      apply(event.payload);
    });
    const pullStatus = () => {
      if (cancelled) return;
      void api
        .getMenuBar()
        .then((status) => {
          // A runtime event is newer than this startup pull even if the command
          // response happens to arrive last.
          if (!eventApplied) apply(status);
          return undefined;
        })
        .catch(() => {});
    };
    // Establish the event stream before taking the initial snapshot, so a
    // change published between the two is not lost.
    void unlisten.then(pullStatus, pullStatus);
    return () => {
      cancelled = true;
      void unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  return supported;
}
