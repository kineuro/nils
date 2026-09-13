// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 5 §10.3: the registry's calendar and its backup schedule. Both are
//! kept in the registry beside its epoch, and the schedule is read in the
//! registry's timezone: every day at 02:00 is two in the morning where the
//! registry is, whatever the host's clock says. The serve that runs the
//! queue queues a backup when the schedule has come round since the last one
//! it queued, so a host that was off at the hour queues it once it is back,
//! and never twice for one hour.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use jiff::Timestamp;
use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;
use nils_ask::hash::Locale;
use nils_registry::audit::{self, Action, Entry};
use nils_registry::home::{Home, Registry};
use nils_registry::time::{iso_of, secs_of};
use serde_json::{Value, json};

const DAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// The principal a scheduled backup is queued under, and audited as.
const PRINCIPAL: &str = "backup-schedule";

/// How often a backup comes round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Every {
    Off,
    Day,
    Week(Weekday),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Schedule {
    pub(crate) every: Every,
    hour: i8,
    minute: i8,
    /// How many archives of the registry a scheduled backup keeps.
    pub(crate) keep: Option<u32>,
}

impl Schedule {
    const OFF: Schedule = Schedule {
        every: Every::Off,
        hour: 2,
        minute: 0,
        keep: None,
    };

    /// A schedule from its words: `every` is off, day or week; `at` a time of
    /// day as `HH:MM`, two in the morning when absent; `day` the day of a
    /// week; `keep` how many archives to keep. A refusal is one sentence.
    pub(crate) fn parse(
        every: &str,
        at: Option<&str>,
        day: Option<&str>,
        keep: Option<i64>,
    ) -> Result<Schedule, String> {
        let at = at.unwrap_or("02:00");
        let refused = || format!("at is a time of day as HH:MM, not {at}");
        let (hour, minute) = match at.split_once(':') {
            Some((h, m)) if h.len() == 2 && m.len() == 2 => {
                match (h.parse::<i8>(), m.parse::<i8>()) {
                    (Ok(h), Ok(m)) if (0..24).contains(&h) && (0..60).contains(&m) => (h, m),
                    _ => return Err(refused()),
                }
            }
            _ => return Err(refused()),
        };
        let every = match every {
            "off" => Every::Off,
            "day" => Every::Day,
            "week" => {
                let named = day.unwrap_or("");
                Every::Week(
                    weekday(named)
                        .ok_or_else(|| format!("day is one of {}, not {named}", DAYS.join(", ")))?,
                )
            }
            other => return Err(format!("every is off, day or week, not {other}")),
        };
        let keep = match keep {
            None => None,
            Some(n) if (1..=1000).contains(&n) => Some(n as u32),
            Some(n) => {
                return Err(format!(
                    "keep is how many archives to keep, 1 to 1000, not {n}"
                ));
            }
        };
        Ok(Schedule {
            every,
            hour,
            minute,
            keep,
        })
    }

    /// The schedule the registry keeps: off when it keeps none, or one this
    /// binary cannot read.
    pub(crate) fn of(registry: &mut Registry) -> Schedule {
        let mut get = |key: &str| {
            registry
                .meta_value(key)
                .ok()
                .flatten()
                .filter(|v| !v.is_empty())
        };
        let every = get("backup_every").unwrap_or_else(|| "off".to_string());
        let at = get("backup_at");
        let day = get("backup_day");
        let keep = get("backup_keep").and_then(|k| k.parse::<i64>().ok());
        Schedule::parse(&every, at.as_deref(), day.as_deref(), keep).unwrap_or(Schedule::OFF)
    }

    /// The schedule in the words a body gives it.
    fn words(&self) -> Value {
        json!({
            "every": match self.every {
                Every::Off => "off",
                Every::Day => "day",
                Every::Week(_) => "week",
            },
            "at": format!("{:02}:{:02}", self.hour, self.minute),
            "day": match self.every {
                Every::Week(w) => Value::from(DAYS[w.to_monday_zero_offset() as usize]),
                _ => Value::Null,
            },
            "keep": self.keep,
        })
    }

