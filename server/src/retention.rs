//! Which copies stay in Filen: grandfather-father-son like restic/borg. For every rule the
//! newest copy of each of the last N periods (that have a copy) is kept. A copy can be kept by
//! several rules. The newest copy is always kept.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, TimeZone};
use chrono_tz::Tz;
use nrzbackup_shared::Retention;

/// Maps a local time to its period (day, ISO week, month, year).
type PeriodFn = fn(&DateTime<Tz>) -> (i32, u32, u32);

#[derive(Debug, Clone, Copy)]
pub struct Copy {
    pub id: i64,
    pub ts: i64,
}

/// id → rules that keep it. Copies missing in the map are to be deleted.
pub fn plan(copies: &[Copy], r: &Retention, tz: Tz) -> BTreeMap<i64, Vec<&'static str>> {
    let mut sorted = copies.to_vec();
    sorted.sort_by(|a, b| b.ts.cmp(&a.ts).then(b.id.cmp(&a.id)));
    let mut keep: BTreeMap<i64, Vec<&'static str>> = BTreeMap::new();

    for c in sorted.iter().take(r.keep_last as usize) {
        keep.entry(c.id).or_default().push("last");
    }
    if let Some(newest) = sorted.first()
        && r.keep_last == 0
    {
        keep.entry(newest.id).or_default().push("last");
    }

    let local = |ts: i64| -> DateTime<Tz> {
        tz.timestamp_opt(ts, 0)
            .single()
            .unwrap_or_else(|| tz.timestamp_opt(0, 0).unwrap())
    };
    let rules: [(&'static str, u32, PeriodFn); 4] = [
        ("daily", r.daily, |d| (d.year(), d.month(), d.day())),
        ("weekly", r.weekly, |d| {
            let w = d.iso_week();
            (w.year(), w.week(), 0)
        }),
        ("monthly", r.monthly, |d| (d.year(), d.month(), 0)),
        ("yearly", r.yearly, |d| (d.year(), 0, 0)),
    ];
    for (label, n, period) in rules {
        if n == 0 {
            continue;
        }
        let mut last = None;
        let mut count = 0;
        for c in &sorted {
            let p = period(&local(c.ts));
            if last != Some(p) {
                last = Some(p);
                keep.entry(c.id).or_default().push(label);
                count += 1;
                if count >= n {
                    break;
                }
            }
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(y: i32, m: u32, d: u32, h: u32) -> i64 {
        chrono_tz::Europe::Zurich
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .unwrap()
            .timestamp()
    }

    #[test]
    fn one_each() {
        // 60 days, one backup per day at 03:00 and a second one at 15:00
        let mut copies = vec![];
        let mut id = 0;
        for day in 0..60 {
            for h in [3, 15] {
                id += 1;
                copies.push(Copy {
                    id,
                    ts: ts(2026, 9, 1, h) + day * 86_400,
                });
            }
        }
        let r = Retention {
            keep_last: 1,
            daily: 1,
            weekly: 1,
            monthly: 1,
            yearly: 1,
        };
        let keep = plan(&copies, &r, chrono_tz::Europe::Zurich);
        // newest is kept by all rules
        assert_eq!(keep.len(), 1);
        assert_eq!(
            keep[&120],
            vec!["last", "daily", "weekly", "monthly", "yearly"]
        );
    }

    #[test]
    fn gfs() {
        let mut copies = vec![];
        for day in 0..400 {
            copies.push(Copy {
                id: day + 1,
                ts: ts(2025, 10, 1, 3) + day * 86_400,
            });
        }
        let r = Retention {
            keep_last: 2,
            daily: 7,
            weekly: 4,
            monthly: 6,
            yearly: 2,
        };
        let keep = plan(&copies, &r, chrono_tz::Europe::Zurich);
        let daily = keep.values().filter(|v| v.contains(&"daily")).count();
        let weekly = keep.values().filter(|v| v.contains(&"weekly")).count();
        let monthly = keep.values().filter(|v| v.contains(&"monthly")).count();
        let yearly = keep.values().filter(|v| v.contains(&"yearly")).count();
        assert_eq!((daily, weekly, monthly, yearly), (7, 4, 6, 2));
        assert!(keep.len() < 20);
        assert!(keep.contains_key(&400));
    }

    #[test]
    fn newest_always_kept() {
        let copies = vec![Copy { id: 1, ts: 10 }, Copy { id: 2, ts: 20 }];
        let r = Retention {
            keep_last: 0,
            daily: 0,
            weekly: 0,
            monthly: 0,
            yearly: 0,
        };
        let keep = plan(&copies, &r, chrono_tz::UTC);
        assert_eq!(keep.keys().copied().collect::<Vec<_>>(), vec![2]);
    }
}
