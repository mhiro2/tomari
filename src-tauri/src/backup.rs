//! Portable configuration files and confirmed, reversible whole-store replacement.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tomari_core::{AppPaths, AppSettings, ConfigurationBackup};
use tomari_keyboard::validation;

use crate::commands::{apply_settings_runtime, lock_config_mutation, off_main};
use crate::error::{CmdError, ErrorCode};
use crate::locks::MutexExt;
use crate::state::AppState;

type Result<T> = std::result::Result<T, CmdError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum ReplacementKind {
    Import,
    Restore,
}

#[derive(Debug, Clone)]
struct PreparedReplacement {
    token: u32,
    kind: ReplacementKind,
    current: ConfigurationBackup,
    incoming: ConfigurationBackup,
}

#[derive(Default)]
struct Session {
    generation: u32,
    pending: Option<PreparedReplacement>,
    last_applied: Option<CompletedReplacement>,
}

/// Only the preview is transient. The previous configuration is stored in the
/// same database transaction as replacement and survives application restarts.
#[derive(Default)]
pub struct BackupSession(Mutex<Session>);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupCounts {
    hotkeys: usize,
    modifier_rules: usize,
    window_placements: usize,
}

impl From<&ConfigurationBackup> for BackupCounts {
    fn from(value: &ConfigurationBackup) -> Self {
        Self {
            hotkeys: value.hotkeys.len(),
            modifier_rules: value.modifier_rules.len(),
            window_placements: value.window_placements.len(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPreview {
    token: u32,
    kind: ReplacementKind,
    current: BackupCounts,
    incoming: BackupCounts,
    current_settings: AppSettings,
    settings: AppSettings,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    can_restore: bool,
    last_applied: Option<CompletedReplacement>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupExport {
    path: String,
    generated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupApplied {
    settings: AppSettings,
    apply_warnings: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompletedReplacement {
    token: u32,
    #[serde(flatten)]
    outcome: BackupApplied,
}

fn invalid(code: ErrorCode, error: impl std::fmt::Display) -> CmdError {
    CmdError::new(code, error.to_string())
}

/// Reuse the same collection validators as ordinary keyboard writes. Checking
/// the entire collection also rejects collisions involving disabled records.
fn validated(mut backup: ConfigurationBackup, code: ErrorCode) -> Result<ConfigurationBackup> {
    backup.validate().map_err(|error| invalid(code, error))?;
    let hotkeys = validation::validate_hotkeys(backup.hotkeys);
    let rules = validation::validate_modifier_rules(backup.modifier_rules);
    if !hotkeys.invalid.is_empty() || !rules.invalid.is_empty() {
        return Err(invalid(
            code,
            "the configuration contains invalid keyboard records",
        ));
    }
    backup.hotkeys = hotkeys.valid;
    backup.modifier_rules = rules.valid;
    backup.to_json().map_err(|error| invalid(code, error))?;
    Ok(backup)
}

/// Preserve the exact stored snapshot for comparison and rollback. Validation
/// happens on a clone: canonicalizing the source would silently change raw IDs.
fn current_configuration(state: &AppState) -> Result<ConfigurationBackup> {
    let current = state
        .db
        .configuration_backup()
        .map_err(|error| invalid(ErrorCode::BackupCurrentInvalid, error))?;
    current
        .to_json()
        .map_err(|error| invalid(ErrorCode::BackupCurrentInvalid, error))?;
    validated(current.clone(), ErrorCode::BackupCurrentInvalid)?;
    Ok(current)
}

fn prepare(
    state: &AppState,
    session: &BackupSession,
    incoming: ConfigurationBackup,
    kind: ReplacementKind,
) -> Result<BackupPreview> {
    let incoming = match kind {
        ReplacementKind::Import => validated(incoming, ErrorCode::BackupInvalid)?,
        ReplacementKind::Restore => {
            validated(incoming.clone(), ErrorCode::BackupInvalid)?;
            incoming
        }
    };
    let current = {
        let _window = state.lock_window_mutation();
        current_configuration(state)?
    };
    let mut session = session.0.lock_safe();
    session.generation = session.generation.checked_add(1).ok_or_else(|| {
        CmdError::other("restart Tomari before preparing another configuration replacement")
    })?;
    let token = session.generation;
    let preview = BackupPreview {
        token,
        kind,
        current: (&current).into(),
        incoming: (&incoming).into(),
        current_settings: current.settings.clone(),
        settings: incoming.settings.clone(),
    };
    session.pending = Some(PreparedReplacement {
        token,
        kind,
        current,
        incoming,
    });
    Ok(preview)
}

#[tauri::command]
pub async fn preview_configuration_import(
    app: AppHandle,
    contents: String,
) -> Result<BackupPreview> {
    off_main(app, move |app, state| {
        let incoming = ConfigurationBackup::from_json(&contents)
            .map_err(|error| invalid(ErrorCode::BackupInvalid, error))?;
        let guard = lock_config_mutation(state)?;
        let result = prepare(
            state,
            &app.state::<BackupSession>(),
            incoming,
            ReplacementKind::Import,
        );
        guard.discard();
        result
    })
    .await
}

#[tauri::command]
pub async fn preview_configuration_restore(app: AppHandle) -> Result<BackupPreview> {
    off_main(app, |app, state| {
        let guard = lock_config_mutation(state)?;
        let result = (|| {
            let incoming = state.db.read_previous_configuration()?.ok_or_else(|| {
                invalid(
                    ErrorCode::BackupUnavailable,
                    "no previous configuration is available",
                )
            })?;
            prepare(
                state,
                &app.state::<BackupSession>(),
                incoming,
                ReplacementKind::Restore,
            )
        })();
        guard.discard();
        result
    })
    .await
}

#[tauri::command]
pub async fn get_configuration_backup_status(app: AppHandle) -> Result<BackupStatus> {
    off_main(app, |app, state| {
        let guard = lock_config_mutation(state)?;
        let result = state
            .db
            .read_previous_configuration()
            .map(|previous| BackupStatus {
                can_restore: previous.is_some(),
                last_applied: app
                    .state::<BackupSession>()
                    .0
                    .lock_safe()
                    .last_applied
                    .clone(),
            })
            .map_err(CmdError::from);
        guard.discard();
        result
    })
    .await
}

/// Called only under the configuration gate. The window gate is confined to
/// persistence and history invalidation; holding it during the runtime's main-
/// thread hops would deadlock a window command waiting on that same gate.
fn replace_prepared(
    state: &AppState,
    session: &BackupSession,
    token: u32,
    apply: impl FnOnce(ConfigurationBackup) -> Vec<&'static str>,
) -> Result<BackupApplied> {
    let prepared = session
        .0
        .lock_safe()
        .pending
        .clone()
        .filter(|prepared| prepared.token == token)
        .ok_or_else(|| {
            invalid(
                ErrorCode::BackupPreviewExpired,
                "prepare and confirm the configuration again",
            )
        })?;
    {
        let _window = state.lock_window_mutation();
        state
            .db
            .replace_configuration(
                &prepared.current,
                &prepared.incoming,
                prepared.kind == ReplacementKind::Import,
            )
            .map_err(|error| match error {
                tomari_core::Error::ConfigurationChanged => {
                    invalid(ErrorCode::BackupPreviewExpired, error)
                }
                other => other.into(),
            })?;
        state.clear_placement_configuration_history();
    }
    session.0.lock_safe().pending = None;
    let settings = prepared.incoming.settings.clone();
    let apply_warnings = apply(prepared.incoming);
    let outcome = BackupApplied {
        settings,
        apply_warnings,
    };
    session.0.lock_safe().last_applied = Some(CompletedReplacement {
        token,
        outcome: outcome.clone(),
    });
    Ok(outcome)
}

#[tauri::command]
pub async fn apply_configuration_backup(app: AppHandle, token: u32) -> Result<BackupApplied> {
    off_main(app, move |app, state| {
        let _guard = lock_config_mutation(state)?;
        replace_prepared(state, &app.state::<BackupSession>(), token, |backup| {
            let rules = validation::validate_modifier_rules(backup.modifier_rules).valid;
            apply_settings_runtime(app, state, backup.settings, Some(rules)).apply_warnings
        })
    })
    .await
}

#[tauri::command]
pub async fn export_configuration_backup(app: AppHandle) -> Result<BackupExport> {
    off_main(app, |app, state| {
        let guard = lock_config_mutation(state)?;
        let snapshot = {
            let _window = state.lock_window_mutation();
            current_configuration(state)
        };
        guard.discard();
        let snapshot = snapshot?;
        let bytes = validated(snapshot, ErrorCode::BackupCurrentInvalid)?.to_json()?;
        let directory = app.state::<AppPaths>().data_dir.join("backups");
        write_export(&directory, bytes.as_bytes())
    })
    .await
}

fn write_export(directory: &Path, contents: &[u8]) -> Result<BackupExport> {
    use std::time::{SystemTime, UNIX_EPOCH};
    std::fs::create_dir_all(directory).map_err(|error| CmdError::other(error.to_string()))?;
    let generated_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| CmdError::other(error.to_string()))?
        .as_millis() as u64;
    // create_new keeps simultaneous exports and existing user files intact.
    for suffix in 0..1000 {
        let path = directory.join(format!("tomari-settings-{generated_at_ms}-{suffix}.json"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(CmdError::other(error.to_string())),
        };
        if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
            let _ = std::fs::remove_file(&path);
            return Err(CmdError::other(error.to_string()));
        }
        return Ok(BackupExport {
            path: path.to_string_lossy().into_owned(),
            generated_at_ms,
        });
    }
    Err(CmdError::other(
        "could not allocate a new configuration backup file",
    ))
}

#[tauri::command]
pub async fn reveal_configuration_backups(app: AppHandle) -> Result<()> {
    off_main(app, |app, _| {
        let directory = app.state::<AppPaths>().data_dir.join("backups");
        std::fs::create_dir_all(&directory).map_err(|error| CmdError::other(error.to_string()))?;
        #[cfg(target_os = "macos")]
        {
            let mut command = std::process::Command::new("/usr/bin/open");
            command.arg(directory);
            match crate::childproc::output_with_deadline(
                &mut command,
                std::time::Duration::from_secs(3),
            ) {
                Ok(crate::childproc::Outcome::Exited { status, .. }) if status.success() => Ok(()),
                _ => Err(CmdError::other(
                    "could not open the configuration backup folder",
                )),
            }
        }
        #[cfg(not(target_os = "macos"))]
        Err(CmdError::other(
            "opening the configuration backup folder requires macOS",
        ))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use tomari_core::{
        Database, Language, NormalizedRect, PlacementSlot, Rect, WindowApplication, WindowPlacement,
    };
    use tomari_keyboard::ModifierEngine;
    use tomari_window::MockWindowManager;

    fn state() -> AppState {
        let db = Database::open_in_memory().unwrap();
        db.save_settings(&AppSettings::default()).unwrap();
        for hotkey in tomari_core::defaults::default_hotkeys() {
            db.upsert_hotkey(&hotkey).unwrap();
        }
        for rule in tomari_core::defaults::default_modifier_rules() {
            db.insert_modifier_rule(&rule).unwrap();
        }
        AppState::new(
            db,
            ModifierEngine::new(Vec::new()),
            Box::new(MockWindowManager::new(Rect::new(0.0, 0.0, 100.0, 100.0))),
            AppSettings::default(),
            false,
        )
    }

    fn placement() -> WindowPlacement {
        WindowPlacement {
            application: WindowApplication {
                bundle_id: "com.example.Editor".into(),
                name: "Editor".into(),
            },
            slot: PlacementSlot::Primary,
            frame: NormalizedRect::new(0.0, 0.0, 0.5, 1.0),
        }
    }

    #[test]
    fn keyboard_validation_rejects_unsafe_and_colliding_imports() {
        let state = state();
        let backup = current_configuration(&state).unwrap();
        let mut unsafe_shortcut = backup.clone();
        unsafe_shortcut.hotkeys[0].accelerator = "A".into();
        assert_eq!(
            validated(unsafe_shortcut, ErrorCode::BackupInvalid)
                .unwrap_err()
                .code,
            ErrorCode::BackupInvalid
        );
        let mut duplicate = backup.clone();
        duplicate.hotkeys[1].accelerator = duplicate.hotkeys[0].accelerator.clone();
        duplicate.hotkeys[1].enabled = false;
        assert!(validated(duplicate, ErrorCode::BackupInvalid).is_err());
        let mut contradictory = backup.clone();
        contradictory.modifier_rules[0].hyper = true;
        assert!(validated(contradictory, ErrorCode::BackupInvalid).is_err());
        let mut reserved = backup;
        reserved.modifier_rules = tomari_core::defaults::command_ime_rules();
        assert!(validated(reserved, ErrorCode::BackupInvalid).is_err());
    }

    #[test]
    fn unreadable_or_quarantined_current_data_prevents_replacement() {
        let state = state();
        let incoming = current_configuration(&state).unwrap();
        let mut invalid_rule = incoming.modifier_rules[0].clone();
        invalid_rule.label.clear();
        state.db.upsert_modifier_rule(&invalid_rule).unwrap();
        let session = BackupSession::default();
        let error = prepare(&state, &session, incoming, ReplacementKind::Import).unwrap_err();
        assert_eq!(error.code, ErrorCode::BackupCurrentInvalid);
        assert!(session.0.lock_safe().pending.is_none());
        assert!(state.db.read_previous_configuration().unwrap().is_none());
        assert!(state.db.list_modifier_rules().unwrap()[0].label.is_empty());
    }

    #[test]
    fn preview_rejects_snapshots_that_cannot_fit_the_persisted_restore_format() {
        let state = state();
        let original = current_configuration(&state).unwrap();
        let mut oversized = original.clone();
        let mut home = placement();
        home.application.name = "x".repeat(tomari_core::MAX_CONFIGURATION_BACKUP_BYTES);
        oversized.window_placements.push(home.clone());
        let session = BackupSession::default();
        let error = prepare(&state, &session, oversized, ReplacementKind::Import).unwrap_err();
        assert_eq!(error.code, ErrorCode::BackupInvalid);
        state.db.save_window_placement(&home).unwrap();
        let error = prepare(&state, &session, original, ReplacementKind::Import).unwrap_err();
        assert_eq!(error.code, ErrorCode::BackupCurrentInvalid);
        assert!(session.0.lock_safe().pending.is_none());
    }

    #[test]
    fn new_preview_invalidates_the_old_token_without_writing() {
        let state = state();
        let original = current_configuration(&state).unwrap();
        let session = BackupSession::default();
        let first = prepare(&state, &session, original.clone(), ReplacementKind::Import).unwrap();
        let second = prepare(&state, &session, original.clone(), ReplacementKind::Import).unwrap();
        assert_ne!(first.token, second.token);
        let error = replace_prepared(&state, &session, first.token, |_| {
            panic!("stale preview must not apply")
        })
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::BackupPreviewExpired);
        assert_eq!(state.db.configuration_backup().unwrap(), original);
    }

    #[test]
    fn settings_or_placement_changes_after_preview_require_confirmation_again() {
        for change_placement in [false, true] {
            let state = state();
            let current = current_configuration(&state).unwrap();
            let session = BackupSession::default();
            let preview = prepare(&state, &session, current, ReplacementKind::Import).unwrap();
            if change_placement {
                state.db.save_window_placement(&placement()).unwrap();
            } else {
                let changed = AppSettings {
                    language: Language::Ja,
                    ..Default::default()
                };
                state.db.save_settings(&changed).unwrap();
            }
            let changed = state.db.configuration_backup().unwrap();
            let error = replace_prepared(&state, &session, preview.token, |_| {
                panic!("unconfirmed change")
            })
            .unwrap_err();
            assert_eq!(error.code, ErrorCode::BackupPreviewExpired);
            assert_eq!(state.db.configuration_backup().unwrap(), changed);
            assert!(state.db.read_previous_configuration().unwrap().is_none());
        }
    }

    #[test]
    fn same_settings_import_still_applies_rules_and_preserves_a_restore_point_on_warnings() {
        let state = state();
        let original = current_configuration(&state).unwrap();
        let mut incoming = original.clone();
        incoming.modifier_rules.clear();
        incoming.hotkeys.clear();
        let session = BackupSession::default();
        let preview = prepare(&state, &session, incoming.clone(), ReplacementKind::Import).unwrap();
        let applied = Cell::new(false);
        let outcome = replace_prepared(&state, &session, preview.token, |configuration| {
            assert!(configuration.modifier_rules.is_empty());
            assert!(configuration.hotkeys.is_empty());
            assert_eq!(state.db.configuration_backup().unwrap(), incoming);
            applied.set(true);
            vec!["globalShortcuts", "capsLockRemap"]
        })
        .unwrap();
        assert!(applied.get());
        assert_eq!(
            outcome.apply_warnings,
            vec!["globalShortcuts", "capsLockRemap"]
        );
        assert_eq!(
            state.db.read_previous_configuration().unwrap(),
            Some(original)
        );
        let completed = session.0.lock_safe().last_applied.clone().unwrap();
        assert_eq!(completed.token, preview.token);
        assert_eq!(completed.outcome.settings, outcome.settings);
        assert_eq!(completed.outcome.apply_warnings, outcome.apply_warnings);
        assert!(session.0.lock_safe().pending.is_none());
        assert!(replace_prepared(&state, &session, preview.token, |_| Vec::new()).is_err());
    }

    #[test]
    fn restore_is_confirmed_against_later_edits_and_consumed_only_on_success() {
        let state = state();
        let original = current_configuration(&state).unwrap();
        let mut incoming = original.clone();
        incoming.hotkeys.clear();
        let session = BackupSession::default();
        let preview = prepare(&state, &session, incoming, ReplacementKind::Import).unwrap();
        replace_prepared(&state, &session, preview.token, |_| Vec::new()).unwrap();
        state.db.save_window_placement(&placement()).unwrap();
        let previous = state.db.read_previous_configuration().unwrap().unwrap();
        let restore = prepare(&state, &session, previous, ReplacementKind::Restore).unwrap();
        assert_eq!(restore.current.window_placements, 1);
        assert_eq!(restore.incoming.window_placements, 0);
        state.push_placement_edit(crate::state::PlacementEdit {
            before: None,
            after: Some(placement()),
        });
        replace_prepared(&state, &session, restore.token, |_| Vec::new()).unwrap();
        assert_eq!(state.db.configuration_backup().unwrap(), original);
        assert!(state.db.read_previous_configuration().unwrap().is_none());
        assert!(state.pop_placement_edit().is_none());
    }

    #[test]
    fn restore_retains_the_exact_noncanonical_stored_identity() {
        let state = state();
        let mut rule = state.db.list_modifier_rules().unwrap().remove(0);
        state.db.delete_modifier_rule(&rule.id).unwrap();
        rule.id = "  saved-rule  ".into();
        rule.label = "  Custom Caps Lock  ".into();
        state.db.insert_modifier_rule(&rule).unwrap();
        let original = current_configuration(&state).unwrap();
        let mut incoming = original.clone();
        incoming.modifier_rules.clear();
        let session = BackupSession::default();
        let preview = prepare(&state, &session, incoming, ReplacementKind::Import).unwrap();
        replace_prepared(&state, &session, preview.token, |_| Vec::new()).unwrap();
        let previous = state.db.read_previous_configuration().unwrap().unwrap();
        let restore = prepare(&state, &session, previous, ReplacementKind::Restore).unwrap();
        replace_prepared(&state, &session, restore.token, |_| Vec::new()).unwrap();
        assert_eq!(state.db.configuration_backup().unwrap(), original);
    }

    #[test]
    fn export_creates_distinct_owner_only_files_with_complete_contents() {
        let directory = tempfile::tempdir().unwrap();
        let first = write_export(directory.path(), b"{\"first\":true}").unwrap();
        let second = write_export(directory.path(), b"{\"second\":true}").unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(
            std::fs::read_to_string(&first.path).unwrap(),
            "{\"first\":true}"
        );
        assert_eq!(
            std::fs::read_to_string(&second.path).unwrap(),
            "{\"second\":true}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(first.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let blocked = directory.path().join("not-a-directory");
        std::fs::write(&blocked, "unchanged").unwrap();
        assert!(write_export(&blocked, b"{}").is_err());
        assert_eq!(std::fs::read_to_string(blocked).unwrap(), "unchanged");
    }
}
