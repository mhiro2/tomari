//! Whole-configuration export and import for the settings backup feature.
//!
//! Export writes a single, diffable JSON file ([`tomari_core::ConfigSnapshot`]);
//! import reads one back, strictly validates it, and *replaces* the entire
//! configuration with it. The flow is deliberately conservative:
//!
//! - **All-or-nothing validation.** Every entry is checked up front and *all*
//!   problems are reported together; a single bad entry rejects the whole file
//!   and changes nothing. (The lenient row-skipping the database does on load
//!   exists to keep the app *starting*; an explicit import has no such excuse to
//!   silently drop a user's binding.) Validation covers the envelope shape
//!   (`deny_unknown_fields`, mandatory collections), unique ids and parseable
//!   accelerators. It does *not* recurse into every value: an unknown field
//!   *inside* an entry falls back to its default rather than erroring, matching
//!   how the database loads rows, and embedded action payloads (a
//!   `SendKeystroke` string) are left to the same runtime tolerance as the
//!   interactive save path. Tightening that would mean a parallel strict-DTO
//!   layer and is left for a future format version.
//! - **Atomic apply.** The database is replaced in one transaction, and the
//!   live engines/shortcuts are rebuilt from the validated snapshot afterwards.
//!   The pre-import database is copied to a timestamped backup first, so the
//!   replace is always recoverable.
//! - **Serialized with every other config mutation** via
//!   [`AppState::lock_config_mutation`], so an interactive save cannot interleave
//!   and desync the in-memory engines from disk.
//!
//! Dialogs and file I/O run entirely in Rust (the frontend invokes the commands
//! with no arguments), so a path never crosses the IPC boundary.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use tomari_core::{AppPaths, ConfigSnapshot};

use crate::shortcuts;
use crate::state::AppState;

/// Refuse to read an import file larger than this. A real config is a few KiB;
/// anything in megabytes is a mistake or a hostile input, not worth parsing.
const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024;

/// How many `pre-import-*.sqlite` backups to keep before pruning the oldest.
const BACKUP_KEEP: usize = 10;

/// The default file name offered in the export save dialog.
const DEFAULT_EXPORT_NAME: &str = "tomari-config.json";

/// The result of an export, surfaced to the frontend.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ExportOutcome {
    /// The user dismissed the save dialog.
    Cancelled,
    /// The configuration was written to `path`. `omitted` counts stored rows
    /// that could not be read and were left out (a lossy backup warning).
    Saved { path: String, omitted: usize },
}

/// The result of an import, surfaced to the frontend.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ImportOutcome {
    /// The user dismissed the open dialog.
    Cancelled,
    /// The file failed validation; nothing was changed. Every problem found is
    /// listed so the user can fix the file in one pass.
    Rejected { errors: Vec<String> },
    /// The configuration was replaced. The report summarizes what was applied.
    Applied { report: ImportReport },
}

/// A summary of a successful import.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub hotkeys: usize,
    pub modifier_rules: usize,
    /// Non-fatal notes: settings clamped into range, device-local fields left
    /// untouched, and so on.
    pub warnings: Vec<String>,
    /// Hotkeys whose accelerator parsed but could not be registered on this
    /// machine right now (e.g. another app holds the combo). The binding is
    /// still saved and behaves like any other once the conflict clears.
    pub registration_failures: Vec<String>,
    /// Where the pre-import configuration was backed up.
    pub backup_path: String,
}

