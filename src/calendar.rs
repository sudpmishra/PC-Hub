//! Today's next events, from calendar links in hub.toml. Google Calendar and Outlook both
//! offer one: Google's "Secret address in iCal format" (calendar settings) and Outlook's
//! "Publish a calendar" ICS link. Each is downloaded every 10 minutes.
//!
//! Repeating events are expanded for the common rules (daily, weekly on chosen days,
//! monthly on a date or "first Monday", yearly). Times with a named time zone are taken
//! as the PC's local time.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Weekday};

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub all_day: bool,
    pub title: String,
    pub location: String,
}

/// Downloads every calendar and returns the events from `now` to the end of tomorrow that
/// haven't ended, soonest first. A calendar that fails is reported in the error list.
pub fn upcoming(urls: &[String], now: NaiveDateTime) -> (Vec<Event>, Vec<String>) {
    let mut events = Vec::new();
    let mut errors = Vec::new();
    let until = (now.date() + Duration::days(2)).and_time(NaiveTime::MIN);
    for url in urls.iter().map(|u| u.trim()).filter(|u| !u.is_empty()) {
        // webcal:// links are https links in disguise.
        let url = url.strip_prefix("webcal://").map(|rest| format!("https://{rest}")).unwrap_or(url.to_string());
        let text = if url.starts_with("http") {
            crate::web::fetch_text(&url)
        } else {
            std::fs::read_to_string(&url).map_err(|e| e.to_string())
        };
        match text {
            Ok(text) => events.extend(between(&parse(&text), now, until)),
            Err(err) => errors.push(err),
        }
    }
    events.sort_by_key(|e| (!e.all_day, e.start));
    events.dedup();
    (events, errors)
}

/// One VEVENT as written in the file.
#[derive(Clone, Debug, Default)]
struct Raw {
    uid: String,
    start: Option<(NaiveDateTime, bool)>,
    end: Option<NaiveDateTime>,
    duration: Option<Duration>,
    title: String,
    location: String,
    rule: Option<String>,
    exceptions: Vec<NaiveDateTime>,
    /// For a changed occurrence of a repeating event: the occurrence it replaces.
    replaces: Option<NaiveDateTime>,
    cancelled: bool,
}

/// Joins folded lines (a line starting with a space continues the one before).
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(line.to_string()),
        }
    }
    lines
}

/// "20261011T140000Z" (UTC), "20261011T140000" (local) or "20261011" (a whole day).
fn date_time(value: &str, params: &str) -> Option<(NaiveDateTime, bool)> {
    let value = value.trim();
    if params.contains("VALUE=DATE") && !params.contains("VALUE=DATE-TIME") || value.len() == 8 {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        return Some((date.and_time(NaiveTime::MIN), true));
    }
    let (value, utc) = match value.strip_suffix('Z') {
        Some(v) => (v, true),
        None => (value, false),
    };
    let time = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    if utc {
        Some((chrono::Utc.from_utc_datetime(&time).with_timezone(&chrono::Local).naive_local(), false))
    } else {
        Some((time, false))
    }
}