    /// The instant the schedule last came round at or before `now`.
    pub(crate) fn previous(&self, tz: &TimeZone, now: Timestamp) -> Option<Timestamp> {
        let today = now.to_zoned(tz.clone()).date();
        let (mut date, step) = match self.every {
            Every::Off => return None,
            Every::Day => (today, 1),
            Every::Week(w) => {
                let back = (today.weekday().to_monday_zero_offset() - w.to_monday_zero_offset())
                    .rem_euclid(7);
                (days_from(today, -i64::from(back))?, 7)
            }
        };
        for _ in 0..3 {
            let at = self.on(date, tz)?;
            if at <= now {
                return Some(at);
            }
            date = days_from(date, -step)?;
        }
        None
    }

    /// The instant the schedule next comes round after `now`.
    pub(crate) fn next(&self, tz: &TimeZone, now: Timestamp) -> Option<Timestamp> {
        let today = now.to_zoned(tz.clone()).date();
        let (mut date, step) = match self.every {
            Every::Off => return None,
            Every::Day => (today, 1),
            Every::Week(w) => {
                let ahead = (w.to_monday_zero_offset() - today.weekday().to_monday_zero_offset())
                    .rem_euclid(7);
                (days_from(today, i64::from(ahead))?, 7)
            }
        };
        for _ in 0..3 {
            let at = self.on(date, tz)?;
            if at > now {
                return Some(at);
            }
            date = days_from(date, step)?;
        }
        None
    }

    /// When a scheduled backup is owed: the instant the schedule came round,
    /// once it has come round since the last backup it queued.
    pub(crate) fn owed(
        &self,
        tz: &TimeZone,
        last: Option<Timestamp>,
        now: Timestamp,
    ) -> Option<Timestamp> {
        let came = self.previous(tz, now)?;
        match last {
            Some(last) if last >= came => None,
            _ => Some(came),
        }
    }

    /// The schedule's time of day on a date, where the clock is `tz`; a time
    /// the clocks skip is the first one after it.
    fn on(&self, date: Date, tz: &TimeZone) -> Option<Timestamp> {
        date.at(self.hour, self.minute, 0, 0)
            .to_zoned(tz.clone())
            .ok()
            .map(|z| z.timestamp())
    }
}

fn days_from(date: Date, days: i64) -> Option<Date> {
    let mut d = date;
    for _ in 0..days.unsigned_abs() {
        d = if days > 0 {
            d.tomorrow().ok()?
        } else {
            d.yesterday().ok()?
        };
    }
    Some(d)
}

fn weekday(name: &str) -> Option<Weekday> {
    let at = DAYS.iter().position(|d| *d == name)?;
    Weekday::from_monday_zero_offset(at as i8).ok()
}

/// A timezone by its name; UTC for a name this engine does not know.
pub(crate) fn zone(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap_or(TimeZone::UTC)
}

/// Whether this engine knows a timezone by that name.
pub(crate) fn known(name: &str) -> bool {
    name == "UTC" || TimeZone::get(name).is_ok()
}

/// The timezones a person chooses from: UTC and the IANA names of places.
pub(crate) fn timezones() -> Vec<String> {
    let mut names: Vec<String> = jiff::tz::db()
        .available()
        .map(|n| n.as_str().to_string())
        .filter(|n| {
            n.contains('/')
                && !["Etc/", "posix/", "right/", "SystemV/"]
                    .iter()
                    .any(|p| n.starts_with(p))
        })
        .collect();
    names.push("UTC".to_string());
    names.sort();
    names.dedup();
    names
}

fn iso(t: Timestamp) -> String {
    iso_of(u64::try_from(t.as_second()).unwrap_or(0))
}

fn last_queued(registry: &mut Registry) -> Option<Timestamp> {
    let text = registry.meta_value("backup_last").ok().flatten()?;
    Timestamp::from_second(i64::try_from(secs_of(&text)?).ok()?).ok()
}

