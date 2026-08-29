//! Pure validation and canonicalization for persisted keyboard configuration.
//!
//! A row that crossed the command boundary successfully can still become
//! invalid later: an older release may have accepted it, the database may have
//! been edited by hand, or two individually valid rows may now collide. These
//! validators therefore operate both on one save candidate and on a complete
//! persisted collection. They have no Tauri or operating-system dependencies,
//! so startup, reload, and save paths can enforce exactly the same rules.

use std::collections::HashMap;
use std::fmt;

use tomari_core::{AppAction, Hotkey, KeySide, ModifierKey, ModifierRule};

use crate::accelerator;

/// Maximum number of Unicode scalar values accepted in a stored identifier.
pub const MAX_ID_CHARS: usize = 128;

/// Maximum number of Unicode scalar values accepted in a user-visible label.
pub const MAX_LABEL_CHARS: usize = 200;

/// The field responsible for rejecting a persisted row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValidationField {
    Id,
    Label,
    Accelerator,
    Action,
    RemapTo,
    Slot,
}

impl ValidationField {
    /// Stable lower-camel-case code suitable for an application DTO.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Label => "label",
            Self::Accelerator => "accelerator",
            Self::Action => "action",
            Self::RemapTo => "remapTo",
            Self::Slot => "slot",
        }
    }
}

impl fmt::Display for ValidationField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A stable, machine-readable reason for rejecting a persisted row.
///
/// The payloads provide diagnostics for logs. Consumers should use [`Self::code`]
/// rather than parsing [`fmt::Display`] output when building a wire DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationReason {
    EmptyId,
    IdTooLong {
        max_chars: usize,
    },
    DuplicateId {
        id: String,
    },
    EmptyLabel,
    LabelTooLong {
        max_chars: usize,
    },
    InvalidAccelerator {
        detail: String,
    },
    UnsafeGlobalShortcut,
    InvalidKeystroke {
        detail: String,
    },
    ReservedRuleId,
    HyperWithRemap,
    ReservedCommandSlot,
    DuplicateAccelerator {
        accelerator: String,
    },
    DuplicateModifierSlot {
        modifier: ModifierKey,
        side: KeySide,
    },
}

impl ValidationReason {
    /// Stable lower-camel-case code suitable for exhaustive frontend handling.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyId => "emptyId",
            Self::IdTooLong { .. } => "idTooLong",
            Self::DuplicateId { .. } => "duplicateId",
            Self::EmptyLabel => "emptyLabel",
            Self::LabelTooLong { .. } => "labelTooLong",
            Self::InvalidAccelerator { .. } => "invalidAccelerator",
            Self::UnsafeGlobalShortcut => "unsafeGlobalShortcut",
            Self::InvalidKeystroke { .. } => "invalidKeystroke",
            Self::ReservedRuleId => "reservedRuleId",
            Self::HyperWithRemap => "hyperWithRemap",
            Self::ReservedCommandSlot => "reservedCommandSlot",
            Self::DuplicateAccelerator { .. } => "duplicateAccelerator",
            Self::DuplicateModifierSlot { .. } => "duplicateModifierSlot",
        }
    }
}

impl fmt::Display for ValidationReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => f.write_str("id must not be empty"),
            Self::IdTooLong { max_chars } => {
                write!(f, "id is too long (max {max_chars} characters)")
            }
            Self::DuplicateId { id } => {
                write!(f, "more than one row canonicalizes to id {id}")
            }
            Self::EmptyLabel => f.write_str("label must not be empty"),
            Self::LabelTooLong { max_chars } => {
                write!(f, "label is too long (max {max_chars} characters)")
            }
            Self::InvalidAccelerator { detail } => write!(f, "invalid shortcut: {detail}"),
            Self::UnsafeGlobalShortcut => {
                f.write_str("a global shortcut needs Ctrl, Alt or Cmd, or a function key")
            }
            Self::InvalidKeystroke { detail } => write!(f, "invalid keystroke: {detail}"),
            Self::ReservedRuleId => f.write_str("this rule id is reserved for a built-in rule"),
            Self::HyperWithRemap => f.write_str("a rule cannot be both a Hyper key and a remap"),
            Self::ReservedCommandSlot => {
                f.write_str("left/right Command is reserved for the Command-key IME toggle")
            }
            Self::DuplicateAccelerator { accelerator } => write!(
                f,
                "shortcut {accelerator} is assigned to more than one hotkey"
            ),
            Self::DuplicateModifierSlot { modifier, side } => write!(
                f,
                "more than one rule handles {} ({})",
                modifier.label(),
                side_label(*side)
            ),
        }
    }
}

