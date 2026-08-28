//! HID-level Caps Lock remapping, used to make Caps Lock usable as a managed
//! modifier at all.
//!
//! macOS delivers Caps Lock as a *lock*: one `flagsChanged` toggle per physical
//! press, no key-up, and the AlphaShift lock (LED, upper-case) is applied below
//! the event tap. So an event tap alone can neither tell when Caps is released
//! nor stop it locking. The fix is to remap the Caps Lock HID usage to an unused
//! ordinary key — **F18** — via the OS `UserKeyMapping` facility (the mechanism
//! behind `hidutil`, documented in Apple TN2450). The remap happens *before* the
//! lock is interpreted, so Caps never locks; F18 is an ordinary key, so it emits
//! real key-down/up the tap can treat as the Caps modifier ([`crate::eventtap`]).
//!
//! We shell out to `/usr/bin/hidutil` rather than call the private
//! `IOHIDEventSystemClient` API. Setting the property replaces the *whole*
//! `UserKeyMapping` list, so taking Caps Lock over ([`apply_with`]) and handing
//! it back ([`clear_with`]) read the current list first and write it back with
//! only our Caps Lock → F18 entry added or removed —
//! a user's own pre-existing `hidutil` mappings (another key remap, say) survive
//! rather than being wiped. Because that read is what the whole property is
//! rebuilt from, [`parse_entries`] is strict: output it does not fully
//! understand is an error, never a partial list, so a format change or a
//! truncated read cannot silently drop the mappings it failed to read.
//!
//! ## Owning the Caps Lock source
//!
//! One mapping cannot merely survive, because it occupies the very slot we need:
//! an entry the user put on the *Caps Lock source itself* (Caps → Escape, say).
//! Taking Caps Lock over has to replace it, and taking it *back* has to restore
//! it — deleting our entry is not enough. Nor can the live list say who owns
//! what: `UserKeyMapping` records no provenance, so a live Caps Lock → F18 is
//! equally plausibly the user's own.
//!
//! So ownership is tracked explicitly, in a [`Claim`] on disk. There is no
//! atomic commit across the record and the OS property, so the record
//! distinguishes *intending* to take the source from *having* taken it:
//!
//! * **No record** ([`Claim::Unowned`]) — Tomari holds nothing. A live
//!   Caps Lock → F18 is the user's and is left strictly alone.
//! * **Pending** — the write-ahead step ran but the OS write is unconfirmed, so
//!   a live Caps Lock → F18 *might* be ours and might be the user's. The record
//!   carries the exact list we set out to write: a live list that matches it is
//!   attributed to us and the claim is confirmed (or released) from there; one
//!   that does not is left strictly alone, claim and warning included.
//! * **Held** — the OS write is confirmed. A live Caps Lock → F18 is ours, and
//!   the record carries whatever destination it displaced for [`clear_with`] to
//!   restore.
//!
//! Every transition fails closed: an unwritable or unreadable record aborts it
//! rather than risk losing the user's mapping. Taking over always rewrites the
//! record, so a stale one left by a crash is replaced rather than deleted — no
//! delete has to succeed for correctness. And every release is gated on our
//! Caps Lock → F18 still being live, so a change made outside Tomari in the
//! meantime wins: the claim is dropped unused instead of written over it.
//!
//! What no record scheme can resolve is our exact mapping disappearing and being
//! recreated by something else between two reconciles: the OS keeps no
//! provenance, so that is indistinguishable from ours never having moved. Since
//! the record is an atomic rename inside our own data directory, reaching that
//! state needs a record write to fail while an OS write succeeds *and* the user
//! to reconfigure Caps Lock in the same window. It is therefore reported rather
//! than resolved — such a reconcile returns
//! [`reconciled: false`](ReconcileOutcome::reconciled), which surfaces as the
//! `capsLockRemap` warning. What happens next depends on which step failed: an
//! unwritten claim or an unapplied mapping is simply retried — a write-ahead
//! whose write is known not to have landed (our entry is not live right after
//! the failure) is retracted at once, so it cannot later match a list the user
//! sets themselves. A *confirm* that
//! failed over a live remap leaves `Pending`, and the next reconcile repairs it
//! by evidence: the write-ahead record names the whole list we were about to
//! write, so a live list equal to it is ours — the claim is confirmed, or the
//! source handed back, exactly as if the confirm had landed. A live
//! Caps Lock → F18 in any *other* list is unattributable; it, the record and the
//! warning all stay put until the user resolves it. What is never done is to
//! quietly drop the claim over a live remap, which would leave Caps Lock stuck
//! on F18 with nothing left to say so.
//!
//! Races *inside* Tomari are closed by serializing the whole reconcile (live
//! read, record, OS write, proxy flag) on [`RECONCILE`] — every path that
//! touches the property goes through it. A writer outside Tomari cannot be
//! locked out and the property offers no atomic swap, so [`commit_entries`]
//! brackets each write with the checks that are possible: the live list must
//! still be what the plan was built from, and our own entry must afterwards be
//! what we wrote. A write that lands in between is still lost.
//!
//! The mapping is per-user, needs no elevated privileges, and persists until
//! reboot or removal — so we reconcile it on every tap (re)start and clear it on
//! quit.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use crate::locks::MutexExt;

/// Full HID usage (`0x7_0000_0000 | usage`) of Caps Lock.
const CAPS_USAGE: u64 = 0x7_0000_0039;
/// Full HID usage of F18 — an ordinary key with no default macOS binding,
/// which Caps Lock is remapped onto.
const F18_USAGE: u64 = 0x7_0000_006D;

/// The virtual keycode F18 arrives as once Caps Lock is remapped to it. The tap
/// treats this keycode as the Caps Lock modifier.
pub const F18_KEYCODE: i64 = 79;

/// How long one `hidutil` call may take before it is killed. It normally
/// returns in milliseconds; a wedged one would otherwise hold a settings save,
/// the wake reset or quit indefinitely.
const HIDUTIL_DEADLINE: Duration = Duration::from_secs(5);

/// Run `hidutil` with `args` under [`HIDUTIL_DEADLINE`], returning its stdout
/// on a clean exit. A timeout, non-zero exit or failure to start is an `Err`
/// like any other, so every caller's fail-closed handling covers it.
fn hidutil(args: &[&str]) -> Result<Vec<u8>, String> {
    let outcome = crate::childproc::output_with_deadline(
        Command::new("/usr/bin/hidutil").args(args),
        HIDUTIL_DEADLINE,
    )
    .map_err(|e| format!("failed to run hidutil: {e}"))?;
    match outcome {
        crate::childproc::Outcome::Exited {
            status,
            stdout,
            stderr,
        } => {
            if status.success() {
                Ok(stdout)
            } else {
                Err(format!(
                    "hidutil exited with {status}: {}",
                    String::from_utf8_lossy(&stderr).trim()
                ))
            }
        }
        crate::childproc::Outcome::TimedOut => Err(format!(
            "hidutil did not finish within {}s and was killed",
            HIDUTIL_DEADLINE.as_secs()
        )),
    }
}

fn set_mapping(json: &str) -> Result<(), String> {
    hidutil(&["property", "--set", json]).map(|_| ())
}

/// Read the current `UserKeyMapping` entries as `(src, dst)` usage pairs. `Err`
/// whenever the list cannot be established *exactly* — `hidutil` unavailable, a
/// non-zero exit, or output this parser does not fully understand — because
/// every write replaces the whole property from what was read here. An
/// approximate read would silently drop the entries it failed to understand.
fn read_entries() -> Result<Vec<(u64, u64)>, String> {
    let stdout = hidutil(&["property", "--get", "UserKeyMapping"])?;
    parse_entries(&String::from_utf8_lossy(&stdout))
}

/// The `UserKeyMapping` entries in `hidutil property --get` output.
///
/// Strict by design: setting the property rewrites the *whole* list from what
/// this returns, so anything less than a complete understanding of the output
/// has to fail rather than parse what it recognizes and drop the rest. A changed
/// output format, an extra field, an unexpected number format, or a truncated
/// read would otherwise delete mappings that have nothing to do with Tomari.
///
/// The format is an old-style property list: a parenthesized list of
/// `{ key = value; … }` dictionaries, printed as `(null)` when the property is
/// not set at all.
fn parse_entries(text: &str) -> Result<Vec<(u64, u64)>, String> {
    let entries = parse_entry_list(text)?;
    // One source mapped twice is not a list we can reason about: which entry
    // owns that key is undefined, so "the mapping on the Caps Lock source" —
    // what the claim is about, and what a write is checked against — would be a
    // guess. Refuse it like any other shape we do not understand.
    for (index, &(src, _)) in entries.iter().enumerate() {
        if entries[..index].iter().any(|&(earlier, _)| earlier == src) {
            return Err(malformed("the same source is mapped twice"));
        }
    }
    Ok(entries)
}

