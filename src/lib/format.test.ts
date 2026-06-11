import { describe, expect, it } from 'vitest';

import { actionLabel, modifierWithSide, presetLabel } from './format';
import { translate, type Translator } from './i18n';

const t: Translator = (key, params) => translate('en', key, params);
const tJa: Translator = (key, params) => translate('ja', key, params);

describe('actionLabel', () => {
  it('labels simple actions', () => {
    expect(actionLabel({ type: 'togglePanel' }, t)).toBe('Toggle Tomari');
    expect(actionLabel({ type: 'noOp' }, t)).toBe('Do Nothing');
  });

  it('labels window snapping', () => {
    expect(actionLabel({ type: 'snapWindow', value: 'leftHalf' }, t)).toBe('Snap: Left Half');
  });

  it('labels display moves and undo', () => {
    expect(actionLabel({ type: 'moveWindowToDisplay', value: 'next' }, t)).toBe(
      'Move to Next Display',
    );
    expect(actionLabel({ type: 'moveWindowToDisplay', value: 'prev' }, t)).toBe(
      'Move to Previous Display',
    );
    expect(actionLabel({ type: 'undoWindow' }, t)).toBe('Undo Window Move');
  });

  it('labels IME switching with Japanese glyphs', () => {
    expect(actionLabel({ type: 'switchIme', value: 'alphanumeric' }, t)).toBe('IME → 英数');
    expect(actionLabel({ type: 'switchIme', value: 'kana' }, t)).toBe('IME → かな');
  });

  it('distinguishes launch vs quick peek', () => {
    expect(
      actionLabel(
        { type: 'launchApp', value: { bundleId: 'com.apple.Safari', quickPeek: false } },
        t,
      ),
    ).toBe('Launch: com.apple.Safari');
    expect(
      actionLabel(
        { type: 'launchApp', value: { bundleId: 'com.apple.finder', quickPeek: true } },
        t,
      ),
    ).toBe('Quick Peek: com.apple.finder');
  });

  it('renders in Japanese', () => {
    expect(actionLabel({ type: 'snapWindow', value: 'leftHalf' }, tJa)).toBe('スナップ: 左半分');
    expect(actionLabel({ type: 'moveWindowToDisplay', value: 'next' }, tJa)).toBe(
      '次のディスプレイへ移動',
    );
  });
});

describe('modifierWithSide', () => {
  it('omits the side when either', () => {
    expect(modifierWithSide('capsLock', 'either', t)).toBe('⇪');
  });

  it('includes the side for paired keys', () => {
    expect(modifierWithSide('command', 'left', t)).toBe('Left ⌘');
    expect(modifierWithSide('command', 'right', t)).toBe('Right ⌘');
  });

  it('renders the side in Japanese', () => {
    expect(modifierWithSide('command', 'left', tJa)).toBe('左 ⌘');
  });
});

describe('presetLabel', () => {
  it('maps every code to a label', () => {
    expect(presetLabel('maximize', t)).toBe('Maximize');
    expect(presetLabel('bottomRightQuarter', t)).toBe('Bottom Right');
  });
});
