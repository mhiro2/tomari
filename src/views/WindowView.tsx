import { useEffect, useState } from 'react';

import { Banner, Group, MasterSwitchHeader, SwitchRow } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { presetLabel } from '../lib/format';
import { useT } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { WindowPreset } from '../lib/types';

export function WindowView() {
  const t = useT();
  const { settings, update } = useSettings();
  const [presets, setPresets] = useState<WindowPreset[]>([]);
  const [granted, setGranted] = useState(true);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    void api.listWindowPresets().then(setPresets);
    void api.accessibilityStatus().then(setGranted);
  }, []);

  async function snap(preset: WindowPreset) {
    try {
      // Repeated half-snaps cycle 1/2 → 1/3 → 2/3, so show what was applied.
      const applied = await api.snapWindow(preset);
      setStatus(
        applied ? t('window.snappedTo', { label: presetLabel(applied, t) }) : t('window.disabled'),
      );
    } catch (err) {
      setStatus(formatCmdError(err, t));
    }
  }

  async function run(label: string, op: () => Promise<void>) {
    try {
      await op();
      setStatus(label);
    } catch (err) {
      setStatus(formatCmdError(err, t));
    }
  }

  async function grant() {
    const ok = await api.requestAccessibility();
    setGranted(ok);
  }

  if (!settings) return <div className="view">{t('common.loading')}</div>;

  const on = settings.windowManagementEnabled;

  return (
    <div className="view">
      <MasterSwitchHeader
        title={t('settings.windowManagement')}
        checked={on}
        onChange={(v) => update({ windowManagementEnabled: v })}
        offNote={t('window.offNote')}
        enableLabel={t('common.turnOn')}
        toggleLabel={t('common.enable', { label: t('settings.windowManagement') })}
      />

      <div className={`view ${on ? '' : 'gated'}`} aria-disabled={!on} inert={!on}>
        {!granted && (
          <Banner tone="warn">
            <div className="banner__body">
              <strong>{t('window.axNeeded')}</strong>
              <p>{t('window.axBody')}</p>
            </div>
            <button type="button" className="btn btn--primary" onClick={() => void grant()}>
              {t('window.grantAccess')}
            </button>
          </Banner>
        )}

        <Group label={t('window.snapSection')} note={t('window.cycleHint')}>
          <div className="preset-grid">
            {presets.map((preset) => (
              <button
                key={preset}
                type="button"
                className="preset"
                onClick={() => void snap(preset)}
              >
                <PresetGlyph id={preset} />
                <span>{presetLabel(preset, t)}</span>
              </button>
            ))}
          </div>
        </Group>

        <Group label={t('window.displaysSection')}>
          <div className="item">
            <div className="row">
              <button
                type="button"
                className="btn"
                onClick={() =>
                  void run(t('window.movedPrev'), () => api.moveWindowToDisplay('prev'))
                }
              >
                {t('window.prevDisplay')}
              </button>
              <button
                type="button"
                className="btn"
                onClick={() =>
                  void run(t('window.movedNext'), () => api.moveWindowToDisplay('next'))
                }
              >
                {t('window.nextDisplay')}
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => void run(t('window.restored'), () => api.undoWindow())}
              >
                {t('window.undoMove')}
              </button>
            </div>
          </div>
        </Group>

        <Group label={t('window.dragToSnap')}>
          <SwitchRow
            title={t('window.dragToSnapToggle')}
            desc={t('window.dragToSnapHint')}
            checked={settings.dragToSnapEnabled}
            onChange={(v) => update({ dragToSnapEnabled: v })}
            toggleLabel={t('window.enableDragToSnap')}
          />
        </Group>

        <Group label={t('window.dragToMove')}>
          <SwitchRow
            title={t('window.dragToMoveToggle')}
            desc={t('window.dragToMoveHint')}
            checked={settings.dragToMoveEnabled}
            onChange={(v) => update({ dragToMoveEnabled: v })}
            toggleLabel={t('window.enableDragToMove')}
          />
        </Group>
      </div>

      {status && <output className="status">{status}</output>}
    </div>
  );
}

// A tiny schematic of where the window lands, drawn with a filled sub-rectangle.
function PresetGlyph({ id }: { id: WindowPreset }) {
  const r = FRACTIONS[id];
  return (
    <svg viewBox="0 0 36 24" className="preset__glyph" aria-hidden="true">
      <rect x="1" y="1" width="34" height="22" rx="3" className="preset__frame" />
      <rect
        x={1 + r.x * 34}
        y={1 + r.y * 22}
        width={r.w * 34}
        height={r.h * 22}
        rx="2"
        className="preset__fill"
      />
    </svg>
  );
}

type Frac = { x: number; y: number; w: number; h: number };
const FRACTIONS: Record<WindowPreset, Frac> = {
  leftHalf: { x: 0, y: 0, w: 0.5, h: 1 },
  rightHalf: { x: 0.5, y: 0, w: 0.5, h: 1 },
  topHalf: { x: 0, y: 0, w: 1, h: 0.5 },
  bottomHalf: { x: 0, y: 0.5, w: 1, h: 0.5 },
  topLeftQuarter: { x: 0, y: 0, w: 0.5, h: 0.5 },
  topRightQuarter: { x: 0.5, y: 0, w: 0.5, h: 0.5 },
  bottomLeftQuarter: { x: 0, y: 0.5, w: 0.5, h: 0.5 },
  bottomRightQuarter: { x: 0.5, y: 0.5, w: 0.5, h: 0.5 },
  leftThird: { x: 0, y: 0, w: 1 / 3, h: 1 },
  centerThird: { x: 1 / 3, y: 0, w: 1 / 3, h: 1 },
  rightThird: { x: 2 / 3, y: 0, w: 1 / 3, h: 1 },
  leftTwoThirds: { x: 0, y: 0, w: 2 / 3, h: 1 },
  rightTwoThirds: { x: 1 / 3, y: 0, w: 2 / 3, h: 1 },
  center: { x: 0.2, y: 0.15, w: 0.6, h: 0.7 },
  maximize: { x: 0, y: 0, w: 1, h: 1 },
};
