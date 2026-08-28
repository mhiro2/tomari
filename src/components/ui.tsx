import { useId, type KeyboardEvent, type ReactNode } from 'react';

export function Toggle({
  checked,
  onChange,
  label,
  describedBy,
  disabled,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label?: string;
  describedBy?: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-describedby={describedBy}
      disabled={disabled}
      className={`toggle ${checked ? 'toggle--on' : ''}`}
      onClick={() => onChange(!checked)}
    >
      <span className="toggle__knob" />
    </button>
  );
}

export function FeaturePageHeader({
  title,
  description,
}: {
  title: string;
  description: ReactNode;
}) {
  return (
    <header className="feature-header">
      <div className="feature-header__copy">
        <h1>{title}</h1>
        <p>{description}</p>
      </div>
    </header>
  );
}

// The page's master switch. It sits first in the content column as its own
// one-row card, so the control lines up with every other row's control and
// reads as "the thing this page turns on" rather than as window chrome.
export function FeatureSwitch({
  title,
  description,
  checked,
  onChange,
  toggleLabel,
  disabled,
  stateLabel,
  trail,
  tone = 'neutral',
}: {
  title: string;
  description?: ReactNode;
  checked: boolean;
  onChange: (next: boolean) => void;
  toggleLabel?: string;
  disabled?: boolean;
  stateLabel?: string;
  trail?: ReactNode;
  tone?: 'neutral' | 'on' | 'pending' | 'danger';
}) {
  const descriptionId = useId();
  return (
    <div className={`settings-list feature-switch feature-switch--${tone}`}>
      <SettingsRow
        title={title}
        description={description && <span id={descriptionId}>{description}</span>}
        trail={
          <>
            {trail}
            {stateLabel && <span className="feature-switch__state">{stateLabel}</span>}
            <Toggle
              checked={checked}
              onChange={onChange}
              label={toggleLabel ?? title}
              describedBy={description ? descriptionId : undefined}
              disabled={disabled}
            />
          </>
        }
      />
    </div>
  );
}

export interface SegmentedPageItem<Value extends string> {
  value: Value;
  label: string;
}

export function SegmentedPageNav<Value extends string>({
  label,
  idBase,
  value,
  onChange,
  items,
}: {
  label: string;
  idBase?: string;
  value: Value;
  onChange: (value: Value) => void;
  items: readonly SegmentedPageItem<Value>[];
}) {
  const generatedId = useId();
  const id = idBase ?? generatedId;

  function move(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const nextIndex =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? items.length - 1
          : event.key === 'ArrowRight' || event.key === 'ArrowDown'
            ? (index + 1) % items.length
            : event.key === 'ArrowLeft' || event.key === 'ArrowUp'
              ? (index - 1 + items.length) % items.length
              : null;
    if (nextIndex === null) return;
    event.preventDefault();
    const next = items[nextIndex];
    if (!next) return;
    onChange(next.value);
    event.currentTarget.parentElement
      ?.querySelectorAll<HTMLButtonElement>('[role="tab"]')
      .item(nextIndex)
      .focus();
  }

  return (
    <div className="segmented" role="tablist" aria-label={label}>
      {items.map((item, index) => (
        <button
          key={item.value}
          id={`${id}-${item.value}-tab`}
          type="button"
          role="tab"
          aria-selected={value === item.value}
          aria-controls={`${id}-panel`}
          tabIndex={value === item.value ? 0 : -1}
          className={
            value === item.value ? 'segmented__item segmented__item--active' : 'segmented__item'
          }
          onClick={() => onChange(item.value)}
          onKeyDown={(event) => move(event, index)}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}

export function FeatureContent({
  enabled,
  children,
  className = '',
}: {
  enabled: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <fieldset
      className={`feature-content ${enabled ? '' : 'feature-content--disabled'} ${className}`.trim()}
      disabled={!enabled}
      aria-disabled={!enabled}
    >
      {children}
    </fieldset>
  );
}

export function SettingsList({
  label,
  description,
  children,
  className = '',
}: {
  label?: string;
  description?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`settings-section ${className}`.trim()}>
      {(label || description) && (
        <header className="settings-section__header">
          {label && <h2>{label}</h2>}
          {description && <p>{description}</p>}
        </header>
      )}
      <div className="settings-list">{children}</div>
    </section>
  );
}

export function SettingsRow({
  title,
  description,
  lead,
  trail,
  children,
  className = '',
}: {
  title?: ReactNode;
  description?: ReactNode;
  lead?: ReactNode;
  trail?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div className={`settings-row ${className}`.trim()}>
      {lead && <div className="settings-row__lead">{lead}</div>}
      {(title || description || children) && (
        <div className="settings-row__body">
          {title && <div className="settings-row__title">{title}</div>}
          {description && <div className="settings-row__description">{description}</div>}
          {children}
        </div>
      )}
      {trail && <div className="settings-row__trail">{trail}</div>}
    </div>
  );
}

export function PermissionStatus({
  ready,
  readyLabel,
  attentionLabel,
  onClick,
}: {
  ready: boolean;
  readyLabel: string;
  attentionLabel: string;
  onClick: () => void;
}) {
  if (ready) {
    return (
      <div className="permission-status permission-status--ready">
        <span className="permission-status__dot" aria-hidden="true" />
        <span>{readyLabel}</span>
      </div>
    );
  }

  return (
    <button
      type="button"
      className="permission-status permission-status--attention"
      onClick={onClick}
    >
      <span className="permission-status__dot" aria-hidden="true" />
      <span>{attentionLabel}</span>
      <span className="permission-status__chevron" aria-hidden="true">
        ›
      </span>
    </button>
  );
}

export function StatusLabel({
  tone = 'muted',
  children,
}: {
  tone?: 'active' | 'ready' | 'attention' | 'danger' | 'muted';
  children: ReactNode;
}) {
  return <span className={`status-label status-label--${tone}`}>{children}</span>;
}

export function HelpDisclosure({ label, children }: { label: string; children: ReactNode }) {
  return (
    <details className="help-disclosure">
      <summary>{label}</summary>
      <div className="help-disclosure__body">{children}</div>
    </details>
  );
}

export function Banner({ tone, children }: { tone: 'warn'; children: ReactNode }) {
  return <div className={`banner banner--${tone}`}>{children}</div>;
}

// Object cards and focused flows can still use a bounded surface. Ordinary
// preferences use SettingsList and SettingsRow instead.
export function Group({
  label,
  description,
  note,
  children,
}: {
  label?: string;
  description?: ReactNode;
  note?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="group">
      {(label || description) && (
        <header className="group__header">
          {label && <h2 className="group__label">{label}</h2>}
          {description && <p className="group__description">{description}</p>}
        </header>
      )}
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
  disabled,
}: {
  title: string;
  desc?: ReactNode;
  lead?: ReactNode;
  checked: boolean;
  onChange: (next: boolean) => void;
  toggleLabel?: string;
  disabled?: boolean;
}) {
  const descriptionId = useId();
  return (
    <SettingsRow
      lead={lead}
      title={title}
      description={desc && <span id={descriptionId}>{desc}</span>}
      trail={
        <Toggle
          checked={checked}
          onChange={onChange}
          label={toggleLabel ?? title}
          describedBy={desc ? descriptionId : undefined}
          disabled={disabled}
        />
      }
    />
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
  return <SettingsRow lead={lead} title={title} description={sub} trail={trail} />;
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