/// One persisted row that must not become live.
///
/// `id` deliberately retains the exact stored value rather than the trimmed,
/// canonical value. That lets callers identify, repair, or delete the original
/// database row. `label` is trimmed and capped at [`MAX_LABEL_CHARS`] for safe
/// display. It may be empty, in which case the UI should use a bounded fallback
/// rather than display the untrusted raw `id` verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidRecord {
    pub id: String,
    pub label: String,
    pub field: ValidationField,
    pub reason: ValidationReason,
}

impl fmt::Display for InvalidRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.reason)
    }
}

impl std::error::Error for InvalidRecord {}

/// Canonical rows safe to make live, and rejected rows safe to warn about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport<T> {
    pub valid: Vec<T>,
    pub invalid: Vec<InvalidRecord>,
}

impl<T> ValidationReport<T> {
    /// Whether every input row passed intrinsic and collection validation.
    pub fn is_valid(&self) -> bool {
        self.invalid.is_empty()
    }

    /// Consume the report and return its canonical live rows.
    pub fn into_valid(self) -> Vec<T> {
        self.valid
    }
}

/// Failure to validate the only open-ended [`AppAction`] variant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActionValidationError {
    #[error("invalid keystroke: {detail}")]
    InvalidKeystroke { detail: String },
}

/// Validate and canonicalize an action crossing any application boundary.
///
/// Closed enum variants pass through unchanged. `SendKeystroke` is parsed by
/// the same accelerator vocabulary used for hotkeys and returned in canonical
/// form. That vocabulary is intentionally limited to keys the native sender
/// supports, keeping this validation independent of the native implementation.
pub fn validate_app_action(
    action: AppAction,
) -> std::result::Result<AppAction, ActionValidationError> {
    match action {
        AppAction::SendKeystroke(value) => accelerator::parse(&value)
            .map(|parsed| AppAction::SendKeystroke(parsed.to_canonical()))
            .map_err(|error| ActionValidationError::InvalidKeystroke {
                detail: error.to_string(),
            }),
        other => Ok(other),
    }
}

/// Validate one hotkey intrinsically and return its canonical representation.
///
/// Collection collisions are intentionally handled by [`validate_hotkeys`]. A
/// save path should build the post-upsert collection and call that function
/// before writing, so an otherwise valid candidate cannot introduce a duplicate
/// accelerator.
pub fn validate_hotkey(hotkey: Hotkey) -> Result<Hotkey, InvalidRecord> {
    let metadata = RowMetadata::new(&hotkey.id, &hotkey.label);
    let id = canonical_id(&metadata, &hotkey.id)?;
    let label = canonical_label(&metadata, &hotkey.label)?;

    let parsed = accelerator::parse_global(&hotkey.accelerator).map_err(|error| {
        metadata.invalid(
            ValidationField::Accelerator,
            ValidationReason::InvalidAccelerator {
                detail: error.to_string(),
            },
        )
    })?;
    if !(parsed.ctrl || parsed.alt || parsed.cmd || is_function_key(&parsed.key)) {
        return Err(metadata.invalid(
            ValidationField::Accelerator,
            ValidationReason::UnsafeGlobalShortcut,
        ));
    }

    let action = validate_app_action(hotkey.action).map_err(|error| {
        let ActionValidationError::InvalidKeystroke { detail } = error;
        metadata.invalid(
            ValidationField::Action,
            ValidationReason::InvalidKeystroke { detail },
        )
    })?;

    Ok(Hotkey {
        id,
        label,
        accelerator: parsed.to_canonical(),
        action,
        enabled: hotkey.enabled,
    })
}