fn parse_entry_list(text: &str) -> Result<Vec<(u64, u64)>, String> {
    let body = text.trim();
    // How `hidutil` reports "no such property" — distinct from a set-but-empty
    // list, but the same thing to us.
    if body == "(null)" {
        return Ok(Vec::new());
    }
    let inner = body
        .strip_prefix('(')
        .and_then(|b| b.strip_suffix(')'))
        .ok_or_else(|| malformed("expected a parenthesized list"))?;
    let mut rest = inner.trim();
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    loop {
        let after_open = rest
            .strip_prefix('{')
            .ok_or_else(|| malformed("expected an entry"))?;
        let (fields, after_close) = after_open
            .split_once('}')
            .ok_or_else(|| malformed("unterminated entry"))?;
        entries.push(parse_entry(fields)?);

        // What may sit between two entries: whitespace, a single comma, or a
        // comma with whitespace around it — some macOS versions print one, some
        // the other. Nothing at all, a doubled comma, or a comma with no entry
        // after it are shapes we do not recognize, and letting one through would
        // read as a shorter list than the property really holds.
        let trimmed = after_close.trim_start();
        let after_comma = trimmed.strip_prefix(',');
        let separated = trimmed.len() < after_close.len() || after_comma.is_some();
        rest = after_comma.map_or(trimmed, str::trim_start);
        if rest.is_empty() {
            return if after_comma.is_some() {
                Err(malformed("trailing separator"))
            } else {
                Ok(entries)
            };
        }
        if !separated {
            return Err(malformed("expected a separator between entries"));
        }
    }
}

/// One entry's `key = value;` fields. Every field must be terminated by its
/// semicolon, be one of the two keys we know, appear exactly once, and carry a
/// parseable usage — a missing terminator, an empty field, or an unrecognized or
/// repeated key means this is not the format we can safely rewrite.
fn parse_entry(fields: &str) -> Result<(u64, u64), String> {
    let mut src = None;
    let mut dst = None;
    let mut rest = fields.trim();
    while !rest.is_empty() {
        let (field, after) = rest
            .split_once(';')
            .ok_or_else(|| malformed("expected `;` after a field"))?;
        rest = after.trim_start();
        let field = field.trim();
        if field.is_empty() {
            return Err(malformed("empty field"));
        }
        let (key, value) = field
            .split_once('=')
            .ok_or_else(|| malformed("expected `key = value`"))?;
        let usage =
            parse_usage(value).ok_or_else(|| malformed("expected a decimal or hex HID usage"))?;
        let slot = match key.trim() {
            "HIDKeyboardModifierMappingSrc" => &mut src,
            "HIDKeyboardModifierMappingDst" => &mut dst,
            _ => return Err(malformed("unrecognized field")),
        };
        if slot.replace(usage).is_some() {
            return Err(malformed("repeated field"));
        }
    }
    match (src, dst) {
        (Some(src), Some(dst)) => Ok((src, dst)),
        _ => Err(malformed("entry is missing a source or destination")),
    }
}

fn malformed(reason: &str) -> String {
    format!("could not parse hidutil key mappings: {reason}")
}

/// Serialize `(src, dst)` pairs into the JSON `hidutil property --set` expects.
fn serialize_mapping(entries: &[(u64, u64)]) -> String {
    let body = entries
        .iter()
        .map(|(src, dst)| {
            format!(
                r#"{{"HIDKeyboardModifierMappingSrc":{src:#x},"HIDKeyboardModifierMappingDst":{dst:#x}}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"UserKeyMapping":[{body}]}}"#)
}

/// Parse a HID usage printed by `hidutil`, which uses decimal or hex (`0x…`)
/// depending on the macOS version.
fn parse_usage(s: &str) -> Option<u64> {
    let t = s.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => t.parse().ok(),
    }
}

/// Whether Tomari holds the Caps Lock source, and what it displaced doing so.
/// `UserKeyMapping` carries no provenance, so this — not the live list — is what
/// distinguishes our remap from an identical one the user set themselves. The
/// payload is the destination displaced to take the source, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Claim {
    /// Tomari does not hold the source. A live Caps Lock → F18 is the user's own
    /// and must be left alone.
    Unowned,
    /// Write-ahead: recorded before the OS write, and left behind if that write
    /// (or its confirmation) never completed. Whether a live Caps Lock → F18 is
    /// ours is decided against the list this names — see [`WriteAhead`].
    Pending(WriteAhead),
    /// The OS write is confirmed: a live Caps Lock → F18 is ours to take back.
    Held(Option<u64>),
}

/// What a take-over recorded before writing: the destination it was about to
/// displace, and the *whole list* it was about to write.
///
/// The list is the evidence a later reconcile attributes by. `UserKeyMapping`
/// has no provenance, but `hidutil --set` replaces the list wholesale, so a live
/// list identical to the one we planned is ours in every case but one — the
/// user independently setting exactly the same complete list while our confirm
/// was failing — and that coincidence is accepted. Anything else with a live
/// Caps Lock → F18 stays unattributable and untouched.
///
/// `planned` is `None` only for a record written before the plan was recorded;
/// such a claim can never be attributed and is reported until resolved by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WriteAhead {
    displaced: Option<u64>,
    planned: Option<Vec<(u64, u64)>>,
}

impl WriteAhead {
    /// Whether `live` is the list this write-ahead set out to write, as a set
    /// — `hidutil` is not guaranteed to print entries back in the order set.
    fn matches(&self, live: &[(u64, u64)]) -> bool {
        self.planned.as_ref().is_some_and(|planned| {
            let mut planned = planned.clone();
            let mut live = live.to_vec();
            planned.sort_unstable();
            live.sort_unstable();
            planned == live
        })
    }
}

const PENDING_MARKER: &str = "pending";
const HELD_MARKER: &str = "held";
const PLAN_MARKER: &str = "plan";

fn serialize_claim(claim: &Claim) -> Option<String> {
    let (marker, displaced, planned) = match claim {
        Claim::Unowned => return None,
        Claim::Pending(ahead) => (PENDING_MARKER, ahead.displaced, ahead.planned.as_deref()),
        Claim::Held(displaced) => (HELD_MARKER, *displaced, None),
    };
    let mut text = marker.to_string();
    if let Some(dst) = displaced {
        text.push_str(&format!(" {dst}"));
    }
    if let Some(planned) = planned {
        text.push(' ');
        text.push_str(PLAN_MARKER);
        for (src, dst) in planned {
            text.push_str(&format!(" {src}:{dst}"));
        }
    }
    Some(text)
}

/// Parse a record's contents into a claim. `Err` for anything unrecognized — a
/// torn or corrupt record must never read as a weaker claim than it was, which
/// could delete a mapping we owed the user back.
///
/// Shape: `held [displaced]` or `pending [displaced] [plan src:dst ...]`. The
/// `plan` section is what a write-ahead is attributed by later; a `pending`
/// record without one is accepted (it predates the section) but never
/// attributable.
fn parse_claim(text: &str) -> Result<Claim, String> {
    let unrecognized = || "unrecognized caps-lock claim record".to_string();
    let mut fields = text.split_whitespace().peekable();
    let marker = fields.next().ok_or_else(unrecognized)?;
    let displaced = match fields.peek() {
        Some(&value) if value != PLAN_MARKER => {
            fields.next();
            Some(parse_usage(value).ok_or_else(unrecognized)?)
        }
        _ => None,
    };
    let planned = match fields.next() {
        None => None,
        Some(PLAN_MARKER) => {
            let mut planned = Vec::new();
            for pair in fields.by_ref() {
                let (src, dst) = pair.split_once(':').ok_or_else(unrecognized)?;
                planned.push((
                    parse_usage(src).ok_or_else(unrecognized)?,
                    parse_usage(dst).ok_or_else(unrecognized)?,
                ));
            }
            Some(planned)
        }
        Some(_) => return Err(unrecognized()),
    };
    match (marker, planned) {
        (PENDING_MARKER, planned) => Ok(Claim::Pending(WriteAhead { displaced, planned })),
        (HELD_MARKER, None) => Ok(Claim::Held(displaced)),
        _ => Err(unrecognized()),
    }
}

/// Path of the ownership record. Its *absence* is [`Claim::Unowned`].
///
/// Deliberately not fsync'd: `UserKeyMapping` itself does not survive a reboot,
/// so a record lost to power failure has nothing left to restore — and the next
/// [`apply_with`] finds no Caps Lock entry to displace and rewrites the record
/// anyway. The rename below is still atomic, so a record is never read torn.
fn claim_path() -> Result<PathBuf, String> {
    tomari_core::AppPaths::resolve()
        .map(|p| p.data_dir.join("capsmap.claim"))
        .map_err(|e| format!("could not resolve the data directory: {e}"))
}

/// The recorded claim. A missing record is [`Claim::Unowned`]; every *other*
/// failure (permissions, I/O, corrupt contents) is an error, so a transition
/// that depends on knowing what we displaced aborts rather than guessing.
fn read_claim() -> Result<Claim, String> {
    let path = claim_path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_claim(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Claim::Unowned),
        Err(e) => Err(format!("could not read the caps-lock claim record: {e}")),
    }
}