/// Run an export end to end: read a consistent snapshot, ask where to save it,
/// and write it atomically. Returns the outcome for the frontend.
pub fn run_export(app: &AppHandle) -> Result<ExportOutcome, String> {
    let state = app.state::<AppState>();

    // Read the whole configuration under the config lock so it is a single,
    // consistent point in time, then release the lock before the dialog (which
    // can stay open arbitrarily long) so interactive saves are not blocked.
    let (json, omitted) = {
        let _guard = state.lock_config_mutation();
        let exported = state.db.export_snapshot().map_err(|e| e.to_string())?;
        (
            exported
                .snapshot
                .to_pretty_json()
                .map_err(|e| e.to_string())?,
            exported.omitted,
        )
    };

    let Some(path) = app
        .dialog()
        .file()
        .add_filter("Tomari config", &["json"])
        .set_file_name(DEFAULT_EXPORT_NAME)
        .blocking_save_file()
    else {
        return Ok(ExportOutcome::Cancelled);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;

    atomic_write(&path, &json).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    if omitted > 0 {
        tracing::warn!(omitted, "export left out rows that could not be read");
    }
    Ok(ExportOutcome::Saved {
        path: path.display().to_string(),
        omitted,
    })
}

/// Run an import end to end: ask for a file, validate it strictly, and — only
/// if it is wholly valid — back up the current configuration and replace it.
pub fn run_import(app: &AppHandle) -> Result<ImportOutcome, String> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("Tomari config", &["json"])
        .blocking_pick_file()
    else {
        return Ok(ImportOutcome::Cancelled);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;

    let raw = read_capped(&path)?;
    let snapshot = ConfigSnapshot::from_json(&raw).map_err(|e| e.to_string())?;

    // Strict validation: collect every problem so the user fixes the file once.
    let validated = match validate(snapshot) {
        Ok(v) => v,
        Err(errors) => return Ok(ImportOutcome::Rejected { errors }),
    };

    let state = app.state::<AppState>();
    let report = apply(app, state.inner(), validated)?;
    Ok(ImportOutcome::Applied { report })
}

/// A snapshot that has passed validation, with any non-fatal adjustments noted.
#[cfg_attr(test, derive(Debug))]
struct Validated {
    snapshot: ConfigSnapshot,
    warnings: Vec<String>,
}

/// Validate every entry strictly. Returns the (possibly normalized) snapshot
/// plus warnings, or the full list of errors when anything is unusable.
fn validate(mut snapshot: ConfigSnapshot) -> Result<Validated, Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Duplicate ids would silently collapse on import (the upsert keeps the
    // last), so reject them rather than lose an entry.
    check_unique_ids(
        snapshot.hotkeys.iter().map(|h| &h.id),
        "hotkey",
        &mut errors,
    );
    check_unique_ids(
        snapshot.modifier_rules.iter().map(|r| &r.id),
        "modifier rule",
        &mut errors,
    );

    // Accelerators must parse; an unparseable one is a malformed file, not a
    // runtime conflict.
    for hk in &snapshot.hotkeys {
        if let Err(e) = tomari_keyboard::accelerator::normalize(&hk.accelerator) {
            errors.push(format!(
                "hotkey \"{}\": invalid accelerator \"{}\": {e}",
                hk.label, hk.accelerator
            ));
        }
    }

    // Settings are clamped to valid ranges (the same sanitize the app runs on
    // every load), but unlike that silent path an import reports the change.
    let before = snapshot.settings.clone();
    snapshot.settings.sanitize();
    if snapshot.settings != before {
        warnings.push(
            "some settings were outside the allowed range and were adjusted \
             (hold threshold clamped)"
                .into(),
        );
    }

    if errors.is_empty() {
        Ok(Validated { snapshot, warnings })
    } else {
        Err(errors)
    }
}

/// Back up the current configuration, then replace it with the validated
/// snapshot and rebuild all live state from the database. Holds the
/// config-mutation lock for the whole operation.
fn apply(app: &AppHandle, state: &AppState, validated: Validated) -> Result<ImportReport, String> {
    let Validated {
        mut snapshot,
        mut warnings,
    } = validated;
    let _guard = state.lock_config_mutation();

    let previous = state.settings.lock().unwrap().clone();

    // Launch-at-login is device-local: the file records it for a faithful
    // backup, but importing must not flip another machine's login item. Keep
    // this machine's current value.
    if snapshot.settings.launch_at_login != previous.launch_at_login {
        warnings
            .push("launch-at-login is device-local and was left unchanged by this import".into());
        snapshot.settings.launch_at_login = previous.launch_at_login;
    }

    // Back up the current config first; abort the whole import if it fails, so
    // the replace is never irreversible.
    let backup_path = write_backup(state).map_err(|e| format!("could not write backup: {e}"))?;

    state
        .db
        .replace_with_snapshot(&snapshot)
        .map_err(|e| e.to_string())?;

    // Rebuild live state from the *validated snapshot we just wrote*, never from
    // a fresh DB read: after the commit there is nothing left that may fail and
    // surface as an error while the configuration is already replaced. The
    // snapshot is the exact data now on disk. These updates are infallible and
    // idempotent.
    let next = snapshot.settings.clone();
    state
        .engine
        .lock()
        .unwrap()
        .set_hold_threshold(next.hold_threshold_ms);
    state
        .engine
        .lock()
        .unwrap()
        .set_rules(snapshot.modifier_rules.clone());

    if previous.show_in_menu_bar != next.show_in_menu_bar {
        crate::tray::set_visible(app, next.show_in_menu_bar);
    }
    let language_changed = previous.language != next.language;
    *state.settings.lock().unwrap() = next;

    if language_changed {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
    }

    // The modifier rules almost certainly changed, so restart the taps
    // unconditionally — an import is a deliberate, rare action and a brief drop
    // in key monitoring is acceptable.
    #[cfg(target_os = "macos")]
    {
        crate::eventtap::restart(app);
        crate::drag_to_snap::restart(app);
    }

    // Register the imported hotkeys. A parse-valid accelerator can still fail to
    // register (another app holds it); report those as warnings rather than
    // failing the import, consistent with startup registration.
    let registration_failures = match shortcuts::register_all(app, state) {
        Ok(failures) => failures
            .into_iter()
            .map(|f| format!("{}: {}", f.accelerator, f.error))
            .collect(),
        Err(e) => {
            warnings.push(format!("re-registering shortcuts failed: {e}"));
            Vec::new()
        }
    };

    Ok(ImportReport {
        hotkeys: snapshot.hotkeys.len(),
        modifier_rules: snapshot.modifier_rules.len(),
        warnings,
        registration_failures,
        backup_path: backup_path.display().to_string(),
    })
}

/// Push every id from an iterator through a set, recording each duplicate.
fn check_unique_ids<'a>(
    ids: impl Iterator<Item = &'a String>,
    kind: &str,
    errors: &mut Vec<String>,
) {
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            errors.push(format!("duplicate {kind} id \"{id}\""));
        }
    }
}