/// Validate all hotkeys, excluding every member of each canonical-ID or
/// duplicate-chord group.
///
/// Disabled rows are still persisted configuration and are deliberately
/// included in collision detection. Otherwise enabling one later could silently
/// displace another binding. Classification does not depend on input order.
pub fn validate_hotkeys(hotkeys: impl IntoIterator<Item = Hotkey>) -> ValidationReport<Hotkey> {
    let rows: Vec<_> = hotkeys
        .into_iter()
        .map(|hotkey| {
            let metadata = RowMetadata::new(&hotkey.id, &hotkey.label);
            match validate_hotkey(hotkey) {
                Ok(value) => ValidatedRow::Valid { value, metadata },
                Err(invalid) => ValidatedRow::Invalid(invalid),
            }
        })
        .collect();

    let mut id_counts = HashMap::<String, usize>::new();
    let mut accelerator_counts = HashMap::<String, usize>::new();
    for row in &rows {
        if let ValidatedRow::Valid { value, .. } = row {
            *id_counts.entry(value.id.clone()).or_default() += 1;
            *accelerator_counts
                .entry(value.accelerator.clone())
                .or_default() += 1;
        }
    }

    let mut valid = Vec::with_capacity(rows.len());
    let mut invalid = Vec::new();
    for row in rows {
        match row {
            ValidatedRow::Valid { value, metadata } if id_counts[&value.id] > 1 => {
                invalid.push(metadata.invalid(
                    ValidationField::Id,
                    ValidationReason::DuplicateId {
                        id: value.id.clone(),
                    },
                ));
            }
            ValidatedRow::Valid { value, metadata }
                if accelerator_counts[&value.accelerator] > 1 =>
            {
                invalid.push(metadata.invalid(
                    ValidationField::Accelerator,
                    ValidationReason::DuplicateAccelerator {
                        accelerator: value.accelerator,
                    },
                ));
            }
            ValidatedRow::Valid { value, .. } => valid.push(value),
            ValidatedRow::Invalid(issue) => invalid.push(issue),
        }
    }

    ValidationReport { valid, invalid }
}

/// Validate one persisted modifier rule intrinsically and canonicalize it.
///
/// This function rejects the IDs and physical slots owned by the built-in
/// Command-key IME pair even when that feature is currently disabled. Those
/// built-ins are appended at runtime and must never be passed into this
/// persisted-row validator themselves.
pub fn validate_modifier_rule(rule: ModifierRule) -> Result<ModifierRule, InvalidRecord> {
    let metadata = RowMetadata::new(&rule.id, &rule.label);
    let id = canonical_id(&metadata, &rule.id)?;
    if is_reserved_rule_id(&id) {
        return Err(metadata.invalid(ValidationField::Id, ValidationReason::ReservedRuleId));
    }
    let label = canonical_label(&metadata, &rule.label)?;

    if rule.hyper && rule.remap_to.is_some() {
        return Err(metadata.invalid(ValidationField::RemapTo, ValidationReason::HyperWithRemap));
    }

    let tap = validate_app_action(rule.tap).map_err(|error| {
        let ActionValidationError::InvalidKeystroke { detail } = error;
        metadata.invalid(
            ValidationField::Action,
            ValidationReason::InvalidKeystroke { detail },
        )
    })?;

    if rule.modifier == ModifierKey::Command && matches!(rule.side, KeySide::Left | KeySide::Right)
    {
        return Err(metadata.invalid(ValidationField::Slot, ValidationReason::ReservedCommandSlot));
    }

    Ok(ModifierRule {
        id,
        label,
        modifier: rule.modifier,
        side: rule.side,
        remap_to: rule.remap_to,
        hyper: rule.hyper,
        tap,
        enabled: rule.enabled,
    })
}

