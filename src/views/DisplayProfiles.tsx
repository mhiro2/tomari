import { useCallback, useEffect, useRef, useState } from 'react';

import * as api from '../lib/displays';
import type { DisplayProfile, DisplaySnapshot } from '../lib/displays';
import { safeDisplayText } from '../lib/format';
import { useT } from '../lib/i18n';

export function DisplayProfiles({
  snapshot,
  busy,
  configurationUnavailable,
  run,
}: {
  snapshot: DisplaySnapshot;
  busy: boolean;
  configurationUnavailable: boolean;
  run: (operation: () => Promise<DisplaySnapshot>) => Promise<void>;
}) {
  const t = useT();
  const [profiles, setProfiles] = useState<DisplayProfile[]>([]);
  const [name, setName] = useState('');
  const [selectedId, setSelectedId] = useState('');
  const [loadError, setLoadError] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    try {
      const next = await api.getDisplayProfiles();
      if (request === generation.current) {
        setProfiles(next);
        setLoadError(false);
        setLoaded(true);
      }
    } catch {
      if (request === generation.current) setLoadError(true);
    }
  }, []);
  useEffect(() => {
    void Promise.resolve().then(refresh);
    return () => {
      generation.current += 1;
    };
  }, [refresh]);

  const selected = profiles.find((profile) => profile.id === selectedId) ?? profiles[0];
  const matchesDisplays =
    selected?.displays.length === snapshot.displays.length &&
    selected?.displays.every((saved) =>
      snapshot.displays.some((display) => display.id === saved.displayId),
    );
  const matchesModes = selected?.displays.every((saved) => {
    const display = snapshot.displays.find((candidate) => candidate.id === saved.displayId);
    return [display?.currentMode, ...(display?.modes ?? [])].some(
      (mode) =>
        mode &&
        mode.width === saved.mode.width &&
        mode.height === saved.mode.height &&
        mode.pixelWidth === saved.mode.pixelWidth &&
        mode.pixelHeight === saved.mode.pixelHeight &&
        Math.abs(mode.refreshRate - saved.mode.refreshRate) < 0.01,
    );
  });
  const disabled = busy || Boolean(snapshot.pending) || configurationUnavailable;
  async function save() {
    const id = await api.saveDisplayProfile(name);
    setSelectedId(id);
    setName('');
    setRenaming(false);
  }
  const edit = (operation: () => Promise<void>) => {
    setRemoving(false);
    void run(async () => {
      await operation();
      await refresh();
      return api.getDisplays();
    });
  };

  return (
    <section className="display-controls display-profiles" aria-label={t('display.profiles')}>
      <p className="muted">{t('display.profilesHint')}</p>
      {loadError ? (
        <>
          <p className="alert" role="alert">
            {t('display.error.profileStorage')}
          </p>
          <div className="display-actions">
            <button
              type="button"
              className="btn btn--ghost"
              disabled={busy}
              onClick={() => {
                void refresh();
              }}
            >
              {t('common.refresh')}
            </button>
            <button
              type="button"
              className="btn btn--ghost"
              disabled={busy}
              onClick={() => setRemoving(true)}
            >
              {t('display.clearProfiles')}
            </button>
          </div>
        </>
      ) : (
        <>
          <form
            className="display-profile-create"
            onSubmit={(event) => {
              event.preventDefault();
              if (!name.trim() || disabled || profiles.length >= 32) return;
              // This invokes an IPC operation from submit, not a React state updater.
              // oxlint-disable-next-line react-doctor/no-impure-state-updater
              edit(save);
            }}
          >
            <label className="display-field">
              <span>{t('display.profileName')}</span>
              <input
                value={name}
                maxLength={80}
                disabled={disabled || !loaded}
                onChange={(event) => setName(event.target.value)}
                placeholder={t('display.profilePlaceholder')}
              />
            </label>
            <button
              type="submit"
              className="btn btn--primary"
              disabled={
                disabled ||
                !loaded ||
                !name.trim() ||
                profiles.length >= 32 ||
                snapshot.displays.length === 0 ||
                snapshot.displays.some((display) => display.mirrored)
              }
            >
              {t('display.saveProfile')}
            </button>
          </form>
          {profiles.length >= 32 && <p className="muted">{t('display.profileLimit')}</p>}
          {selected ? (
            <>
              <label className="display-field">
                <span>{t('display.savedProfiles')}</span>
                <select
                  value={selected.id}
                  disabled={busy}
                  onChange={(event) => {
                    setSelectedId(event.target.value);
                    setRemoving(false);
                    setRenaming(false);
                  }}
                >
                  {profiles.map((profile) => (
                    <option key={profile.id} value={profile.id}>
                      {safeDisplayText(profile.name) || t('display.unnamedProfile')}
                    </option>
                  ))}
                </select>
              </label>
              <p className="muted">
                {!matchesDisplays
                  ? t('display.error.profileUnavailable')
                  : matchesModes
                    ? t('display.profileReady', { count: selected.displays.length })
                    : t('display.error.modeUnavailable')}
              </p>
              <ul className="display-profile-summary" aria-label={t('display.profileContents')}>
                {selected.displays.map((saved) => {
                  const index = snapshot.displays.findIndex((item) => item.id === saved.displayId);
                  const display = snapshot.displays[index];
                  const label = display
                    ? `${index + 1}. ${api.displayName(display, t)}`
                    : t('display.disconnectedScreen');
                  return (
                    <li key={saved.displayId}>
                      <strong>
                        {label}
                        {saved.x === 0 && saved.y === 0 ? ` · ${t('display.main')}` : ''}
                      </strong>
                      <span className="muted">
                        {api.modeLabel({
                          ...saved.mode,
                          hiDpi:
                            saved.mode.pixelWidth > saved.mode.width &&
                            saved.mode.pixelHeight > saved.mode.height,
                        })}
                        {saved.mode.refreshRate === 0 ? ` · ${t('display.automaticRate')}` : ''}
                      </span>
                    </li>
                  );
                })}
              </ul>
              <div className="display-actions">
                <button
                  type="button"
                  className="btn btn--primary"
                  disabled={disabled || !matchesDisplays || !matchesModes}
                  onClick={() => {
                    void run(() => api.previewDisplayProfile(selected.id));
                  }}
                >
                  {t('display.applyProfile')}
                </button>
                <button
                  type="button"
                  className="btn btn--ghost"
                  disabled={busy || renaming}
                  onClick={() => {
                    setRemoving(false);
                    setRenaming(true);
                  }}
                >
                  {t('display.renameProfile')}
                </button>
                <button
                  type="button"
                  className="btn btn--ghost"
                  disabled={busy}
                  onClick={() => {
                    setRenaming(false);
                    setRemoving(true);
                  }}
                >
                  {t('display.deleteProfile')}
                </button>
              </div>
              {renaming && (
                <RenameProfile
                  key={selected.id}
                  profile={selected}
                  busy={busy}
                  onCancel={() => setRenaming(false)}
                  onSave={(nextName) => {
                    // edit runs an IPC operation; it is not a React state setter.
                    // oxlint-disable-next-line react-doctor/no-impure-state-updater
                    edit(async () => {
                      await api.renameDisplayProfile(selected.id, nextName);
                      setRenaming(false);
                    });
                  }}
                />
              )}
            </>
          ) : (
            loaded && <p className="muted">{t('display.noProfiles')}</p>
          )}
        </>
      )}
      {removing && (
        <section className="display-notice" aria-label={t('display.deleteProfile')}>
          <p>
            {loadError
              ? t('display.clearProfilesHint')
              : t('display.deleteProfileHint', {
                  name: safeDisplayText(selected?.name ?? '') || t('display.unnamedProfile'),
                })}
          </p>
          <div className="display-actions">
            <button
              type="button"
              className="btn btn--ghost"
              disabled={busy}
              onClick={() => setRemoving(false)}
            >
              {t('common.cancel')}
            </button>
            <button
              type="button"
              className="btn btn--primary"
              disabled={busy}
              onClick={() => {
                if (loadError) edit(api.clearDisplayProfiles);
                else if (selected) edit(() => api.deleteDisplayProfile(selected.id));
              }}
            >
              {t('common.delete')}
            </button>
          </div>
        </section>
      )}
    </section>
  );
}

function RenameProfile({
  profile,
  busy,
  onSave,
  onCancel,
}: {
  profile: DisplayProfile;
  busy: boolean;
  onSave: (name: string) => void;
  onCancel: () => void;
}) {
  const t = useT();
  const [name, setName] = useState(profile.name);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  const disabled = busy || !name.trim() || name.trim() === profile.name;
  return (
    <form
      className="display-notice"
      aria-label={t('display.renameProfile')}
      onSubmit={(event) => {
        event.preventDefault();
        if (!disabled) onSave(name);
      }}
    >
      <label className="display-field">
        <span>{t('display.newProfileName')}</span>
        <input
          ref={input}
          value={name}
          maxLength={80}
          disabled={busy}
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <div className="display-actions">
        <button type="button" className="btn btn--ghost" disabled={busy} onClick={onCancel}>
          {t('common.cancel')}
        </button>
        <button type="submit" className="btn btn--primary" disabled={disabled}>
          {t('display.saveProfileName')}
        </button>
      </div>
    </form>
  );
}
