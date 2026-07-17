use chrono::{Datelike, NaiveDate};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::calendar::CalendarEventStore;

use crate::core::dashboard::DailyActivity;
use crate::ui::colors;

/// Background shades for activity levels 1-4: GitHub-dark semantics built on
/// our design green (level 4 is the base color).
const ACTIVITY_BG: [Color; 4] = [
    Color::from_u32(0x1a433b),
    Color::from_u32(0x237664),
    Color::from_u32(0x2ba88d),
    colors::GREEN,
];

/// Points that make up a day's activity.
pub fn activity_total(day: &DailyActivity) -> usize {
    day.sessions + day.new_topics + day.completed_topics
}

/// Spike-resistant scale maximum: the 90th percentile of non-zero totals, so a
/// single huge day does not flatten everything else into level 1.
pub fn robust_max(totals: &[usize]) -> f64 {
    let mut non_zero: Vec<usize> = totals.iter().copied().filter(|t| *t > 0).collect();
    if non_zero.is_empty() {
        return 1.0;
    }
    non_zero.sort_unstable();
    let idx = (0.9 * (non_zero.len() - 1) as f64).floor() as usize;
    (non_zero[idx] as f64).max(1.0)
}

/// Maps a day's total to an intensity level 0..=4 against the scale maximum.
pub fn intensity_level(total: usize, max: f64) -> usize {
    if total == 0 {
        return 0;
    }
    ((total as f64 / max) * 4.0).ceil().clamp(1.0, 4.0) as usize
}

pub fn chrono_to_time(date: NaiveDate) -> time::Date {
    time::Date::from_calendar_date(
        date.year(),
        time::Month::try_from(date.month() as u8).unwrap_or(time::Month::January),
        date.day() as u8,
    )
    .expect("chrono date is always valid")
}

fn days_in_month(date: NaiveDate) -> u16 {
    let (y, m) = (date.year(), date.month());
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let next = NaiveDate::from_ymd_opt(ny, nm, 1).expect("valid date");
    (next - date.with_day(1).expect("day 1 exists")).num_days() as u16
}

/// Height the activity calendar block needs: borders + month/weekday headers +
/// the number of Sunday-based week rows of the month containing `date`.
pub fn block_height(date: NaiveDate) -> u16 {
    let first = date.with_day(1).expect("day 1 exists");
    let offset = first.weekday().num_days_from_sunday() as u16;
    2 + 1 + 1 + (offset + days_in_month(date) + 6) / 7
}

fn level_style(level: usize) -> Style {
    debug_assert!((1..=4).contains(&level));
    let style = Style::default().bg(ACTIVITY_BG[level - 1]);
    if level >= 3 {
        style.fg(Color::Black)
    } else {
        style
    }
}

fn parse_day(date: &str) -> Option<time::Date> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(chrono_to_time)
}

/// Builds per-day styles for the calendar: green background shades on a
/// relative p90 scale (GitHub-style), no background on quiet days, and a light
/// marker on today (bold underline on its activity shade, or green text when
/// the day is quiet).
pub fn build_event_store(activity: &[DailyActivity], today: NaiveDate) -> CalendarEventStore {
    let totals: Vec<usize> = activity.iter().map(activity_total).collect();
    let max = robust_max(&totals);
    let today_time = chrono_to_time(today);

    let mut store = CalendarEventStore::default();
    let mut today_has_activity = false;
    for day in activity {
        let level = intensity_level(activity_total(day), max);
        if level == 0 {
            continue;
        }
        let Some(date) = parse_day(&day.date) else {
            continue;
        };
        let mut style = level_style(level);
        if date == today_time {
            today_has_activity = true;
            style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        }
        store.add(date, style);
    }

    if !today_has_activity {
        store.add(
            today_time,
            Style::default()
                .fg(colors::GREEN)
                .add_modifier(Modifier::BOLD),
        );
    }
    store
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str, sessions: usize, new_topics: usize, completed_topics: usize) -> DailyActivity {
        DailyActivity {
            date: date.to_string(),
            sessions,
            new_topics,
            completed_topics,
        }
    }

    #[test]
    fn robust_max_ignores_spikes() {
        assert_eq!(robust_max(&[]), 1.0);
        assert_eq!(robust_max(&[5]), 5.0);
        assert_eq!(robust_max(&[2, 2, 2, 100]), 2.0);
        assert_eq!(robust_max(&[0, 0, 3]), 3.0);
    }

    #[test]
    fn intensity_levels() {
        assert_eq!(intensity_level(0, 10.0), 0);
        assert_eq!(intensity_level(1, 10.0), 1);
        assert_eq!(intensity_level(5, 10.0), 2);
        assert_eq!(intensity_level(10, 10.0), 4);
        assert_eq!(intensity_level(50, 10.0), 4);
    }

    #[test]
    fn block_height_matches_week_rows() {
        // Feb 2026: starts on Sunday, 28 days -> 4 week rows.
        let feb = NaiveDate::from_ymd_opt(2026, 2, 15).unwrap();
        assert_eq!(block_height(feb), 2 + 1 + 1 + 4);
        // Jul 2026: starts on Wednesday, 31 days -> 5 week rows.
        let jul = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
        assert_eq!(block_height(jul), 2 + 1 + 1 + 5);
        // Aug 2026: starts on Saturday, 31 days -> 6 week rows.
        let aug = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        assert_eq!(block_height(aug), 2 + 1 + 1 + 6);
    }

    #[test]
    fn event_store_marks_activity_and_quiet_today() {
        let today = NaiveDate::from_ymd_opt(2024, 5, 10).unwrap();
        let activity = vec![day("2024-05-09", 3, 0, 0), day("2024-05-10", 0, 0, 0)];
        let store = build_event_store(&activity, today);

        let active = store
            .0
            .get(&chrono_to_time(NaiveDate::from_ymd_opt(2024, 5, 9).unwrap()))
            .copied()
            .unwrap();
        assert_eq!(active.bg, Some(ACTIVITY_BG[3]));
        assert_eq!(active.fg, Some(Color::Black));

        let today_style = store.0.get(&chrono_to_time(today)).copied().unwrap();
        assert_eq!(today_style.bg, None);
        assert_eq!(today_style.fg, Some(colors::GREEN));
        assert!(today_style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn event_store_today_with_activity_keeps_marker() {
        let today = NaiveDate::from_ymd_opt(2024, 5, 10).unwrap();
        let activity = vec![day("2024-05-10", 2, 0, 0)];
        let store = build_event_store(&activity, today);

        let style = store.0.get(&chrono_to_time(today)).copied().unwrap();
        assert_eq!(style.bg, Some(ACTIVITY_BG[3]));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }
}
