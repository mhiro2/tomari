//! Server-side validation of user-editable records that cross the command
//! boundary. The frontend already guards these, but the boundary must not trust
//! it: a malformed accelerator could break global-shortcut registration, a bare
//! letter would shadow normal typing once registered globally, and an unbounded
//! id/label could be persisted unchecked.

use tomari_core::{AppAction, AppSettings, Hotkey, KeySide, ModifierKey, ModifierRule};
use tomari_keyboard::accelerator;

use crate::error::CmdError;

/// Upper bounds (in characters) on stored identifiers and labels — generous for
/// any real entry (a UUID-based id, a human label) while rejecting pathological
/// input.
const MAX_ID_LEN: usize = 128;
const MAX_LABEL_LEN: usize = 200;

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

/// Validate an action wherever one crosses the command boundary — a hotkey's
/// or a modifier rule's action on save, a `run_action` call, a `tomari://`
/// URL — so the same rules apply on every path and a broken action can neither
/// be persisted (to fail on every fire) nor run once.
///
/// Most variants are closed enums and need nothing; `SendKeystroke` carries a
/// free-form accelerator, which must parse and — on macOS — map to a keycode
/// the synthesizer can emit. The parser's key set is kept in lockstep with
/// `keysend::keycode_for` (see its coverage test), so the keycode check guards
/// against future drift rather than a known gap. The accelerator is returned
/// in its canonical spelling.
pub fn sanitize_app_action(action: AppAction) -> Result<AppAction, CmdError> {
    match action {
        AppAction::SendKeystroke(accel) => {
            let parsed = accelerator::parse(&accel)
                .map_err(|e| CmdError::other(format!("invalid keystroke: {e}")))?;
            #[cfg(target_os = "macos")]
            if crate::keysend::keycode_for(&parsed.key).is_none() {
                return Err(CmdError::other(format!(
                    "the key \"{}\" cannot be sent as a keystroke",
                    parsed.key
                )));
            }
            Ok(AppAction::SendKeystroke(parsed.to_canonical()))
        }
        other => Ok(other),
    }
}

/// Validate and canonicalize a hotkey before it is stored. Returns a sanitized
/// copy — trimmed id/label, normalized accelerator, validated action — or a
/// [`CmdError`] describing the first problem found.
pub fn sanitize_hotkey(hotkey: Hotkey) -> Result<Hotkey, CmdError> {
    let id = hotkey.id.trim();
    if id.is_empty() {
        return Err(CmdError::other("hotkey id must not be empty"));
    }
    if id.chars().count() > MAX_ID_LEN {
        return Err(CmdError::other(format!(
            "hotkey id is too long (max {MAX_ID_LEN} characters)"
        )));
    }

    let label = hotkey.label.trim();
    if label.is_empty() {
        return Err(CmdError::other("hotkey label must not be empty"));
    }
    if label.chars().count() > MAX_LABEL_LEN {
        return Err(CmdError::other(format!(
            "hotkey label is too long (max {MAX_LABEL_LEN} characters)"
        )));
    }

    // Parse here so an unregisterable string is rejected with a clear reason
    // rather than surfacing later as a misleading "conflict", and normalize to
    // the canonical spelling the rest of the app compares against.
    let parsed = accelerator::parse(&hotkey.accelerator)
        .map_err(|e| CmdError::other(format!("invalid shortcut: {e}")))?;

    // A global shortcut with no Ctrl/Alt/Cmd would shadow normal typing
    // system-wide (Shift alone is not enough); function keys may stand alone.
    // Mirrors the recorder's rule in `src/lib/recorder.ts`.
    if !(parsed.ctrl || parsed.alt || parsed.cmd || is_function_key(&parsed.key)) {
        return Err(CmdError::other(
            "a global shortcut needs ⌃, ⌥ or ⌘ — or a function key",
        ));
    }

    Ok(Hotkey {
        id: id.to_string(),
        label: label.to_string(),
        accelerator: parsed.to_canonical(),
        action: sanitize_app_action(hotkey.action)?,
        enabled: hotkey.enabled,
    })
}

/// Whether a canonical key token is a function key (`F1`..=`F20`, the range
/// the accelerator parser accepts).
fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F')
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| (1..=20).contains(&n))
}

