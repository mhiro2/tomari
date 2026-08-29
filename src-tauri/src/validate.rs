//! Validation and repair for settings and actions crossing the Tauri command
//! boundary. Persisted keyboard records use `tomari_keyboard::validation`
//! directly at their collection load and save boundaries; this module retains
//! the settings bounds and platform-specific checks needed by Tauri commands.

use tomari_core::{AppAction, AppSettings};
#[cfg(any(test, target_os = "macos"))]
use tomari_keyboard::accelerator;
use tomari_keyboard::validation as keyboard_validation;

use crate::error::CmdError;

/// Longest menu-bar auto-collapse delay accepted: one hour. The panel offers
/// seconds-scale choices; anything beyond this is a value no UI produced, and a
/// timer armed for `u32::MAX` seconds would be a deadline 136 years out.
pub const MAX_AUTO_COLLAPSE_SECS: u32 = 3600;

/// Don't trust the frontend's `AppSettings`: bound every numeric field that
/// drives a timer or a resource. `0` for the auto-collapse delay means off.
pub fn sanitize_settings(settings: AppSettings) -> Result<AppSettings, CmdError> {
    if settings.menu_bar_auto_collapse_secs > MAX_AUTO_COLLAPSE_SECS {
        return Err(CmdError::other(format!(
            "menuBarAutoCollapseSecs must be 0 (off) or at most {MAX_AUTO_COLLAPSE_SECS} seconds"
        )));
    }
    Ok(settings)
}

/// Bring settings read back from the database into range, for launch. A save
/// is *rejected* when out of range ([`sanitize_settings`]); a stored row that
/// is out of range anyway — written by an older build, or edited by hand — is
/// repaired to the default instead, with a warning, so the live settings the
/// engines run from are always ones this build would have accepted.
pub fn repair_settings(mut settings: AppSettings) -> AppSettings {
    if settings.menu_bar_auto_collapse_secs > MAX_AUTO_COLLAPSE_SECS {
        tracing::warn!(
            stored = settings.menu_bar_auto_collapse_secs,
            max = MAX_AUTO_COLLAPSE_SECS,
            "stored menu bar auto-collapse delay is out of range; using the default"
        );
        settings.menu_bar_auto_collapse_secs = AppSettings::default().menu_bar_auto_collapse_secs;
    }
    settings
}

/// Validate an action before Tauri dispatches it from `run_action` or a
/// `tomari://` URL.
///
/// Most variants are closed enums and need nothing; `SendKeystroke` carries a
/// free-form accelerator, which must parse and — on macOS — map to a keycode
/// the synthesizer can emit. The parser's key set is kept in lockstep with
/// `keysend::keycode_for` (see its coverage test), so the keycode check guards
/// against future drift rather than a known gap. The accelerator is returned
/// in its canonical spelling.
pub fn sanitize_app_action(action: AppAction) -> Result<AppAction, CmdError> {
    let action = keyboard_validation::validate_app_action(action)
        .map_err(|error| CmdError::other(error.to_string()))?;
    #[cfg(target_os = "macos")]
    if let AppAction::SendKeystroke(accelerator) = &action {
        let parsed =
            accelerator::parse(accelerator).map_err(|error| CmdError::other(error.to_string()))?;
        if crate::keysend::keycode_for(&parsed.key).is_none() {
            return Err(CmdError::other(format!(
                "the key \"{}\" cannot be sent as a keystroke",
                parsed.key
            )));
        }
    }
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_action_boundary_canonicalizes_and_rejects_keystrokes() {
        let saved = sanitize_app_action(AppAction::SendKeystroke("cmd+shift+4".into()))
            .expect("valid keystroke");
        assert_eq!(
            saved,
            AppAction::SendKeystroke(accelerator::parse("cmd+shift+4").unwrap().to_canonical())
        );
        assert!(sanitize_app_action(AppAction::SendKeystroke("Frobnicate".into())).is_err());

        // Closed-enum actions pass through untouched.
        assert_eq!(
            sanitize_app_action(AppAction::UndoWindow).unwrap(),
            AppAction::UndoWindow
        );
    }

    #[test]
    fn auto_collapse_delay_is_bounded() {
        let with = |secs: u32| AppSettings {
            menu_bar_auto_collapse_secs: secs,
            ..Default::default()
        };
        assert!(sanitize_settings(with(0)).is_ok());
        assert!(sanitize_settings(with(MAX_AUTO_COLLAPSE_SECS)).is_ok());
        assert!(sanitize_settings(with(MAX_AUTO_COLLAPSE_SECS + 1)).is_err());
        assert!(sanitize_settings(with(u32::MAX)).is_err());
    }

    #[test]
    fn a_stored_out_of_range_delay_is_repaired_to_the_default_at_launch() {
        let with = |secs: u32| AppSettings {
            menu_bar_auto_collapse_secs: secs,
            ..Default::default()
        };
        assert_eq!(repair_settings(with(30)).menu_bar_auto_collapse_secs, 30);
        assert_eq!(
            repair_settings(with(u32::MAX)).menu_bar_auto_collapse_secs,
            AppSettings::default().menu_bar_auto_collapse_secs
        );
    }
}
