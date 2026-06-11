import { afterEach, describe, expect, it } from 'vitest';

import { DICTS, type MessageKey, resolveLang, translate } from './i18n';

/** The set of `{name}` placeholders appearing in a message. */
function placeholders(message: string): string[] {
  return [...message.matchAll(/\{(\w+)\}/g)]
    .map((m) => m[1] ?? '')
    .toSorted((a, b) => a.localeCompare(b));
}

function setNavigatorLanguage(value: string) {
  Object.defineProperty(window.navigator, 'language', { value, configurable: true });
}

describe('dictionaries', () => {
  // MessageKey only guarantees the keys match; a translation missing (or
  // misspelling) a `{name}` placeholder would silently render it literally.
  it('ja carries the same placeholders as en for every key', () => {
    for (const key of Object.keys(DICTS.en) as MessageKey[]) {
      expect(placeholders(DICTS.ja[key]), `placeholders differ for "${key}"`).toEqual(
        placeholders(DICTS.en[key]),
      );
    }
  });
});

describe('translate', () => {
  it('substitutes a single parameter', () => {
    expect(translate('en', 'common.enable', { label: 'Gestures' })).toBe('Enable Gestures');
    expect(translate('ja', 'common.enable', { label: 'ジェスチャ' })).toBe('ジェスチャ を有効化');
  });

  it('substitutes several parameters, numbers included', () => {
    expect(translate('en', 'settings.importApplied', { hotkeys: 3, modifierRules: 1 })).toBe(
      'Imported 3 hotkeys and 1 modifier rules.',
    );
  });

  it('returns the raw message when no parameters are given', () => {
    expect(translate('en', 'settings.updateAvailable')).toBe('Version {version} is available.');
  });

  it('leaves unknown placeholders intact and ignores extra parameters', () => {
    expect(translate('en', 'settings.updateAvailable', { unrelated: 'x' })).toBe(
      'Version {version} is available.',
    );
  });
});

describe('resolveLang', () => {
  const original = navigator.language;

  afterEach(() => {
    setNavigatorLanguage(original);
  });

  it('passes explicit languages through', () => {
    expect(resolveLang('en')).toBe('en');
    expect(resolveLang('ja')).toBe('ja');
  });

  it('maps system to ja only for Japanese locales', () => {
    setNavigatorLanguage('ja-JP');
    expect(resolveLang('system')).toBe('ja');

    setNavigatorLanguage('en-US');
    expect(resolveLang('system')).toBe('en');
  });
});
