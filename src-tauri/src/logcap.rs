//! A per-day byte cap on the log file.
//!
//! The daily-rotated file keeps seven days, but nothing bounded how much one
//! day could hold: a peer that produces a log line per event — a local sender
//! spraying `tomari://` URLs, a target app failing every write — could fill the
//! disk within the day. Every line therefore passes a [`DailyBudget`]; once the
//! day's budget is spent the file gets one notice and the rest of that day's
//! lines are dropped (stderr still receives them). The budget resets with the
//! calendar day, in step with the file's own rotation, and is seeded at start
//! from what an earlier run wrote to today's file.
//!
//! It is a *soft* cap: the budget lives in this process and is charged as a
//! line is written, so the file can overshoot by a bounded amount — the few
//! startup lines a second instance writes before the instance lock sends it
//! away, a line that lands in the old day's file at the UTC boundary because
//! the appender picked the file a moment before the budget picked the day, or
//! one repeated notice after a restart. None of these is the unbounded growth
//! the cap exists to prevent, and closing them would mean a cross-process
//! lock around every log line.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_subscriber::fmt::MakeWriter;

use crate::locks::MutexExt;

/// Bytes the log file may take per day, notice line included. Generous for a
/// resident app that logs state transitions, not events: a busy day is well
/// under a megabyte.
pub const DAILY_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// The one line written when the cap is reached. Its size is reserved inside
/// the budget, so the file never exceeds the cap even by this line.
const NOTICE: &[u8] =
    b"tomari: daily log size cap reached; further lines today go to stderr only\n";

/// Tracks bytes written per calendar day. The day, the bytes spent and the
/// notice flag change together under one lock, so a midnight rollover seen by
/// two writers at once cannot reset a charge the other just made, nor let the
/// notice go out twice.
pub struct DailyBudget {
    limit: u64,
    state: Mutex<BudgetState>,
}

#[derive(Debug, Clone, Copy)]
struct BudgetState {
    /// The day (days since the Unix epoch) `spent` refers to.
    day: u64,
    spent: u64,
    /// Whether the "cap reached" notice was written for `day`.
    notified: bool,
}

/// What to do with a line of `len` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Within budget: write it.
    Write,
    /// The budget was just exhausted: write the notice instead, then drop.
    Notice,
    /// Over budget for the rest of the day: drop it.
    Drop,
}

impl DailyBudget {
    pub const fn new(limit: u64) -> Self {
        Self {
            limit,
            state: Mutex::new(BudgetState {
                day: 0,
                spent: 0,
                notified: false,
            }),
        }
    }

    /// Start `day` with `spent` bytes already used — the size of the day's
    /// existing file when the process starts, so a restart does not hand the
    /// day a fresh budget on top of what an earlier run wrote.
    pub fn seed(&self, day: u64, spent: u64) {
        let mut state = self.state.lock_safe();
        *state = BudgetState {
            day,
            spent,
            // Already past the cap before this run wrote a byte: the notice
            // was (or should have been) written by the run that got there.
            notified: spent > self.limit.saturating_sub(NOTICE.len() as u64),
        };
    }

    /// Decide `len` bytes on `day`, charging them if admitted.
    pub fn admit(&self, day: u64, len: u64) -> Admission {
        let mut state = self.state.lock_safe();
        if state.day != day {
            // A new day, a new file (the appender rotates daily): fresh budget.
            *state = BudgetState {
                day,
                spent: 0,
                notified: false,
            };
        }
        // Ordinary lines may use everything but the notice's reserved bytes.
        let usable = self.limit.saturating_sub(NOTICE.len() as u64);
        if state.spent + len <= usable {
            state.spent += len;
            return Admission::Write;
        }
        if state.notified {
            Admission::Drop
        } else {
            state.notified = true;
            state.spent += NOTICE.len() as u64;
            Admission::Notice
        }
    }
}

/// Days since the Unix epoch, now.
pub fn today() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