/// The schedule as a door says it: its words, the timezone it is read in,
/// when it last queued a backup, and when it next comes round, as an instant
/// and as the registry's clock reads it.
pub(crate) fn document(registry: &mut Registry, now: Timestamp) -> Value {
    let schedule = Schedule::of(registry);
    let name = registry.meta().timezone.clone();
    let tz = zone(&name);
    let next = schedule.next(&tz, now);
    let mut doc = schedule.words();
    doc["timezone"] = json!(name);
    doc["next"] = json!(next.map(iso));
    doc["next_local"] = json!(next.map(|t| {
        t.to_zoned(tz.clone())
            .strftime("%Y-%m-%dT%H:%M")
            .to_string()
    }));
    doc["last"] = json!(last_queued(registry).map(iso));
    doc
}

/// Keep a schedule in the registry, audited. What came round before it was
/// set is not owed: the first backup it queues is the next one.
pub(crate) fn set(
    registry: &mut Registry,
    schedule: &Schedule,
    principal: &str,
    now: Timestamp,
) -> Result<(), String> {
    let tz = zone(&registry.meta().timezone);
    let came = schedule.previous(&tz, now).map(iso).unwrap_or_default();
    let words = schedule.words();
    let write = |registry: &mut Registry| -> Result<(), String> {
        let text = |v: &Value| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
                .unwrap_or_default()
        };
        for (key, field) in [
            ("backup_every", "every"),
            ("backup_at", "at"),
            ("backup_day", "day"),
            ("backup_keep", "keep"),
        ] {
            registry
                .set_meta(key, &text(&words[field]))
                .map_err(|e| e.to_string())?;
        }
        registry
            .set_meta("backup_last", &came)
            .map_err(|e| e.to_string())?;
        audit::record(
            registry,
            &Entry {
                principal,
                action: Action::BackupSchedule,
                scope: words.clone(),
                policy: None,
                job_id: None,
                details: None,
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    };
    registry.store().begin().map_err(|e| e.to_string())?;
    match write(registry) {
        Ok(()) => registry.store().commit().map_err(|e| e.to_string()),
        Err(why) => {
            let _ = registry.store().rollback();
            Err(why)
        }
    }
}

/// The registry's calendar as a door says it, with the choices a person has.
pub(crate) fn calendar(registry: &Registry) -> Value {
    let meta = registry.meta();
    json!({
        "timezone": meta.timezone,
        "week_start": meta.week_start,
        "epoch": meta.epoch,
        "week_starts": Locale::WEEK_STARTS,
        "timezones": timezones(),
    })
}

/// Change the registry's calendar, audited. The epoch moves, since every
/// answer's dates are read under it; the same calendar again changes nothing.
/// A refusal carries the status a door answers with.
pub(crate) fn set_calendar(
    registry: &mut Registry,
    locale: &Locale,
    principal: &str,
) -> Result<bool, (u16, String)> {
    locale.check().map_err(|m| (400, m))?;
    if !known(&locale.timezone) {
        return Err((
            400,
            format!("{} is not a timezone this engine knows", locale.timezone),
        ));
    }
    let meta = registry.meta();
    if meta.timezone == locale.timezone && meta.week_start == locale.week_start {
        return Ok(false);
    }
    let write = |registry: &mut Registry| -> Result<(), String> {
        registry
            .set_meta("timezone", &locale.timezone)
            .map_err(|e| e.to_string())?;
        registry
            .set_meta("week_start", &locale.week_start)
            .map_err(|e| e.to_string())?;
        audit::record(
            registry,
            &Entry {
                principal,
                action: Action::SettingsSet,
                scope: json!({"timezone": locale.timezone, "week_start": locale.week_start}),
                policy: None,
                job_id: None,
                details: None,
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    };
    registry.store().begin().map_err(|e| (500, e.to_string()))?;
    if let Err(why) = write(registry) {
        let _ = registry.store().rollback();
        return Err((500, why));
    }
    registry
        .store()
        .commit()
        .map_err(|e| (500, e.to_string()))?;
    registry.refresh_meta().map_err(|e| (500, e.to_string()))?;
    Ok(true)
}

/// The schedule beside the queue that runs it: every half minute, whether a
/// backup is owed, and one queued when it is.
pub(crate) fn run(home: &Home, dir: &Path, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        if let Err(why) = tick(home, dir, Timestamp::now()) {
            let _ = writeln!(std::io::stderr(), "nils serve: the backup schedule: {why}");
        }
        for _ in 0..30 {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

/// One look at the schedule: the job it queued, when a backup was owed. A
/// backup still queued or running is let finish first.
pub(crate) fn tick(home: &Home, dir: &Path, now: Timestamp) -> Result<Option<i64>, String> {
    let mut registry = home.open().map_err(|e| e.to_string())?;
    let schedule = Schedule::of(&mut registry);
    let tz = zone(&registry.meta().timezone);
    let Some(came) = schedule.owed(&tz, last_queued(&mut registry), now) else {
        return Ok(None);
    };
    let jobs = nils_registry::job::list(registry.store(), false, 500).map_err(|e| e.to_string())?;
    if jobs.iter().any(|j| j.kind == "backup") {
        return Ok(None);
    }
    let mut argv = vec![
        "backup".to_string(),
        "--dir".to_string(),
        dir.display().to_string(),
        "--rehearse".to_string(),
    ];
    if let Some(keep) = schedule.keep {
        argv.extend(["--keep".to_string(), keep.to_string()]);
    }
    registry.store().begin().map_err(|e| e.to_string())?;
    let queued = nils_registry::job::enqueue(
        registry.store(),
        &argv,
        Some("scheduled backup"),
        Some(PRINCIPAL),
    )
    .map_err(|e| e.to_string())
    .and_then(|id| {
        registry
            .set_meta("backup_last", &iso(came))
            .map(|()| id)
            .map_err(|e| e.to_string())
    });
    match queued {
        Ok(id) => {
            registry.store().commit().map_err(|e| e.to_string())?;
            Ok(Some(id))
        }
        Err(why) => {
            let _ = registry.store().rollback();
            Err(why)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn a_day_comes_round_at_its_hour_where_the_registry_is() {
        let s = Schedule::parse("day", Some("02:00"), None, Some(14)).unwrap();
        let tz = TimeZone::get("Europe/Stockholm").unwrap();
        // in September Stockholm is two hours ahead: 02:00 there is midnight UTC
        assert_eq!(
            s.previous(&tz, at("2026-09-13T10:00:00Z")),
            Some(at("2026-09-13T00:00:00Z"))
        );
        assert_eq!(
            s.next(&tz, at("2026-09-13T10:00:00Z")),
            Some(at("2026-09-14T00:00:00Z"))
        );
        assert_eq!(
            s.previous(&tz, at("2026-09-12T23:59:00Z")),
            Some(at("2026-09-12T00:00:00Z"))
        );
        // the clocks go back on the last Sunday of October: 02:00 is 01:00 UTC after it
        assert_eq!(
            s.next(&tz, at("2026-10-26T12:00:00Z")),
            Some(at("2026-10-27T01:00:00Z"))
        );
    }

    #[test]
    fn a_week_comes_round_on_its_day() {
        let s = Schedule::parse("week", Some("03:30"), Some("sunday"), None).unwrap();
        let tz = TimeZone::UTC;
        // 2026-09-13 is a Sunday
        assert_eq!(
            s.previous(&tz, at("2026-09-16T00:00:00Z")),
            Some(at("2026-09-13T03:30:00Z"))
        );
        assert_eq!(
            s.next(&tz, at("2026-09-13T03:29:00Z")),
            Some(at("2026-09-13T03:30:00Z"))
        );
        assert_eq!(
            s.next(&tz, at("2026-09-13T03:30:00Z")),
            Some(at("2026-09-20T03:30:00Z"))
        );
        assert_eq!(
            s.previous(&tz, at("2026-09-13T03:29:00Z")),
            Some(at("2026-09-06T03:30:00Z"))
        );
    }

    #[test]
    fn a_backup_is_owed_once_each_time_the_schedule_comes_round() {
        let s = Schedule::parse("day", None, None, None).unwrap();
        let tz = TimeZone::UTC;
        let now = at("2026-09-13T09:00:00Z");
        assert_eq!(
            s.owed(&tz, Some(at("2026-09-12T02:00:00Z")), now),
            Some(at("2026-09-13T02:00:00Z"))
        );
        assert_eq!(s.owed(&tz, Some(at("2026-09-13T02:00:00Z")), now), None);
        // a host that was off for days owes one backup, not one a day
        assert_eq!(
            s.owed(&tz, Some(at("2026-09-01T02:00:00Z")), now),
            Some(at("2026-09-13T02:00:00Z"))
        );
        assert_eq!(Schedule::OFF.owed(&tz, None, now), None);
    }

    #[test]
    fn a_schedule_is_refused_in_one_sentence() {
        let refused = |every, at, day, keep| Schedule::parse(every, at, day, keep).unwrap_err();
        assert!(refused("hourly", None, None, None).contains("off, day or week"));
        assert!(refused("day", Some("25:00"), None, None).contains("HH:MM"));
        assert!(refused("day", Some("2:00"), None, None).contains("HH:MM"));
        assert!(refused("week", None, Some("someday"), None).contains("monday"));
        assert!(refused("day", None, None, Some(0)).contains("1 to 1000"));
        let words = Schedule::parse("week", Some("23:15"), Some("friday"), Some(3))
            .unwrap()
            .words();
        assert_eq!(
            words,
            json!({"every": "week", "at": "23:15", "day": "friday", "keep": 3})
        );
    }

    #[test]
    fn the_timezones_are_places_and_utc() {
        let zones = timezones();
        assert!(zones.iter().any(|z| z == "Europe/Stockholm"), "{zones:?}");
        assert!(zones.iter().any(|z| z == "UTC"));
        assert!(!zones.iter().any(|z| z.starts_with("Etc/")));
        assert!(known("Europe/Stockholm") && known("UTC") && !known("Mars/Olympus_Mons"));
    }

    #[test]
    fn the_schedule_queues_one_backup_each_time_it_comes_round() {
        let dir = nils_dicom::synth::TempDir::new("schedule");
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        let mut registry = home
            .init(&nils_registry::InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::pseudonym::Scheme::DEFAULT,
                key: "k".to_string(),
                display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
                session_scheme: None,
            })
            .unwrap();
        let backups = dir.path().join("backups");
        let schedule = Schedule::parse("day", Some("02:00"), None, Some(3)).unwrap();
        set(
            &mut registry,
            &schedule,
            "admin@lab",
            at("2026-09-13T09:00:00Z"),
        )
        .unwrap();
        drop(registry);
        // set at nine in the morning, nothing is owed before two the next
        assert_eq!(
            tick(&home, &backups, at("2026-09-13T23:00:00Z")).unwrap(),
            None
        );
        let queued = tick(&home, &backups, at("2026-09-14T02:00:05Z"))
            .unwrap()
            .unwrap();
        // the hour came round once, and its backup is queued once
        assert_eq!(
            tick(&home, &backups, at("2026-09-14T02:00:35Z")).unwrap(),
            None
        );
        let mut registry = home.open().unwrap();
        let job = nils_registry::job::show(registry.store(), queued)
            .unwrap()
            .unwrap();
        assert_eq!(
            job.argv().unwrap(),
            vec![
                "backup",
                "--dir",
                backups.to_str().unwrap(),
                "--rehearse",
                "--keep",
                "3"
            ]
        );
        assert_eq!(job.principal(), Some(PRINCIPAL));
        let doc = document(&mut registry, at("2026-09-14T02:00:35Z"));
        assert_eq!(doc["last"], "2026-09-14T02:00:00Z");
        assert_eq!(doc["next"], "2026-09-15T02:00:00Z");
        assert_eq!(doc["next_local"], "2026-09-15T02:00");
    }
}