/// Validate and canonicalize a modifier rule before it is stored. Like
/// [`sanitize_hotkey`], the command boundary must not trust the frontend:
/// modifier rules were previously persisted verbatim.
///
/// `existing` is the current stored rule set — used to reject a second rule for
/// the same modifier and side, which would be equally specific and let storage
/// order (DB label order) silently decide which one the engine honors.
///
/// Returns a sanitized copy — trimmed id/label — or a [`CmdError`] describing
/// the first problem found.
pub fn sanitize_modifier_rule(
    rule: ModifierRule,
    existing: &[ModifierRule],
) -> Result<ModifierRule, CmdError> {
    let id = rule.id.trim();
    if id.is_empty() {
        return Err(CmdError::other("rule id must not be empty"));
    }
    if id.chars().count() > MAX_ID_LEN {
        return Err(CmdError::other(format!(
            "rule id is too long (max {MAX_ID_LEN} characters)"
        )));
    }
    // The built-in ⌘ IME-toggle rules are assembled at runtime rather than
    // stored (see `defaults::command_ime_rules`); reusing one of their ids would
    // put two rules with the same id into the engine the moment the setting is
    // turned on.
    if is_reserved_rule_id(id) {
        return Err(CmdError::other(
            "this rule id is reserved for a built-in rule",
        ));
    }

    let label = rule.label.trim();
    if label.is_empty() {
        return Err(CmdError::other("rule label must not be empty"));
    }
    if label.chars().count() > MAX_LABEL_LEN {
        return Err(CmdError::other(format!(
            "rule label is too long (max {MAX_LABEL_LEN} characters)"
        )));
    }

    // `hyper` takes precedence over `remap_to` for the held role, so asking for
    // both is contradictory — reject it rather than silently drop the remap.
    if rule.hyper && rule.remap_to.is_some() {
        return Err(CmdError::other(
            "a rule cannot be both a Hyper key and a remap",
        ));
    }

    // The tap action goes through the same validator as every other action
    // that crosses the boundary: a SendKeystroke tap whose accelerator does
    // not parse, or names a key the synthesizer cannot emit, would save yet
    // fail every time the tap fired.
    let tap = sanitize_app_action(rule.tap.clone())?;

    // Two rules with the same modifier *and* side are equally specific, so the
    // engine picks whichever the DB lists first (by label) — renaming one could
    // silently flip which wins. Reject the collision. Updating a rule in place
    // (same id) is not a collision with itself.
    if let Some(other) = existing
        .iter()
        .find(|r| r.id != id && r.modifier == rule.modifier && r.side == rule.side)
    {
        return Err(CmdError::other(format!(
            "\"{}\" already handles {} ({})",
            other.label,
            rule.modifier.label(),
            side_label(rule.side),
        )));
    }

    // The left/right ⌘ tap slots are reserved for the built-in Command-key IME
    // toggle (`defaults::command_ime_rules`), which is assembled onto exactly
    // those slots whenever its setting is on. Reserving them unconditionally —
    // not only while the setting is on — is what keeps that toggle reliable: a
    // rule saved on the slot while the toggle was off would otherwise silently
    // shadow the built-in the moment it was turned back on (a same-specificity
    // tie the DB row wins, sorting ahead of the appended built-in), and neither
    // the save nor the settings toggle re-checks for it.
    if rule.modifier == ModifierKey::Command && matches!(rule.side, KeySide::Left | KeySide::Right)
    {
        return Err(CmdError::other(
            "left/right ⌘ is reserved for the Command-key IME toggle",
        ));
    }

    Ok(ModifierRule {
        id: id.to_string(),
        label: label.to_string(),
        modifier: rule.modifier,
        side: rule.side,
        remap_to: rule.remap_to,
        hyper: rule.hyper,
        tap,
        enabled: rule.enabled,
    })
}

/// Whether `id` is claimed by a built-in rule that is assembled at runtime
/// rather than stored (currently the left/right ⌘ IME-toggle pair).
fn is_reserved_rule_id(id: &str) -> bool {
    tomari_core::defaults::command_ime_rules()
        .iter()
        .any(|r| r.id == id)
}

