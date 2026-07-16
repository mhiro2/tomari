//! The tap/hold modifier engine.
//!
//! This is the pure, deterministic core of the keyboard feature. A native event
//! tap (which requires the macOS *Input Monitoring* permission) would feed
//! [`KeyEvent`]s in and act on the [`AppAction`]s that come out; keeping the
//! decision logic here means it is fully unit-testable without any OS hooks.
//!
//! Behavior:
//! * A modifier pressed and released *alone and quickly* fires its `tap` action.
//! * A modifier *held* past the threshold — or used in a chord with another
//!   key — behaves as a normal (possibly remapped) modifier and fires nothing.

use tomari_core::domain::action::AppAction;
use tomari_core::domain::keyboard::{KeySide, ModifierKey, ModifierRule};

/// The four modifiers a *hyper* key stands in for: ⌃⌥⇧⌘.
pub const HYPER_MODIFIERS: [ModifierKey; 4] = [
    ModifierKey::Control,
    ModifierKey::Option,
    ModifierKey::Shift,
    ModifierKey::Command,
];

/// A low-level keyboard event handed to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// A managed modifier went down.
    ModifierDown {
        key: ModifierKey,
        side: KeySide,
        at_ms: u64,
    },
    /// A managed modifier came up.
    ModifierUp {
        key: ModifierKey,
        side: KeySide,
        at_ms: u64,
    },
    /// Any other key went down (turns a pending tap into a chord).
    OtherKeyDown { at_ms: u64 },
}

#[derive(Debug, Clone, Copy)]
struct Press {
    key: ModifierKey,
    side: KeySide,
    down_at: u64,
    interrupted: bool,
}

/// Decides whether modifier activity should fire a tap action.
#[derive(Debug, Clone)]
pub struct ModifierEngine {
    rules: Vec<ModifierRule>,
    hold_threshold_ms: u64,
    press: Option<Press>,
    /// Managed modifiers currently physically held, whether or not they have a
    /// rule. A solo tap must begin from a clean slate: if another modifier is
    /// already down when a press starts, the press is part of a chord (e.g.
    /// holding Shift then tapping ⌘ must not fire ⌘'s tap).
    held: Vec<(ModifierKey, KeySide)>,
}

/// How long a modifier must be held before its release no longer counts as a
/// tap. Fixed internally rather than user-configurable: the IME-toggle taps it
/// gates are a built-in habit, not something worth exposing as a knob.
const DEFAULT_HOLD_THRESHOLD_MS: u64 = 200;

impl ModifierEngine {
    pub fn new(rules: Vec<ModifierRule>) -> Self {
        Self {
            rules,
            hold_threshold_ms: DEFAULT_HOLD_THRESHOLD_MS,
            press: None,
            held: Vec::new(),
        }
    }

    /// Replace the active rule set, discarding any in-flight press state. The
    /// physical-hold tracking is left intact — changing rules does not change
    /// which keys the user is holding.
    pub fn set_rules(&mut self, rules: Vec<ModifierRule>) {
        self.rules = rules;
        self.press = None;
    }

    /// Clear all transient state (in-flight press and physical-hold tracking),
    /// e.g. when the feature is toggled off or the system wakes from sleep —
    /// any "key is held" belief from before is no longer trustworthy.
    pub fn reset(&mut self) {
        self.press = None;
        self.held.clear();
    }

    /// The matching rule for this key/side, preferring an exact `Left`/`Right`
    /// match over an `Either` one; ties within the same specificity keep the
    /// stored order (DB rows by label, with the built-in ⌘ IME-toggle rules
    /// appended). Exact-side rules would otherwise be shadowed whenever an
    /// `Either` rule for the same modifier happens to be stored first — e.g. a
    /// user-created Command/Either rule hiding the left/right ⌘ IME toggle.
    pub fn find_rule(&self, key: ModifierKey, side: KeySide) -> Option<&ModifierRule> {
        let mut fallback: Option<&ModifierRule> = None;
        for r in &self.rules {
            if !r.enabled || r.modifier != key || !r.side.matches(side) {
                continue;
            }
            if r.side == side {
                // Exact `Left`/`Right` match: most specific, wins outright.
                return Some(r);
            }
            // An `Either` rule matching a `Left`/`Right` event side: keep the
            // first one as a fallback, in case no exact match ever appears.
            if fallback.is_none() {
                fallback = Some(r);
            }
        }
        fallback
    }