/// Record `claim`, replacing whatever was there. Written via a temporary file
/// and an atomic rename, so a crash mid-write cannot leave a torn record.
fn write_claim(claim: Claim) -> Result<(), String> {
    let Some(body) = serialize_claim(&claim) else {
        return clear_claim();
    };
    let path = claim_path()?;
    let tmp = path.with_extension("claim.tmp");
    std::fs::write(&tmp, body)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("could not record the caps-lock claim: {e}"))
}

/// Drop the record, releasing the claim. A record that is already gone is
/// success.
fn clear_claim() -> Result<(), String> {
    let path = claim_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not drop the caps-lock claim record: {e}")),
    }
}

/// The side effects the Caps Lock remap drives, factored behind a trait so
/// [`apply_with`], [`clear_with`] and [`reconcile_with`] can be unit tested
/// against a fake — exercising a conflicting user mapping, a user-owned
/// Caps Lock → F18, an unwritable and an unreadable claim, and a foreign change
/// to the Caps Lock source without a real `hidutil` or on-disk record.
trait CapsMapSys {
    /// The live `UserKeyMapping` entries, or why they could not be established
    /// exactly.
    fn read_entries(&self) -> Result<Vec<(u64, u64)>, String>;
    /// Replace the whole `UserKeyMapping` list with `entries`.
    fn set_entries(&self, entries: &[(u64, u64)]) -> Result<(), String>;
    /// The recorded claim (a missing record is [`Claim::Unowned`]).
    fn read_claim(&self) -> Result<Claim, String>;
    /// Record `claim`, replacing any previous record.
    fn write_claim(&self, claim: Claim) -> Result<(), String>;
    /// Drop the record, releasing the claim.
    fn clear_claim(&self) -> Result<(), String>;
}

/// Whether Caps Lock → F18 is live, whoever put it there. Checked
/// *structurally*, so the two usages appearing in unrelated entries (e.g.
/// `Caps → X` plus `Y → F18`) is not mistaken for the remap.
fn caps_to_f18_live(sys: &impl CapsMapSys) -> bool {
    sys.read_entries()
        .is_ok_and(|entries| entries.contains(&(CAPS_USAGE, F18_USAGE)))
}

/// What [`apply_with`] should do given the live entry list and our claim.
#[derive(Debug, PartialEq, Eq)]
enum ApplyPlan {
    /// Caps Lock → F18 is already live and accounted for — held by us, or the
    /// user's own with no claim against it. Either way the tap gets the F18
    /// events it needs, so nothing is written: an entry we did not create must
    /// not become ours to remove later.
    AlreadyInEffect,
    /// Caps Lock → F18 is live and the list is exactly the one our unconfirmed
    /// write-ahead set out to write: the write landed and only the confirm was
    /// lost. Finish it — record the claim as held, displacement and all.
    Confirm { displaced: Option<u64> },
    /// Caps Lock → F18 is live under an unconfirmed claim, in a list that is
    /// *not* the one we planned. Whether we set it is unknowable, so nothing is
    /// written and nothing is dropped: the claim stays as the record that this
    /// is unresolved, and the reconcile reports it.
    Unattributable,
    /// Record the write-ahead claim, write `entries`, then confirm the claim.
    Take {
        entries: Vec<(u64, u64)>,
        displaced: Option<u64>,
    },
}

/// Plan taking the Caps Lock source over: every foreign mapping is kept as-is,
/// any entry on the Caps Lock source is replaced by ours, and the destination it
/// pointed at is reported so it can be recorded before being overwritten. The
/// record is *rewritten*, never merged, so a stale claim from an earlier session
/// is replaced outright rather than having to be deleted first.
fn plan_apply(mut entries: Vec<(u64, u64)>, claim: Claim) -> ApplyPlan {
    if entries.contains(&(CAPS_USAGE, F18_USAGE)) {
        return match claim {
            Claim::Unowned | Claim::Held(_) => ApplyPlan::AlreadyInEffect,
            Claim::Pending(ahead) if ahead.matches(&entries) => ApplyPlan::Confirm {
                displaced: ahead.displaced,
            },
            Claim::Pending(_) => ApplyPlan::Unattributable,
        };
    }
    let displaced = caps_source(&entries);
    entries.retain(|&(src, _)| src != CAPS_USAGE);
    entries.push((CAPS_USAGE, F18_USAGE));
    ApplyPlan::Take { entries, displaced }
}

/// What [`clear_with`] should do given the live entry list and our claim.
#[derive(Debug, PartialEq, Eq)]
enum ClearPlan {
    /// We never held the Caps Lock source: leave the list — and any
    /// Caps Lock → F18 of the user's own — completely alone.
    Unclaimed,
    /// We hold a claim we cannot act on: our entry is not live — something
    /// outside Tomari moved the source and that newer intent wins, or our write
    /// never landed. Drop it without touching the list.
    DropClaim,
    /// Caps Lock → F18 is live under an unconfirmed claim, in a list that is
    /// not the one we planned. Not ours to take back, not ours to disown — see
    /// [`ApplyPlan::Unattributable`].
    Unattributable,
    /// Write `entries`: ours removed, and the displaced mapping restored when
    /// the claim named one. Then drop the claim.
    Release { entries: Vec<(u64, u64)> },
}

/// Plan giving the Caps Lock source back. A write is authorized by a *confirmed*
/// claim over a still live Caps Lock → F18 — or by a write-ahead whose planned
/// list is exactly what is live, which is the same thing minus the lost confirm.
/// Every weaker state lets go, except the one that cannot tell whose the live
/// remap is.
fn plan_clear(mut entries: Vec<(u64, u64)>, claim: Claim) -> ClearPlan {
    let live = entries.contains(&(CAPS_USAGE, F18_USAGE));
    let displaced = match claim {
        Claim::Unowned => return ClearPlan::Unclaimed,
        Claim::Pending(ahead) => {
            if !live {
                return ClearPlan::DropClaim;
            }
            if !ahead.matches(&entries) {
                return ClearPlan::Unattributable;
            }
            ahead.displaced
        }
        Claim::Held(displaced) => displaced,
    };
    if !live {
        return ClearPlan::DropClaim;
    }
    entries.retain(|&pair| pair != (CAPS_USAGE, F18_USAGE));
    if let Some(dst) = displaced {
        entries.push((CAPS_USAGE, dst));
    }
    ClearPlan::Release { entries }
}

/// Write `entries` as the whole `UserKeyMapping`, bracketed by the two checks
/// `hidutil` makes possible.
///
/// **Before**: the live list must still be exactly the `expected` one the plan
/// was built from. Anything else — including a change to an entry that is none
/// of our business — means our list is stale, and writing it would revert that
/// change; so we do not write, and the next reconcile re-plans from the new
/// list.
///
/// **After**: only the Caps Lock source has to be what we wrote. Deliberately
/// narrower than the pre-check: another writer landing on an unrelated mapping
/// in the same instant is not a failure of ours, and calling it one would leave
/// the claim unconfirmed over a remap we really did make — which the next
/// reconcile would then disown, stranding it.
///
/// This is not a compare-and-swap. The property has no revision to swap on and
/// nothing here is atomic, so an outside write that lands between the pre-check
/// and ours is still lost — the brackets narrow that window and catch the races
/// that fall outside it, rather than eliminating them.
fn commit_entries(
    sys: &impl CapsMapSys,
    expected: &[(u64, u64)],
    entries: &[(u64, u64)],
) -> Result<(), CommitError> {
    let live = sys.read_entries().map_err(CommitError::NotAttempted)?;
    if live != expected {
        return Err(CommitError::NotAttempted(
            "hidutil key mappings changed while Tomari was updating them".into(),
        ));
    }
    sys.set_entries(entries).map_err(CommitError::Uncertain)?;
    let after = sys.read_entries().map_err(CommitError::Uncertain)?;
    if caps_source(&after) != caps_source(entries) {
        return Err(CommitError::Uncertain(
            "the Caps Lock key mapping did not take the value Tomari wrote".into(),
        ));
    }
    Ok(())
}

/// How a [`commit_entries`] failed — split by whether the write was ever handed
/// to the OS, because a write-ahead left behind has to be treated differently in
/// each case (see [`retract_write_ahead`]).
#[derive(Debug, PartialEq, Eq)]
enum CommitError {
    /// Failed before `hidutil --set` ran: the list is exactly as it was.
    NotAttempted(String),
    /// `hidutil --set` ran, or may have; whether it took effect is not known
    /// from the error alone.
    Uncertain(String),
}

impl CommitError {
    fn into_message(self) -> String {
        match self {
            Self::NotAttempted(m) | Self::Uncertain(m) => m,
        }
    }
}

/// The destination on the Caps Lock source, if any. Both plans leave at most one
/// such entry, so this is the whole of what a write of ours is responsible for.
fn caps_source(entries: &[(u64, u64)]) -> Option<u64> {
    entries
        .iter()
        .find(|&&(src, _)| src == CAPS_USAGE)
        .map(|&(_, dst)| dst)
}

