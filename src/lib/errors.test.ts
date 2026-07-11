import { describe, expect, it } from 'vitest';

import { cmdErrorMessage, errorText, formatCmdError } from './errors';
import { translate } from './i18n';
import type { CmdError } from './types';

const t = (key: Parameters<typeof translate>[1], params?: Record<string, string | number>) =>
  translate('en', key, params);

describe('formatCmdError', () => {
  it('translates a known code, ignoring any attached message', () => {
    const err: CmdError = { code: 'permissionRequired', message: 'raw backend text' };
    expect(formatCmdError(err, t)).toBe(t('error.permissionRequired'));
  });

  it.each([
    ['permissionRequired', 'error.permissionRequired'],
    ['noFocusedWindow', 'error.noFocusedWindow'],
    ['shortcutConflict', 'error.shortcutConflict'],
  ] as const)('localizes code %s', (code, key) => {
    const err: CmdError = { code, message: 'ignored' };
    expect(formatCmdError(err, t)).toBe(t(key));
  });

  it('falls back to the message for an unknown code', () => {
    const err: CmdError = { code: 'other', message: 'disk write failed' };
    expect(formatCmdError(err, t)).toBe('disk write failed');
  });

  it('falls back to errorText when an unknown code has no message', () => {
    const err = { code: 'other', message: '' };
    expect(formatCmdError(err, t)).toBe('[object Object]');
  });

  it('falls back to errorText for a non-CmdError input', () => {
    expect(formatCmdError(new Error('network down'), t)).toBe('network down');
    expect(formatCmdError('plain string', t)).toBe('plain string');
    expect(formatCmdError(null, t)).toBe('null');
  });
});

describe('cmdErrorMessage', () => {
  it('returns the message field verbatim, without translation', () => {
    const err: CmdError = { code: 'permissionRequired', message: 'update check failed' };
    expect(cmdErrorMessage(err)).toBe('update check failed');
  });

  it('falls back to errorText when message is missing or empty', () => {
    expect(cmdErrorMessage({ code: 'other', message: '' })).toBe('[object Object]');
    expect(cmdErrorMessage(new Error('boom'))).toBe('boom');
  });
});

describe('errorText', () => {
  it('returns a string input unchanged', () => {
    expect(errorText('already a string')).toBe('already a string');
  });

  it('returns an Error message', () => {
    expect(errorText(new Error('failure'))).toBe('failure');
  });

  it('stringifies anything else', () => {
    expect(errorText(42)).toBe('42');
    expect(errorText(null)).toBe('null');
  });
});
