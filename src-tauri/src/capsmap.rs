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
//! rather than being wiped.
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
//!   a live Caps Lock → F18 *might* be ours and might be the user's. Nothing is
//!   ever deleted or restored from this state; it is given up instead.
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
//! unwritten claim or an unapplied mapping is simply retried, but a *confirm*
//! that failed over a live remap leaves the unattributable `Pending` state,
//! which the next reconcile gives up rather than retries — the mapping stays,
//! and Tomari stops claiming it. The remaining multi-step races are closed by
//! serializing the whole reconcile (live read, record, OS write, proxy flag) on
//! [`RECONCILE`].
//!
//! The mapping is per-user, needs no elevated privileges, and persists until
//! reboot or removal — so we reconcile it on every tap (re)start and clear it on
//! quit.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::locks::MutexExt;

/// Full HID usage (`0x7_0000_0000 | usage`) of Caps Lock.
const CAPS_USAGE: u64 = 0x7_0000_0039;
/// Full HID usage of F18 — an ordinary key with no default macOS binding,
/// which Caps Lock is remapped onto.
const F18_USAGE: u64 = 0x7_0000_006D;

/// The virtual keycode F18 arrives as once Caps Lock is remapped to it. The tap
/// treats this keycode as the Caps Lock modifier.
pub const F18_KEYCODE: i64 = 79;