/// Read a file, refusing anything implausibly large before loading it.
fn read_capped(path: &Path) -> Result<String, String> {
    let meta =
        std::fs::metadata(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    if meta.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "file is too large to be a Tomari config ({} bytes)",
            meta.len()
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))
}

/// Back up the current database to a timestamped file under the data directory,
/// pruning old backups. Returns the path written.
///
/// This is a complete copy of the SQLite database (`VACUUM INTO`), not a config
/// snapshot, so it preserves even rows the app can no longer read — the replace
/// is always fully recoverable. Restore it by quitting Tomari and putting the
/// file back as `tomari.sqlite`.
fn write_backup(state: &AppState) -> Result<PathBuf, String> {
    let paths = AppPaths::resolve().map_err(|e| e.to_string())?;
    let dir = paths.data_dir.join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = dir.join(format!(
        "pre-import-{}.sqlite",
        tomari_core::clock::now_millis()
    ));
    state.db.backup_to(&path).map_err(|e| e.to_string())?;
    prune_backups(&dir);
    Ok(path)
}

/// Keep only the newest [`BACKUP_KEEP`] `pre-import-*.sqlite` files.
fn prune_backups(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut backups: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("pre-import-") && n.ends_with(".sqlite"))
        })
        .collect();
    if backups.len() <= BACKUP_KEEP {
        return;
    }
    // The timestamp in the name sorts chronologically, so lexical order works.
    backups.sort();
    for old in &backups[..backups.len() - BACKUP_KEEP] {
        if let Err(e) = std::fs::remove_file(old) {
            tracing::warn!(error = %e, path = %old.display(), "could not prune old backup");
        }
    }
}

/// Write `contents` to `path` atomically: write a sibling temp file, then
/// rename it over the target so a crash mid-write never leaves a truncated file.
fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| std::io::Error::other("destination has no file name"))?;
    let tmp = path.with_file_name(format!(
        ".{file_name}.tmp-{}",
        tomari_core::clock::now_millis()
    ));
    std::fs::write(&tmp, contents)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomari_core::{AppAction, AppSettings, Hotkey};

    fn hotkey(id: &str, accelerator: &str) -> Hotkey {
        Hotkey {
            id: id.into(),
            label: id.into(),
            accelerator: accelerator.into(),
            action: AppAction::TogglePanel,
            enabled: true,
        }
    }

    fn snapshot(hotkeys: Vec<Hotkey>) -> ConfigSnapshot {
        ConfigSnapshot::new(AppSettings::default(), hotkeys, vec![])
    }

    #[test]
    fn valid_snapshot_passes() {
        let snap = snapshot(vec![hotkey("a", "Cmd+Shift+K")]);
        let validated = validate(snap).expect("should be valid");
        assert!(validated.warnings.is_empty());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let snap = snapshot(vec![hotkey("dup", "Cmd+1"), hotkey("dup", "Cmd+2")]);
        let errors = validate(snap).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("duplicate hotkey id")));
    }

    #[test]
    fn invalid_accelerator_is_rejected() {
        let snap = snapshot(vec![hotkey("a", "")]);
        let errors = validate(snap).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("invalid accelerator")));
    }

    #[test]
    fn all_problems_are_collected_at_once() {
        // A duplicate id and a bad accelerator in one file must surface together
        // rather than failing on the first.
        let snap = snapshot(vec![hotkey("x", ""), hotkey("x", "Cmd+2")]);
        let errors = validate(snap).unwrap_err();
        assert!(errors.len() >= 2, "expected several errors, got {errors:?}");
    }

    #[test]
    fn out_of_range_settings_are_clamped_with_a_warning() {
        let mut snap = snapshot(vec![]);
        snap.settings.hold_threshold_ms = 99_999;
        let validated = validate(snap).expect("settings are clamped, not rejected");
        assert!(!validated.warnings.is_empty());
        // Clamped to the top of the allowed range.
        assert_eq!(validated.snapshot.settings.hold_threshold_ms, 500);
    }
}
