import { describe, expect, it } from 'vitest';

import {
  displayErrorKey,
  displayName,
  modeLabel,
  preferredMode,
  resolutionGroups,
} from './displays';
import { translate, type Translator } from './i18n';

const t: Translator = (key, params) => translate('en', key, params);

describe('display labels and failures', () => {
  it('keeps native screen names readable and localizes unnamed or built-in screens', () => {
    expect(displayName({ builtIn: false, name: 'Studio\nDisplay\u202e' }, t)).toBe(
      'Studio Display',
    );
    expect(displayName({ builtIn: false, name: '\u202e' }, t)).toBe('External display');
    expect(displayName({ builtIn: true, name: 'System name' }, t)).toBe('Built-in display');
    expect(displayName({ builtIn: false, name: 'x'.repeat(200) }, t)).toBe(`${'x'.repeat(96)}…`);
  });
  it('preserves fractional rates without inventing a refresh rate for adaptive modes', () => {
    const mode = {
      id: 1,
      width: 1920,
      height: 1080,
      pixelWidth: 3840,
      pixelHeight: 2160,
      hiDpi: true,
      refreshRate: 59.94,
    };
    expect(modeLabel(mode)).toBe('1920 × 1080 · HiDPI · 59.94 Hz');
    expect(modeLabel({ ...mode, refreshRate: 0 })).toBe('1920 × 1080 · HiDPI');
  });
  it('localizes known backend errors and handles unknown IPC failures', () => {
    expect(displayErrorKey({ code: 'previewExpired' })).toBe('display.error.previewExpired');
    expect(displayErrorKey(null)).toBe('display.error.system');
    expect(displayErrorKey({ code: 'newError' })).toBe('display.error.system');
    expect(displayErrorKey({ code: 'constructor' })).toBe('display.error.system');
    expect(displayErrorKey({ code: '__proto__' })).toBe('display.error.system');
  });
});

describe('resolution groups', () => {
  const mode = {
    id: 1,
    width: 1920,
    height: 1080,
    pixelWidth: 3840,
    pixelHeight: 2160,
    hiDpi: true,
    refreshRate: 60,
  };
  it('groups refresh rates and keeps HiDPI and native pixel modes distinct', () => {
    const groups = resolutionGroups(
      [
        mode,
        { ...mode, id: 2, refreshRate: 120 },
        { ...mode, id: 3, hiDpi: false, pixelWidth: 1920, pixelHeight: 1080 },
      ],
      1,
    );
    expect(groups).toHaveLength(2);
    expect(groups[0]?.modes.map((item) => item.id)).toEqual([2, 1]);
    expect(groups[1]?.label).toBe('1920 × 1080');
  });
  it('preserves the active mode when Quartz reports duplicate timings', () => {
    expect(resolutionGroups([mode, { ...mode, id: 2 }], 2)[0]?.modes).toEqual([{ ...mode, id: 2 }]);
  });
  it('preserves the current refresh rate when choosing another resolution', () => {
    const group = resolutionGroups([{ ...mode, id: 2, refreshRate: 120 }, mode], 1)[0];
    if (!group) throw new Error('missing fixture group');
    expect(preferredMode(group, { ...mode, id: 3 })?.id).toBe(1);
    expect(preferredMode(group, undefined)?.id).toBe(2);
  });
});