fn set_mapping(json: &str) -> Result<(), String> {
    let output = Command::new("/usr/bin/hidutil")
        .args(["property", "--set", json])
        .output()
        .map_err(|e| format!("failed to run hidutil: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "hidutil exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Read the current `UserKeyMapping` entries as `(src, dst)` usage pairs.
/// `None` when `hidutil` could not be run at all — distinct from an empty list,
/// so callers never mistake "unreadable" for "no mappings" and clobber the
/// user's own remaps.
fn read_entries() -> Option<Vec<(u64, u64)>> {
    let output = Command::new("/usr/bin/hidutil")
        .args(["property", "--get", "UserKeyMapping"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_entries(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse every `UserKeyMapping` entry out of `hidutil property --get` text into
/// `(src, dst)` usage pairs. Splitting on `}` yields one block per entry (the
/// trailing fragment carries neither field and is dropped).
fn parse_entries(text: &str) -> Vec<(u64, u64)> {
    text.split('}')
        .filter_map(|entry| {
            let src = entry_field(entry, "HIDKeyboardModifierMappingSrc")?;
            let dst = entry_field(entry, "HIDKeyboardModifierMappingDst")?;
            Some((src, dst))
        })
        .collect()
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

/// The usage value of `key` within one `UserKeyMapping` entry, if present.
fn entry_field(entry: &str, key: &str) -> Option<u64> {
    let after_key = entry.get(entry.find(key)? + key.len()..)?;
    let after_eq = after_key.get(after_key.find('=')? + 1..)?;
    let value = after_eq.get(..after_eq.find(';')?)?;
    parse_usage(value)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    /// Tomari does not hold the source. A live Caps Lock → F18 is the user's own
    /// and must be left alone.
    Unowned,
    /// Write-ahead: recorded before the OS write, and left behind if that write
    /// (or its confirmation) never completed. A live Caps Lock → F18 is then
    /// *unattributable* — possibly ours, possibly one the user made afterwards
    /// — so this state never deletes or restores anything.
    Pending(Option<u64>),
    /// The OS write is confirmed: a live Caps Lock → F18 is ours to take back.
    Held(Option<u64>),
}

const PENDING_MARKER: &str = "pending";
const HELD_MARKER: &str = "held";

fn serialize_claim(claim: Claim) -> Option<String> {
    let (marker, displaced) = match claim {
        Claim::Unowned => return None,
        Claim::Pending(displaced) => (PENDING_MARKER, displaced),
        Claim::Held(displaced) => (HELD_MARKER, displaced),
    };
    Some(match displaced {
        Some(dst) => format!("{marker} {dst}"),
        None => marker.to_string(),
    })
}

/// Parse a record's contents into a claim. `Err` for anything unrecognized — a
/// torn or corrupt record must never read as a weaker claim than it was, which
/// could delete a mapping we owed the user back.
fn parse_claim(text: &str) -> Result<Claim, String> {
    let unrecognized = || "unrecognized caps-lock claim record".to_string();
    let mut fields = text.split_whitespace();
    let marker = fields.next().ok_or_else(unrecognized)?;
    let displaced = match fields.next() {
        Some(value) => Some(parse_usage(value).ok_or_else(unrecognized)?),
        None => None,
    };
    if fields.next().is_some() {
        return Err(unrecognized());
    }
    match marker {
        PENDING_MARKER => Ok(Claim::Pending(displaced)),
        HELD_MARKER => Ok(Claim::Held(displaced)),
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
    let Some(body) = serialize_claim(claim) else {
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
    /// The live `UserKeyMapping` entries (`None` = could not be read).
    fn read_entries(&self) -> Option<Vec<(u64, u64)>>;
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
        .is_some_and(|entries| entries.contains(&(CAPS_USAGE, F18_USAGE)))
}

/// What [`apply_with`] should do given the live entry list and our claim.
#[derive(Debug, PartialEq, Eq)]
enum ApplyPlan {
    /// Caps Lock → F18 is already live and accounted for — held by us, or the
    /// user's own with no claim against it. Either way the tap gets the F18
    /// events it needs, so nothing is written: an entry we did not create must
    /// not become ours to remove later.
    AlreadyInEffect,
    /// Caps Lock → F18 is live but our claim never got past write-ahead, so
    /// there is no telling whether we set it. Give the claim up: the entry
    /// stays, and a later release will treat it as the user's rather than risk
    /// deleting a mapping of theirs.
    Disown,
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
            Claim::Pending(_) => ApplyPlan::Disown,
        };
    }
    let displaced = entries
        .iter()
        .find(|&&(src, _)| src == CAPS_USAGE)
        .map(|&(_, dst)| dst);
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
    /// We hold a claim we cannot act on — our entry is no longer live (something
    /// outside Tomari moved the source, and that newer intent wins), or the
    /// claim never got past write-ahead. Drop it without touching the list.
    DropClaim,
    /// Write `entries`: ours removed, and the displaced mapping restored when
    /// the claim named one. Then drop the claim.
    Release { entries: Vec<(u64, u64)> },
}

/// Plan giving the Caps Lock source back. Only a *confirmed* claim over a still
/// live Caps Lock → F18 authorizes a write; every weaker state just lets go.
fn plan_clear(mut entries: Vec<(u64, u64)>, claim: Claim) -> ClearPlan {
    let displaced = match claim {
        Claim::Unowned => return ClearPlan::Unclaimed,
        // Unattributable, so not ours to take back — see [`Claim::Pending`].
        Claim::Pending(_) => return ClearPlan::DropClaim,
        Claim::Held(displaced) => displaced,
    };
    if !entries.contains(&(CAPS_USAGE, F18_USAGE)) {
        return ClearPlan::DropClaim;
    }
    entries.retain(|&pair| pair != (CAPS_USAGE, F18_USAGE));
    if let Some(dst) = displaced {
        entries.push((CAPS_USAGE, dst));
    }
    ClearPlan::Release { entries }
}

fn apply_with(sys: &impl CapsMapSys) -> Result<(), String> {
    let entries = sys
        .read_entries()
        .ok_or("could not read current hidutil key mappings")?;
    match plan_apply(entries, sys.read_claim()?) {
        ApplyPlan::AlreadyInEffect => Ok(()),
        ApplyPlan::Disown => {
            tracing::warn!(
                "an unconfirmed caps-lock claim is being given up; the live Caps Lock → F18 \
                 mapping will be left in place rather than removed later"
            );
            sys.clear_claim()
        }
        // Write-ahead, then confirm: what we are about to overwrite has to be
        // recorded before it is gone, or nothing could ever put it back, and the
        // record must not read as *held* until the OS write has actually landed.
        // A record that cannot be written aborts the remap rather than risking
        // the user's mapping — `reconcile` then reports it inactive, which
        // surfaces as the `capsLockRemap` warning.
        ApplyPlan::Take { entries, displaced } => {
            sys.write_claim(Claim::Pending(displaced))?;
            sys.set_entries(&entries)?;
            sys.write_claim(Claim::Held(displaced))
        }
    }
}

fn clear_with(sys: &impl CapsMapSys) -> Result<(), String> {
    let entries = sys
        .read_entries()
        .ok_or("could not read current hidutil key mappings")?;
    // Fail closed: without a readable claim we do not know whether a live
    // Caps Lock → F18 is ours, nor what it displaced. Leave everything as it is.
    match plan_clear(entries, sys.read_claim()?) {
        ClearPlan::Unclaimed => Ok(()),
        ClearPlan::DropClaim => sys.clear_claim(),
        // The list first, the claim second: releasing the claim while our entry
        // is still live would orphan it — no later clear would recognize it as
        // ours. The reverse order merely leaves a claim the next clear drops.
        ClearPlan::Release { entries } => {
            sys.set_entries(&entries)?;
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
    fn read_entries(&self) -> Option<Vec<(u64, u64)>> {
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

/// Whether Caps Lock is currently remapped to F18, so F18 key events are the
/// Caps Lock modifier rather than a real F18 key ([`crate::eventtap`]). Written
/// only under [`RECONCILE`], from the reconcile's *actual* outcome; read on the
/// tap thread for every keystroke, so it is an atomic rather than behind a lock.
static CAPS_PROXY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether F18 key events should be treated as Caps Lock.
pub fn caps_proxy_active() -> bool {
    CAPS_PROXY_ACTIVE.load(Ordering::SeqCst)
}

/// See [`reconcile_with`]. The proxy flag is published from inside the lock, so
/// it always reflects the reconcile that most recently *ran*, not whichever one
/// happened to return last.
#[must_use]
pub fn reconcile(should_manage: bool) -> ReconcileOutcome {
    let _serialized = RECONCILE.lock_safe();
    let outcome = reconcile_with(&RealSys, should_manage);
    CAPS_PROXY_ACTIVE.store(outcome.proxy_active, Ordering::SeqCst);
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
        entries: RefCell<Option<Vec<(u64, u64)>>>,
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
        /// How many times the entry list was written.
        writes: RefCell<usize>,
    }

    impl FakeSys {
        fn new(entries: &[(u64, u64)]) -> Self {
            Self {
                entries: RefCell::new(Some(entries.to_vec())),
                claim: RefCell::new(Claim::Unowned),
                claim_readable: true,
                claim_writable: true,
                claim_writes_left: RefCell::new(None),
                claim_erasable: true,
                can_set: true,
                writes: RefCell::new(0),
            }
        }

        fn claiming(self, claim: Claim) -> Self {
            *self.claim.borrow_mut() = claim;
            self
        }

        fn entries(&self) -> Vec<(u64, u64)> {
            self.entries.borrow().clone().unwrap_or_default()
        }

        fn claim(&self) -> Claim {
            *self.claim.borrow()
        }

        fn writes(&self) -> usize {
            *self.writes.borrow()
        }
    }

    impl CapsMapSys for FakeSys {
        fn read_entries(&self) -> Option<Vec<(u64, u64)>> {
            self.entries.borrow().clone()
        }
        fn set_entries(&self, entries: &[(u64, u64)]) -> Result<(), String> {
            *self.writes.borrow_mut() += 1;
            if !self.can_set {
                return Err("hidutil failed".into());
            }
            *self.entries.borrow_mut() = Some(entries.to_vec());
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
            vec![(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)]
        );
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
        assert_eq!(serialize_claim(Claim::Unowned), None);
        for claim in [
            Claim::Pending(None),
            Claim::Pending(Some(USER_CAPS_DST)),
            Claim::Held(None),
            Claim::Held(Some(USER_CAPS_DST)),
        ] {
            let text = serialize_claim(claim).expect("an owning claim has a record");
            assert_eq!(parse_claim(&text), Ok(claim));
            // Trailing whitespace from an editor or a partial flush is fine.
            assert_eq!(parse_claim(&format!("{text}\n")), Ok(claim));
        }
    }

    #[test]
    fn an_unrecognized_claim_record_is_an_error() {
        // Never a *weaker* claim than what was recorded: reading `held 41` as
        // "held nothing" would delete a mapping we owed the user back, and
        // reading it as unowned would strand our own remap.
        for text in ["", "  ", "owned", "held 0xzz", "held 1 2", "41"] {
            assert!(parse_claim(text).is_err(), "{text:?} must not parse");
        }
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
        // looking like a mapping of ours to take back.
        let sys = FakeSys {
            can_set: false,
            ..FakeSys::new(&[(CAPS_USAGE, USER_CAPS_DST)])
        };
        assert!(apply_with(&sys).is_err());
        assert_eq!(sys.claim(), Claim::Pending(Some(USER_CAPS_DST)));
    }

    #[test]
    fn apply_gives_up_an_unconfirmed_claim_over_a_live_remap() {
        // Codex's scenario: our OS write failed, the user then mapped Caps → F18
        // themselves. Which of us set the live entry is unknowable, so the claim
        // is dropped — and the entry left strictly alone.
        let sys =
            FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(Claim::Pending(Some(USER_CAPS_DST)));
        apply_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Unowned);
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
    fn clear_gives_up_an_unconfirmed_claim_without_touching_the_list() {
        // Same unattributable state reached from the release direction.
        let sys =
            FakeSys::new(&[(CAPS_USAGE, F18_USAGE)]).claiming(Claim::Pending(Some(USER_CAPS_DST)));
        clear_with(&sys).unwrap();
        assert_eq!(sys.claim(), Claim::Unowned);
        assert_eq!(sys.writes(), 0);
        assert_eq!(sys.entries(), vec![(CAPS_USAGE, F18_USAGE)]);
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
        let sys = FakeSys::new(&[]);
        *sys.entries.borrow_mut() = None;
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
        assert_eq!(sys.claim(), Claim::Pending(None));
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
        parse_entries(text).contains(&(CAPS_USAGE, F18_USAGE))
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
