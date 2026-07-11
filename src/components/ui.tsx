// Shared presentational building blocks.
//
// Rows come in two shapes so a 400px-wide panel never forces a one-size row:
//   SwitchRow  — label (+ description) with a trailing toggle
//   EntityRow  — a leading glyph/keycap, a body, and trailing actions
// They sit inside a Group: a single inset surface with hairline dividers,
// rather than one card per setting.

import type { ReactNode } from 'react';

export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label?: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      className={`toggle ${checked ? 'toggle--on' : ''}`}
      onClick={() => onChange(!checked)}
    >
      <span className="toggle__knob" />
    </button>
  );
}

export function Group({
  label,
  note,
  children,
}: {
  label?: string;
  note?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="group">
      {label && <h2 className="group__label">{label}</h2>}
      <div className="group__body">{children}</div>
      {note && <p className="group__note">{note}</p>}
    </section>
  );
}

export function SwitchRow({
  title,
  desc,
  lead,
  checked,
  onChange,
  toggleLabel,
}: {
  title: string;
  desc?: ReactNode;
  lead?: ReactNode;
  checked: boolean;
  onChange: (next: boolean) => void;
  toggleLabel?: string;
}) {
  return (
    <div className="item">
      {lead && <span className="item__lead">{lead}</span>}
      <div className="item__body">
        <span className="item__title">{title}</span>
        {desc && <span className="item__desc">{desc}</span>}
      </div>
      <div className="item__trail">
        <Toggle checked={checked} onChange={onChange} label={toggleLabel ?? title} />
      </div>
    </div>
  );
}

export function EntityRow({
  lead,
  title,
  sub,
  trail,
}: {
  lead: ReactNode;
  title: ReactNode;
  sub?: ReactNode;
  trail?: ReactNode;
}) {
  return (
    <div className="item">
      <span className="item__lead">{lead}</span>
      <div className="item__body">
        <span className="item__title">{title}</span>
        {sub && <span className="item__desc">{sub}</span>}
      </div>
      {trail && <div className="item__trail">{trail}</div>}
    </div>
  );
}

export function Chip({
  tone = 'muted',
  children,
}: {
  tone?: 'on' | 'ok' | 'warn' | 'err' | 'muted';
  children: ReactNode;
}) {
  return <span className={`chip chip--${tone}`}>{children}</span>;
}

// A feature's master switch, shown at the top of its tab. When off, the content
// below stays readable but is made non-interactive by the caller; this header
// explains the state and offers a one-click way back to the active path.
export function MasterSwitchHeader({
  title,
  checked,
  onChange,
  offNote,
  enableLabel,
  toggleLabel,
}: {
  title: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  offNote: string;
  enableLabel: string;
  toggleLabel?: string;
}) {
  return (
    <div className={`master ${checked ? '' : 'master--off'}`}>
      <div className="master__row">
        <span className="master__title">{title}</span>
        <Toggle checked={checked} onChange={onChange} label={toggleLabel} />
      </div>
      {!checked && (
        <div className="master__off">
          <p>{offNote}</p>
          <button type="button" className="btn btn--amber" onClick={() => onChange(true)}>
            {enableLabel}
          </button>
        </div>
      )}
    </div>
  );
}

export function Banner({ tone, children }: { tone: 'warn'; children: ReactNode }) {
  return <div className={`banner banner--${tone}`}>{children}</div>;
}