/// Validate all persisted modifier rules, excluding every member of each
/// canonical-ID or duplicate `(modifier, side)` group.
///
/// Disabled rules participate in collision detection. The built-in Command IME
/// rules must be appended only after this report's `valid` rows are selected.
pub fn validate_modifier_rules(
    rules: impl IntoIterator<Item = ModifierRule>,
) -> ValidationReport<ModifierRule> {
    let rows: Vec<_> = rules
        .into_iter()
        .map(|rule| {
            let metadata = RowMetadata::new(&rule.id, &rule.label);
            match validate_modifier_rule(rule) {
                Ok(value) => ValidatedRow::Valid { value, metadata },
                Err(invalid) => ValidatedRow::Invalid(invalid),
            }
        })
        .collect();

    let mut id_counts = HashMap::<String, usize>::new();
    let mut slot_counts = HashMap::<(ModifierKey, KeySide), usize>::new();
    for row in &rows {
        if let ValidatedRow::Valid { value, .. } = row {
            *id_counts.entry(value.id.clone()).or_default() += 1;
            *slot_counts.entry((value.modifier, value.side)).or_default() += 1;
        }
    }

    let mut valid = Vec::with_capacity(rows.len());
    let mut invalid = Vec::new();
    for row in rows {
        match row {
            ValidatedRow::Valid { value, metadata } if id_counts[&value.id] > 1 => {
                invalid.push(metadata.invalid(
                    ValidationField::Id,
                    ValidationReason::DuplicateId {
                        id: value.id.clone(),
                    },
                ));
            }
            ValidatedRow::Valid { value, metadata }
                if slot_counts[&(value.modifier, value.side)] > 1 =>
            {
                invalid.push(metadata.invalid(
                    ValidationField::Slot,
                    ValidationReason::DuplicateModifierSlot {
                        modifier: value.modifier,
                        side: value.side,
                    },
                ));
            }
            ValidatedRow::Valid { value, .. } => valid.push(value),
            ValidatedRow::Invalid(issue) => invalid.push(issue),
        }
    }

    ValidationReport { valid, invalid }
}

#[derive(Debug, Clone)]
struct RowMetadata {
    raw_id: String,
    display_label: String,
}

impl RowMetadata {
    fn new(id: &str, label: &str) -> Self {
        Self {
            raw_id: id.to_owned(),
            display_label: label.trim().chars().take(MAX_LABEL_CHARS).collect(),
        }
    }

    fn invalid(&self, field: ValidationField, reason: ValidationReason) -> InvalidRecord {
        InvalidRecord {
            id: self.raw_id.clone(),
            label: self.display_label.clone(),
            field,
            reason,
        }
    }
}

enum ValidatedRow<T> {
    Valid { value: T, metadata: RowMetadata },
    Invalid(InvalidRecord),
}

fn canonical_id(metadata: &RowMetadata, value: &str) -> Result<String, InvalidRecord> {
    let value = value.trim();
    if value.is_empty() {
        return Err(metadata.invalid(ValidationField::Id, ValidationReason::EmptyId));
    }
    if value.chars().count() > MAX_ID_CHARS {
        return Err(metadata.invalid(
            ValidationField::Id,
            ValidationReason::IdTooLong {
                max_chars: MAX_ID_CHARS,
            },
        ));
    }
    Ok(value.to_owned())
}

fn canonical_label(metadata: &RowMetadata, value: &str) -> Result<String, InvalidRecord> {
    let value = value.trim();
    if value.is_empty() {
        return Err(metadata.invalid(ValidationField::Label, ValidationReason::EmptyLabel));
    }
    if value.chars().count() > MAX_LABEL_CHARS {
        return Err(metadata.invalid(
            ValidationField::Label,
            ValidationReason::LabelTooLong {
                max_chars: MAX_LABEL_CHARS,
            },
        ));
    }
    Ok(value.to_owned())
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F')
        .and_then(|number| number.parse::<u32>().ok())
        .is_some_and(|number| (1..=20).contains(&number))
}