fn apply_with(sys: &impl CapsMapSys) -> Result<(), String> {
    let live = sys.read_entries()?;
    match plan_apply(live.clone(), sys.read_claim()?) {
        ApplyPlan::AlreadyInEffect => Ok(()),
        ApplyPlan::Confirm { displaced } => {
            tracing::info!(
                "the live key mappings are exactly what an unconfirmed caps-lock claim set out \
                 to write; confirming it"
            );
            sys.write_claim(Claim::Held(displaced))
        }
        ApplyPlan::Unattributable => Err(UNATTRIBUTABLE.into()),
        // Write-ahead, then confirm: what we are about to overwrite has to be
        // recorded before it is gone, or nothing could ever put it back, and the
        // record must not read as *held* until the OS write has actually landed.
        // A record that cannot be written aborts the remap rather than risking
        // the user's mapping — `reconcile` then reports it inactive, which
        // surfaces as the `capsLockRemap` warning.
        ApplyPlan::Take { entries, displaced } => {
            sys.write_claim(Claim::Pending(WriteAhead {
                displaced,
                planned: Some(entries.clone()),
            }))?;
            if let Err(e) = commit_entries(sys, &live, &entries) {
                retract_write_ahead(sys, &e);
                return Err(e.into_message());
            }
            sys.write_claim(Claim::Held(displaced))
        }
    }
}

/// After a failed commit, drop the write-ahead whenever it is certain nothing of
/// ours reached the OS. A write-ahead kept past that would still name a plan,
/// and a list the user later sets that happens to equal it (a lone
/// Caps Lock → F18, say) would read as ours to take back.
///
/// * The write was never attempted: retract unconditionally. Whatever is live —
///   even our exact plan, set by someone else in the race the pre-check caught —
///   is not ours.
/// * The write was attempted: retract only if our entry is not live *right
///   now*, in the same reconcile, before anyone else could have put it there.
///   When it is live the write plausibly landed, so the write-ahead stays for the
///   next reconcile to attribute; when the list cannot be read at all, it stays
///   too — not knowing is not the same as nothing having landed.
///
/// Best effort: the commit has already failed, so a retraction that fails as
/// well is logged and the write-ahead simply remains.
fn retract_write_ahead(sys: &impl CapsMapSys, failure: &CommitError) {
    let nothing_landed = match failure {
        CommitError::NotAttempted(_) => true,
        CommitError::Uncertain(_) => match sys.read_entries() {
            Ok(now) => !now.contains(&(CAPS_USAGE, F18_USAGE)),
            Err(e) => {
                tracing::warn!(error = %e, "cannot tell whether the caps-lock write landed; keeping the write-ahead");
                false
            }
        },
    };
    if nothing_landed && let Err(e) = sys.clear_claim() {
        tracing::warn!(error = %e, "could not retract a caps-lock write-ahead whose write did not land");
    }
}

/// Why a reconcile over a live Caps Lock → F18 under an unconfirmed claim does
/// nothing: it cannot be told apart from one the user set.
const UNATTRIBUTABLE: &str = "a Caps Lock → F18 key mapping is live under an unconfirmed \
     claim, in a list Tomari did not write; leaving it and the claim in place";

fn clear_with(sys: &impl CapsMapSys) -> Result<(), String> {
    let live = sys.read_entries()?;
    // Fail closed: without a readable claim we do not know whether a live
    // Caps Lock → F18 is ours, nor what it displaced. Leave everything as it is.
    match plan_clear(live.clone(), sys.read_claim()?) {
        ClearPlan::Unclaimed => Ok(()),
        ClearPlan::DropClaim => sys.clear_claim(),
        ClearPlan::Unattributable => Err(UNATTRIBUTABLE.into()),
        // The list first, the claim second: releasing the claim while our entry
        // is still live would orphan it — no later clear would recognize it as
        // ours. The reverse order merely leaves a claim the next clear drops.
        ClearPlan::Release { entries } => {
            commit_entries(sys, &live, &entries).map_err(CommitError::into_message)?;
            sys.clear_claim()
        }
    }
}

/// The result of a reconcile. Two separate facts, because the OS mapping and the
/// ownership record can disagree: a `hidutil` write that lands while the record
/// that governs undoing it does not leaves the *remap* working and the
/// *reconcile* incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileOutcome {
    /// Whether F18 events should be treated as Caps Lock — the real live state,
    /// not the request, so the tap stays in step even when `hidutil` fails.
    pub proxy_active: bool,
    /// Whether the requested state was fully reached, ownership record
    /// included. `false` is a degraded state worth surfacing: the mapping may
    /// look right while the record that decides how it is undone does not.
    pub reconciled: bool,
}

/// Bring the HID remap into line with whether Caps Lock should be managed:
/// take the source over when it should be and we do not hold it, hand it back
/// (restoring what we displaced) when it should not be. Both directions start
/// by reading the live state, so an already-correct one costs a single
/// `hidutil --get` and no write — and running the release direction
/// unconditionally is what drops a claim invalidated while we were not looking.
fn reconcile_with(sys: &impl CapsMapSys, should_manage: bool) -> ReconcileOutcome {
    let result = if should_manage {
        apply_with(sys)
    } else {
        clear_with(sys)
    };
    match result {
        Ok(()) => ReconcileOutcome {
            proxy_active: should_manage,
            reconciled: true,
        },
        Err(e) => {
            tracing::warn!(error = %e, should_manage, "failed to reconcile the caps-lock HID remap");
            ReconcileOutcome {
                proxy_active: caps_to_f18_live(sys),
                reconciled: false,
            }
        }
    }
}

/// Production [`CapsMapSys`]: the real `hidutil` calls and on-disk record.
struct RealSys;

impl CapsMapSys for RealSys {
    fn read_entries(&self) -> Result<Vec<(u64, u64)>, String> {
        read_entries()
    }
    fn set_entries(&self, entries: &[(u64, u64)]) -> Result<(), String> {
        set_mapping(&serialize_mapping(entries))
    }
    fn read_claim(&self) -> Result<Claim, String> {
        read_claim()
    }
    fn write_claim(&self, claim: Claim) -> Result<(), String> {
        write_claim(claim)
    }
    fn clear_claim(&self) -> Result<(), String> {
        clear_claim()
    }
}

/// Serializes every Caps Lock reconcile. Each one is a read-modify-write across
/// two stores (the claim record and the OS property) plus the proxy flag, and it
/// is reachable concurrently from the settings commands, the wake and session
/// callbacks, the permission poll and quit. Without this, one caller's live read
/// could straddle another's write — dropping a claim whose mapping the other had
/// just recorded, or leaving the flag set from whichever call happened to return
/// last. Holding it across the whole sequence also gives the record's temporary
/// file a single writer.
static RECONCILE: Mutex<()> = Mutex::new(());

/// The live Caps Lock remap status, packed into one word so the two facts a
/// reader combines — is F18 standing in for Caps Lock, and did the reconcile
/// that made it so fully succeed — always come from the *same* reconcile. Two
/// separate atomics could pair an older mapping with a newer verdict when a
/// reconcile lands between the loads. Written only under [`RECONCILE`], from the
/// reconcile's *actual* outcome; read on the tap thread for every keystroke, so
/// it is an atomic rather than behind a lock. Starts as "not remapped, in
/// step" — nothing has been asked for yet.
static CAPS_STATUS: AtomicU8 = AtomicU8::new(STATUS_RECONCILED);

/// [`CAPS_STATUS`] bit: Caps Lock is remapped to F18.
const STATUS_PROXY_ACTIVE: u8 = 0b01;
/// [`CAPS_STATUS`] bit: the last reconcile fully reached its request.
const STATUS_RECONCILED: u8 = 0b10;

fn pack_status(outcome: ReconcileOutcome) -> u8 {
    (if outcome.proxy_active {
        STATUS_PROXY_ACTIVE
    } else {
        0
    }) | (if outcome.reconciled {
        STATUS_RECONCILED
    } else {
        0
    })
}

/// The outcome of the reconcile that most recently ran, read as one snapshot.
pub fn live_status() -> ReconcileOutcome {
    let bits = CAPS_STATUS.load(Ordering::SeqCst);
    ReconcileOutcome {
        proxy_active: bits & STATUS_PROXY_ACTIVE != 0,
        reconciled: bits & STATUS_RECONCILED != 0,
    }
}

/// Whether F18 key events should be treated as Caps Lock.
pub fn caps_proxy_active() -> bool {
    live_status().proxy_active
}

/// Whether the live mapping already matches `should_manage` *and* the reconcile
/// that produced it was clean — the answer a caller that is putting a reconcile
/// off (Caps Lock is held) gives in its place, so a mismatch left by an earlier
/// failure keeps surfacing instead of being masked.
pub fn matches(should_manage: bool) -> bool {
    let status = live_status();
    status.proxy_active == should_manage && status.reconciled
}

