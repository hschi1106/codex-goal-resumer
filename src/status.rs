use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
};
use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaState {
    Available,
    Exhausted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub quota: QuotaState,
    pub next_reset: Option<DateTime<FixedOffset>>,
}

#[derive(Debug)]
struct QuotaRecord<'a> {
    label: String,
    percentage: u8,
    line: &'a str,
}

pub fn parse_latest_status(output: &str, now: DateTime<FixedOffset>) -> Status {
    let records = quota_records(output);
    if records.is_empty() {
        return Status {
            quota: QuotaState::Unknown,
            next_reset: parse_try_again(output, now),
        };
    }

    let latest = latest_generation(&records);
    let quota = if latest.iter().any(|record| record.percentage == 0) {
        QuotaState::Exhausted
    } else {
        QuotaState::Available
    };
    let next_reset = latest
        .iter()
        .filter(|record| record.percentage == 0)
        .filter_map(|record| parse_reset(record.line, now))
        .min()
        .or_else(|| parse_try_again(output, now));

    Status { quota, next_reset }
}

pub fn contains_usage_limit(output: &str) -> bool {
    usage_regex().is_match(output)
}

fn quota_records(output: &str) -> Vec<QuotaRecord<'_>> {
    quota_regex()
        .captures_iter(output)
        .filter_map(|capture| {
            let percentage = capture.name("pct")?.as_str().parse().ok()?;
            if percentage > 100 {
                return None;
            }
            Some(QuotaRecord {
                label: normalize_label(capture.name("label")?.as_str()),
                percentage,
                line: capture.get(0)?.as_str(),
            })
        })
        .collect()
}

fn normalize_label(label: &str) -> String {
    let normalized = label
        .trim_matches(|character: char| !character.is_alphanumeric())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if normalized.contains("weekly") {
        "weekly limit".to_owned()
    } else if normalized.contains("5h") || normalized.contains("5 h") {
        "5h limit".to_owned()
    } else {
        normalized
    }
}

fn latest_generation<'a>(records: &'a [QuotaRecord<'a>]) -> &'a [QuotaRecord<'a>] {
    let mut labels = HashSet::new();
    let mut start = 0;
    for (index, record) in records.iter().enumerate() {
        if !labels.insert(record.label.as_str()) {
            start = index;
            labels.clear();
            labels.insert(record.label.as_str());
        }
    }
    &records[start..]
}

fn parse_reset(text: &str, now: DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
    if let Some(capture) = dated_reset_regex().captures(text) {
        let time = parse_time(&capture[1])?;
        let day = capture[2].parse().ok()?;
        let month = parse_month(&capture[3])?;
        return next_dated_reset(now, month, day, time);
    }
    let capture = same_day_reset_regex().captures(text)?;
    let time = parse_time(&capture[1])?;
    let date = now.date_naive();
    let mut reset = local_datetime(now.offset(), date.and_time(time))?;
    if reset <= now {
        reset = local_datetime(now.offset(), (date + Duration::days(1)).and_time(time))?;
    }
    Some(reset)
}

fn next_dated_reset(
    now: DateTime<FixedOffset>,
    month: u32,
    day: u32,
    time: NaiveTime,
) -> Option<DateTime<FixedOffset>> {
    for year in [now.year(), now.year() + 1] {
        let date = NaiveDate::from_ymd_opt(year, month, day)?;
        let candidate = local_datetime(now.offset(), date.and_time(time))?;
        if candidate > now {
            return Some(candidate);
        }
    }
    None
}

fn parse_try_again(text: &str, now: DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
    let capture = try_again_regex().captures_iter(text).last()?;
    let month = parse_month(&capture[1])?;
    let day = capture[2].parse().ok()?;
    let year = capture[3].parse().ok()?;
    let hour: u32 = capture[4].parse().ok()?;
    let minute = capture[5].parse().ok()?;
    let hour = match capture[6].to_ascii_uppercase().as_str() {
        "AM" => hour % 12,
        "PM" => hour % 12 + 12,
        _ => return None,
    };
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    local_datetime(now.offset(), date.and_time(time))
}

fn local_datetime(offset: &FixedOffset, datetime: NaiveDateTime) -> Option<DateTime<FixedOffset>> {
    offset.from_local_datetime(&datetime).single()
}

fn parse_time(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").ok()
}

fn parse_month(value: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    MONTHS
        .iter()
        .position(|month| month.eq_ignore_ascii_case(value))
        .map(|index| index as u32 + 1)
}

fn quota_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?im)^(?P<label>[^\r\n:]{1,60}(?:limit|usage))\s*:\s*[^\r\n]*?(?P<pct>\d{1,3})\s*%\s*left[^\r\n]*").unwrap()
    })
}

fn same_day_reset_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?i)resets?\s+(\d{1,2}:\d{2})(?:\s*[)])?").unwrap())
}

