//! Retention: the most recent backup per day (`keep_daily` days), per ISO week (`keep_weekly`)
//! and per month (`keep_monthly`) is kept; the newest backup is always kept.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, Utc};

/// Indices (in `times`) of the backups to keep. `last` = always keep this many newest (at least 1).
pub fn keep(times: &[DateTime<Utc>], last: usize, daily: usize, weekly: usize, monthly: usize) -> BTreeSet<usize> {
    let mut idx: Vec<usize> = (0..times.len()).collect();
    // newest first
    idx.sort_by(|a, b| times[*b].cmp(&times[*a]));
    let mut out: BTreeSet<usize> = idx.iter().take(last.max(1)).copied().collect();
    let mut bucket = |n: usize, key: &dyn Fn(&DateTime<Utc>) -> (i32, u32)| {
        let mut seen = BTreeSet::new();
        for &i in &idx {
            if seen.len() >= n {
                break;
            }
            if seen.insert(key(&times[i])) {
                out.insert(i);
            }
        }
    };
    bucket(daily, &|t| (t.year(), t.ordinal()));
    bucket(weekly, &|t| {
        let w = t.iso_week();
        (w.year(), w.week())
    });
    bucket(monthly, &|t| (t.year(), t.month()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn daily_weekly_monthly() {
        // a backup every night at 02:15 for 400 days
        let start = Utc.with_ymd_and_hms(2025, 10, 1, 2, 15, 0).unwrap();
        let times: Vec<_> = (0..400).map(|d| start + chrono::Duration::days(d)).collect();
        let k = keep(&times, 1, 7, 4, 12);
        // the newest 7 days
        for i in 393..400 {
            assert!(k.contains(&i), "{i}");
        }
        // 7 + up to 4 weeks + 12 months, with overlaps
        assert!(k.len() <= 23 && k.len() >= 12 + 7 - 1, "{}", k.len());
        // oldest retained is just under a year back
        let oldest = k.iter().map(|i| times[*i]).min().unwrap();
        assert!(times[399] - oldest > chrono::Duration::days(300));
        assert!(times[399] - oldest < chrono::Duration::days(370));
    }

    #[test]
    fn several_per_day_and_empty() {
        assert!(keep(&[], 1, 7, 4, 12).is_empty());
        let t = Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap();
        let times = vec![t, t + chrono::Duration::hours(1), t + chrono::Duration::hours(2)];
        assert_eq!(keep(&times, 1, 7, 0, 0).into_iter().collect::<Vec<_>>(), vec![2]);
        assert_eq!(keep(&times, 0, 0, 0, 0).len(), 1, "neueste bleibt immer");
        assert_eq!(keep(&times, 3, 7, 0, 0).len(), 3);
    }
}
