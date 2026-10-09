//! Changes at set times (`[[schedule]]`): what a display shows, its brightness and whether its
//! screen is on.
//!
//! An entry sets what it names from its time until another entry sets it again. The state at a
//! moment is, for each of show, brightness and power, the latest occurrence of an entry that sets
//! it, looking back up to a week on the days each entry applies. A `show` also switches the
//! screen on, unless its entry sets `power`. The daemon applies the state to a display when it
//! connects, and each entry at its time; changes by hand last until the next entry. While
//! paused, nothing is applied; resuming applies the state at once.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{ToSpan, Zoned};

use crate::config::{Config, Power, ScheduleEntry, StartupShow};
use crate::manager::Manager;
use crate::metrics::Metrics;
use crate::sources::Content;

/// The scheduler looks at the clock at least this often, so that it notices a computer that
/// slept or a clock that was changed.
const MAX_WAIT: Duration = Duration::from_secs(60);

/// The entries of the config, ready to apply.
pub struct Schedule {
    entries: Vec<Entry>,
    control: Mutex<Control>,
    changed: Condvar,
}

struct Entry {
    /// Position in the config, from 1.
    number: usize,
    hour: i8,
    minute: i8,
    /// Monday first.
    days: [bool; 7],
    display: Option<String>,
    show: Option<Content>,
    brightness: Option<u8>,
    /// On or off; `show` means on unless `power` is given.
    power: Option<bool>,
    /// What it does, for status and logs, e.g. `"show clock, brightness 40"`.
    does: String,
}

#[derive(Default)]
struct Control {
    paused: bool,
    stopped: bool,
}

/// What the schedule says for one display: each part is `None` when no entry sets it.
#[derive(Clone, Default)]
pub struct State {
    /// What to show.
    pub show: Option<Content>,
    /// Backlight in percent.
    pub brightness: Option<u8>,
    /// Screen on (`true`) or off.
    pub power: Option<bool>,
}

/// When entries happen and what they do, for status.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// When.
    pub at: Zoned,
    /// Positions of the entries in the config, from 1.
    pub entries: Vec<usize>,
    /// What each does.
    pub does: Vec<String>,
}

/// The schedule as `GET /schedule` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// Number of entries.
    pub entries: usize,
    /// Whether applying is paused.
    pub paused: bool,
    /// The latest entries that happened.
    pub last: Option<Event>,
    /// The next entries to happen.
    pub next: Option<Event>,
}

impl Entry {
    fn new(
        number: usize,
        config: &ScheduleEntry,
        full: &Config,
        metrics: &Metrics,
    ) -> Result<Self, String> {
        let (hour, minute) = config.time()?;
        let show = config
            .spec()
            .map(|spec| Content::from_spec(spec, full, metrics))
            .transpose()?;
        let mut does = Vec::new();
        if let Some(spec) = config.spec() {
            let what = match spec.show {
                StartupShow::Web => format!("show web {}", spec.url.unwrap_or_default()),
                StartupShow::Image => {
                    let path = spec.image.map(|p| p.display().to_string());
                    format!("show image {}", path.unwrap_or_default())
                }
                other => format!("show {}", show_name(other)),
            };
            does.push(what);
        }
        if let Some(b) = config.brightness {
            does.push(format!("brightness {b}"));
        }
        if let Some(power) = config.power {
            does.push(format!(
                "power {}",
                if power == Power::On { "on" } else { "off" }
            ));
        }
        let mut does = does.join(", ");
        if let Some(display) = &config.display {
            does = format!("{display}: {does}");
        }
        Ok(Self {
            number,
            hour,
            minute,
            days: config.weekdays()?,
            display: config.display.clone(),
            show,
            brightness: config.brightness,
            power: config
                .power
                .map(|p| p == Power::On)
                .or(config.show.map(|_| true)),
            does,
        })
    }

    fn applies_to(&self, display: &str) -> bool {
        self.display.as_deref().is_none_or(|d| d == display)
    }

    /// When the entry happens on `date`, if it applies that day. A time skipped by a change to
    /// daylight saving time happens right after the change; a time that occurs twice, once.
    fn on(&self, date: Date, tz: &TimeZone) -> Option<Zoned> {
        let weekday = date.weekday().to_monday_zero_offset() as usize;
        if !self.days[weekday] {
            return None;
        }
        date.at(self.hour, self.minute, 0, 0)
            .to_zoned(tz.clone())
            .ok()
    }

    /// The latest time it happened, up to a week back.
    fn latest(&self, now: &Zoned) -> Option<Zoned> {
        (0..=7)
            .filter_map(|back| now.date().checked_sub(back.days()).ok())
            .filter_map(|date| self.on(date, now.time_zone()))
            .find(|at| at.timestamp() <= now.timestamp())
    }

