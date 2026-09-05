//! Sanitized runtime health snapshots and support-bundle export.
//!
//! The boundary is intentionally narrow: no key or pointer events are stored,
//! Menu Bar health comes only from cached runtime flags (never an Accessibility
//! inventory scan), and persisted labels, accelerators, process details, paths,
//! and error strings are omitted entirely.

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::capsmap::CapsDiagnostics;
use crate::keepawake::{self, KeepAwakeStatus};
use crate::state::AppState;

const SUPPORT_BUNDLE_FORMAT_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSnapshot {
    pub generated_at_ms: u64,
    pub app: AppDiagnostic,
    pub permissions: PermissionDiagnostics,
    pub taps: Vec<TapDiagnostic>,
    pub caps_lock: CapsDiagnostics,
    pub shortcuts: ShortcutDiagnostics,
    pub menu_bar: MenuBarDiagnostics,
    pub keep_awake: KeepAwakeDiagnostics,
    pub database: Option<tomari_core::DatabaseHealth>,
    pub updater: UpdaterDiagnostics,
    pub privacy: PrivacyDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDiagnostic {
    pub version: String,
    pub os: &'static str,
    pub architecture: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDiagnostics {
    pub accessibility: bool,
    pub input_monitoring: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TapKind {
    Keyboard,
    DragToSnap,
    DragToMove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TapDiagnostic {
    pub kind: TapKind,
    pub enabled: bool,
    #[serde(flatten)]
    pub health: crate::tap::TapHealthSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutDiagnostics {
    pub enabled: bool,
    pub registration_incomplete: bool,
    pub registered_count: usize,
    pub invalid_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuBarDiagnostics {
    pub enabled: bool,
    pub supported: bool,
    pub permission_granted: bool,
    pub divider_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwakeDiagnostics {
    pub active: bool,
    pub phase: keepawake::KeepAwakePhase,
    pub marker_present: bool,
    pub kernel_sleep_disabled: Option<bool>,
    pub owns_lid_close: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterDiagnostics {
    pub signature_configured: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyDiagnostics {
    pub raw_input_included: bool,
    pub accessibility_labels_included: bool,
    pub process_details_included: bool,
    pub filesystem_paths_included: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundleExport {
    pub path: String,
    pub generated_at_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportBundle<'a> {
    format_version: u8,
    diagnostics: &'a DiagnosticsSnapshot,
}

pub fn collect(app: &AppHandle, state: &AppState) -> DiagnosticsSnapshot {
    collect_consistently(
        || state.configuration_snapshot(),
        || state.configuration_revision(),
        |settings| collect_for_settings(app, state, settings),
    )
}

fn collect_for_settings(
    app: &AppHandle,
    state: &AppState,
    settings: &tomari_core::AppSettings,
) -> DiagnosticsSnapshot {
    let configuration_warnings = state.configuration_warnings.snapshot();
    let keep_awake = keepawake::status(state);
    let database = state.db.health().ok();

    DiagnosticsSnapshot {
        generated_at_ms: unix_time_ms(),
        app: AppDiagnostic {
            version: app.package_info().version.to_string(),
            os: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
        },
        permissions: PermissionDiagnostics {
            accessibility: state.windows.permission_granted(),
            input_monitoring: input_monitoring_granted(),
        },
        taps: tap_diagnostics(settings),
        caps_lock: caps_diagnostics(),
        shortcuts: ShortcutDiagnostics {
            enabled: settings.keyboard_tap_enabled(),
            registration_incomplete: state.shortcut_registration_incomplete(),
            registered_count: state.registered_shortcut_count(),
            invalid_count: configuration_warnings.invalid_hotkeys.len(),
        },
        menu_bar: menu_bar_diagnostics(settings.menu_bar_tidy_enabled, state),
        keep_awake: keep_awake_diagnostics(keep_awake),
        database,
        updater: UpdaterDiagnostics {
            signature_configured: updater_signature_configured(app),
        },
        privacy: PrivacyDiagnostics {
            raw_input_included: false,
            accessibility_labels_included: false,
            process_details_included: false,
            filesystem_paths_included: false,
        },
    }
}

fn collect_consistently<S, T>(
    mut settings_snapshot: impl FnMut() -> (u64, S),
    mut current_revision: impl FnMut() -> u64,
    mut collect: impl FnMut(&S) -> T,
) -> T {
    let (revision, settings) = settings_snapshot();
    let result = collect(&settings);
    if current_revision() == revision {
        return result;
    }

    // A settings save raced the first probe. Retry once from a fresh snapshot;
    // diagnostics are best-effort and must never loop on continuous changes.
    let (_, settings) = settings_snapshot();
    collect(&settings)
}

pub fn export(app: &AppHandle, state: &AppState) -> Result<SupportBundleExport, String> {
    let snapshot = collect(app, state);
    let bytes = serde_json::to_vec_pretty(&SupportBundle {
        format_version: SUPPORT_BUNDLE_FORMAT_VERSION,
        diagnostics: &snapshot,
    })
    .map_err(|error| format!("could not encode the support bundle: {error}"))?;

    let directory = app
        .state::<tomari_core::AppPaths>()
        .data_dir
        .join("support");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("could not create the support directory: {error}"))?;
    let filename = format!("tomari-support-{}.json", snapshot.generated_at_ms);
    let path = directory.join(filename);
    let temporary = directory.join(format!(".tomari-support-{}.tmp", snapshot.generated_at_ms));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("could not create the support bundle: {error}"))?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("could not write the support bundle: {error}"));
    }
    std::fs::rename(&temporary, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("could not finish the support bundle: {error}")
    })?;

    Ok(SupportBundleExport {
        path: path.to_string_lossy().into_owned(),
        generated_at_ms: snapshot.generated_at_ms,
    })
}

fn menu_bar_diagnostics(enabled: bool, state: &AppState) -> MenuBarDiagnostics {
    MenuBarDiagnostics {
        enabled,
        supported: cfg!(target_os = "macos"),
        permission_granted: state.windows.permission_granted(),
        divider_available: enabled && crate::menubar::diagnostics_divider_available(),
    }
}

fn keep_awake_diagnostics(status: KeepAwakeStatus) -> KeepAwakeDiagnostics {
    KeepAwakeDiagnostics {
        active: status.active,
        phase: status.phase,
        marker_present: keepawake::diagnostics_marker_present(),
        kernel_sleep_disabled: status.kernel_sleep_disabled,
        owns_lid_close: status.owns_lid_close,
    }
}

fn updater_signature_configured(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
}

#[cfg(target_os = "macos")]
fn input_monitoring_granted() -> bool {
    crate::eventtap::input_monitoring_granted()
}

#[cfg(not(target_os = "macos"))]
fn input_monitoring_granted() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn caps_diagnostics() -> CapsDiagnostics {
    crate::capsmap::diagnostics()
}

#[cfg(not(target_os = "macos"))]
fn caps_diagnostics() -> CapsDiagnostics {
    CapsDiagnostics {
        ownership: crate::capsmap::CapsOwnership::Unowned,
        mapping_active: false,
        reconciled: true,
    }
}

#[cfg(target_os = "macos")]
fn tap_diagnostics(settings: &tomari_core::AppSettings) -> Vec<TapDiagnostic> {
    let tap = |kind, enabled, health: crate::tap::TapHealthSnapshot| TapDiagnostic {
        kind,
        enabled,
        health,
    };
    vec![
        tap(
            TapKind::Keyboard,
            settings.keyboard_tap_enabled(),
            crate::eventtap::health_snapshot(),
        ),
        tap(
            TapKind::DragToSnap,
            settings.drag_to_snap_tap_enabled(),
            crate::drag_to_snap::health_snapshot(),
        ),
        tap(
            TapKind::DragToMove,
            settings.drag_to_move_tap_enabled(),
            crate::drag_to_move::health_snapshot(),
        ),
    ]
}

#[cfg(not(target_os = "macos"))]
fn tap_diagnostics(settings: &tomari_core::AppSettings) -> Vec<TapDiagnostic> {
    let _ = settings;
    Vec::new()
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_retries_when_configuration_changes_during_a_probe() {
        let snapshots = std::cell::RefCell::new(vec![(1, 1), (2, 2)].into_iter());
        let attempts = std::cell::RefCell::new(Vec::new());

        let result = collect_consistently(
            || snapshots.borrow_mut().next().unwrap(),
            || 2,
            |settings| {
                attempts.borrow_mut().push(*settings);
                *settings * 10
            },
        );

        assert_eq!(result, 20);
        assert_eq!(*attempts.borrow(), vec![1, 2]);
    }

    #[test]
    fn collection_is_bounded_when_configuration_keeps_changing() {
        let snapshots = std::cell::RefCell::new(vec![(1, 1), (2, 2)].into_iter());
        let attempts = std::cell::RefCell::new(0);

        let result = collect_consistently(
            || snapshots.borrow_mut().next().unwrap(),
            || u64::MAX,
            |settings| {
                *attempts.borrow_mut() += 1;
                *settings
            },
        );

        assert_eq!(result, 2);
        assert_eq!(*attempts.borrow(), 2);
    }

    #[test]
    fn serialized_snapshot_contains_only_sanitized_diagnostics() {
        let snapshot = DiagnosticsSnapshot {
            generated_at_ms: 1,
            app: AppDiagnostic {
                version: "1.2.3".into(),
                os: "macos",
                architecture: "aarch64",
            },
            permissions: PermissionDiagnostics {
                accessibility: true,
                input_monitoring: true,
            },
            taps: vec![TapDiagnostic {
                kind: TapKind::Keyboard,
                enabled: true,
                health: crate::tap::TapHealthSnapshot {
                    state: crate::tap::TapHealth::Healthy,
                    restart_count: 2,
                    disable_count: 1,
                    recovery_count: 1,
                },
            }],
            caps_lock: CapsDiagnostics {
                ownership: crate::capsmap::CapsOwnership::Held,
                mapping_active: true,
                reconciled: true,
            },
            shortcuts: ShortcutDiagnostics {
                enabled: true,
                registration_incomplete: false,
                registered_count: 3,
                invalid_count: 0,
            },
            menu_bar: MenuBarDiagnostics {
                enabled: true,
                supported: true,
                permission_granted: true,
                divider_available: true,
            },
            keep_awake: KeepAwakeDiagnostics {
                active: false,
                phase: keepawake::KeepAwakePhase::Off,
                marker_present: false,
                kernel_sleep_disabled: Some(false),
                owns_lid_close: false,
            },
            database: Some(tomari_core::DatabaseHealth {
                integrity_ok: true,
                schema_version: 3,
                latest_schema_version: 3,
            }),
            updater: UpdaterDiagnostics {
                signature_configured: true,
            },
            privacy: PrivacyDiagnostics {
                raw_input_included: false,
                accessibility_labels_included: false,
                process_details_included: false,
                filesystem_paths_included: false,
            },
        };
        let json = serde_json::to_string(&SupportBundle {
            format_version: SUPPORT_BUNDLE_FORMAT_VERSION,
            diagnostics: &snapshot,
        })
        .unwrap();

        for forbidden in [
            "accelerator",
            "bundleId",
            "dataDir",
            "label",
            "ownerName",
            "path",
            "pid",
            "processName",
        ] {
            assert!(
                !json.contains(forbidden),
                "support bundle leaked {forbidden}"
            );
        }
        assert!(json.contains("\"rawInputIncluded\":false"));
        assert!(json.contains("\"accessibilityLabelsIncluded\":false"));
        assert!(json.contains("\"kind\":\"keyboard\""));
        assert!(json.contains("\"state\":\"healthy\""));
        assert!(json.contains("\"phase\":\"off\""));
    }
}