/// A short label for a rule's side, for collision error messages.
fn side_label(side: KeySide) -> &'static str {
    match side {
        KeySide::Left => "left",
        KeySide::Right => "right",
        KeySide::Either => "either side",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_crossing_the_boundary_goes_through_the_same_validator() {
        // A hotkey whose action is a keystroke: the accelerator is validated and
        // canonicalised like a rule's, not stored verbatim.
        let hotkey = Hotkey {
            id: "k".into(),
            label: "K".into(),
            accelerator: "Ctrl+Alt+K".into(),
            action: AppAction::SendKeystroke("cmd+shift+4".into()),
            enabled: true,
        };
        let saved = sanitize_hotkey(hotkey.clone()).expect("valid");
        assert_eq!(
            saved.action,
            AppAction::SendKeystroke(accelerator::parse("cmd+shift+4").unwrap().to_canonical())
        );
        let broken = Hotkey {
            action: AppAction::SendKeystroke("Frobnicate".into()),
            ..hotkey
        };
        assert!(sanitize_hotkey(broken).is_err());
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
    use tomari_core::AppAction;

    fn hotkey(accelerator: &str) -> Hotkey {
        Hotkey {
            id: "hk-1".into(),
            label: "Test".into(),
            accelerator: accelerator.into(),
            action: AppAction::TogglePanel,
            enabled: true,
        }
    }

    #[test]
    fn normalizes_accelerator_to_canonical_form() {
        let out = sanitize_hotkey(hotkey("command+shift+r")).unwrap();
        assert_eq!(out.accelerator, "Shift+Cmd+R");
    }

    #[test]
    fn trims_id_and_label() {
        let mut hk = hotkey("Cmd+K");
        hk.id = "  hk-1  ".into();
        hk.label = "  Snap left  ".into();
        let out = sanitize_hotkey(hk).unwrap();
        assert_eq!(out.id, "hk-1");
        assert_eq!(out.label, "Snap left");
    }

    #[test]
    fn rejects_bare_letter_or_shift_only_chord() {
        // A lone letter, or one held only with Shift, would swallow plain typing
        // once registered as a global shortcut.
        assert!(sanitize_hotkey(hotkey("A")).is_err());
        assert!(sanitize_hotkey(hotkey("Shift+A")).is_err());
    }

    #[test]
    fn allows_bare_function_key_and_real_chords() {
        assert!(sanitize_hotkey(hotkey("F5")).is_ok());
        assert!(sanitize_hotkey(hotkey("Cmd+Alt+Left")).is_ok());
    }

    #[test]
    fn rejects_invalid_or_empty_accelerator() {
        assert!(sanitize_hotkey(hotkey("Cmd+Frobnicate")).is_err());
        assert!(sanitize_hotkey(hotkey("")).is_err());
    }

    #[test]
    fn rejects_empty_or_overlong_id_and_label() {
        let mut blank = hotkey("Cmd+K");
        blank.label = "   ".into();
        assert!(sanitize_hotkey(blank).is_err());

        let mut long_label = hotkey("Cmd+K");
        long_label.label = "x".repeat(MAX_LABEL_LEN + 1);
        assert!(sanitize_hotkey(long_label).is_err());

        let mut long_id = hotkey("Cmd+K");
        long_id.id = "x".repeat(MAX_ID_LEN + 1);
        assert!(sanitize_hotkey(long_id).is_err());
    }

    fn mod_rule(id: &str, modifier: ModifierKey, side: KeySide) -> ModifierRule {
        ModifierRule {
            id: id.into(),
            label: "Rule".into(),
            modifier,
            side,
            remap_to: None,
            hyper: false,
            tap: AppAction::NoOp,
            enabled: true,
        }
    }

    #[test]
    fn accepts_a_plain_rule_and_trims_id_and_label() {
        let mut rule = mod_rule("  mr-1  ", ModifierKey::Control, KeySide::Either);
        rule.label = "  Control tap  ".into();
        let out = sanitize_modifier_rule(rule, &[]).unwrap();
        assert_eq!(out.id, "mr-1");
        assert_eq!(out.label, "Control tap");
    }

    #[test]
    fn accepts_the_seeded_defaults() {
        // Toggling a seeded rule re-saves it verbatim, so the defaults must all
        // pass validation against themselves.
        let defaults = tomari_core::defaults::default_modifier_rules();
        for rule in &defaults {
            assert!(
                sanitize_modifier_rule(rule.clone(), &defaults).is_ok(),
                "seeded rule {} should validate",
                rule.id
            );
        }
    }

    #[test]
    fn rejects_empty_or_overlong_rule_id_and_label() {
        let mut blank_id = mod_rule("   ", ModifierKey::Control, KeySide::Either);
        blank_id.label = "ok".into();
        assert!(sanitize_modifier_rule(blank_id, &[]).is_err());

        let mut blank_label = mod_rule("mr-1", ModifierKey::Control, KeySide::Either);
        blank_label.label = "   ".into();
        assert!(sanitize_modifier_rule(blank_label, &[]).is_err());

        let mut long_id = mod_rule("mr-1", ModifierKey::Control, KeySide::Either);
        long_id.id = "x".repeat(MAX_ID_LEN + 1);
        assert!(sanitize_modifier_rule(long_id, &[]).is_err());

        let mut long_label = mod_rule("mr-1", ModifierKey::Control, KeySide::Either);
        long_label.label = "x".repeat(MAX_LABEL_LEN + 1);
        assert!(sanitize_modifier_rule(long_label, &[]).is_err());
    }

    #[test]
    fn rejects_built_in_ime_rule_ids() {
        for reserved in tomari_core::defaults::command_ime_rules() {
            // Use a non-⌘ modifier so the reserved-id check — not the reserved
            // ⌘ slot check — is what rejects it.
            let rule = mod_rule(&reserved.id, ModifierKey::Control, KeySide::Either);
            assert!(
                sanitize_modifier_rule(rule, &[]).is_err(),
                "reserved id {} should be rejected",
                reserved.id
            );
        }
    }

    #[test]
    fn rejects_hyper_combined_with_remap() {
        let mut rule = mod_rule("mr-1", ModifierKey::CapsLock, KeySide::Either);
        rule.hyper = true;
        rule.remap_to = Some(ModifierKey::Control);
        assert!(sanitize_modifier_rule(rule, &[]).is_err());
    }

    #[test]
    fn rejects_an_unparseable_tap_keystroke_but_accepts_a_valid_one() {
        let mut bad = mod_rule("mr-1", ModifierKey::Control, KeySide::Either);
        bad.tap = AppAction::SendKeystroke("Frobnicate".into());
        assert!(sanitize_modifier_rule(bad, &[]).is_err());

        let mut good = mod_rule("mr-2", ModifierKey::Control, KeySide::Either);
        good.tap = AppAction::SendKeystroke("Cmd+Shift+4".into());
        assert!(sanitize_modifier_rule(good, &[]).is_ok());
    }

    #[test]
    fn rejects_a_tap_keystroke_macos_cannot_send() {
        // macOS defines no virtual keycode past F20, so F21–F24 cannot be
        // synthesized. The accelerator parser rejects them outright (on every
        // platform), so such a tap can never be saved.
        let mut rule = mod_rule("mr-1", ModifierKey::Control, KeySide::Either);
        rule.tap = AppAction::SendKeystroke("F21".into());
        assert!(sanitize_modifier_rule(rule, &[]).is_err());
    }

    #[test]
    fn rejects_a_second_rule_for_the_same_modifier_and_side() {
        let existing = vec![mod_rule("mr-1", ModifierKey::Control, KeySide::Either)];
        // A different id but the same modifier+side is an equally-specific
        // collision.
        let dup = mod_rule("mr-2", ModifierKey::Control, KeySide::Either);
        assert!(sanitize_modifier_rule(dup, &existing).is_err());

        // Updating the same rule in place is not a self-collision.
        let update = mod_rule("mr-1", ModifierKey::Control, KeySide::Either);
        assert!(sanitize_modifier_rule(update, &existing).is_ok());

        // A different side is a different specificity, so it is allowed.
        let other_side = mod_rule("mr-3", ModifierKey::Control, KeySide::Left);
        assert!(sanitize_modifier_rule(other_side, &existing).is_ok());
    }

    #[test]
    fn reserves_the_left_and_right_command_tap_slots() {
        // The built-in ⌘ IME toggle owns ⌘ Left/Right, so a user rule there is
        // rejected regardless of whether the toggle is currently on — otherwise
        // it could be saved while off and shadow the built-in once turned on.
        let left = mod_rule("mr-1", ModifierKey::Command, KeySide::Left);
        assert!(sanitize_modifier_rule(left, &[]).is_err());
        let right = mod_rule("mr-2", ModifierKey::Command, KeySide::Right);
        assert!(sanitize_modifier_rule(right, &[]).is_err());

        // ⌘ Either is not one of the reserved side-specific slots.
        let either = mod_rule("mr-3", ModifierKey::Command, KeySide::Either);
        assert!(sanitize_modifier_rule(either, &[]).is_ok());
    }
}