    /// The modifier this key should be remapped to, if a rule says so.
    pub fn remap_for(&self, key: ModifierKey, side: KeySide) -> Option<ModifierKey> {
        self.find_rule(key, side).and_then(|r| r.remap_to)
    }

    /// Whether the live rule set currently contains a rule with this id. Lets a
    /// caller verify that a rule it expects to be active — e.g. the built-in ⌘
    /// IME-toggle rules, added only when their setting is on — actually made it
    /// into the engine, so a reload that silently failed can be detected.
    pub fn contains_rule_id(&self, id: &str) -> bool {
        self.rules.iter().any(|r| r.id == id)
    }

    /// Whether any enabled rule manages Caps Lock. macOS gives Caps Lock no
    /// usable key-up and lets it lock, so the tap can only handle it once the OS
    /// remaps the Caps Lock HID usage to an ordinary key — the caller drives
    /// that HID remap on/off from this.
    pub fn has_caps_lock_rule(&self) -> bool {
        self.rules
            .iter()
            .any(|r| r.enabled && r.modifier == ModifierKey::CapsLock)
    }

    /// Which modifier flags to `(remove, add)` on keystrokes typed while
    /// remapped keys are held, so the chord carries the target modifier instead
    /// of the source. Rewriting only a remapped key's own `flagsChanged` event
    /// is not enough for a chord through Caps Lock: it is a lock key, so the OS
    /// keeps no "held modifier" state for it and does not carry the rewritten
    /// flag onto the following keystrokes (Caps Lock→Control held + C would
    /// otherwise reach the app as a bare C).
    ///
    /// A source flag is removed only when no *other* held key still contributes
    /// that same modifier natively — so remapping the left Control to Command
    /// while the right Control is also held (un-remapped) keeps Control set, and
    /// a chord lands as Control+Command rather than dropping Control. Hyper keys
    /// are excluded: their held role is the ⌃⌥⇧⌘ combo, stamped separately, and
    /// takes precedence over any `remap_to`.
    pub fn held_remap_stamp(&self) -> (Vec<ModifierKey>, Vec<ModifierKey>) {
        // Modifiers a held key contributes in its native role (held, not hyper,
        // and not remapped to a *different* modifier).
        let mut native: Vec<ModifierKey> = Vec::new();
        let mut sources: Vec<ModifierKey> = Vec::new();
        let mut targets: Vec<ModifierKey> = Vec::new();
        for &(key, side) in &self.held {
            match self.find_rule(key, side) {
                Some(rule) if !rule.hyper && rule.remap_to.is_some_and(|target| target != key) => {
                    let target = rule.remap_to.expect("checked above");
                    if !sources.contains(&key) {
                        sources.push(key);
                    }
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
                // No rule, hyper, or remap-to-self: the key keeps its own role.
                _ if !native.contains(&key) => native.push(key),
                _ => {}
            }
        }
        // Keep a source flag that another held key still provides natively.
        sources.retain(|source| !native.contains(source));
        (sources, targets)
    }

    /// Whether holding/chording this key should act as the hyper combo (⌃⌥⇧⌘).
    pub fn is_hyper(&self, key: ModifierKey, side: KeySide) -> bool {
        self.find_rule(key, side).is_some_and(|r| r.hyper)
    }

    /// Whether *any* currently-held managed modifier is a hyper key. The tap
    /// keeps the ⌃⌥⇧⌘ stamp active for as long as this holds, so releasing one
    /// of two simultaneously-held hyper keys does not drop hyper while the other
    /// is still down (tracking a single `bool` off each key's own up/down would).
    pub fn is_any_hyper_held(&self) -> bool {
        self.held
            .iter()
            .any(|&(key, side)| self.is_hyper(key, side))
    }

    /// Feed one event in; returns an action to perform, if a tap completed.
    pub fn process(&mut self, event: KeyEvent) -> Option<AppAction> {
        match event {
            KeyEvent::ModifierDown { key, side, at_ms } => {
                // Was any *other* modifier already down when this one pressed?
                // If so, a tap that completes now is part of a chord.
                let others_held = self.held.iter().any(|&held| held != (key, side));
                if !self.held.contains(&(key, side)) {
                    self.held.push((key, side));
                }
                match self.press.as_mut() {
                    // A second key while one is held → the first becomes a chord.
                    Some(p) => p.interrupted = true,
                    None => {
                        if self.find_rule(key, side).is_some() {
                            self.press = Some(Press {
                                key,
                                side,
                                down_at: at_ms,
                                interrupted: others_held,
                            });
                        }
                    }
                }
                None
            }
            KeyEvent::OtherKeyDown { .. } => {
                if let Some(p) = self.press.as_mut() {
                    p.interrupted = true;
                }
                None
            }
            KeyEvent::ModifierUp { key, side, at_ms } => {
                self.held.retain(|&held| held != (key, side));
                let p = self.press?;
                // Match on (key, side), not just key: with a complete event
                // stream the pending press and its release always share both,
                // but a dropped event (e.g. `reset()` on wake from sleep can
                // leave a stale press behind) must not let the release of the
                // *other* side's same-key press consume it and fire its tap.
                if (p.key, p.side) != (key, side) {
                    return None;
                }
                self.press = None;

                let duration = at_ms.saturating_sub(p.down_at);
                if p.interrupted || duration >= self.hold_threshold_ms {
                    // Held / chorded: acted as a (remapped) modifier — no tap.
                    return None;
                }
                self.find_rule(p.key, p.side)
                    .map(|r| r.tap.clone())
                    .filter(|action| !action.is_noop())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomari_core::domain::action::ImeMode;
    use tomari_core::domain::window::WindowPreset;

    fn rule(modifier: ModifierKey, side: KeySide, tap: AppAction) -> ModifierRule {
        ModifierRule {
            id: "r".into(),
            label: "r".into(),
            modifier,
            side,
            remap_to: None,
            hyper: false,
            tap,
            enabled: true,
        }
    }

    fn engine(rules: Vec<ModifierRule>) -> ModifierEngine {
        ModifierEngine::new(rules)
    }

    #[test]
    fn quick_solo_tap_fires() {
        let mut e = engine(vec![rule(
            ModifierKey::CapsLock,
            KeySide::Either,
            AppAction::TogglePanel,
        )]);
        assert_eq!(
            e.process(KeyEvent::ModifierDown {
                key: ModifierKey::CapsLock,
                side: KeySide::Left,
                at_ms: 0,
            }),
            None
        );
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::CapsLock,
                side: KeySide::Left,
                at_ms: 80,
            }),
            Some(AppAction::TogglePanel)
        );
    }

    #[test]
    fn long_hold_does_not_fire() {
        let mut e = engine(vec![rule(
            ModifierKey::CapsLock,
            KeySide::Either,
            AppAction::TogglePanel,
        )]);
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::CapsLock,
            side: KeySide::Either,
            at_ms: 0,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::CapsLock,
                side: KeySide::Either,
                at_ms: 500,
            }),
            None
        );
    }

    #[test]
    fn chord_with_other_key_does_not_fire() {
        let mut e = engine(vec![rule(
            ModifierKey::Control,
            KeySide::Either,
            AppAction::TogglePanel,
        )]);
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Control,
            side: KeySide::Left,
            at_ms: 0,
        });
        e.process(KeyEvent::OtherKeyDown { at_ms: 10 });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Control,
                side: KeySide::Left,
                at_ms: 30,
            }),
            None
        );
    }

    #[test]
    fn left_and_right_command_switch_different_imes() {
        let mut e = engine(vec![
            rule(
                ModifierKey::Command,
                KeySide::Left,
                AppAction::SwitchIme(ImeMode::Alphanumeric),
            ),
            rule(
                ModifierKey::Command,
                KeySide::Right,
                AppAction::SwitchIme(ImeMode::Kana),
            ),
        ]);

        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Command,
            side: KeySide::Left,
            at_ms: 0,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Command,
                side: KeySide::Left,
                at_ms: 50,
            }),
            Some(AppAction::SwitchIme(ImeMode::Alphanumeric))
        );

        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Command,
            side: KeySide::Right,
            at_ms: 100,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Command,
                side: KeySide::Right,
                at_ms: 150,
            }),
            Some(AppAction::SwitchIme(ImeMode::Kana))
        );
    }

    #[test]
    fn unmatched_side_is_ignored() {
        // Rule is for the right side only.
        let mut e = engine(vec![rule(
            ModifierKey::Command,
            KeySide::Right,
            AppAction::SwitchIme(ImeMode::Kana),
        )]);
        // A left-side press has no matching rule, so no state is started.
        assert_eq!(
            e.process(KeyEvent::ModifierDown {
                key: ModifierKey::Command,
                side: KeySide::Left,
                at_ms: 0,
            }),
            None
        );
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Command,
                side: KeySide::Left,
                at_ms: 20,
            }),
            None
        );
    }

    #[test]
    fn disabled_rule_does_not_fire() {
        let mut r = rule(
            ModifierKey::CapsLock,
            KeySide::Either,
            AppAction::TogglePanel,
        );
        r.enabled = false;
        let mut e = engine(vec![r]);
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::CapsLock,
            side: KeySide::Either,
            at_ms: 0,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::CapsLock,
                side: KeySide::Either,
                at_ms: 20,
            }),
            None
        );
    }

    #[test]
    fn noop_tap_yields_nothing() {
        let mut e = engine(vec![rule(
            ModifierKey::Shift,
            KeySide::Either,
            AppAction::NoOp,
        )]);
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Shift,
            side: KeySide::Either,
            at_ms: 0,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Shift,
                side: KeySide::Either,
                at_ms: 10,
            }),
            None
        );
    }

    #[test]
    fn remap_is_exposed() {
        let mut r = rule(ModifierKey::CapsLock, KeySide::Either, AppAction::NoOp);
        r.remap_to = Some(ModifierKey::Control);
        let e = engine(vec![r]);
        assert_eq!(
            e.remap_for(ModifierKey::CapsLock, KeySide::Left),
            Some(ModifierKey::Control)
        );
        assert_eq!(e.remap_for(ModifierKey::Shift, KeySide::Left), None);
    }

    #[test]
    fn held_remap_stamp_replaces_source_with_target_while_held() {
        let mut caps = rule(ModifierKey::CapsLock, KeySide::Either, AppAction::NoOp);
        caps.remap_to = Some(ModifierKey::Control);
        let mut e = engine(vec![caps]);

        // Nothing held: nothing to remove or add.
        assert_eq!(e.held_remap_stamp(), (vec![], vec![]));

        // Caps Lock held: remove AlphaShift, add Control on keystrokes typed
        // while held.
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::CapsLock,
            side: KeySide::Left,
            at_ms: 0,
        });
        assert_eq!(
            e.held_remap_stamp(),
            (vec![ModifierKey::CapsLock], vec![ModifierKey::Control])
        );

        // Released: back to nothing.
        e.process(KeyEvent::ModifierUp {
            key: ModifierKey::CapsLock,
            side: KeySide::Left,
            at_ms: 10,
        });
        assert_eq!(e.held_remap_stamp(), (vec![], vec![]));
    }

    #[test]
    fn held_remap_stamp_keeps_a_source_another_held_key_provides_natively() {
        // Only the left Control is remapped to Command. Holding it together
        // with the (un-remapped) right Control must not strip Control: the
        // chord should land as Control+Command, not just Command.
        let mut left = rule(ModifierKey::Control, KeySide::Left, AppAction::NoOp);
        left.remap_to = Some(ModifierKey::Command);
        let mut e = engine(vec![left]);

        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Control,
            side: KeySide::Left,
            at_ms: 0,
        });
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Control,
            side: KeySide::Right,
            at_ms: 10,
        });

        let (remove, add) = e.held_remap_stamp();
        // Control still held natively on the right, so it must not be removed.
        assert!(!remove.contains(&ModifierKey::Control));
        assert_eq!(add, vec![ModifierKey::Command]);
    }

    #[test]
    fn has_caps_lock_rule_reflects_enabled_caps_rules() {
        // No caps rule: false.
        let mut e = engine(vec![rule(
            ModifierKey::Command,
            KeySide::Left,
            AppAction::NoOp,
        )]);
        assert!(!e.has_caps_lock_rule());

        // An enabled caps rule: true.
        let caps = rule(ModifierKey::CapsLock, KeySide::Either, AppAction::NoOp);
        e = engine(vec![caps]);
        assert!(e.has_caps_lock_rule());

        // A disabled caps rule does not count.
        let mut disabled = rule(ModifierKey::CapsLock, KeySide::Either, AppAction::NoOp);
        disabled.enabled = false;
        e = engine(vec![disabled]);
        assert!(!e.has_caps_lock_rule());
    }

    #[test]
    fn held_remap_stamp_skips_hyper_and_unmapped_keys() {
        // A hyper key contributes the ⌃⌥⇧⌘ combo (stamped elsewhere), not a
        // remap, even if it also carries a `remap_to`.
        let mut hyper = rule(ModifierKey::CapsLock, KeySide::Either, AppAction::NoOp);
        hyper.hyper = true;
        hyper.remap_to = Some(ModifierKey::Control);
        // A plain modifier with no rule must not contribute either.
        let mut e = engine(vec![hyper]);

        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::CapsLock,
            side: KeySide::Left,
            at_ms: 0,
        });
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Shift,
            side: KeySide::Left,
            at_ms: 10,
        });
        // Nothing to add; the source set holds no remap (only native keys).
        assert!(e.held_remap_stamp().1.is_empty());
    }

    #[test]
    fn hyper_flag_is_exposed() {
        let mut r = rule(ModifierKey::CapsLock, KeySide::Either, AppAction::NoOp);
        r.hyper = true;
        let e = engine(vec![r]);
        assert!(e.is_hyper(ModifierKey::CapsLock, KeySide::Left));
        assert!(!e.is_hyper(ModifierKey::Shift, KeySide::Left));
    }

    #[test]
    fn hyper_stays_held_while_any_hyper_key_is_down() {
        // Two independent hyper keys. Releasing one must not drop hyper while the
        // other is still held — the regression a single `hyper_active` bool had.
        let mut caps = rule(ModifierKey::CapsLock, KeySide::Either, AppAction::NoOp);
        caps.hyper = true;
        let mut right_cmd = rule(ModifierKey::Command, KeySide::Right, AppAction::NoOp);
        right_cmd.hyper = true;
        let mut e = engine(vec![caps, right_cmd]);

        assert!(!e.is_any_hyper_held());
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::CapsLock,
            side: KeySide::Left,
            at_ms: 0,
        });
        assert!(e.is_any_hyper_held());
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Command,
            side: KeySide::Right,
            at_ms: 10,
        });
        assert!(e.is_any_hyper_held());

        // Release the first: the second is still down, so hyper holds.
        e.process(KeyEvent::ModifierUp {
            key: ModifierKey::CapsLock,
            side: KeySide::Left,
            at_ms: 20,
        });
        assert!(
            e.is_any_hyper_held(),
            "hyper must persist while another hyper key is still held"
        );

        // Release the second: now nothing is held.
        e.process(KeyEvent::ModifierUp {
            key: ModifierKey::Command,
            side: KeySide::Right,
            at_ms: 30,
        });
        assert!(!e.is_any_hyper_held());
    }

    #[test]
    fn non_hyper_held_key_is_not_counted_as_hyper() {
        // A held modifier with no hyper rule must not register as hyper.
        let mut e = engine(vec![rule(
            ModifierKey::Shift,
            KeySide::Left,
            AppAction::NoOp,
        )]);
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Shift,
            side: KeySide::Left,
            at_ms: 0,
        });
        assert!(!e.is_any_hyper_held());
    }

    #[test]
    fn snap_tap_fires() {
        let mut e = engine(vec![rule(
            ModifierKey::Function,
            KeySide::Either,
            AppAction::SnapWindow(WindowPreset::LeftHalf),
        )]);
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Function,
            side: KeySide::Either,
            at_ms: 0,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Function,
                side: KeySide::Either,
                at_ms: 30,
            }),
            Some(AppAction::SnapWindow(WindowPreset::LeftHalf))
        );
    }

    #[test]
    fn holding_another_modifier_makes_a_tap_a_chord() {
        // Shift has no rule; Command does. Holding Shift, then tapping Command
        // must not fire Command's tap — the user is making a Shift+⌘ combo.
        let mut e = engine(vec![rule(
            ModifierKey::Command,
            KeySide::Left,
            AppAction::SwitchIme(ImeMode::Alphanumeric),
        )]);
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Shift,
            side: KeySide::Left,
            at_ms: 0,
        });
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Command,
            side: KeySide::Left,
            at_ms: 10,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Command,
                side: KeySide::Left,
                at_ms: 30,
            }),
            None
        );

        // With Shift released, a solo Command tap fires again.
        e.process(KeyEvent::ModifierUp {
            key: ModifierKey::Shift,
            side: KeySide::Left,
            at_ms: 40,
        });
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Command,
            side: KeySide::Left,
            at_ms: 50,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Command,
                side: KeySide::Left,
                at_ms: 70,
            }),
            Some(AppAction::SwitchIme(ImeMode::Alphanumeric))
        );
    }

    #[test]
    fn reset_clears_held_so_a_later_solo_tap_still_fires() {
        let mut e = engine(vec![rule(
            ModifierKey::Command,
            KeySide::Left,
            AppAction::SwitchIme(ImeMode::Alphanumeric),
        )]);
        // A modifier is held when the system resets (e.g. wake from sleep): its
        // release will never arrive, so the hold tracking must be dropped.
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Shift,
            side: KeySide::Left,
            at_ms: 0,
        });
        e.reset();
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Command,
            side: KeySide::Left,
            at_ms: 100,
        });
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Command,
                side: KeySide::Left,
                at_ms: 120,
            }),
            Some(AppAction::SwitchIme(ImeMode::Alphanumeric))
        );
    }

    #[test]
    fn opposite_side_release_does_not_consume_a_pending_press() {
        // Left and right ⌘ both carry a tap rule. If an event goes missing
        // (e.g. `reset()` on wake from sleep drops a physical key's tracked
        // state), the release that does arrive must not be matched against a
        // pending press for the *other* side of the same key.
        let mut e = engine(vec![
            rule(
                ModifierKey::Command,
                KeySide::Left,
                AppAction::SwitchIme(ImeMode::Alphanumeric),
            ),
            rule(
                ModifierKey::Command,
                KeySide::Right,
                AppAction::SwitchIme(ImeMode::Kana),
            ),
        ]);

        // Right ⌘ was already held before the reset (its down event never
        // reached this engine instance — simulating a dropped event), then
        // left ⌘ presses down for real.
        e.process(KeyEvent::ModifierDown {
            key: ModifierKey::Command,
            side: KeySide::Left,
            at_ms: 0,
        });

        // The right ⌘ release arrives. It must not match the pending left ⌘
        // press: no tap should fire, and the left press must survive so its
        // own later release still fires normally.
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Command,
                side: KeySide::Right,
                at_ms: 10,
            }),
            None
        );

        // The left ⌘ press is untouched by the mismatched release above, so
        // its own release still fires its tap.
        assert_eq!(
            e.process(KeyEvent::ModifierUp {
                key: ModifierKey::Command,
                side: KeySide::Left,
                at_ms: 30,
            }),
            Some(AppAction::SwitchIme(ImeMode::Alphanumeric))
        );
    }

    #[test]
    fn exact_side_rule_wins_over_either_rule_regardless_of_storage_order() {
        // A side-specific rule (e.g. the built-in left/right ⌘ IME toggle)
        // must be found even when a same-modifier `Either` rule (e.g.
        // user-created) is stored first — and the reverse order must give the
        // same result, since specificity — not storage order — decides here.
        let either = rule(
            ModifierKey::Command,
            KeySide::Either,
            AppAction::TogglePanel,
        );
        let right = rule(
            ModifierKey::Command,
            KeySide::Right,
            AppAction::SwitchIme(ImeMode::Kana),
        );

        let e_either_first = engine(vec![either.clone(), right.clone()]);
        assert_eq!(
            e_either_first
                .find_rule(ModifierKey::Command, KeySide::Right)
                .map(|r| r.tap.clone()),
            Some(AppAction::SwitchIme(ImeMode::Kana))
        );

        let e_right_first = engine(vec![right, either]);
        assert_eq!(
            e_right_first
                .find_rule(ModifierKey::Command, KeySide::Right)
                .map(|r| r.tap.clone()),
            Some(AppAction::SwitchIme(ImeMode::Kana))
        );

        // The left side has no exact match in either engine, so the `Either`
        // rule is still the one that applies there.
        assert_eq!(
            e_either_first
                .find_rule(ModifierKey::Command, KeySide::Left)
                .map(|r| r.tap.clone()),
            Some(AppAction::TogglePanel)
        );
    }
}
