//! Go-style duration parsing — `"30s"`, `"5m"`, `"1h"`, `"100ms"`.
//!
//! Used by every verb that accepts the universal `--timeout` flag (see
//! [docs/cli-contract.md §5](../../../docs/cli-contract.md#5-time-bounds)).
//! Match Go's `time.ParseDuration` for a small useful subset; reject
//! anything we don't understand rather than silently coercing — a
//! mistyped `--timeout=5min` shouldn't quietly fall back to a default.

use std::time::Duration;

/// Parse a Go-style duration string. Accepts a non-negative number
/// followed by one of the units `ns | us | ms | s | m | h`. No
/// fractional input today (KISS); revisit if a verb actually needs it.
///
/// Examples: `"30s"`, `"5m"`, `"100ms"`, `"1h"`.
pub fn parse_go_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    let split = s
        .find(|c: char| c.is_ascii_alphabetic())
        .ok_or_else(|| format!("missing unit in `{s}` (expected one of ns/us/ms/s/m/h)"))?;
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|e| format!("invalid number `{num}` in `{s}`: {e}"))?;
    let d = match unit {
        "ns" => Duration::from_nanos(n),
        "us" | "µs" => Duration::from_micros(n),
        "ms" => Duration::from_millis(n),
        "s" => Duration::from_secs(n),
        "m" => Duration::from_secs(n * 60),
        "h" => Duration::from_secs(n * 60 * 60),
        other => {
            return Err(format!(
                "unknown unit `{other}` in `{s}` (expected ns/us/ms/s/m/h)"
            ));
        }
    };
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_seconds_minutes_hours() {
        assert_eq!(parse_go_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_go_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_go_duration("1h").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn parses_subsecond_units() {
        assert_eq!(
            parse_go_duration("250ms").unwrap(),
            Duration::from_millis(250)
        );
        assert_eq!(
            parse_go_duration("100us").unwrap(),
            Duration::from_micros(100)
        );
        assert_eq!(parse_go_duration("5ns").unwrap(), Duration::from_nanos(5));
    }

    #[test]
    fn rejects_missing_unit() {
        assert!(parse_go_duration("30")
            .unwrap_err()
            .contains("missing unit"));
    }

    #[test]
    fn rejects_unknown_unit() {
        let err = parse_go_duration("5min").unwrap_err();
        assert!(err.contains("unknown unit"), "{err}");
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_go_duration("").is_err());
    }
}
