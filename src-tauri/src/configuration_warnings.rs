//! User-visible warnings for persisted keyboard records that were quarantined.
//!
//! Validation lives in `tomari-keyboard`; this module keeps the Tauri wire
//! representation and the latest coherent snapshot. Invalid rows stay in the
//! database, but callers omit them from the live shortcut and modifier-rule
//! engines and publish this snapshot so the panel can explain what happened.

use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tomari_keyboard::validation::{InvalidRecord, ValidationField, ValidationReason};

/// Event carrying the same complete snapshot returned by the pull command.
pub(crate) const CONFIGURATION_WARNINGS_CHANGED_EVENT: &str =
    "tomari:configuration-warnings-changed";

/// Stable, localizable explanation for one quarantined record.
///
/// Payload details from the validator are intentionally not exposed: they are
/// developer diagnostics and may contain persisted text. The frontend only
/// needs a stable code to select actionable, localized copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationIssueReason {
    EmptyId,
    IdTooLong,
    EmptyLabel,
    LabelTooLong,
    InvalidAccelerator,
    UnsafeGlobalShortcut,
    InvalidKeystroke,
    ReservedRuleId,
    HyperWithRemap,
    ReservedCommandSlot,
    DuplicateId,
    DuplicateAccelerator,
    DuplicateModifierSlot,
}

impl From<ValidationReason> for ConfigurationIssueReason {
    fn from(reason: ValidationReason) -> Self {
        match reason {
            ValidationReason::EmptyId => Self::EmptyId,
            ValidationReason::IdTooLong { .. } => Self::IdTooLong,
            ValidationReason::EmptyLabel => Self::EmptyLabel,
            ValidationReason::LabelTooLong { .. } => Self::LabelTooLong,
            ValidationReason::InvalidAccelerator { .. } => Self::InvalidAccelerator,
            ValidationReason::UnsafeGlobalShortcut => Self::UnsafeGlobalShortcut,
            ValidationReason::InvalidKeystroke { .. } => Self::InvalidKeystroke,
            ValidationReason::ReservedRuleId => Self::ReservedRuleId,
            ValidationReason::HyperWithRemap => Self::HyperWithRemap,
            ValidationReason::ReservedCommandSlot => Self::ReservedCommandSlot,
            ValidationReason::DuplicateId { .. } => Self::DuplicateId,
            ValidationReason::DuplicateAccelerator { .. } => Self::DuplicateAccelerator,
            ValidationReason::DuplicateModifierSlot { .. } => Self::DuplicateModifierSlot,
        }
    }
}

/// One persisted keyboard record that was excluded from the live runtime.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationIssue {
    /// Persisted row identifier. It may be empty when that is the defect; the
    /// frontend supplies a localized display fallback without changing it.
    pub id: String,
    /// Persisted label, trimmed by the validator. The frontend bounds and
    /// sanitizes it for display and supplies a localized fallback when empty.
    pub label: String,
    /// Stable code for frontend localization.
    pub reason: ConfigurationIssueReason,
}

impl ConfigurationIssue {
    /// Build a wire issue from the validator's structured output.
    ///
    /// `field` is accepted deliberately even though the current wire contract
    /// does not expose it. Keeping the complete conversion boundary here makes
    /// it hard for integration code to fall back to developer-facing prose.
    pub fn from_validation(
        id: impl Into<String>,
        label: impl Into<String>,
        _field: ValidationField,
        reason: ValidationReason,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            reason: reason.into(),
        }
    }
}

impl From<InvalidRecord> for ConfigurationIssue {
    fn from(record: InvalidRecord) -> Self {
        Self::from_validation(record.id, record.label, record.field, record.reason)
    }
}

/// Complete warning snapshot shared by the pull command and change event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationWarnings {
    pub invalid_hotkeys: Vec<ConfigurationIssue>,
    pub invalid_modifier_rules: Vec<ConfigurationIssue>,
    /// Ordering stamp for listener-first-then-pull synchronization.
    pub revision: u64,
}

/// Coherent, poison-tolerant storage for [`ConfigurationWarnings`].
///
/// Replacement methods return `Some(snapshot)` only when visible contents
/// changed. The clone is made while the mutex is held but returned after the
/// guard is dropped, so callers can emit it without holding an application
/// lock across Tauri callbacks.
#[derive(Debug, Default)]
pub struct ConfigurationWarningState {
    inner: Mutex<ConfigurationWarnings>,
}

impl ConfigurationWarningState {
    pub fn snapshot(&self) -> ConfigurationWarnings {
        self.lock().clone()
    }

    pub fn replace_all(
        &self,
        invalid_hotkeys: impl IntoIterator<Item = ConfigurationIssue>,
        invalid_modifier_rules: impl IntoIterator<Item = ConfigurationIssue>,
    ) -> Option<ConfigurationWarnings> {
        let invalid_hotkeys = normalize_issues(invalid_hotkeys);
        let invalid_modifier_rules = normalize_issues(invalid_modifier_rules);
        let mut current = self.lock();

        if current.invalid_hotkeys == invalid_hotkeys
            && current.invalid_modifier_rules == invalid_modifier_rules
        {
            return None;
        }

        current.invalid_hotkeys = invalid_hotkeys;
        current.invalid_modifier_rules = invalid_modifier_rules;
        current.revision = next_revision(current.revision);
        let snapshot = current.clone();
        drop(current);
        Some(snapshot)
    }

