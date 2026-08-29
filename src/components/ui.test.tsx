import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import {
  FeatureContent,
  FeaturePageHeader,
  FeatureSwitch,
  PermissionStatus,
  SegmentedPageNav,
  SwitchRow,
} from './ui';

describe('SwitchRow', () => {
  it('associates its explanation with the switch and toggles the value', () => {
    const onChange = vi.fn();
    render(
      <SwitchRow
        title="Launch at login"
        desc="Open on sign-in"
        checked={false}
        onChange={onChange}
      />,
    );

    const toggle = screen.getByRole('switch', { name: 'Launch at login' });
    expect(toggle).toHaveAccessibleDescription('Open on sign-in');
    fireEvent.click(toggle);
    expect(onChange).toHaveBeenCalledWith(true);
  });
});

describe('FeaturePageHeader', () => {
  it('shows one concise introduction without any control of its own', () => {
    render(
      <FeaturePageHeader
        title="Windows"
        description="Configure saved positions, shortcuts, and mouse controls."
      />,
    );

    expect(screen.getByRole('heading', { name: 'Windows', level: 1 })).toBeInTheDocument();
    expect(
      screen.getByText('Configure saved positions, shortcuts, and mouse controls.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  });
});

describe('FeatureSwitch', () => {
  it('is the only control that turns a feature on and reports its state', () => {
    const onChange = vi.fn();
    render(
      <FeatureSwitch
        title="Enable window placement"
        checked={false}
        onChange={onChange}
        stateLabel="Off"
      />,
    );

    expect(screen.getByText('Off')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Turn On' })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('switch', { name: 'Enable window placement' }));
    expect(onChange).toHaveBeenCalledWith(true);
  });
});

describe('SegmentedPageNav', () => {
  const items = [
    { value: 'saved', label: 'Saved Positions' },
    { value: 'shortcuts', label: 'Shortcuts' },
    { value: 'mouse', label: 'Mouse' },
  ] as const;

  it('exposes the selected segment as a tab and changes it on click', () => {
    const onChange = vi.fn();
    render(
      <SegmentedPageNav
        label="Window settings"
        idBase="window-tabs"
        value="saved"
        onChange={onChange}
        items={items}
      />,
    );

    expect(screen.getByRole('tablist', { name: 'Window settings' })).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'Saved Positions' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(screen.getByRole('tab', { name: 'Saved Positions' })).toHaveAttribute(
      'aria-controls',
      'window-tabs-panel',
    );
    expect(screen.getByRole('tab', { name: 'Shortcuts' })).toHaveAttribute(
      'aria-selected',
      'false',
    );

    fireEvent.click(screen.getByRole('tab', { name: 'Shortcuts' }));
    expect(onChange).toHaveBeenCalledWith('shortcuts');
  });

  it('moves and wraps with arrow keys while moving focus to the next segment', () => {
    const onChange = vi.fn();
    render(
      <SegmentedPageNav
        label="Window settings"
        idBase="window-tabs"
        value="saved"
        onChange={onChange}
        items={items}
      />,
    );

    const saved = screen.getByRole('tab', { name: 'Saved Positions' });
    const shortcuts = screen.getByRole('tab', { name: 'Shortcuts' });
    const mouse = screen.getByRole('tab', { name: 'Mouse' });

    saved.focus();
    fireEvent.keyDown(saved, { key: 'ArrowRight' });
    expect(onChange).toHaveBeenLastCalledWith('shortcuts');
    expect(shortcuts).toHaveFocus();

    saved.focus();
    fireEvent.keyDown(saved, { key: 'ArrowLeft' });
    expect(onChange).toHaveBeenLastCalledWith('mouse');
    expect(mouse).toHaveFocus();
  });
});

describe('FeatureContent', () => {
  it('keeps an off feature visible while disabling its controls', () => {
    const { rerender } = render(
      <FeatureContent enabled={false}>
        <p>Saved positions remain visible</p>
        <button type="button">Replace position</button>
      </FeatureContent>,
    );

    expect(screen.getByText('Saved positions remain visible')).toBeInTheDocument();
    expect(screen.getByRole('group')).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('button', { name: 'Replace position' })).toBeDisabled();

    rerender(
      <FeatureContent enabled>
        <p>Saved positions remain visible</p>
        <button type="button">Replace position</button>
      </FeatureContent>,
    );
    expect(screen.getByRole('group')).toHaveAttribute('aria-disabled', 'false');
    expect(screen.getByRole('button', { name: 'Replace position' })).toBeEnabled();
  });
});

describe('PermissionStatus', () => {
  it('distinguishes ready from attention and opens details from the attention state', () => {
    const onClick = vi.fn();
    const { rerender } = render(
      <PermissionStatus
        state="ready"
        readyLabel="Permissions: Ready"
        attentionLabel="Permissions: Needs attention"
        unknownLabel="Permissions: Checking…"
        onClick={onClick}
      />,
    );

    expect(screen.getByText('Permissions: Ready')).toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();

    rerender(
      <PermissionStatus
        state="attention"
        readyLabel="Permissions: Ready"
        attentionLabel="Permissions: Needs attention"
        unknownLabel="Permissions: Checking…"
        onClick={onClick}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Permissions: Needs attention' }));
    expect(onClick).toHaveBeenCalledOnce();

    // Unknown is never rendered as ready; it is a retry control.
    rerender(
      <PermissionStatus
        state="unknown"
        readyLabel="Permissions: Ready"
        attentionLabel="Permissions: Needs attention"
        unknownLabel="Permissions: Checking…"
        onClick={onClick}
      />,
    );
    expect(screen.queryByText('Permissions: Ready')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Permissions: Checking…' }));
    expect(onClick).toHaveBeenCalledTimes(2);
  });
});