/// See [`reconcile_with`]. The status is published from inside the lock, so it
/// always reflects the reconcile that most recently *ran*, not whichever one
/// happened to return last.
#[must_use]
pub fn reconcile(should_manage: bool) -> ReconcileOutcome {
    let _serialized = RECONCILE.lock_safe();
    let outcome = reconcile_with(&RealSys, should_manage);
    CAPS_STATUS.store(pack_status(outcome), Ordering::SeqCst);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A stand-in for a user's own pre-existing remap (some other key), used to
    /// prove take/release never touch mappings that are not ours.
    const OTHER_SRC: u64 = 0x7_0000_0004;
    const OTHER_DST: u64 = 0x7_0000_0005;
    /// A stand-in for a mapping the user put on the Caps Lock source itself
    /// (Caps → Escape, say) — the one entry taking Caps Lock over must displace.
    const USER_CAPS_DST: u64 = 0x7_0000_0029;

    /// Fake [`CapsMapSys`] over an in-memory entry list and claim, with each
    /// side effect independently failable.
    struct FakeSys {
        entries: RefCell<Result<Vec<(u64, u64)>, String>>,
        claim: RefCell<Claim>,
        /// Whether the claim record can be read at all (permissions, corruption).
        claim_readable: bool,
        /// Whether the claim record can be written.
        claim_writable: bool,
        /// How many claim writes still succeed. Taking the source over writes
        /// twice (write-ahead, then confirm), so `Some(1)` fails the *confirm*
        /// alone — the case where the OS mapping lands but ownership does not.
        claim_writes_left: RefCell<Option<usize>>,
        /// Whether the claim record can be dropped.
        claim_erasable: bool,
        /// Whether `hidutil property --set` is allowed to succeed.
        can_set: bool,
        /// A list an outside writer installs right after our next read — how a
        /// concurrent `hidutil` lands between the plan and the write.
        steal_after_read: RefCell<Option<Vec<(u64, u64)>>>,
        /// What the live list actually becomes when we set it, when that is not
        /// what we asked for.
        set_lands_as: RefCell<Option<Vec<(u64, u64)>>>,
        /// How many times the entry list was written.
        writes: RefCell<usize>,
        /// Reads still allowed before the list becomes unreadable — a `hidutil`
        /// that stops answering part-way through a reconcile.
        fail_reads_after: RefCell<Option<usize>>,
    }

    impl FakeSys {
        fn new(entries: &[(u64, u64)]) -> Self {
            Self {
                entries: RefCell::new(Ok(entries.to_vec())),
                claim: RefCell::new(Claim::Unowned),
                claim_readable: true,
                claim_writable: true,
                claim_writes_left: RefCell::new(None),
                claim_erasable: true,
                can_set: true,
                steal_after_read: RefCell::new(None),
                set_lands_as: RefCell::new(None),
                writes: RefCell::new(0),
                fail_reads_after: RefCell::new(None),
            }
        }

        fn claiming(self, claim: Claim) -> Self {
            *self.claim.borrow_mut() = claim;
            self
        }

        fn entries(&self) -> Vec<(u64, u64)> {
            self.entries.borrow().clone().unwrap_or_default()
        }

        /// Make the live list unreadable, as a changed `hidutil` output format
        /// or a failed spawn would.
        fn unreadable(self) -> Self {
            *self.entries.borrow_mut() = Err("unreadable".into());
            self
        }

        fn claim(&self) -> Claim {
            self.claim.borrow().clone()
        }

        /// A write-ahead that recorded `planned` as the list it was about to
        /// write, having displaced `displaced`.
        fn pending(displaced: Option<u64>, planned: &[(u64, u64)]) -> Claim {
            Claim::Pending(WriteAhead {
                displaced,
                planned: Some(planned.to_vec()),
            })
        }

        fn writes(&self) -> usize {
            *self.writes.borrow()
        }
    }

    impl CapsMapSys for FakeSys {
        fn read_entries(&self) -> Result<Vec<(u64, u64)>, String> {
            if let Some(left) = self.fail_reads_after.borrow_mut().as_mut() {
                if *left == 0 {
                    return Err("hidutil stopped answering".into());
                }
                *left -= 1;
            }
            let live = self.entries.borrow().clone();
            if let Some(stolen) = self.steal_after_read.borrow_mut().take() {
                *self.entries.borrow_mut() = Ok(stolen);
            }
            live
        }
        fn set_entries(&self, entries: &[(u64, u64)]) -> Result<(), String> {
            *self.writes.borrow_mut() += 1;
            if !self.can_set {
                return Err("hidutil failed".into());
            }
            let landed = self.set_lands_as.borrow_mut().take();
            *self.entries.borrow_mut() = Ok(landed.unwrap_or_else(|| entries.to_vec()));
            Ok(())
        }
        fn read_claim(&self) -> Result<Claim, String> {
            if !self.claim_readable {
                return Err("claim unreadable".into());
            }
            Ok(self.claim())
        }
        fn write_claim(&self, claim: Claim) -> Result<(), String> {
            if !self.claim_writable {
                return Err("claim unwritable".into());
            }
            if let Some(left) = self.claim_writes_left.borrow_mut().as_mut() {
                if *left == 0 {
                    return Err("claim unwritable".into());
                }
                *left -= 1;
            }
            *self.claim.borrow_mut() = claim;
            Ok(())
        }
        fn clear_claim(&self) -> Result<(), String> {
            if !self.claim_erasable {
                return Err("claim not erasable".into());
            }
            *self.claim.borrow_mut() = Claim::Unowned;
            Ok(())
        }
    }

    #[test]
    fn parse_entries_reads_every_pair() {
        // The decimal shape `hidutil property --get` prints, with two entries.
        let text = format!(
            "(\n  {{\n    HIDKeyboardModifierMappingSrc = {CAPS_USAGE};\n    \
             HIDKeyboardModifierMappingDst = {F18_USAGE};\n  }}\n  {{\n    \
             HIDKeyboardModifierMappingSrc = {OTHER_SRC};\n    \
             HIDKeyboardModifierMappingDst = {OTHER_DST};\n  }}\n)"
        );
        assert_eq!(
            parse_entries(&text),
            Ok(vec![(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)])
        );
    }

    #[test]
    fn parse_entries_accepts_the_shapes_hidutil_prints() {
        // Comma-separated entries, hex usages, reversed field order, and the two
        // ways of saying "nothing set" — all seen across macOS versions.
        assert_eq!(
            parse_entries(&format!(
                "({{HIDKeyboardModifierMappingDst = 0x70000006d; \
                 HIDKeyboardModifierMappingSrc = 0x700000039;}},\
                 {{HIDKeyboardModifierMappingSrc = {OTHER_SRC}; \
                 HIDKeyboardModifierMappingDst = {OTHER_DST};}})"
            )),
            Ok(vec![(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)])
        );
        for empty in ["(null)", "()", "(\n)", "  (null)\n"] {
            assert_eq!(parse_entries(empty), Ok(Vec::new()), "{empty:?}");
        }
    }

    #[test]
    fn parse_entries_rejects_anything_it_does_not_fully_understand() {
        // Each of these would previously have parsed as a *shorter* list, and
        // writing that back would delete the entries it could not read.
        let src = "HIDKeyboardModifierMappingSrc";
        let dst = "HIDKeyboardModifierMappingDst";
        for (case, text) in [
            (
                "truncated mid-entry",
                format!("(\n  {{\n    {src} = {OTHER_SRC};\n"),
            ),
            (
                "missing closing paren",
                format!("({{{src} = {OTHER_SRC}; {dst} = {OTHER_DST};}}"),
            ),
            (
                "an unrecognized field",
                format!("({{{src} = {OTHER_SRC}; {dst} = {OTHER_DST}; Flags = 1;}})"),
            ),
            ("no destination", format!("({{{src} = {OTHER_SRC};}})")),
            ("no source", format!("({{{dst} = {OTHER_DST};}})")),
            (
                "a repeated field",
                format!("({{{src} = {OTHER_SRC}; {src} = {CAPS_USAGE}; {dst} = {OTHER_DST};}})"),
            ),
            (
                "a non-numeric usage",
                format!("({{{src} = kHIDUsage_KeyboardA; {dst} = {OTHER_DST};}})"),
            ),
            (
                "a field with no value",
                format!("({{{src}; {dst} = {OTHER_DST};}})"),
            ),
            (
                "an unexpected wrapper",
                format!("[{{{src} = {OTHER_SRC}; {dst} = {OTHER_DST};}}]"),
            ),
            (
                "stray text between entries",
                format!("({{{src} = {OTHER_SRC}; {dst} = {OTHER_DST};}} and more)"),
            ),
            (
                "a missing field terminator",
                format!("({{{src} = {OTHER_SRC}; {dst} = {OTHER_DST}}})"),
            ),
            (
                "an empty field",
                format!("({{{src} = {OTHER_SRC};; {dst} = {OTHER_DST};}})"),
            ),
            ("a lone separator", "(,)".to_string()),
            ("only separators", "(,,)".to_string()),
            (
                "a leading separator",
                format!("(,{{{src} = {OTHER_SRC}; {dst} = {OTHER_DST};}})"),
            ),
            (
                "a trailing separator",
                format!("({{{src} = {OTHER_SRC}; {dst} = {OTHER_DST};}},)"),
            ),
            (
                "the same source mapped twice",
                format!(
                    "({{{src} = {OTHER_SRC}; {dst} = {OTHER_DST};}} \
                     {{{src} = {OTHER_SRC}; {dst} = {F18_USAGE};}})"
                ),
            ),
            (
                "the Caps Lock source mapped twice",
                format!(
                    "({{{src} = {CAPS_USAGE}; {dst} = {USER_CAPS_DST};}} \
                     {{{src} = {CAPS_USAGE}; {dst} = {F18_USAGE};}})"
                ),
            ),
            (
                "no separator between entries",
                format!(
                    "({{{src} = {OTHER_SRC}; {dst} = {OTHER_DST};}}\
                     {{{src} = {CAPS_USAGE}; {dst} = {F18_USAGE};}})"
                ),
            ),
            (
                "a doubled separator",
                format!(
                    "({{{src} = {OTHER_SRC}; {dst} = {OTHER_DST};}},,\
                     {{{src} = {CAPS_USAGE}; {dst} = {F18_USAGE};}})"
                ),
            ),
        ] {
            assert!(
                parse_entries(&text).is_err(),
                "{case} must not parse: {text:?}"
            );
        }
    }

    #[test]
    fn serialize_empty_is_an_empty_list() {
        assert_eq!(serialize_mapping(&[]), r#"{"UserKeyMapping":[]}"#);
    }

    #[test]
    fn serialize_emits_each_entry_as_hex() {
        // The exact shape `hidutil property --set` previously received for our
        // lone entry, so the merge path keeps feeding hidutil what it accepts.
        assert_eq!(
            serialize_mapping(&[(CAPS_USAGE, F18_USAGE)]),
            r#"{"UserKeyMapping":[{"HIDKeyboardModifierMappingSrc":0x700000039,"HIDKeyboardModifierMappingDst":0x70000006d}]}"#
        );
    }

    #[test]
    fn claim_records_round_trip() {
        assert_eq!(serialize_claim(&Claim::Unowned), None);
        for claim in [
            FakeSys::pending(None, &[]),
            FakeSys::pending(None, &[(CAPS_USAGE, F18_USAGE)]),
            FakeSys::pending(
                Some(USER_CAPS_DST),
                &[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)],
            ),
            Claim::Held(None),
            Claim::Held(Some(USER_CAPS_DST)),
        ] {
            let text = serialize_claim(&claim).expect("an owning claim has a record");
            assert_eq!(parse_claim(&text), Ok(claim.clone()));
            // Trailing whitespace from an editor or a partial flush is fine.
            assert_eq!(parse_claim(&format!("{text}\n")), Ok(claim));
        }
    }

    #[test]
    fn a_pending_record_without_a_plan_still_parses_but_never_attributes() {
        // Records from before the plan was written down: still a claim, never
        // evidence that any live list is ours.
        for text in ["pending", &format!("pending {USER_CAPS_DST}")] {
            let claim = parse_claim(text).unwrap();
            let Claim::Pending(ahead) = &claim else {
                panic!("{text:?} must parse as pending, got {claim:?}");
            };
            assert_eq!(ahead.planned, None);
            assert!(!ahead.matches(&[(CAPS_USAGE, F18_USAGE)]));
            assert!(!ahead.matches(&[]));
        }
    }

    #[test]
    fn an_unrecognized_claim_record_is_an_error() {
        // Never a *weaker* claim than what was recorded: reading `held 41` as
        // "held nothing" would delete a mapping we owed the user back, and
        // reading it as unowned would strand our own remap.
        for text in [
            "",
            "  ",
            "owned",
            "held 0xzz",
            "held 1 2",
            "41",
            "held plan 1:2",
            "pending plan 1",
            "pending plan 1:x",
            "pending 41 41",
        ] {
            assert!(parse_claim(text).is_err(), "{text:?} must not parse");
        }
    }

    #[test]
    fn a_write_ahead_matches_its_planned_list_in_any_order() {
        let ahead = WriteAhead {
            displaced: None,
            planned: Some(vec![(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)]),
        };
        assert!(ahead.matches(&[(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)]));
        assert!(!ahead.matches(&[(CAPS_USAGE, F18_USAGE)]));
        assert!(!ahead.matches(&[
            (CAPS_USAGE, F18_USAGE),
            (OTHER_SRC, OTHER_DST),
            (OTHER_DST, OTHER_SRC)
        ]));
    }

    #[test]
    fn apply_preserves_a_foreign_mapping() {
        // Turning Caps management on must keep the user's other remap, and
        // claim the source having displaced nothing.
        let sys = FakeSys::new(&[(OTHER_SRC, OTHER_DST)]);
        apply_with(&sys).unwrap();
        assert!(sys.entries().contains(&(OTHER_SRC, OTHER_DST)));
        assert!(sys.entries().contains(&(CAPS_USAGE, F18_USAGE)));
        assert_eq!(sys.claim(), Claim::Held(None));
    }

    #[test]
    fn apply_records_the_user_caps_mapping_it_replaces() {
        // A pre-existing Caps Lock remap of the user's own is the one entry we
        // must overwrite — so its destination goes into the claim before we do.
        let sys = FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST), (OTHER_SRC, OTHER_DST)]);
        apply_with(&sys).unwrap();
        assert_eq!(
            sys.entries()
                .iter()
                .filter(|&&(src, _)| src == CAPS_USAGE)
                .count(),
            1
        );
        assert!(sys.entries().contains(&(CAPS_USAGE, F18_USAGE)));
        assert!(sys.entries().contains(&(OTHER_SRC, OTHER_DST)));
        assert_eq!(sys.claim(), Claim::Held(Some(USER_CAPS_DST)));
    }

    #[test]
    fn apply_refuses_when_the_claim_cannot_be_recorded() {
        // Fail closed: without a claim the user's Caps mapping could never be
        // put back, so leave it alone and report the failure.
        let sys = FakeSys {
            claim_writable: false,
            ..FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST)])
        };
        assert!(apply_with(&sys).is_err());
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, USER_CAPS_DST)]);
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn apply_confirms_the_claim_only_after_the_os_write_lands() {
        // The record must not read as *held* while the write it describes has
        // not happened: a crash in between would leave a live Caps → Escape
        // looking like a mapping of ours to take back. And once the write is
        // known to have failed — our entry is not live — the write-ahead is
        // retracted, so nothing is claimed over a list we never changed.
        let sys = FakeSys {
            can_set: false,
            ..FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST)])
        };
        assert!(apply_with(&sys).is_err());
        assert_eq!(sys.claim(), Claim::Unowned);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, USER_CAPS_DST)]);
    }

    #[test]
    fn a_failed_write_does_not_claim_an_identical_list_the_user_sets_later() {
        // Codex's scenario: `hidutil --set` failed, then the user set exactly the
        // list we had planned (a lone Caps → F18). Had the write-ahead survived
        // the failure, its plan would now match and the entry would be "ours" to
        // remove on off. It did not survive, so the entry is the user's.
        let sys = FakeSys {
            can_set: false,
            ..FakeSys::new(&[])
        };
        assert!(apply_with(&sys).is_err());
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(sys.claim());
        clear_with(&sys).unwrap();
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, F18_USAGE)]);
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn a_write_never_attempted_is_retracted_even_if_our_plan_is_now_live() {
        // The pre-check caught someone else's write landing between our read and
        // ours — and what they wrote happens to be exactly our plan. We never
        // ran `hidutil --set`, so it is theirs: the write-ahead must go, or the
        // next reconcile would confirm their mapping as ours.
        let sys = FakeSys::new(&[]);
        *sys.steal_after_read.borrow_mut() = Some(vec![(CAPS_USAGE, F18_USAGE)]);
        assert!(apply_with(&sys).is_err());
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.claim(), Claim::Unowned);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, F18_USAGE)]);
    }

    #[test]
    fn a_write_ahead_is_kept_when_the_write_may_have_landed() {
        // The post-write read failed: our entry may well be live. Retracting
        // now would orphan it, so the write-ahead stays for the next reconcile.
        let sys = FakeSys::new(&[]);
        *sys.fail_reads_after.borrow_mut() = Some(2);
        assert!(apply_with(&sys).is_err());
        assert!(sys.entries().contains(&(CAPS_USAGE, F18_USAGE)));
        assert!(matches!(sys.claim(), Claim::Pending(_)));
    }

    #[test]
    fn apply_confirms_an_unconfirmed_claim_whose_planned_list_is_live() {
        // The OS write landed and only the confirm was lost: the live list is
        // exactly what the write-ahead set out to write, so it is ours, and the
        // claim is completed — displacement included — without another write.
        let sys = FakeSys::new(&[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)]).claiming(
            FakeSys::pending(
                Some(USER_CAPS_DST),
                &[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)],
            ),
        );
        apply_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Held(Some(USER_CAPS_DST)));
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn apply_keeps_an_unconfirmed_claim_it_cannot_attribute() {
        // Codex's scenario: our OS write failed, the user then mapped Caps → F18
        // themselves — so the live list is not the one we planned. Which of us
        // set the live entry is unknowable. The entry is left strictly alone,
        // and so is the claim: dropping it would turn this into "the user's
        // mapping" with the warning gone, and a later quit would leave Caps Lock
        // stuck on F18 with nothing to say why.
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(FakeSys::pending(
            Some(USER_CAPS_DST),
            &[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)],
        ));
        assert!(apply_with(&sys).is_err());
        assert_eq!(
            sys.claim(),
            FakeSys::pending(
                Some(USER_CAPS_DST),
                &[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)]
            )
        );
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, F18_USAGE)]);
    }

    #[test]
    fn apply_errors_when_the_claim_cannot_be_read() {
        // Fail closed in this direction too: without the claim we cannot tell a
        // live Caps → F18 of ours from an unconfirmed one.
        let sys = FakeSys {
            claim_readable: false,
            ..FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST)])
        };
        assert!(apply_with(&sys).is_err());
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, USER_CAPS_DST)]);
    }

    #[test]
    fn clear_restores_through_an_unconfirmed_claim_whose_planned_list_is_live() {
        // Off (or quit) straight from the lost-confirm state: the live list is
        // the one we wrote, so the remap is ours to take back, and the mapping
        // it displaced comes back with it.
        let sys = FakeSys::new(&[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)]).claiming(
            FakeSys::pending(
                Some(USER_CAPS_DST),
                &[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)],
            ),
        );
        clear_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Unowned);
        assert_eq!(
            sys.entries(),
            vec![(OTHER_SRC, OTHER_DST), (CAPS_USAGE, USER_CAPS_DST)]
        );
    }

    #[test]
    fn clear_keeps_an_unconfirmed_claim_it_cannot_attribute() {
        // Same unattributable state reached from the release direction: the
        // list is untouched, and the claim stays so the state keeps being
        // reported rather than silently becoming "the user's own".
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(FakeSys::pending(
            Some(USER_CAPS_DST),
            &[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)],
        ));
        assert!(clear_with(&sys).is_err());
        assert!(matches!(sys.claim(), Claim::Pending(_)));
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, F18_USAGE)]);
    }

    #[test]
    fn clear_drops_an_unconfirmed_claim_when_our_entry_is_not_live() {
        // The write never landed (or was undone since): nothing to restore, and
        // the write-ahead has nothing left to say.
        let sys = FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST)]).claiming(FakeSys::pending(
            Some(USER_CAPS_DST),
            &[(CAPS_USAGE, F18_USAGE)],
        ));
        clear_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Unowned);
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn a_lost_confirm_is_repaired_on_the_next_reconcile_and_released_on_off() {
        // The whole sequence: the OS write lands but the confirm fails; the
        // reconcile reports degraded and the remap stays live for the tap. The
        // next reconcile (a relaunch, say) finds the planned list live, confirms
        // the claim, and reports clean. Turning management off then hands the
        // source back — displaced mapping restored — as if nothing had failed.
        let sys = FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST), (OTHER_SRC, OTHER_DST)]);
        *sys.claim_writes_left.borrow_mut() = Some(1);
        assert_eq!(
            reconcile_with(&sys, true),
            ReconcileOutcome {
                proxy_active: true,
                reconciled: false,
            }
        );
        assert!(matches!(sys.claim(), Claim::Pending(_)));

        *sys.claim_writes_left.borrow_mut() = None;
        assert_eq!(
            reconcile_with(&sys, true),
            ReconcileOutcome {
                proxy_active: true,
                reconciled: true,
            }
        );
        assert_eq!(sys.claim(), Claim::Held(Some(USER_CAPS_DST)));

        assert_eq!(
            reconcile_with(&sys, false),
            ReconcileOutcome {
                proxy_active: false,
                reconciled: true,
            }
        );
        assert_eq!(sys.claim(), Claim::Unowned);
        assert_eq!(
            sys.entries(),
            vec![(OTHER_SRC, OTHER_DST), (CAPS_USAGE, USER_CAPS_DST)]
        );
    }

    #[test]
    fn an_unattributable_remap_keeps_reporting_degraded_but_keeps_the_tap_in_step() {
        // The unresolved state must neither vanish from the UI nor leave the
        // tap treating F18 as a real key while Caps Lock is live on it.
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(FakeSys::pending(
            None,
            &[(OTHER_SRC, OTHER_DST), (CAPS_USAGE, F18_USAGE)],
        ));
        for should_manage in [true, false, true] {
            let outcome = reconcile_with(&sys, should_manage);
            assert!(!outcome.reconciled);
            assert!(outcome.proxy_active);
        }
        assert!(matches!(sys.claim(), Claim::Pending(_)));
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn a_released_claim_left_undropped_is_not_reused() {
        // `Release` wrote the list but dropping the record failed. The claim is
        // still *held*, yet our entry is gone — so the next release must drop it
        // rather than act on it, and a Caps → F18 the user makes meanwhile is
        // not mistaken for ours.
        let sys =
            FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(Claim::Held(Some(USER_CAPS_DST)));
        let sys = FakeSys {
            claim_erasable: false,
            ..sys
        };
        assert!(clear_with(&sys).is_err());
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, USER_CAPS_DST)]);
        assert_eq!(sys.claim(), Claim::Held(Some(USER_CAPS_DST)));

        // The retry drops it without writing, since our entry is no longer live
        // — so the restore cannot be applied a second time.
        let retry =
            FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST)]).claiming(Claim::Held(Some(USER_CAPS_DST)));
        clear_with(&retry).unwrap();
        assert_eq!(retry.claim(), Claim::Unowned);
        assert_eq!(retry.writes(), 0);
    }

    #[test]
    fn apply_replaces_a_stale_claim_rather_than_deleting_it() {
        // A reboot drops `UserKeyMapping` but not the record. Taking the source
        // over again overwrites the claim, so no delete has to succeed for a
        // later clear to restore the right thing (here: nothing).
        let sys = FakeSys {
            claim_erasable: false,
            ..FakeSys::new(&[])
        }
        .claiming(Claim::Held(Some(USER_CAPS_DST)));
        apply_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Held(None));
        assert!(sys.entries().contains(&(CAPS_USAGE, F18_USAGE)));
    }

    #[test]
    fn apply_does_not_claim_a_caps_to_f18_the_user_set_themselves() {
        // Identical to our remap, but not ours: the tap gets its F18 events
        // either way, and claiming it would make a later clear delete it.
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]);
        apply_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Unowned);
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn apply_is_a_noop_when_already_claimed() {
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(Claim::Held(None));
        apply_with(&sys).unwrap();
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.claim(), Claim::Held(None));
    }

    #[test]
    fn apply_errors_when_the_list_cannot_be_read() {
        // Unreadable is not "empty": writing then would wipe the user's remaps.
        let sys = FakeSys::new(&[]).unreadable();
        assert!(apply_with(&sys).is_err());
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn clear_removes_only_our_entry() {
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)])
            .claiming(Claim::Held(None));
        clear_with(&sys).unwrap();
        assert_eq!(sys.entries(), vec![(OTHER_SRC, OTHER_DST)]);
        assert_eq!(sys.claim(), Claim::Unowned);
    }

    #[test]
    fn clear_restores_the_user_caps_mapping_it_replaced() {
        // The whole point of the claim: Caps Lock goes back to what the user
        // had it doing, not merely back to unmapped.
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)])
            .claiming(Claim::Held(Some(USER_CAPS_DST)));
        clear_with(&sys).unwrap();
        assert!(sys.entries().contains(&(CAPS_USAGE, USER_CAPS_DST)));
        assert!(sys.entries().contains(&(OTHER_SRC, OTHER_DST)));
        assert_eq!(sys.claim(), Claim::Unowned);
    }

    #[test]
    fn clear_leaves_a_caps_to_f18_the_user_set_themselves() {
        // No claim, so the identical-looking entry is theirs: enabling and
        // disabling Caps management must leave it exactly as it was.
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]);
        clear_with(&sys).unwrap();
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, F18_USAGE)]);
    }

    #[test]
    fn clear_errors_when_the_claim_cannot_be_read() {
        // Fail closed: an unreadable claim means we cannot tell whose entry
        // this is, nor what it displaced. Touch nothing.
        let sys = FakeSys {
            claim_readable: false,
            ..FakeSys::new(&[(CAPS_USAGE, F18_USAGE)])
        }
        .claiming(Claim::Held(Some(USER_CAPS_DST)));
        assert!(clear_with(&sys).is_err());
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, F18_USAGE)]);
        assert_eq!(sys.claim(), Claim::Held(Some(USER_CAPS_DST)));
    }

    #[test]
    fn clear_drops_a_claim_an_external_change_invalidated() {
        // Something outside Tomari moved the Caps Lock source while we held it:
        // that newer intent wins, so write nothing and discard the claim.
        let sys = FakeSys::new(&[(CAPS_USAGE, OTHER_DST), (OTHER_SRC, OTHER_DST)])
            .claiming(Claim::Held(Some(USER_CAPS_DST)));
        clear_with(&sys).unwrap();
        assert_eq!(sys.writes(), 0);
        assert_eq!(
            sys.entries(),
            vec![(CAPS_USAGE, OTHER_DST), (OTHER_SRC, OTHER_DST)]
        );
        assert_eq!(sys.claim(), Claim::Unowned);
    }

    #[test]
    fn clear_keeps_the_claim_when_the_write_fails() {
        // A failed `hidutil` still leaves Caps Lock ours, so the claim has to
        // survive for the next reconcile to retry the restore.
        let sys = FakeSys {
            can_set: false,
            ..FakeSys::new(&[(CAPS_USAGE, F18_USAGE)])
        }
        .claiming(Claim::Held(Some(USER_CAPS_DST)));
        assert!(clear_with(&sys).is_err());
        assert_eq!(sys.claim(), Claim::Held(Some(USER_CAPS_DST)));
    }

    #[test]
    fn clear_is_a_noop_when_unclaimed() {
        let sys = FakeSys::new(&[]);
        clear_with(&sys).unwrap();
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn apply_refuses_a_write_over_a_concurrent_change() {
        // Something outside Tomari rewrote the list between our read and our
        // write, so the list we planned no longer exists: writing it would undo
        // their change wholesale.
        let sys = FakeSys::new(&[(OTHER_SRC, OTHER_DST)]);
        *sys.steal_after_read.borrow_mut() = Some(vec![(CAPS_USAGE, USER_CAPS_DST)]);
        assert!(apply_with(&sys).is_err());
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, USER_CAPS_DST)]);
        assert_eq!(sys.claim(), Claim::Unowned);
    }

    #[test]
    fn apply_refuses_to_report_a_write_that_did_not_land() {
        // `hidutil` exited zero but the property is not what we set, so the
        // reconcile must not read as complete.
        let sys = FakeSys::new(&[]);
        *sys.set_lands_as.borrow_mut() = Some(vec![(OTHER_SRC, OTHER_DST)]);
        assert!(apply_with(&sys).is_err());
        // Our entry is not live, so nothing is claimed either.
        assert_eq!(sys.claim(), Claim::Unowned);
    }

    #[test]
    fn apply_tolerates_an_unrelated_change_landing_with_its_write() {
        // Our entry is live, so the remap *is* ours and the claim must be
        // confirmed — an outside writer adding an unrelated mapping in the same
        // instant is not a failure of ours, and treating it as one would strand
        // the remap we just made.
        let sys = FakeSys::new(&[]);
        *sys.set_lands_as.borrow_mut() =
            Some(vec![(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)]);
        apply_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Held(None));
    }

    #[test]
    fn clear_tolerates_an_unrelated_change_landing_with_its_write() {
        let sys = FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(Claim::Held(None));
        *sys.set_lands_as.borrow_mut() = Some(vec![(OTHER_SRC, OTHER_DST)]);
        clear_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Unowned);
    }

    #[test]
    fn clear_refuses_a_write_over_a_concurrent_change() {
        let sys =
            FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(Claim::Held(Some(USER_CAPS_DST)));
        *sys.steal_after_read.borrow_mut() =
            Some(vec![(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)]);
        assert!(clear_with(&sys).is_err());
        assert_eq!(sys.writes(), 0);
        // The claim survives, so the next reconcile retries the release.
        assert_eq!(sys.claim(), Claim::Held(Some(USER_CAPS_DST)));
    }

    /// The outcome of a fully successful reconcile toward `should_manage`.
    fn settled(should_manage: bool) -> ReconcileOutcome {
        ReconcileOutcome {
            proxy_active: should_manage,
            reconciled: true,
        }
    }

    #[test]
    fn reconcile_off_drops_a_claim_an_external_change_invalidated() {
        // Reached through the public direction: turning Caps management off runs
        // the release path even when our entry is already gone, which is what
        // stops a stale claim from outliving it.
        let sys = FakeSys::new(&[(CAPS_USAGE, OTHER_DST)]).claiming(Claim::Held(None));
        assert_eq!(reconcile_with(&sys, false), settled(false));
        assert_eq!(sys.claim(), Claim::Unowned);
        assert_eq!(sys.writes(), 0);
    }

    #[test]
    fn reconcile_on_then_off_round_trips_a_user_caps_mapping() {
        let sys = FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST), (OTHER_SRC, OTHER_DST)]);
        assert_eq!(reconcile_with(&sys, true), settled(true));
        assert!(sys.entries().contains(&(CAPS_USAGE, F18_USAGE)));
        assert_eq!(reconcile_with(&sys, false), settled(false));
        assert_eq!(sys.claim(), Claim::Unowned);
        assert!(sys.entries().contains(&(CAPS_USAGE, USER_CAPS_DST)));
        assert!(sys.entries().contains(&(OTHER_SRC, OTHER_DST)));
    }

    #[test]
    fn reconcile_reports_the_live_state_when_it_fails() {
        // An unrecordable claim aborts the remap, so the caller must be told the
        // remap is *not* in effect however the request read.
        let sys = FakeSys {
            claim_writable: false,
            ..FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST)])
        };
        assert_eq!(
            reconcile_with(&sys, true),
            ReconcileOutcome {
                proxy_active: false,
                reconciled: false,
            }
        );
    }

    #[test]
    fn reconcile_reports_degraded_when_ownership_lands_after_the_mapping() {
        // The OS write succeeded but the confirming record did not: the remap is
        // live (so the tap must treat F18 as Caps) yet the reconcile did *not*
        // complete, and reporting otherwise would hide the mismatch from the UI.
        let sys = FakeSys::new(&[]);
        *sys.claim_writes_left.borrow_mut() = Some(1);
        assert_eq!(
            reconcile_with(&sys, true),
            ReconcileOutcome {
                proxy_active: true,
                reconciled: false,
            }
        );
        assert!(sys.entries().contains(&(CAPS_USAGE, F18_USAGE)));
        assert_eq!(
            sys.claim(),
            FakeSys::pending(None, &[(CAPS_USAGE, F18_USAGE)])
        );
    }

    #[test]
    fn reconcile_reports_degraded_when_the_claim_cannot_be_dropped() {
        // Mirror image: the mapping was handed back but the record still claims
        // it, so the next reconcile has a drop to retry — and the UI is told.
        let sys = FakeSys {
            claim_erasable: false,
            ..FakeSys::new(&[(CAPS_USAGE, F18_USAGE)])
        }
        .claiming(Claim::Held(None));
        assert_eq!(
            reconcile_with(&sys, false),
            ReconcileOutcome {
                proxy_active: false,
                reconciled: false,
            }
        );
        assert_eq!(sys.entries(), vec![]);
        assert_eq!(sys.claim(), Claim::Held(None));
    }

    /// The structural check behind [`caps_to_f18_live`], over parsed text.
    fn maps_caps_to_f18(text: &str) -> bool {
        parse_entries(text).is_ok_and(|e| e.contains(&(CAPS_USAGE, F18_USAGE)))
    }

    #[test]
    fn detects_our_caps_to_f18_entry_decimal() {
        // The shape `hidutil property --get` prints (decimal usages).
        let text = "(\n    {\n        HIDKeyboardModifierMappingDst = 30064771181;\n        \
             HIDKeyboardModifierMappingSrc = 30064771129;\n    }\n)";
        assert!(maps_caps_to_f18(text));
    }

    #[test]
    fn detects_our_entry_in_hex() {
        let text = "({HIDKeyboardModifierMappingSrc = 0x700000039; \
             HIDKeyboardModifierMappingDst = 0x70000006d;})";
        assert!(maps_caps_to_f18(text));
    }

    #[test]
    fn empty_or_null_is_not_active() {
        assert!(!maps_caps_to_f18("(null)"));
        assert!(!maps_caps_to_f18("(\n)"));
        assert!(!maps_caps_to_f18(""));
    }

    #[test]
    fn caps_and_f18_in_separate_entries_is_not_ours() {
        // Caps mapped elsewhere AND F18 used as some other key's target: neither
        // entry is Caps→F18, so this must not read as our remap.
        let text = "(\n  {\n    HIDKeyboardModifierMappingSrc = 30064771129;\n    \
             HIDKeyboardModifierMappingDst = 30064771072;\n  }\n  {\n    \
             HIDKeyboardModifierMappingSrc = 30064771070;\n    \
             HIDKeyboardModifierMappingDst = 30064771181;\n  }\n)";
        assert!(!maps_caps_to_f18(text));
    }
}
