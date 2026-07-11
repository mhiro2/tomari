import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { MasterSwitchHeader, SwitchRow } from './ui';

describe('SwitchRow', () => {
  it('shows the title and description and toggles to the opposite value', () => {
    const onChange = vi.fn();
    render(
      <SwitchRow
        title="Launch at login"
        desc="Open on sign-in"
        checked={false}
        onChange={onChange}
      />,
    );

    expect(screen.getByText('Launch at login')).toBeInTheDocument();
    expect(screen.getByText('Open on sign-in')).toBeInTheDocument();

    // The switch borrows the title as its accessible name when none is given.
    const toggle = screen.getByRole('switch', { name: 'Launch at login' });
    fireEvent.click(toggle);
    expect(onChange).toHaveBeenCalledWith(true);
  });
});

describe('MasterSwitchHeader', () => {
  it('offers a way back to the active path when off', () => {
    const onChange = vi.fn();
    render(
      <MasterSwitchHeader
        title="Keyboard customization"
        checked={false}
        onChange={onChange}
        offNote="Keyboard customization is off."
        enableLabel="Turn on"
      />,
    );

    expect(screen.getByText('Keyboard customization is off.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Turn on' }));
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it('hides the off note when on', () => {
    render(
      <MasterSwitchHeader
        title="Window management"
        checked={true}
        onChange={vi.fn()}
        offNote="Window management is off."
        enableLabel="Turn on"
      />,
    );

    expect(screen.queryByText('Window management is off.')).not.toBeInTheDocument();
  });
});