/// "PT1H30M", "P1D" and the like.
fn duration(value: &str) -> Option<Duration> {
    let value = value.trim().trim_start_matches('+');
    let rest = value.strip_prefix('P')?;
    let (mut total, mut number) = (Duration::zero(), String::new());
    for c in rest.chars() {
        match c {
            '0'..='9' => number.push(c),
            'T' => {}
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += match unit {
                    'W' => Duration::weeks(n),
                    'D' => Duration::days(n),
                    'H' => Duration::hours(n),
                    'M' => Duration::minutes(n),
                    'S' => Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    Some(total)
}

/// Undoes iCalendar's escaping: "\\," -> ",", "\\n" -> " ".
fn text(value: &str) -> String {
    value.replace("\\n", " ").replace("\\N", " ").replace("\\,", ",").replace("\\;", ";").replace("\\\\", "\\").trim().to_string()
}

fn parse(text_: &str) -> Vec<Raw> {
    let mut events = Vec::new();
    let mut current: Option<Raw> = None;
    for line in unfold(text_) {
        let Some((key, value)) = line.split_once(':') else { continue };
        let (name, params) = key.split_once(';').unwrap_or((key, ""));
        match (name.to_ascii_uppercase().as_str(), current.as_mut()) {
            ("BEGIN", None) if value.trim() == "VEVENT" => current = Some(Raw::default()),
            ("END", Some(_)) if value.trim() == "VEVENT" => events.extend(current.take()),
            ("UID", Some(e)) => e.uid = value.trim().to_string(),
            ("DTSTART", Some(e)) => e.start = date_time(value, params),
            ("DTEND", Some(e)) => e.end = date_time(value, params).map(|(t, _)| t),
            ("DURATION", Some(e)) => e.duration = duration(value),
            ("SUMMARY", Some(e)) => e.title = text(value),
            ("LOCATION", Some(e)) => e.location = text(value),
            ("RRULE", Some(e)) => e.rule = Some(value.trim().to_string()),
            ("EXDATE", Some(e)) => e.exceptions.extend(value.split(',').filter_map(|v| date_time(v, params)).map(|(t, _)| t)),
            ("RECURRENCE-ID", Some(e)) => e.replaces = date_time(value, params).map(|(t, _)| t),
            ("STATUS", Some(e)) => e.cancelled = value.trim().eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }
    events
}

/// The occurrences of every event that overlap `from`..`until`.
fn between(raws: &[Raw], from: NaiveDateTime, until: NaiveDateTime) -> Vec<Event> {
    // Changed occurrences replace the ones the rule would make.
    let replaced: Vec<(&str, NaiveDateTime)> =
        raws.iter().filter_map(|r| r.replaces.map(|t| (r.uid.as_str(), t))).collect();
    let mut events = Vec::new();
    for raw in raws.iter().filter(|r| !r.cancelled) {
        let Some((start, all_day)) = raw.start else { continue };
        let length = match (raw.end, raw.duration) {
            (Some(end), _) if end > start => end - start,
            (_, Some(d)) => d,
            _ if all_day => Duration::days(1),
            _ => Duration::zero(),
        };
        let starts = match raw.rule.as_deref().filter(|_| raw.replaces.is_none()) {
            Some(rule) => occurrences(start, rule, until),
            None => vec![start],
        };
        for start in starts {
            let end = start + length;
            let skipped = raw.exceptions.contains(&start)
                || (raw.replaces.is_none() && replaced.iter().any(|(uid, t)| *uid == raw.uid && *t == start));
            // All-day events last while their day lasts; others until they end.
            let still_on = if length.is_zero() { start >= from } else { end > from };
            if !skipped && still_on && start < until {
                events.push(Event {
                    start,
                    end,
                    all_day,
                    title: if raw.title.is_empty() { "(No title)".into() } else { raw.title.clone() },
                    location: raw.location.clone(),
                });
            }
        }
    }
    events
}

fn weekday(code: &str) -> Option<Weekday> {
    Some(match code {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// The `nth` (1-based, or -1 for the last) `day` of a month.
fn nth_weekday(year: i32, month: u32, day: Weekday, nth: i32) -> Option<NaiveDate> {
    if nth > 0 {
        NaiveDate::from_weekday_of_month_opt(year, month, day, nth as u8)
    } else {
        let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
        let mut date = NaiveDate::from_ymd_opt(next_year, next_month, 1)?.pred_opt()?;
        while date.weekday() != day {
            date = date.pred_opt()?;
        }
        Some(date - Duration::weeks(i64::from(-nth - 1)))
    }
}

fn add_months(date: NaiveDate, months: i32) -> Option<NaiveDate> {
    let total = date.year() * 12 + date.month0() as i32 + months;
    NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, date.day())
}

/// Start times of a repeating event up to `until`, from its rule ("FREQ=WEEKLY;BYDAY=MO,WE").
fn occurrences(start: NaiveDateTime, rule: &str, until: NaiveDateTime) -> Vec<NaiveDateTime> {
    let part = |name: &str| {
        rule.split(';')
            .find_map(|p| p.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.to_string()))
    };
    let frequency = part("FREQ").unwrap_or_default().to_ascii_uppercase();
    let interval: i64 = part("INTERVAL").and_then(|v| v.parse().ok()).filter(|&n| n > 0).unwrap_or(1);
    let count: Option<usize> = part("COUNT").and_then(|v| v.parse().ok());
    let last = part("UNTIL").and_then(|v| date_time(&v, "")).map(|(t, all_day)| {
        if all_day { t + Duration::days(1) - Duration::seconds(1) } else { t }
    });
    let end = last.map_or(until, |l| l.min(until));
    // Weekdays, each with an optional position in the month ("2MO", "-1FR").
    let days: Vec<(i32, Weekday)> = part("BYDAY")
        .map(|v| {
            v.split(',')
                .filter_map(|d| {
                    let d = d.trim().to_ascii_uppercase();
                    let split = d.len().checked_sub(2)?;
                    Some((d[..split].parse().unwrap_or(0), weekday(&d[split..])?))
                })
                .collect()
        })
        .unwrap_or_default();

    let time = start.time();
    let mut found = Vec::new();
    let take = |t: NaiveDateTime, found: &mut Vec<NaiveDateTime>| -> bool {
        if t < start {
            return true;
        }
        if t > end || count.is_some_and(|c| found.len() >= c) {
            return false;
        }
        found.push(t);
        true
    };
    // A guard against rules that would run for a very long time.
    for step in 0..20_000i64 {
        let more = match frequency.as_str() {
            "DAILY" => take(start + Duration::days(step * interval), &mut found),
            "WEEKLY" if !days.is_empty() => {
                let monday = start.date() - Duration::days(i64::from(start.weekday().num_days_from_monday()));
                let week = monday + Duration::weeks(step * interval);
                let mut dates: Vec<NaiveDate> =
                    days.iter().map(|(_, d)| week + Duration::days(i64::from(d.num_days_from_monday()))).collect();
                dates.sort();
                dates.into_iter().all(|date| take(date.and_time(time), &mut found))
            }
            "WEEKLY" => take(start + Duration::weeks(step * interval), &mut found),
            "MONTHLY" => {
                let Some(month) = add_months(start.date().with_day(1).unwrap_or(start.date()), (step * interval) as i32) else {
                    break;
                };
                let mut dates: Vec<NaiveDate> = if days.is_empty() {
                    month.with_day(start.day()).into_iter().collect()
                } else {
                    days.iter()
                        .filter_map(|(nth, day)| nth_weekday(month.year(), month.month(), *day, if *nth == 0 { 1 } else { *nth }))
                        .collect()
                };
                dates.sort();
                if month.and_time(time) > end {
                    false
                } else {
                    dates.into_iter().all(|date| take(date.and_time(time), &mut found))
                }
            }
            "YEARLY" => match add_months(start.date(), (step * interval * 12) as i32) {
                Some(date) => take(date.and_time(time), &mut found),
                // February 29th in a year without one.
                None => start.date().year() + ((step * interval) as i32) <= end.year(),
            },
            _ => {
                found.push(start);
                false
            }
        };
        if !more {
            break;
        }
    }
    found
}

/// "All day", "14:00–15:30" or "2:00 PM", for the hub.
pub fn when(event: &Event, clock_24h: bool) -> String {
    if event.all_day {
        return "All day".into();
    }
    let format = if clock_24h { "%H:%M" } else { "%-I:%M %p" };
    if event.end > event.start {
        format!("{}–{}", event.start.format(format), event.end.format(format))
    } else {
        event.start.format(format).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    const CALENDAR: &str = "BEGIN:VCALENDAR\r
BEGIN:VEVENT\r
UID:standup\r
DTSTART;TZID=Asia/Kathmandu:20260901T093000\r
DTEND;TZID=Asia/Kathmandu:20260901T094500\r
RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR\r
EXDATE;TZID=Asia/Kathmandu:20261012T093000\r
SUMMARY:Stand-up\\, team\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:lunch\r
DTSTART:20261012T120000\r
DURATION:PT1H\r
SUMMARY:Lunch with\r
  Sam\r
LOCATION:Cafe\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:holiday\r
DTSTART;VALUE=DATE:20261013\r
DTEND;VALUE=DATE:20261014\r
SUMMARY:Holiday\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:old\r
DTSTART:20261001T100000\r
DTEND:20261001T110000\r
SUMMARY:Long gone\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:review\r
DTSTART:20260105T150000\r
RRULE:FREQ=MONTHLY;BYDAY=2MO\r
SUMMARY:Review\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:standup\r
RECURRENCE-ID;TZID=Asia/Kathmandu:20261013T093000\r
DTSTART;TZID=Asia/Kathmandu:20261013T100000\r
DTEND;TZID=Asia/Kathmandu:20261013T101500\r
SUMMARY:Stand-up (late)\r
END:VEVENT\r
END:VCALENDAR\r
";

    #[test]
    fn upcoming_events() {
        // Monday 12 October 2026, 9 in the morning.
        let now = at("2026-10-12 09:00");
        let until = at("2026-10-14 00:00");
        let mut events = between(&parse(CALENDAR), now, until);
        events.sort_by_key(|e| (!e.all_day, e.start));
        let titles: Vec<(&str, NaiveDateTime)> = events.iter().map(|e| (e.title.as_str(), e.start)).collect();
        assert_eq!(
            titles,
            [
                ("Holiday", at("2026-10-13 00:00")),
                // Monday's stand-up is excepted; the second Monday's review is on.
                ("Lunch with Sam", at("2026-10-12 12:00")),
                ("Review", at("2026-10-12 15:00")),
                // Tuesday's stand-up moved to 10:00.
                ("Stand-up (late)", at("2026-10-13 10:00")),
            ]
        );
        assert_eq!(events[1].location, "Cafe");
        assert_eq!(when(&events[1], true), "12:00–13:00");
        assert_eq!(when(&events[0], true), "All day");
    }

    #[test]
    fn rules() {
        let start = at("2026-01-31 08:00");
        let until = at("2026-06-01 00:00");
        // Months without a 31st are skipped.
        assert_eq!(occurrences(start, "FREQ=MONTHLY", until).len(), 3);
        assert_eq!(occurrences(start, "FREQ=DAILY;COUNT=3", until).len(), 3);
        assert_eq!(occurrences(start, "FREQ=DAILY;UNTIL=20260202", until).len(), 3);
        assert_eq!(occurrences(start, "FREQ=WEEKLY;INTERVAL=2", at("2026-03-01 00:00")).len(), 3);
        let last_friday = occurrences(at("2026-01-30 17:00"), "FREQ=MONTHLY;BYDAY=-1FR", until);
        assert_eq!(last_friday[1], at("2026-02-27 17:00"));
        assert_eq!(duration("PT1H30M"), Some(Duration::minutes(90)));
        assert_eq!(duration("P1D"), Some(Duration::days(1)));
    }
}