    /// The next time it happens after `now`.
    fn next(&self, now: &Zoned) -> Option<Zoned> {
        (0..=8)
            .filter_map(|ahead| now.date().checked_add(ahead.days()).ok())
            .filter_map(|date| self.on(date, now.time_zone()))
            .find(|at| at.timestamp() > now.timestamp())
    }

    /// The times it happens after `from` up to and including `to` (at most a week back).
    fn between(&self, from: &Zoned, to: &Zoned) -> Vec<Zoned> {
        (0..=8)
            .rev()
            .filter_map(|back| to.date().checked_sub(back.days()).ok())
            .filter_map(|date| self.on(date, to.time_zone()))
            .filter(|at| at.timestamp() > from.timestamp() && at.timestamp() <= to.timestamp())
            .collect()
    }
}

fn show_name(show: StartupShow) -> &'static str {
    match show {
        StartupShow::Nothing => "nothing",
        StartupShow::Clock => "clock",
        StartupShow::Dashboard => "dashboard",
        StartupShow::Image => "image",
        StartupShow::Web => "web",
        StartupShow::Rotation => "rotation",
    }
}

impl Schedule {
    /// The `[[schedule]]` entries of `config`, with their contents loaded; `None` without
    /// entries.
    pub fn new(config: &Config, metrics: &Metrics) -> Result<Option<Arc<Self>>, String> {
        if config.schedule.is_empty() {
            return Ok(None);
        }
        let entries = config
            .schedule
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                Entry::new(i + 1, entry, config, metrics)
                    .map_err(|e| format!("schedule entry {} ({}): {e}", i + 1, entry.at))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Arc::new(Self {
            entries,
            control: Mutex::default(),
            changed: Condvar::new(),
        })))
    }

    fn lock(&self) -> MutexGuard<'_, Control> {
        self.control.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether applying is paused.
    pub fn is_paused(&self) -> bool {
        self.lock().paused
    }

    /// What the schedule says for display `id` at `now`. Nothing while paused.
    pub fn state(&self, id: &str, now: &Zoned) -> State {
        if self.is_paused() {
            return State::default();
        }
        let mut latest: Vec<(jiff::Timestamp, &Entry)> = self
            .entries
            .iter()
            .filter(|e| e.applies_to(id))
            .filter_map(|e| Some((e.latest(now)?.timestamp(), e)))
            .collect();
        // Oldest first, so the latest entry setting a part wins; at the same time, the later
        // one in the config.
        latest.sort_by_key(|(at, e)| (*at, e.number));
        Self::combine(latest.iter().map(|(_, e)| *e))
    }

    /// The parts set by `entries`, the last one setting each part winning.
    fn combine<'a>(entries: impl DoubleEndedIterator<Item = &'a Entry> + Clone) -> State {
        State {
            show: entries.clone().rev().find_map(|e| e.show.clone()),
            brightness: entries.clone().rev().find_map(|e| e.brightness),
            power: entries.rev().find_map(|e| e.power),
        }
    }

    /// The latest and next entries, as of `now`.
    pub fn status(&self, now: &Zoned) -> Status {
        let event = |times: Vec<(Zoned, &Entry)>, pick_latest: bool| {
            let at = if pick_latest {
                times
                    .iter()
                    .map(|(at, _)| at.clone())
                    .max_by_key(Zoned::timestamp)
            } else {
                times
                    .iter()
                    .map(|(at, _)| at.clone())
                    .min_by_key(Zoned::timestamp)
            }?;
            let mut here: Vec<&Entry> = times
                .iter()
                .filter(|(t, _)| t.timestamp() == at.timestamp())
                .map(|(_, e)| *e)
                .collect();
            here.sort_by_key(|e| e.number);
            Some(Event {
                at,
                entries: here.iter().map(|e| e.number).collect(),
                does: here.iter().map(|e| e.does.clone()).collect(),
            })
        };
        let latest = self
            .entries
            .iter()
            .filter_map(|e| Some((e.latest(now)?, e)))
            .collect();
        let next = self
            .entries
            .iter()
            .filter_map(|e| Some((e.next(now)?, e)))
            .collect();
        Status {
            entries: self.entries.len(),
            paused: self.is_paused(),
            last: event(latest, true),
            next: event(next, false),
        }
    }

    /// Applies the entries happening after `from` up to `to` to every display.
    fn apply_between(&self, manager: &Manager, from: &Zoned, to: &Zoned) {
        let mut due: Vec<(jiff::Timestamp, &Entry)> = self
            .entries
            .iter()
            .flat_map(|e| {
                e.between(from, to)
                    .into_iter()
                    .map(move |at| (at.timestamp(), e))
            })
            .collect();
        if due.is_empty() {
            return;
        }
        due.sort_by_key(|(at, e)| (*at, e.number));
        for (_, entry) in &due {
            tracing::info!(entry = entry.number, "schedule: {}", entry.does);
        }
        for id in manager.ids() {
            let here = due.iter().map(|(_, e)| *e).filter(|e| e.applies_to(&id));
            manager.apply(&id, &Self::combine(here));
        }
    }

    /// Applies the state as of now to every display.
    fn apply_now(&self, manager: &Manager) {
        let now = Zoned::now();
        for id in manager.ids() {
            manager.apply(&id, &self.state(&id, &now));
        }
    }

    /// Stops applying entries until [`Schedule::resume`].
    pub fn pause(&self) {
        self.lock().paused = true;
        self.changed.notify_all();
    }

    /// Applies the state as of now to every display, and the entries at their times again.
    pub fn resume(&self, manager: &Manager) {
        self.lock().paused = false;
        self.changed.notify_all();
        self.apply_now(manager);
    }

    /// Applies the entries at their times on a thread of its own, until [`Scheduler::stop`].
    pub fn spawn(self: &Arc<Self>, manager: Arc<Manager>) -> Scheduler {
        let schedule = self.clone();
        let thread = std::thread::Builder::new()
            .name("ssp-schedule".into())
            .spawn(move || schedule.run(&manager))
            .expect("failed to spawn a thread");
        Scheduler {
            schedule: self.clone(),
            thread,
        }
    }

    fn run(&self, manager: &Manager) {
        let mut checked = Zoned::now();
        loop {
            let next = self
                .entries
                .iter()
                .filter_map(|e| e.next(&checked))
                .min_by_key(Zoned::timestamp);
            let wait = next.map_or(MAX_WAIT, |at| {
                let left = at.timestamp().duration_since(Zoned::now().timestamp());
                Duration::try_from(left).unwrap_or_default().min(MAX_WAIT)
            });
            {
                let control = self.lock();
                if control.stopped {
                    return;
                }
                let control = self
                    .changed
                    .wait_timeout(control, wait)
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
                if control.stopped {
                    return;
                }
                if control.paused {
                    checked = Zoned::now();
                    continue;
                }
            }
            let now = Zoned::now();
            self.apply_between(manager, &checked, &now);
            checked = now;
        }
    }
}

