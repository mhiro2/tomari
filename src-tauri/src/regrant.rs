//! Detecting permissions silently revoked by an app update.
//!
//! Tomari is ad-hoc signed for now, so macOS drops its Accessibility and
//! Input Monitoring grants on every update — silently, with no error anywhere
//! (see `docs/permissions.md`). To turn "it worked yesterday" into an
//! actionable prompt, each launch compares the current permission state
//! against a snapshot persisted by the previous run: a permission that was
//! granted then, is missing now, *and* an app version that changed in between
//! reads as an update-caused revocation. Same version with a missing grant
//! means the user revoked it by hand, which deliberately does not qualify —
//! nagging a deliberate choice would be worse than staying quiet.
//!
//! This is a heuristic, not proof: a manual revocation followed by an update
//! before the next launch also matches (the correlation is real, the cause is
//! not). The prompt's wording stays correlational ("went missing after the
//! update") for that reason. And the release that *introduces* the snapshot
//! cannot detect its own update's revocation — there is nothing yet to
//! compare against — so detection only starts with the following update.
//!
//! Everything here is best-effort UX: a snapshot that fails to read or write
//! is ignored, never surfaced as an error. Once Tomari ships with a stable
//! Developer ID signature this detection loses its trigger and can be removed.

use serde::{Deserialize, Serialize};
use tomari_core::Database;

/// The `meta`-table key the snapshot is stored under.
const META_KEY: &str = "permission_snapshot";

/// The permission state one run leaves behind for the next one to compare
/// against, stored as one JSON line in the `meta` table.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSnapshot {
    pub accessibility: bool,
    pub input_monitoring: bool,
    pub app_version: String,
}

/// Whether the current launch looks like an update revoked permissions: the
/// stored snapshot has a grant the current state lacks, and the app version
/// changed since the snapshot was taken. `current` is `(accessibility,
/// input_monitoring)`, matching `tray::permission_state`. No snapshot (a
/// first run, or a pre-snapshot database) is never a regrant.
pub fn is_update_regrant(
    prev: Option<&PermissionSnapshot>,
    current: (bool, bool),
    current_version: &str,
) -> bool {
    let Some(prev) = prev else {
        return false;
    };
    prev.app_version != current_version
        && ((prev.accessibility && !current.0) || (prev.input_monitoring && !current.1))
}

/// The previously stored snapshot, or `None` when there is none or it cannot
/// be read (a failure is logged and swallowed — this is UX support, not state
/// the app depends on).
pub fn load_snapshot(db: &Database) -> Option<PermissionSnapshot> {
    let raw = db
        .get_meta(META_KEY)
        .map_err(|e| tracing::warn!(error = %e, "could not read the permission snapshot"))
        .ok()??;
    serde_json::from_str(&raw)
        .map_err(|e| tracing::warn!(error = %e, "stored permission snapshot does not decode"))
        .ok()
}

/// Persist the current permission state for the next launch to compare
/// against. Idempotent, so callers overwrite freely; a write failure is
/// logged and swallowed.
pub fn store_snapshot(db: &Database, current: (bool, bool), app_version: &str) {
    let snapshot = PermissionSnapshot {
        accessibility: current.0,
        input_monitoring: current.1,
        app_version: app_version.to_string(),
    };
    let json = match serde_json::to_string(&snapshot) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!(error = %e, "could not encode the permission snapshot");
            return;
        }
    };
    if let Err(e) = db.set_meta(META_KEY, &json) {
        tracing::warn!(error = %e, "could not store the permission snapshot");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(accessibility: bool, input_monitoring: bool, version: &str) -> PermissionSnapshot {
        PermissionSnapshot {
            accessibility,
            input_monitoring,
            app_version: version.into(),
        }
    }

    #[test]
    fn a_grant_lost_across_versions_is_a_regrant() {
        let prev = snapshot(true, true, "0.0.12");
        assert!(is_update_regrant(Some(&prev), (false, true), "0.0.13"));
        assert!(is_update_regrant(Some(&prev), (true, false), "0.0.13"));
        assert!(is_update_regrant(Some(&prev), (false, false), "0.0.13"));
    }

    #[test]
    fn the_same_version_means_the_user_revoked_by_hand() {
        let prev = snapshot(true, true, "0.0.12");
        assert!(!is_update_regrant(Some(&prev), (false, false), "0.0.12"));
    }

    #[test]
    fn nothing_missing_is_not_a_regrant_whatever_the_version() {
        let prev = snapshot(true, true, "0.0.12");
        assert!(!is_update_regrant(Some(&prev), (true, true), "0.0.13"));
    }

    #[test]
    fn a_permission_that_was_never_granted_does_not_count() {
        // Only losing a grant the snapshot had qualifies; still-missing
        // permissions across an update are the ordinary not-set-up state.
        let prev = snapshot(false, false, "0.0.12");
        assert!(!is_update_regrant(Some(&prev), (false, false), "0.0.13"));
        let prev = snapshot(false, true, "0.0.12");
        assert!(!is_update_regrant(Some(&prev), (false, true), "0.0.13"));
    }

    #[test]
    fn no_snapshot_is_never_a_regrant() {
        assert!(!is_update_regrant(None, (false, false), "0.0.13"));
    }

    #[test]
    fn snapshot_round_trips_through_the_meta_table() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(load_snapshot(&db), None, "empty database has no snapshot");

        store_snapshot(&db, (true, false), "0.0.12");
        assert_eq!(load_snapshot(&db), Some(snapshot(true, false, "0.0.12")));

        // Overwriting is idempotent — the latest state wins.
        store_snapshot(&db, (true, true), "0.0.13");
        assert_eq!(load_snapshot(&db), Some(snapshot(true, true, "0.0.13")));
    }

    #[test]
    fn snapshot_json_uses_the_documented_camel_case_shape() {
        // The stored value is a small public contract with future versions of
        // the app (an old snapshot must keep decoding), so pin its exact shape.
        let json = serde_json::to_string(&snapshot(true, true, "0.0.12")).unwrap();
        assert_eq!(
            json,
            r#"{"accessibility":true,"inputMonitoring":true,"appVersion":"0.0.12"}"#
        );
    }

    #[test]
    fn an_undecodable_stored_snapshot_reads_as_none() {
        let db = Database::open_in_memory().unwrap();
        db.set_meta("permission_snapshot", "not json").unwrap();
        assert_eq!(load_snapshot(&db), None);
    }
}