fn is_reserved_rule_id(id: &str) -> bool {
    tomari_core::defaults::command_ime_rules()
        .iter()
        .any(|rule| rule.id == id)
}

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

    fn hotkey(id: &str, accelerator: &str) -> Hotkey {
        Hotkey {
            id: id.into(),
            label: format!("Hotkey {id}"),
            accelerator: accelerator.into(),
            action: AppAction::TogglePanel,
            enabled: true,
        }
    }

    fn rule(id: &str, modifier: ModifierKey, side: KeySide) -> ModifierRule {
        ModifierRule {
            id: id.into(),
            label: format!("Rule {id}"),
            modifier,
            side,
            remap_to: None,
            hyper: false,
            tap: AppAction::NoOp,
            enabled: true,
        }
    }

    fn assert_issue(issue: InvalidRecord, field: ValidationField, reason_code: &'static str) {
        assert_eq!(issue.field, field);
        assert_eq!(issue.reason.code(), reason_code);
    }

    #[test]
    fn warning_codes_match_the_frontend_contract() {
        let fields = [
            ValidationField::Id,
            ValidationField::Label,
            ValidationField::Accelerator,
            ValidationField::Action,
            ValidationField::RemapTo,
            ValidationField::Slot,
        ];
        assert_eq!(
            fields.map(ValidationField::code),
            ["id", "label", "accelerator", "action", "remapTo", "slot"]
        );

        let reasons = [
            ValidationReason::EmptyId,
            ValidationReason::IdTooLong { max_chars: 1 },
            ValidationReason::DuplicateId { id: String::new() },
            ValidationReason::EmptyLabel,
            ValidationReason::LabelTooLong { max_chars: 1 },
            ValidationReason::InvalidAccelerator {
                detail: String::new(),
            },
            ValidationReason::UnsafeGlobalShortcut,
            ValidationReason::InvalidKeystroke {
                detail: String::new(),
            },
            ValidationReason::ReservedRuleId,
            ValidationReason::HyperWithRemap,
            ValidationReason::ReservedCommandSlot,
            ValidationReason::DuplicateAccelerator {
                accelerator: String::new(),
            },
            ValidationReason::DuplicateModifierSlot {
                modifier: ModifierKey::Control,
                side: KeySide::Either,
            },
        ];
        assert_eq!(
            reasons.map(|reason| reason.code()),
            [
                "emptyId",
                "idTooLong",
                "duplicateId",
                "emptyLabel",
                "labelTooLong",
                "invalidAccelerator",
                "unsafeGlobalShortcut",
                "invalidKeystroke",
                "reservedRuleId",
                "hyperWithRemap",
                "reservedCommandSlot",
                "duplicateAccelerator",
                "duplicateModifierSlot",
            ]
        );
    }

    #[test]
    fn action_validation_canonicalizes_keystrokes_and_preserves_closed_variants() {
        assert_eq!(
            validate_app_action(AppAction::SendKeystroke(" command + shift + 4 ".into())).unwrap(),
            AppAction::SendKeystroke("Shift+Cmd+4".into())
        );
        assert_eq!(
            validate_app_action(AppAction::UndoWindow).unwrap(),
            AppAction::UndoWindow
        );
        assert!(validate_app_action(AppAction::SendKeystroke("F21".into())).is_err());
    }

    #[test]
    fn hotkey_validation_trims_and_canonicalizes_every_open_field() {
        let mut input = hotkey("  hk-one  ", " command + shift + r ");
        input.label = "  Reload  ".into();
        input.action = AppAction::SendKeystroke(" ctrl + alt + escape ".into());

        let output = validate_hotkey(input).unwrap();
        assert_eq!(output.id, "hk-one");
        assert_eq!(output.label, "Reload");
        assert_eq!(output.accelerator, "Shift+Cmd+R");
        assert_eq!(
            output.action,
            AppAction::SendKeystroke("Ctrl+Alt+Escape".into())
        );
    }

    #[test]
    fn hotkey_validation_canonicalizes_plus_for_global_registration() {
        let output = validate_hotkey(hotkey("zoom", "Cmd+Plus")).unwrap();

        assert_eq!(output.accelerator, "Shift+Cmd+Equal");
    }

    #[test]
    fn plus_and_shift_equal_are_the_same_global_shortcut() {
        let report = validate_hotkeys([
            hotkey("plus", "Cmd+Plus"),
            hotkey("equal", "Shift+Cmd+Equal"),
        ]);

        assert!(report.valid.is_empty());
        assert_eq!(report.invalid.len(), 2);
        assert!(
            report
                .invalid
                .iter()
                .all(|issue| issue.reason.code() == "duplicateAccelerator")
        );
    }

    #[test]
    fn hotkey_id_and_label_boundaries_are_measured_in_characters() {
        let mut input = hotkey(&"é".repeat(MAX_ID_CHARS), "Cmd+K");
        input.label = "界".repeat(MAX_LABEL_CHARS);
        assert!(validate_hotkey(input.clone()).is_ok());

        input.id.push('é');
        let issue = validate_hotkey(input).unwrap_err();
        assert_issue(issue, ValidationField::Id, "idTooLong");

        let mut input = hotkey("hk", "Cmd+K");
        input.label = format!("{}界", "界".repeat(MAX_LABEL_CHARS));
        let issue = validate_hotkey(input).unwrap_err();
        assert_eq!(issue.label.chars().count(), MAX_LABEL_CHARS);
        assert_issue(issue, ValidationField::Label, "labelTooLong");
    }

    #[test]
    fn invalid_hotkey_rows_keep_raw_identity_and_safe_display_label() {
        let mut input = hotkey("  raw-id  ", "Cmd+K");
        input.label = "   ".into();
        let issue = validate_hotkey(input).unwrap_err();
        assert_eq!(issue.id, "  raw-id  ");
        assert_eq!(issue.label, "");
        assert_issue(issue, ValidationField::Label, "emptyLabel");

        let issue = validate_hotkey(hotkey("   ", "Cmd+K")).unwrap_err();
        assert_eq!(issue.id, "   ");
        assert_issue(issue, ValidationField::Id, "emptyId");
    }

    #[test]
    fn hotkeys_reject_malformed_and_system_wide_typing_shortcuts() {
        let malformed = validate_hotkey(hotkey("bad", "Cmd+Frobnicate")).unwrap_err();
        assert_issue(
            malformed,
            ValidationField::Accelerator,
            "invalidAccelerator",
        );

        for accelerator in ["A", "Shift+A"] {
            let unsafe_shortcut = validate_hotkey(hotkey("unsafe", accelerator)).unwrap_err();
            assert_issue(
                unsafe_shortcut,
                ValidationField::Accelerator,
                "unsafeGlobalShortcut",
            );
        }

        assert!(validate_hotkey(hotkey("function", "F1")).is_ok());
        assert!(validate_hotkey(hotkey("chord", "Alt+K")).is_ok());
    }

    #[test]
    fn hotkeys_reject_invalid_send_keystrokes() {
        let mut input = hotkey("bad-action", "Cmd+K");
        input.action = AppAction::SendKeystroke("Frobnicate".into());
        let issue = validate_hotkey(input).unwrap_err();
        assert_issue(issue, ValidationField::Action, "invalidKeystroke");
    }

    #[test]
    fn duplicate_accelerator_quarantines_every_member_including_disabled_rows() {
        let first = hotkey("first", "Cmd+Shift+K");
        let mut disabled_alias = hotkey("disabled", "shift+command+k");
        disabled_alias.enabled = false;
        let unique = hotkey("unique", "Alt+U");

        let report = validate_hotkeys([first, unique, disabled_alias]);
        assert_eq!(
            report
                .valid
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["unique"]
        );
        assert_eq!(report.invalid.len(), 2);
        assert!(report.invalid.iter().all(|issue| {
            issue.field == ValidationField::Accelerator
                && issue.reason.code() == "duplicateAccelerator"
        }));
        let mut ids: Vec<_> = report
            .invalid
            .iter()
            .map(|issue| issue.id.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, ["disabled", "first"]);
    }

    #[test]
    fn hotkey_collision_classification_is_order_independent() {
        let rows = vec![
            hotkey("a", "Cmd+A"),
            hotkey("b", "command+a"),
            hotkey("c", "Alt+C"),
        ];
        let reversed: Vec<_> = rows.iter().cloned().rev().collect();

        let summarize = |report: ValidationReport<Hotkey>| {
            let mut valid: Vec<_> = report.valid.into_iter().map(|row| row.id).collect();
            let mut invalid: Vec<_> = report
                .invalid
                .into_iter()
                .map(|issue| (issue.id, issue.reason.code()))
                .collect();
            valid.sort();
            invalid.sort();
            (valid, invalid)
        };
        assert_eq!(
            summarize(validate_hotkeys(rows)),
            summarize(validate_hotkeys(reversed))
        );
    }

    #[test]
    fn canonical_id_collisions_keep_raw_hotkey_identities_and_reject_every_member() {
        let report = validate_hotkeys([hotkey("same", "Cmd+A"), hotkey(" same ", "Cmd+B")]);

        assert!(report.valid.is_empty());
        assert_eq!(report.invalid.len(), 2);
        assert!(report.invalid.iter().all(|issue| {
            issue.field == ValidationField::Id && issue.reason.code() == "duplicateId"
        }));
        assert_eq!(report.invalid[0].id, "same");
        assert_eq!(report.invalid[1].id, " same ");
    }

    #[test]
    fn post_upsert_hotkey_collection_rejects_the_candidate_and_its_peer() {
        let mut persisted = vec![hotkey("first", "Cmd+A"), hotkey("second", "Cmd+B")];
        let candidate = hotkey("second", "command+a");
        let position = persisted
            .iter()
            .position(|row| row.id == candidate.id)
            .unwrap();
        persisted[position] = candidate;

        let report = validate_hotkeys(persisted);
        assert!(report.valid.is_empty());
        let mut ids: Vec<_> = report.invalid.into_iter().map(|issue| issue.id).collect();
        ids.sort();
        assert_eq!(ids, ["first", "second"]);
    }

    #[test]
    fn modifier_rule_validation_trims_and_canonicalizes_tap_actions() {
        let mut input = rule("  rule-one  ", ModifierKey::Control, KeySide::Either);
        input.label = "  Control tap  ".into();
        input.tap = AppAction::SendKeystroke(" command + space ".into());

        let output = validate_modifier_rule(input).unwrap();
        assert_eq!(output.id, "rule-one");
        assert_eq!(output.label, "Control tap");
        assert_eq!(output.tap, AppAction::SendKeystroke("Cmd+Space".into()));
    }

    #[test]
    fn modifier_rule_id_and_label_boundaries_match_hotkeys() {
        let mut input = rule(
            &"é".repeat(MAX_ID_CHARS),
            ModifierKey::Control,
            KeySide::Either,
        );
        input.label = "界".repeat(MAX_LABEL_CHARS);
        assert!(validate_modifier_rule(input.clone()).is_ok());

        input.id.push('é');
        assert_issue(
            validate_modifier_rule(input).unwrap_err(),
            ValidationField::Id,
            "idTooLong",
        );

        let mut blank = rule("rule", ModifierKey::Control, KeySide::Either);
        blank.label = " ".into();
        assert_issue(
            validate_modifier_rule(blank).unwrap_err(),
            ValidationField::Label,
            "emptyLabel",
        );
    }

    #[test]
    fn persisted_rules_reject_built_in_ids_and_command_slots_even_when_disabled() {
        for built_in in tomari_core::defaults::command_ime_rules() {
            let mut reserved_id = rule(&built_in.id, ModifierKey::Control, KeySide::Either);
            reserved_id.enabled = false;
            assert_issue(
                validate_modifier_rule(reserved_id).unwrap_err(),
                ValidationField::Id,
                "reservedRuleId",
            );
        }

        for side in [KeySide::Left, KeySide::Right] {
            let mut reserved_slot = rule("user", ModifierKey::Command, side);
            reserved_slot.enabled = false;
            assert_issue(
                validate_modifier_rule(reserved_slot).unwrap_err(),
                ValidationField::Slot,
                "reservedCommandSlot",
            );
        }
        assert!(
            validate_modifier_rule(rule("either", ModifierKey::Command, KeySide::Either)).is_ok()
        );
    }

    #[test]
    fn modifier_rules_reject_contradictory_roles_and_invalid_taps() {
        let mut contradictory = rule("hyper", ModifierKey::CapsLock, KeySide::Either);
        contradictory.hyper = true;
        contradictory.remap_to = Some(ModifierKey::Control);
        assert_issue(
            validate_modifier_rule(contradictory).unwrap_err(),
            ValidationField::RemapTo,
            "hyperWithRemap",
        );

        let mut bad_tap = rule("tap", ModifierKey::Control, KeySide::Either);
        bad_tap.tap = AppAction::SendKeystroke("F21".into());
        assert_issue(
            validate_modifier_rule(bad_tap).unwrap_err(),
            ValidationField::Action,
            "invalidKeystroke",
        );
    }

    #[test]
    fn duplicate_modifier_slot_quarantines_every_member_including_disabled_rows() {
        let first = rule("first", ModifierKey::Option, KeySide::Left);
        let mut disabled = rule("disabled", ModifierKey::Option, KeySide::Left);
        disabled.enabled = false;
        let other_side = rule("other", ModifierKey::Option, KeySide::Right);

        let report = validate_modifier_rules([first, other_side, disabled]);
        assert_eq!(
            report
                .valid
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["other"]
        );
        assert_eq!(report.invalid.len(), 2);
        assert!(report.invalid.iter().all(|issue| {
            issue.field == ValidationField::Slot && issue.reason.code() == "duplicateModifierSlot"
        }));
    }

    #[test]
    fn post_upsert_modifier_collection_rejects_the_candidate_and_its_peer() {
        let mut persisted = vec![
            rule("first", ModifierKey::Control, KeySide::Left),
            rule("second", ModifierKey::Option, KeySide::Right),
        ];
        let candidate = rule("second", ModifierKey::Control, KeySide::Left);
        let position = persisted
            .iter()
            .position(|row| row.id == candidate.id)
            .unwrap();
        persisted[position] = candidate;

        let report = validate_modifier_rules(persisted);
        assert!(report.valid.is_empty());
        let mut ids: Vec<_> = report.invalid.into_iter().map(|issue| issue.id).collect();
        ids.sort();
        assert_eq!(ids, ["first", "second"]);
    }

    #[test]
    fn modifier_slot_collision_classification_is_order_independent() {
        let rows = vec![
            rule("a", ModifierKey::Shift, KeySide::Either),
            rule("b", ModifierKey::Shift, KeySide::Either),
            rule("c", ModifierKey::Control, KeySide::Left),
        ];
        let reversed: Vec<_> = rows.iter().cloned().rev().collect();

        let summarize = |report: ValidationReport<ModifierRule>| {
            let mut valid: Vec<_> = report.valid.into_iter().map(|row| row.id).collect();
            let mut invalid: Vec<_> = report
                .invalid
                .into_iter()
                .map(|issue| (issue.id, issue.reason.code()))
                .collect();
            valid.sort();
            invalid.sort();
            (valid, invalid)
        };
        assert_eq!(
            summarize(validate_modifier_rules(rows)),
            summarize(validate_modifier_rules(reversed))
        );
    }

    #[test]
    fn canonical_id_collisions_reject_every_modifier_rule_member() {
        let report = validate_modifier_rules([
            rule("same", ModifierKey::Control, KeySide::Left),
            rule(" same ", ModifierKey::Option, KeySide::Right),
        ]);

        assert!(report.valid.is_empty());
        assert_eq!(report.invalid.len(), 2);
        assert!(report.invalid.iter().all(|issue| {
            issue.field == ValidationField::Id && issue.reason.code() == "duplicateId"
        }));
    }

    #[test]
    fn shipped_defaults_pass_the_same_collection_validators_as_persisted_rows() {
        let hotkeys = validate_hotkeys(tomari_core::defaults::default_hotkeys());
        assert!(hotkeys.is_valid(), "{:?}", hotkeys.invalid);

        let rules = validate_modifier_rules(tomari_core::defaults::default_modifier_rules());
        assert!(rules.is_valid(), "{:?}", rules.invalid);
    }
}
