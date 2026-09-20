//! Rendering the occurrence log as readable text.
//!
//! This module reads only [`OccurrenceRecord`], which [`crate::TraceStore::log`]
//! itself produces without ever touching `raw_carrier_records`. Sharing this
//! renderer between the `semon log` command and the store's own tests is what
//! lets the empty-raw-region property be pinned as a byte-for-byte assertion
//! rather than an eyeballed comparison of two separately written formatters.

use serde_json::Value;

use crate::OccurrenceRecord;

const MAX_SUMMARY_CHARS: usize = 240;

/// Renders one occurrence, joined to its trace, as a single line of text.
///
/// The line never depends on anything outside `occurrence` and its trace's
/// semantic core, so re-rendering the same occurrence always produces the
/// same line, regardless of what has happened to the forensic region.
pub fn render_occurrence_line(occurrence: &OccurrenceRecord) -> String {
    let timestamp = format_timestamp_ns(occurrence.timestamp());
    let repo = if occurrence.repo().is_empty() {
        "-"
    } else {
        occurrence.repo()
    };
    let parent = occurrence
        .parent_sequence()
        .map_or_else(|| "-".to_owned(), |value| value.to_string());
    let agent = occurrence.agent().unwrap_or("-");
    format!(
        "{timestamp} {repo} [{source}] {session}#{sequence} parent={parent} agent={agent} by={by} {summary}",
        source = occurrence.repo_source().as_str(),
        session = occurrence.session(),
        sequence = occurrence.sequence(),
        by = occurrence.authored_by().as_str(),
        summary = summarize(occurrence.semantic_core().value()),
    )
}

fn summarize(value: &Value) -> String {
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("?");
    match kind {
        "intent" | "outcome" => {
            let content = value.get("content").and_then(Value::as_str).unwrap_or("");
            format!("{kind}: {}", truncate(&content.replace('\n', " / ")))
        }
        _ => {
            let rest = serde_json::to_string(value).unwrap_or_default();
            format!("{kind}: {}", truncate(&rest))
        }
    }
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_SUMMARY_CHARS {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(MAX_SUMMARY_CHARS).collect();
    truncated.push('…');
    truncated
}

/// Returns the half-open nanosecond range `[start, end)` since the Unix
/// epoch spanning one UTC calendar day.
pub fn day_bounds_ns(year: i64, month: u32, day: u32) -> (i64, i64) {
    let start_day = days_from_civil(year, month, day);
    let start_ns = start_day * 86_400 * 1_000_000_000;
    let end_ns = start_ns + 86_400 * 1_000_000_000;
    (start_ns, end_ns)
}

/// Formats nanoseconds since the Unix epoch as an ISO-8601 UTC timestamp,
/// truncated to whole seconds (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn format_timestamp_ns(nanoseconds: i64) -> String {
    let total_seconds = nanoseconds.div_euclid(1_000_000_000);
    let days = total_seconds.div_euclid(86_400);
    let seconds_of_day = total_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The zero-based count of days since the Unix epoch (1970-01-01) for one
/// UTC calendar date. Howard Hinnant's `days_from_civil`.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`]: the UTC calendar date for a given
/// zero-based day count since the Unix epoch. Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let adjusted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * adjusted_month + 2) / 5 + 1) as u32;
    let month = if adjusted_month < 10 {
        adjusted_month + 3
    } else {
        adjusted_month - 9
    } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_conversion_round_trips_a_known_date() {
        let days = days_from_civil(2026, 9, 19);
        assert_eq!(civil_from_days(days), (2026, 9, 19));
    }

    #[test]
    fn formats_a_known_instant() {
        let days = days_from_civil(2026, 9, 19);
        let ns = days * 86_400 * 1_000_000_000 + 12 * 3600 * 1_000_000_000;
        assert_eq!(format_timestamp_ns(ns), "2026-09-19T12:00:00Z");
    }

    #[test]
    fn epoch_formats_as_the_epoch_instant() {
        assert_eq!(format_timestamp_ns(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn day_bounds_cover_exactly_one_day() {
        let (start, end) = day_bounds_ns(2026, 9, 19);
        assert_eq!(format_timestamp_ns(start), "2026-09-19T00:00:00Z");
        assert_eq!(format_timestamp_ns(end - 1), "2026-09-19T23:59:59Z");
        assert_eq!(format_timestamp_ns(end), "2026-09-20T00:00:00Z");
    }
}
