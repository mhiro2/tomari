import { useEffect, useRef, useState } from 'react';

import * as api from '../lib/api';
import { acceleratorChips } from '../lib/format';
import { useT } from '../lib/i18n';
import { captureAccelerator, heldModifierGlyphs } from '../lib/recorder';
import { acquireSuspension, releaseSuspension } from '../lib/suspension';

/**
 * A click-to-record shortcut field: click it, type the chord, and the
 * normalized accelerator is committed. While recording, Tomari's own global
 * shortcuts are suspended so the typed chord reaches this field instead of
 * firing an action; plain Escape or clicking away cancels.
 *
 * macOS WebKit does not move keyboard focus to a <button> on click, so without
 * an explicit focus the field would never see a keydown — neither the chord
 * nor Escape. start() focuses the button so capture and cancel actually work.
 */
export function ShortcutRecorder({
  value,
  onCapture,
  ariaLabel,
}: {
  value?: string;
  onCapture: (normalized: string) => void;
  ariaLabel: string;
}) {
  const t = useT();
  const [recording, setRecording] = useState(false);
  const [held, setHeld] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Mirrors `recording` for unmount cleanup and for guarding the async
  // capture path against a stop() that happened while awaiting.
  const recordingRef = useRef(false);
  // True while start() awaits the suspension, so a second click cannot take a
  // second lease that a single stop() would then never give back.
  const startingRef = useRef(false);
  const mountedRef = useRef(true);
  const buttonRef = useRef<HTMLButtonElement>(null);

  async function start() {
    if (recordingRef.current || startingRef.current) return;
    startingRef.current = true;
    setError(null);
    try {
      // Keys are only accepted once the suspension is actually in place;
      // otherwise the typed chord could fire its currently-bound action.
      await acquireSuspension();
    } catch {
      startingRef.current = false;
      setError(t('recorder.startFailed'));
      return;
    }
    startingRef.current = false;
    if (!mountedRef.current) {
      // Unmounted while the suspend was in flight: the cleanup below could
      // not see this lease, so hand it straight back.
      releaseSuspension();
      return;
    }
    recordingRef.current = true;
    setRecording(true);
    setHeld([]);
    // The click that started us did not focus this button (macOS WebKit), so do
    // it now — otherwise keydown/keyup never reach onKeyDown/onKeyUp and neither
    // the chord nor Escape would register.
    buttonRef.current?.focus();
  }

  function stop() {
    if (!recordingRef.current) return;
    recordingRef.current = false;
    setRecording(false);
    setHeld([]);
    setError(null);
    releaseSuspension();
  }

  // Never leave the lease held if unmounted mid-recording. The setup arm
  // re-arms `mountedRef` for StrictMode's mount → unmount → mount cycle.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (recordingRef.current) {
        recordingRef.current = false;
        releaseSuspension();
      }
    };
  }, []);

  async function onKeyDown(event: React.KeyboardEvent) {
    if (!recording) return;
    event.preventDefault();
    event.stopPropagation();

    if (event.code === 'Escape' && heldModifierGlyphs(event).length === 0) {
      stop();
      return;
    }

    const result = captureAccelerator(event);
    if (result.status === 'ignored') {
      setHeld(heldModifierGlyphs(event));
      return;
    }
    if (result.status === 'unsupported') {
      setError(t('recorder.unsupported'));
      return;
    }
    if (result.status === 'needModifier') {
      setError(t('recorder.needModifier'));
      return;
    }

    // The backend owns the canonical form, so normalize through it.
    const check = await api.validateAccelerator(result.accelerator);
    // A blur (e.g. focus moved to another recorder) may have stopped this
    // recording while awaiting — the capture is then stale.
    if (!recordingRef.current) return;
    if (check.valid && check.normalized !== null) {
      onCapture(check.normalized);
      stop();
    } else {
      setError(check.error ?? t('recorder.unsupported'));
    }
  }

  function onKeyUp(event: React.KeyboardEvent) {
    if (recording) setHeld(heldModifierGlyphs(event));
  }

  // While recording we already show live glyphs; at rest, render the stored
  // canonical accelerator with the same macOS glyphs so the two states match.
  const chips = recording ? held : acceleratorChips(value);

  return (
    <button
      ref={buttonRef}
      type="button"
      className={`recorder ${recording ? 'recorder--active' : ''}`}
      aria-label={ariaLabel}
      onClick={() => (recording ? stop() : void start())}
      onKeyDown={(e) => void onKeyDown(e)}
      onKeyUp={onKeyUp}
      onBlur={() => recording && stop()}
    >
      {chips.length > 0 ? (
        <span className="accel">
          {chips.map((part) => (
            <kbd key={part}>{part}</kbd>
          ))}
          {recording && <span className="recorder__placeholder">…</span>}
        </span>
      ) : (
        <span className="recorder__placeholder">
          {recording ? t('recorder.typing') : t('recorder.click')}
        </span>
      )}
      {error && <span className="recorder__err">{error}</span>}
    </button>
  );
}