/// The civil date `YYYY-MM-DD` of a day count since the Unix epoch — the
/// suffix `tracing_appender` gives a daily file — without a calendar crate.
/// Howard Hinnant's `civil_from_days`.
pub fn date_string(days_since_epoch: u64) -> String {
    let z = days_since_epoch as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// The bytes already in today's log file, if it exists: what an earlier run of
/// the same day wrote.
pub fn existing_bytes_today(logs_dir: &Path, prefix: &str, suffix: &str) -> u64 {
    let name = format!("{prefix}.{}.{suffix}", date_string(today()));
    std::fs::metadata(logs_dir.join(name))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Wraps a `MakeWriter` (the rolling file appender) so every line is admitted
/// by `budget` first.
pub struct Capped<M> {
    inner: M,
    budget: &'static DailyBudget,
}

impl<M> Capped<M> {
    pub fn new(inner: M, budget: &'static DailyBudget) -> Self {
        Self { inner, budget }
    }
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for Capped<M> {
    type Writer = CappedWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        CappedWriter {
            inner: self.inner.make_writer(),
            budget: self.budget,
        }
    }
}

pub struct CappedWriter<W> {
    inner: W,
    budget: &'static DailyBudget,
}

impl<W: Write> Write for CappedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.budget.admit(today(), buf.len() as u64) {
            Admission::Write => self.inner.write(buf),
            Admission::Notice => {
                let _ = self.inner.write_all(NOTICE);
                Ok(buf.len())
            }
            // Report the bytes as written so the layer does not retry or error.
            Admission::Drop => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: u64 = NOTICE.len() as u64;

    #[test]
    fn the_budget_admits_up_to_the_cap_less_the_notice_then_notifies_once_then_drops() {
        let budget = DailyBudget::new(100 + N);
        assert_eq!(budget.admit(1, 60), Admission::Write);
        assert_eq!(budget.admit(1, 40), Admission::Write);
        assert_eq!(budget.admit(1, 1), Admission::Notice);
        assert_eq!(budget.admit(1, 1), Admission::Drop);
        assert_eq!(budget.admit(1, 1), Admission::Drop);
    }

    #[test]
    fn a_new_day_resets_the_budget_and_the_notice() {
        let budget = DailyBudget::new(10 + N);
        assert_eq!(budget.admit(7, 10), Admission::Write);
        assert_eq!(budget.admit(7, 1), Admission::Notice);
        assert_eq!(budget.admit(8, 5), Admission::Write);
        assert_eq!(budget.admit(8, 5), Admission::Write);
        assert_eq!(budget.admit(8, 1), Admission::Notice);
    }

    #[test]
    fn seeding_with_an_earlier_run_s_bytes_counts_them_against_today() {
        let budget = DailyBudget::new(100 + N);
        budget.seed(3, 90);
        assert_eq!(budget.admit(3, 10), Admission::Write);
        assert_eq!(budget.admit(3, 1), Admission::Notice);
        // Seeded already past the cap: the earlier run wrote the notice.
        let full = DailyBudget::new(100 + N);
        full.seed(3, 100 + N);
        assert_eq!(full.admit(3, 1), Admission::Drop);
    }

    #[test]
    fn the_writer_never_lets_the_file_exceed_the_cap() {
        static BUDGET: DailyBudget = DailyBudget::new(20 + NOTICE.len() as u64);
        let mut sink: Vec<u8> = Vec::new();
        {
            let mut w = CappedWriter {
                inner: &mut sink,
                budget: &BUDGET,
            };
            w.write_all(b"0123456789").unwrap();
            w.write_all(b"0123456789").unwrap();
            w.write_all(b"dropped line").unwrap();
            w.write_all(b"dropped too").unwrap();
        }
        assert!(sink.len() as u64 <= 20 + N, "{}", sink.len());
        let text = String::from_utf8(sink).unwrap();
        assert!(text.starts_with("01234567890123456789"));
        assert!(text.contains("daily log size cap reached"));
        assert!(!text.contains("dropped"));
    }

    #[test]
    fn date_string_matches_the_appender_s_daily_suffix() {
        assert_eq!(date_string(0), "1970-01-01");
        assert_eq!(date_string(18_993), "2022-01-01");
        assert_eq!(date_string(19_000), "2022-01-08");
        assert_eq!(date_string(20_513), "2026-03-01");
    }
}