fn dated_reset_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)resets?\s+(\d{1,2}:\d{2})\s+on\s+(\d{1,2})\s+([A-Za-z]{3})").unwrap()
    })
}

fn try_again_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)try again at\s+([A-Za-z]{3})\s+(\d{1,2})(?:st|nd|rd|th)?,\s*(\d{4})\s+(\d{1,2}):(\d{2})\s*(AM|PM)").unwrap()
    })
}

fn usage_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b(?:usage\s+(?:limited|limit\s+(?:reached|exceeded))|you(?:'|’)ve\s+hit\s+your\s+usage\s+limit|rate\s+(?:limited|limit\s+(?:reached|exceeded))|try\s+again\s+at)\b").unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now(month: u32, day: u32, hour: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, month, day, hour, 0, 0)
            .unwrap()
    }

    #[test]
    fn zero_is_exhausted() {
        assert_eq!(
            parse_latest_status("5h limit: 0% left", now(9, 11, 12)).quota,
            QuotaState::Exhausted
        );
    }

    #[test]
    fn one_hundred_is_available() {
        assert_eq!(
            parse_latest_status("5h limit: 100% left", now(9, 11, 12)).quota,
            QuotaState::Available
        );
    }

    #[test]
    fn nonzero_is_available() {
        assert_eq!(
            parse_latest_status("5h limit: 43% left", now(9, 11, 12)).quota,
            QuotaState::Available
        );
    }

    #[test]
    fn any_window_exhausted_is_exhausted() {
        let text = "5h limit: 80% left\nWeekly limit: 0% left";
        assert_eq!(
            parse_latest_status(text, now(9, 11, 12)).quota,
            QuotaState::Exhausted
        );
    }

    #[test]
    fn all_windows_nonzero_is_available() {
        let text = "5h limit: 80% left\nWeekly limit: 10% left";
        assert_eq!(
            parse_latest_status(text, now(9, 11, 12)).quota,
            QuotaState::Available
        );
    }

    #[test]
    fn parses_same_day_reset() {
        let status = parse_latest_status("5h limit: 0% left (resets 15:35)", now(9, 11, 12));
        assert_eq!(
            status
                .next_reset
                .unwrap()
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            "2026-09-11 15:35"
        );
    }

    #[test]
    fn parses_weekly_reset() {
        let status = parse_latest_status(
            "Weekly limit: 0% left (resets 06:20 on 20 Sep)",
            now(9, 11, 12),
        );
        assert_eq!(
            status
                .next_reset
                .unwrap()
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            "2026-09-20 06:20"
        );
    }

    #[test]
    fn infers_next_year() {
        let status = parse_latest_status(
            "Weekly limit: 0% left (resets 06:20 on 02 Jan)",
            now(12, 29, 12),
        );
        assert_eq!(
            status
                .next_reset
                .unwrap()
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            "2027-01-02 06:20"
        );
    }

    #[test]
    fn parses_try_again_timestamp() {
        let status = parse_latest_status("try again at Sep 11th, 2026 4:37 PM", now(9, 11, 12));
        assert_eq!(
            status
                .next_reset
                .unwrap()
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            "2026-09-11 16:37"
        );
    }

    #[test]
    fn latest_status_wins() {
        let text = "5h limit: 0% left\nWeekly limit: 0% left\n\n...\n\n5h limit: 100% left\nWeekly limit: 100% left";
        assert_eq!(
            parse_latest_status(text, now(9, 11, 12)).quota,
            QuotaState::Available
        );
    }

    #[test]
    fn malformed_is_unknown() {
        assert_eq!(
            parse_latest_status("random unrelated output", now(9, 11, 12)).quota,
            QuotaState::Unknown
        );
    }

    #[test]
    fn old_available_new_exhausted_uses_new_generation() {
        let text =
            "5h limit: 50% left\nWeekly limit: 10% left\n5h limit: 0% left\nWeekly limit: 0% left";
        assert_eq!(
            parse_latest_status(text, now(9, 11, 12)).quota,
            QuotaState::Exhausted
        );
    }

    #[test]
    fn available_manual_reset_message_is_not_a_limit_event() {
        assert!(!contains_usage_limit(
            "You have 1 usage limit reset available. Run /usage to use one."
        ));
    }

    #[test]
    fn status_rate_limit_description_is_not_a_limit_event() {
        assert!(!contains_usage_limit(
            "Visit the settings page for information on rate limits and credits"
        ));
    }

    #[test]
    fn explicit_limit_messages_are_detected() {
        for message in [
            "usage limited",
            "usage limit reached",
            "usage limit exceeded",
            "You've hit your usage limit",
            "rate limited",
            "rate limit reached",
            "try again at Sep 11th, 2026 4:37 PM",
        ] {
            assert!(contains_usage_limit(message), "missed: {message}");
        }
    }
}
