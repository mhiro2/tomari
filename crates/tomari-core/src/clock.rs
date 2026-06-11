//! Minimal time helpers so the rest of the codebase does not depend on a
//! date-time crate just to record millisecond timestamps.

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix timestamp in milliseconds for "now".
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Format a millisecond timestamp as an ISO-8601 / RFC-3339 UTC string.
///
/// This is a dependency-free formatter good enough for display and logging;
/// it handles the proleptic Gregorian calendar correctly for all dates that
/// fit in an `i64` millisecond range.
pub fn to_rfc3339(millis: i64) -> String {
    let secs = millis.div_euclid(1000);
    let ms = millis.rem_euclid(1000);

    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

/// Convert a day count since the Unix epoch into `(year, month, day)`.
///
/// Algorithm from Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats_correctly() {
        assert_eq!(to_rfc3339(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn known_timestamp_formats_correctly() {
        // 2021-01-01T00:00:00.000Z == 1609459200000 ms
        assert_eq!(to_rfc3339(1_609_459_200_000), "2021-01-01T00:00:00.000Z");
    }

    #[test]
    fn millis_are_preserved() {
        assert_eq!(to_rfc3339(1_609_459_200_123), "2021-01-01T00:00:00.123Z");
    }

    #[test]
    fn now_is_after_2024() {
        // 2024-01-01T00:00:00Z
        assert!(now_millis() > 1_704_067_200_000);
    }
}