/// The thread of [`Schedule::spawn`].
pub struct Scheduler {
    schedule: Arc<Schedule>,
    thread: JoinHandle<()>,
}

impl Scheduler {
    /// Stops applying entries and waits for the thread to end.
    pub fn stop(self) {
        self.schedule.lock().stopped = true;
        self.schedule.changed.notify_all();
        let _ = self.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at: &str, days: &[&str]) -> ScheduleEntry {
        ScheduleEntry {
            at: at.into(),
            days: days.iter().map(|d| (*d).to_owned()).collect(),
            display: None,
            show: None,
            image: None,
            url: None,
            reload: None,
            fit: None,
            brightness: None,
            power: None,
        }
    }

    fn schedule(entries: Vec<ScheduleEntry>) -> Arc<Schedule> {
        let config = Config {
            schedule: entries,
            ..Config::default()
        };
        config.validate().unwrap();
        Schedule::new(&config, &Metrics::default())
            .unwrap()
            .unwrap()
    }

    /// A time in Tokyo (no daylight saving) on a known day: 2026-10-09 is a Friday.
    fn at(text: &str) -> Zoned {
        format!("{text}[Asia/Tokyo]").parse().unwrap()
    }

    fn kind(state: &State) -> Option<&'static str> {
        state.show.as_ref().map(Content::kind)
    }

    /// The weekday plan of the issue: dashboard at 9 on weekdays, clock at 19, off at 1, and the
    /// clock at 9 on weekends.
    fn week() -> Arc<Schedule> {
        schedule(vec![
            ScheduleEntry {
                show: Some(StartupShow::Dashboard),
                brightness: Some(100),
                ..entry("09:00", &["mon", "tue", "wed", "thu", "fri"])
            },
            ScheduleEntry {
                show: Some(StartupShow::Clock),
                brightness: Some(40),
                ..entry("19:00", &[])
            },
            ScheduleEntry {
                power: Some(Power::Off),
                ..entry("01:00", &[])
            },
            ScheduleEntry {
                show: Some(StartupShow::Clock),
                brightness: Some(60),
                ..entry("09:00", &["sat", "sun"])
            },
        ])
    }

    #[test]
    fn the_state_is_the_latest_entry_for_each_part() {
        let week = week();
        // Friday noon: the weekday entry.
        let state = week.state("d", &at("2026-10-09T12:00:00+09:00"));
        assert_eq!(
            (kind(&state), state.brightness, state.power),
            (Some("dashboard"), Some(100), Some(true))
        );
        // Friday evening: the clock, dimmer.
        let state = week.state("d", &at("2026-10-09T20:00:00+09:00"));
        assert_eq!(
            (kind(&state), state.brightness, state.power),
            (Some("clock"), Some(40), Some(true))
        );
        // Saturday 3 am: off, still showing the clock underneath.
        let state = week.state("d", &at("2026-10-10T03:00:00+09:00"));
        assert_eq!(
            (kind(&state), state.brightness, state.power),
            (Some("clock"), Some(40), Some(false))
        );
        // Saturday 10 am: the weekend entry switches it on again.
        let state = week.state("d", &at("2026-10-10T10:00:00+09:00"));
        assert_eq!(
            (kind(&state), state.brightness, state.power),
            (Some("clock"), Some(60), Some(true))
        );
        // Monday 8 am: off since 1 am, the weekday entry comes at 9.
        let state = week.state("d", &at("2026-10-12T08:00:00+09:00"));
        assert_eq!(state.power, Some(false));
    }

    #[test]
    fn entries_for_one_display_and_ties() {
        let both = schedule(vec![
            ScheduleEntry {
                show: Some(StartupShow::Clock),
                ..entry("08:00", &[])
            },
            ScheduleEntry {
                show: Some(StartupShow::Dashboard),
                display: Some("left".into()),
                ..entry("08:00", &[])
            },
        ]);
        let now = at("2026-10-09T09:00:00+09:00");
        assert_eq!(kind(&both.state("left", &now)), Some("dashboard"));
        assert_eq!(kind(&both.state("right", &now)), Some("clock"));
    }

    #[test]
    fn reports_the_last_and_next_entries() {
        let week = week();
        let status = week.status(&at("2026-10-09T20:00:00+09:00"));
        assert_eq!(status.entries, 4);
        let last = status.last.unwrap();
        assert_eq!(last.at, at("2026-10-09T19:00:00+09:00"));
        assert_eq!(last.entries, [2]);
        assert_eq!(last.does, ["show clock, brightness 40"]);
        let next = status.next.unwrap();
        assert_eq!(next.at, at("2026-10-10T01:00:00+09:00"));
        assert_eq!(next.does, ["power off"]);
    }

    #[test]
    fn finds_the_entries_between_two_times() {
        let week = week();
        let from = at("2026-10-09T18:59:00+09:00");
        let entries = |to: &str| -> Vec<usize> {
            let mut found: Vec<(jiff::Timestamp, usize)> = week
                .entries
                .iter()
                .flat_map(|e| {
                    e.between(&from, &at(to))
                        .into_iter()
                        .map(move |z| (z.timestamp(), e.number))
                })
                .collect();
            found.sort();
            found.into_iter().map(|(_, n)| n).collect()
        };
        assert_eq!(entries("2026-10-09T19:00:00+09:00"), [2]);
        // A computer that slept until Saturday 10 am missed three.
        assert_eq!(entries("2026-10-10T10:00:00+09:00"), [2, 3, 4]);
        assert!(entries("2026-10-09T18:59:30+09:00").is_empty());
    }

    #[test]
    fn handles_daylight_saving_time() {
        let late = schedule(vec![ScheduleEntry {
            power: Some(Power::Off),
            ..entry("02:30", &[])
        }]);
        let entry = &late.entries[0];
        // 2026-03-08 02:30 does not exist in New York: it happens right after the change.
        let tz = TimeZone::get("America/New_York").unwrap();
        let date = jiff::civil::date(2026, 3, 8);
        let at = entry.on(date, &tz).unwrap();
        assert_eq!(at.hour(), 3);
        // 2026-11-01 01:00-02:00 happens twice; 02:30 once.
        let date = jiff::civil::date(2026, 11, 1);
        assert!(entry.on(date, &tz).is_some());
    }

    #[test]
    fn pausing_stops_the_state() {
        let week = week();
        week.pause();
        assert!(week.is_paused());
        let state = week.state("d", &at("2026-10-09T12:00:00+09:00"));
        assert!(state.show.is_none() && state.brightness.is_none() && state.power.is_none());
        assert!(week.status(&at("2026-10-09T12:00:00+09:00")).paused);
    }
}