    pub fn replace_hotkeys(
        &self,
        invalid_hotkeys: impl IntoIterator<Item = ConfigurationIssue>,
    ) -> Option<ConfigurationWarnings> {
        let invalid_hotkeys = normalize_issues(invalid_hotkeys);
        let mut current = self.lock();

        if current.invalid_hotkeys == invalid_hotkeys {
            return None;
        }

        current.invalid_hotkeys = invalid_hotkeys;
        current.revision = next_revision(current.revision);
        let snapshot = current.clone();
        drop(current);
        Some(snapshot)
    }

    pub fn replace_modifier_rules(
        &self,
        invalid_modifier_rules: impl IntoIterator<Item = ConfigurationIssue>,
    ) -> Option<ConfigurationWarnings> {
        let invalid_modifier_rules = normalize_issues(invalid_modifier_rules);
        let mut current = self.lock();

        if current.invalid_modifier_rules == invalid_modifier_rules {
            return None;
        }

        current.invalid_modifier_rules = invalid_modifier_rules;
        current.revision = next_revision(current.revision);
        let snapshot = current.clone();
        drop(current);
        Some(snapshot)
    }

    fn lock(&self) -> MutexGuard<'_, ConfigurationWarnings> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Replace the quarantined-hotkey warning set and notify the panel when it
/// visibly changed. Event delivery is advisory: the snapshot remains available
/// through the pull command even when no panel is listening or emission fails.
pub(crate) fn publish_hotkey_issues(
    app: &AppHandle,
    state: &ConfigurationWarningState,
    issues: Vec<InvalidRecord>,
) {
    let issue_count = issues.len();
    publish_changed_snapshot(
        app,
        state.replace_hotkeys(issues.into_iter().map(ConfigurationIssue::from)),
        issue_count,
        "hotkey",
    );
}

/// Replace the quarantined-modifier warning set and notify the panel when it
/// visibly changed. See [`publish_hotkey_issues`] for delivery semantics.
pub(crate) fn publish_modifier_rule_issues(
    app: &AppHandle,
    state: &ConfigurationWarningState,
    issues: Vec<InvalidRecord>,
) {
    let issue_count = issues.len();
    publish_changed_snapshot(
        app,
        state.replace_modifier_rules(issues.into_iter().map(ConfigurationIssue::from)),
        issue_count,
        "modifier rule",
    );
}

fn publish_changed_snapshot(
    app: &AppHandle,
    changed: Option<ConfigurationWarnings>,
    issue_count: usize,
    kind: &'static str,
) {
    let Some(snapshot) = changed else {
        return;
    };

    let revision = snapshot.revision;
    if let Err(error) = app.emit(CONFIGURATION_WARNINGS_CHANGED_EVENT, snapshot) {
        tracing::warn!(
            %error,
            issue_count,
            revision,
            kind,
            "failed to emit configuration warning snapshot"
        );
    }
}

fn normalize_issues(
    issues: impl IntoIterator<Item = ConfigurationIssue>,
) -> Vec<ConfigurationIssue> {
    let mut issues: Vec<_> = issues.into_iter().collect();
    issues.sort_unstable();
    issues.dedup();
    issues
}

fn next_revision(revision: u64) -> u64 {
    // Zero identifies the never-published default snapshot. Reserve it across
    // rollover too, so every actual content change differs from its predecessor.
    let next = revision.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    fn issue(id: &str, label: &str, reason: ConfigurationIssueReason) -> ConfigurationIssue {
        ConfigurationIssue {
            id: id.into(),
            label: label.into(),
            reason,
        }
    }

    #[test]
    fn serializes_the_frontend_wire_contract() {
        assert_eq!(
            CONFIGURATION_WARNINGS_CHANGED_EVENT,
            "tomari:configuration-warnings-changed"
        );

        let warnings = ConfigurationWarnings {
            invalid_hotkeys: vec![issue(
                "hotkey-1",
                "Unsafe shortcut",
                ConfigurationIssueReason::UnsafeGlobalShortcut,
            )],
            invalid_modifier_rules: vec![issue(
                "rule-1",
                "Conflicting rule",
                ConfigurationIssueReason::DuplicateModifierSlot,
            )],
            revision: 7,
        };

        assert_eq!(
            serde_json::to_value(warnings).unwrap(),
            serde_json::json!({
                "invalidHotkeys": [{
                    "id": "hotkey-1",
                    "label": "Unsafe shortcut",
                    "reason": "unsafeGlobalShortcut"
                }],
                "invalidModifierRules": [{
                    "id": "rule-1",
                    "label": "Conflicting rule",
                    "reason": "duplicateModifierSlot"
                }],
                "revision": 7
            })
        );
    }

    #[test]
    fn serializes_every_reason_as_a_stable_code() {
        let reasons = [
            ConfigurationIssueReason::EmptyId,
            ConfigurationIssueReason::IdTooLong,
            ConfigurationIssueReason::EmptyLabel,
            ConfigurationIssueReason::LabelTooLong,
            ConfigurationIssueReason::InvalidAccelerator,
            ConfigurationIssueReason::UnsafeGlobalShortcut,
            ConfigurationIssueReason::InvalidKeystroke,
            ConfigurationIssueReason::ReservedRuleId,
            ConfigurationIssueReason::HyperWithRemap,
            ConfigurationIssueReason::ReservedCommandSlot,
            ConfigurationIssueReason::DuplicateId,
            ConfigurationIssueReason::DuplicateAccelerator,
            ConfigurationIssueReason::DuplicateModifierSlot,
        ];

        assert_eq!(
            serde_json::to_value(reasons).unwrap(),
            serde_json::json!([
                "emptyId",
                "idTooLong",
                "emptyLabel",
                "labelTooLong",
                "invalidAccelerator",
                "unsafeGlobalShortcut",
                "invalidKeystroke",
                "reservedRuleId",
                "hyperWithRemap",
                "reservedCommandSlot",
                "duplicateId",
                "duplicateAccelerator",
                "duplicateModifierSlot"
            ])
        );
    }

    #[test]
    fn equivalent_replacements_are_no_ops_after_normalization() {
        let state = ConfigurationWarningState::default();
        let first = issue(
            "hotkey-1",
            "Unsafe shortcut",
            ConfigurationIssueReason::UnsafeGlobalShortcut,
        );
        let second = issue(
            "hotkey-2",
            "Broken shortcut",
            ConfigurationIssueReason::InvalidAccelerator,
        );

        let changed = state
            .replace_hotkeys([second.clone(), first.clone(), first.clone()])
            .unwrap();
        assert_eq!(changed.revision, 1);
        assert_eq!(changed.invalid_hotkeys, vec![first.clone(), second.clone()]);

        assert!(state.replace_hotkeys([first, second]).is_none());
        assert_eq!(state.snapshot().revision, 1);
    }

    #[test]
    fn replacing_one_collection_preserves_the_other() {
        let state = ConfigurationWarningState::default();
        let hotkey = issue(
            "hotkey-1",
            "Unsafe shortcut",
            ConfigurationIssueReason::UnsafeGlobalShortcut,
        );
        let rule = issue(
            "rule-1",
            "Conflicting rule",
            ConfigurationIssueReason::DuplicateModifierSlot,
        );

        let first = state.replace_all([hotkey.clone()], [rule.clone()]).unwrap();
        assert_eq!(first.revision, 1);

        let replacement = issue(
            "hotkey-2",
            "Broken shortcut",
            ConfigurationIssueReason::InvalidAccelerator,
        );
        let second = state.replace_hotkeys([replacement.clone()]).unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(second.invalid_hotkeys, vec![replacement]);
        assert_eq!(second.invalid_modifier_rules, vec![rule.clone()]);

        let rule_replacement = issue(
            "rule-2",
            "Broken rule",
            ConfigurationIssueReason::HyperWithRemap,
        );
        let third = state
            .replace_modifier_rules([rule_replacement.clone()])
            .unwrap();
        assert_eq!(third.revision, 3);
        assert_eq!(third.invalid_hotkeys, second.invalid_hotkeys);
        assert_eq!(third.invalid_modifier_rules, vec![rule_replacement]);
    }

    #[test]
    fn concurrent_snapshots_never_mix_replace_all_pairs() {
        const WRITES: usize = 2_000;

        let state = Arc::new(ConfigurationWarningState::default());
        let barrier = Arc::new(Barrier::new(2));
        let writer_state = Arc::clone(&state);
        let writer_barrier = Arc::clone(&barrier);
        let writer = std::thread::spawn(move || {
            for index in 0..WRITES {
                let suffix = if index % 2 == 0 { "a" } else { "b" };
                writer_state.replace_all(
                    [issue(
                        &format!("hotkey-{suffix}"),
                        suffix,
                        ConfigurationIssueReason::InvalidAccelerator,
                    )],
                    [issue(
                        &format!("rule-{suffix}"),
                        suffix,
                        ConfigurationIssueReason::HyperWithRemap,
                    )],
                );
                writer_barrier.wait();
                writer_barrier.wait();
            }
        });

        for index in 0..WRITES {
            barrier.wait();
            let snapshot = state.snapshot();
            assert_eq!(snapshot.invalid_hotkeys.len(), 1);
            assert_eq!(snapshot.invalid_modifier_rules.len(), 1);
            let expected = if index % 2 == 0 { "a" } else { "b" };
            assert_eq!(
                snapshot.invalid_hotkeys[0].label,
                snapshot.invalid_modifier_rules[0].label
            );
            assert_eq!(snapshot.invalid_hotkeys[0].label, expected);
            barrier.wait();
        }
        writer.join().unwrap();
    }

    #[test]
    fn revision_rollover_never_reuses_zero_or_the_previous_value() {
        assert_eq!(next_revision(u64::MAX), 1);
        assert_ne!(next_revision(u64::MAX), u64::MAX);
    }
}
