//! Scheduled tasks: saved prompts that run in a chat at set times while the app is open
//! (Settings → Background). The timing is pure and tested here; `ChatApp` starts the runs.
//!
//! Times are local wall-clock times. An interval counts from the task's last run, or before
//! its first run from when it was saved, so the first run is one interval after saving. A
//! daily or weekly task runs at its time on its days. A run that is due but missed (the app
//! was closed, or the machine slept) runs once if `catch_up` is on, and otherwise is skipped
//! to the next time.

use std::path::PathBuf;

use chrono::{
    DateTime, Datelike, Days, Local, NaiveDate, NaiveTime, TimeDelta, TimeZone, Utc, Weekday,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::approval::ApprovalMode;

/// Shortest interval between runs, in minutes.
pub const MIN_INTERVAL_MINS: u32 = 5;
/// Longest interval between runs, in minutes (a week).
pub const MAX_INTERVAL_MINS: u32 = 10_080;
/// How often the app looks for due tasks, at most. Idle windows are woken for this.
pub const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(20);
/// A run this late or less still counts as on time (checks run every `CHECK_EVERY`).
const ON_TIME_SECS: i64 = 90;

fn default_minutes() -> u32 {
    60
}

fn default_hour() -> u8 {
    9
}

/// When a task runs, in local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Every `minutes`, counted from the last run (see the module docs).
    Interval {
        #[serde(default = "default_minutes")]
        minutes: u32,
    },
    /// Every day at `hour:minute`.
    Daily {
        #[serde(default = "default_hour")]
        hour: u8,
        #[serde(default)]
        minute: u8,
    },
    /// Every week on `weekday` (0 = Monday, 6 = Sunday) at `hour:minute`.
    Weekly {
        #[serde(default)]
        weekday: u8,
        #[serde(default = "default_hour")]
        hour: u8,
        #[serde(default)]
        minute: u8,
    },
}

impl Default for Schedule {
    fn default() -> Self {
        Self::Daily {
            hour: default_hour(),
            minute: 0,
        }
    }
}

/// The kinds of schedule, for the settings' choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Interval,
    Daily,
    Weekly,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Interval, Kind::Daily, Kind::Weekly];

    pub fn of(schedule: Schedule) -> Kind {
        match schedule {
            Schedule::Interval { .. } => Kind::Interval,
            Schedule::Daily { .. } => Kind::Daily,
            Schedule::Weekly { .. } => Kind::Weekly,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Interval => "Every so many minutes",
            Kind::Daily => "Every day",
            Kind::Weekly => "Every week",
        }
    }
}

/// `schedule` as another kind, keeping its time of day and weekday where the kinds have them.
pub fn with_kind(schedule: Schedule, kind: Kind) -> Schedule {
    let (hour, minute) = match schedule {
        Schedule::Interval { .. } => (default_hour(), 0),
        Schedule::Daily { hour, minute } | Schedule::Weekly { hour, minute, .. } => (hour, minute),
    };
    match (kind, schedule) {
        (Kind::Interval, Schedule::Interval { .. }) => schedule,
        (Kind::Interval, _) => Schedule::Interval {
            minutes: default_minutes(),
        },
        (Kind::Daily, _) => Schedule::Daily { hour, minute },
        (Kind::Weekly, Schedule::Weekly { .. }) => schedule,
        (Kind::Weekly, _) => Schedule::Weekly {
            weekday: 0,
            hour,
            minute,
        },
    }
}

/// Where a task's prompt goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// A new chat for each run, titled with the task's name and the time.
    #[default]
    NewChat,
    /// A chat the user picked. Its approval mode is set to the task's on each run.
    Chat(Uuid),
}

/// A saved prompt that runs on a schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub name: String,
    pub prompt: String,
    pub schedule: Schedule,
    pub target: Target,
    /// How the run's tool calls are approved. Read only by default.
    pub approval_mode: ApprovalMode,
    /// The folder a new chat works in (a chat target keeps its own).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// The provider's id; `None` for the active provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub enabled: bool,
    /// Run once after the app was closed past a due time, rather than skipping to the next.
    pub catch_up: bool,
    /// When the task last ran (or was skipped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<DateTime<Utc>>,
    /// When the task was saved; its first interval counts from here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<DateTime<Utc>>,
}

impl Default for ScheduledTask {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Scheduled task".into(),
            prompt: String::new(),
            schedule: Schedule::default(),
            target: Target::NewChat,
            approval_mode: ApprovalMode::ReadOnly,
            workspace: None,
            provider: None,
            enabled: true,
            catch_up: false,
            last_run: None,
            created: None,
        }
    }
}

impl ScheduledTask {
    /// Keeps the fields in range: a name, interval and times that can be scheduled.
    fn normalize(&mut self) {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() {
            self.name = "Scheduled task".into();
        }
        self.schedule = match self.schedule {
            Schedule::Interval { minutes } => Schedule::Interval {
                minutes: minutes.clamp(MIN_INTERVAL_MINS, MAX_INTERVAL_MINS),
            },
            Schedule::Daily { hour, minute } => Schedule::Daily {
                hour: hour.min(23),
                minute: minute.min(59),
            },
            Schedule::Weekly {
                weekday,
                hour,
                minute,
            } => Schedule::Weekly {
                weekday: weekday.min(6),
                hour: hour.min(23),
                minute: minute.min(59),
            },
        };
        if self
            .provider
            .as_deref()
            .is_some_and(|p| p.trim().is_empty())
        {
            self.provider = None;
        }
    }
}

/// Scheduled tasks (Settings → Background): a master switch and the tasks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleSettings {
    /// Run the tasks while the app is open.
    pub enabled: bool,
    pub tasks: Vec<ScheduledTask>,
}

impl Default for ScheduleSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            tasks: Vec::new(),
        }
    }
}

impl ScheduleSettings {
    pub fn normalize(&mut self) {
        for task in &mut self.tasks {
            task.normalize();
        }
    }
}

/// The time `hour:minute` on `date` in `tz`. A time the clocks skip when they go forward falls
/// forward by the gap, so 02:30 on such a day becomes 03:30.
pub fn local_at<Tz: TimeZone>(tz: &Tz, date: NaiveDate, hour: u8, minute: u8) -> DateTime<Tz> {
    let time = NaiveTime::from_hms_opt(u32::from(hour.min(23)), u32::from(minute.min(59)), 0)
        .unwrap_or(NaiveTime::MIN);
    let mut naive = date.and_time(time);
    for _ in 0..3 {
        if let Some(at) = tz.from_local_datetime(&naive).earliest() {
            return at;
        }
        naive += TimeDelta::hours(1);
    }
    tz.from_utc_datetime(&naive)
}

/// The first occurrence of `schedule` strictly after `after`, in the same timezone.
pub fn next_occurrence<Tz: TimeZone>(schedule: Schedule, after: &DateTime<Tz>) -> DateTime<Tz> {
    let tz = after.timezone();
    let today = after.date_naive();
    match schedule {
        Schedule::Interval { minutes } => {
            let minutes = minutes.clamp(MIN_INTERVAL_MINS, MAX_INTERVAL_MINS);
            after.clone() + TimeDelta::minutes(i64::from(minutes))
        }
        Schedule::Daily { hour, minute } => {
            for days in 0..=2 {
                let Some(date) = today.checked_add_days(Days::new(days)) else {
                    break;
                };
                let at = local_at(&tz, date, hour, minute);
                if at > *after {
                    return at;
                }
            }
            after.clone() + TimeDelta::days(1)
        }
        Schedule::Weekly {
            weekday,
            hour,
            minute,
        } => {
            let want = Weekday::try_from(weekday.min(6)).unwrap_or(Weekday::Mon);
            for days in 0..=7 {
                let Some(date) = today.checked_add_days(Days::new(days)) else {
                    break;
                };
                if date.weekday() != want {
                    continue;
                }
                let at = local_at(&tz, date, hour, minute);
                if at > *after {
                    return at;
                }
            }
            after.clone() + TimeDelta::days(7)
        }
    }
}

/// When a task is next due: one occurrence after its last run, or after `created` before its
/// first.
pub fn next_run(
    schedule: Schedule,
    last_run: Option<DateTime<Utc>>,
    created: DateTime<Utc>,
) -> DateTime<Utc> {
    let anchor = last_run.unwrap_or(created);
    next_occurrence(schedule, &anchor.with_timezone(&Local)).with_timezone(&Utc)
}

/// What a check does with a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// Start the task now. Its last run becomes the time it was due (now, after a catch-up).
    Run {
        last_run: DateTime<Utc>,
    },
    /// Due while the app was closed, without catch-up: skip it. The last run becomes now, so
    /// the next time is the next occurrence after now.
    Skip {
        last_run: DateTime<Utc>,
    },
}

/// What a check does with `task` at `now`. A task without a prompt, or without a save time
/// yet (a check stamps it), waits.
pub fn step(task: &ScheduledTask, now: DateTime<Utc>) -> Step {
    if !task.enabled || task.prompt.trim().is_empty() {
        return Step::Wait;
    }
    let Some(created) = task.created else {
        return Step::Wait;
    };
    let next = next_run(task.schedule, task.last_run, created);
    if next > now {
        return Step::Wait;
    }
    if now - next <= TimeDelta::seconds(ON_TIME_SECS) {
        Step::Run { last_run: next }
    } else if task.catch_up {
        Step::Run { last_run: now }
    } else {
        Step::Skip { last_run: now }
    }
}

/// What one check of the settings found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// Tasks to start now.
    pub fire: Vec<Uuid>,
    /// Tasks whose save time or run times changed, so the settings need saving.
    pub touched: Vec<Uuid>,
}

/// Looks over the tasks at `now`: stamps a save time on any task that lacks one, moves the
/// last run of the due and missed ones, and lists the tasks to start.
pub fn check(tasks: &mut [ScheduledTask], now: DateTime<Utc>) -> Tick {
    let mut tick = Tick::default();
    for task in tasks.iter_mut() {
        let mut changed = false;
        if task.created.is_none() {
            task.created = Some(now);
            changed = true;
        }
        match step(task, now) {
            Step::Wait => {}
            Step::Run { last_run } => {
                task.last_run = Some(last_run);
                tick.fire.push(task.id);
                changed = true;
            }
            Step::Skip { last_run } => {
                task.last_run = Some(last_run);
                changed = true;
            }
        }
        if changed {
            tick.touched.push(task.id);
        }
    }
    tick
}

/// When a task will next run, for the settings. `None` until the task has a save time.
pub fn upcoming(task: &ScheduledTask) -> Option<DateTime<Utc>> {
    task.created
        .map(|created| next_run(task.schedule, task.last_run, created))
}

/// The title of a new chat for a run: the task's name and when it started (local time).
pub fn chat_title(name: &str, started: DateTime<Local>) -> String {
    format!("{name} — {}", started.format("%Y-%m-%d %H:%M"))
}

/// The notice when a task's chat has been deleted, and the task is switched off.
pub fn chat_gone_message(name: &str) -> String {
    format!("Scheduled task \"{name}\" is off: its chat no longer exists (Settings → Background).")
}

/// The notice when a task's chat is still replying, so this run is skipped.
pub fn chat_busy_message(name: &str) -> String {
    format!("Scheduled task \"{name}\" skipped this time: its chat is still replying.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use chrono::offset::{MappedLocalTime, Offset};

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    /// Local wall-clock times for the tests (the test machine's zone; the dates avoid DST).
    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    /// Whether `task` should start at `now`.
    fn due(task: &ScheduledTask, now: DateTime<Utc>) -> bool {
        matches!(step(task, now), Step::Run { .. })
    }

    fn task(schedule: Schedule, created: DateTime<Utc>) -> ScheduledTask {
        ScheduledTask {
            prompt: "Summarise the news".into(),
            schedule,
            created: Some(created),
            ..Default::default()
        }
    }

    #[test]
    fn interval_counts_from_the_last_run_or_from_saving() {
        let created = local(2026, 1, 15, 10, 0);
        let s = Schedule::Interval { minutes: 30 };
        // Never run: one interval after saving.
        assert_eq!(next_run(s, None, created), local(2026, 1, 15, 10, 30));
        // After a run: one interval after it.
        let ran = local(2026, 1, 15, 10, 30);
        assert_eq!(next_run(s, Some(ran), created), local(2026, 1, 15, 11, 0));
    }

    #[test]
    fn daily_runs_at_its_time_on_the_same_or_next_day() {
        let s = Schedule::Daily {
            hour: 9,
            minute: 30,
        };
        // Saved before the time: today.
        assert_eq!(
            next_run(s, None, local(2026, 1, 15, 8, 0)),
            local(2026, 1, 15, 9, 30)
        );
        // Saved after the time: tomorrow.
        assert_eq!(
            next_run(s, None, local(2026, 1, 15, 10, 0)),
            local(2026, 1, 16, 9, 30)
        );
        // Ran at the time: tomorrow, not the same minute again.
        assert_eq!(
            next_run(s, Some(local(2026, 1, 15, 9, 30)), local(2026, 1, 1, 0, 0)),
            local(2026, 1, 16, 9, 30)
        );
    }

    #[test]
    fn weekly_runs_on_its_weekday() {
        // 2026-01-15 is a Thursday; weekday 0 is Monday, so the next Monday is 19 January.
        let monday = Schedule::Weekly {
            weekday: 0,
            hour: 8,
            minute: 0,
        };
        assert_eq!(
            next_run(monday, None, local(2026, 1, 15, 12, 0)),
            local(2026, 1, 19, 8, 0)
        );
        // Sunday (6) from a Thursday: 18 January.
        let sunday = Schedule::Weekly {
            weekday: 6,
            hour: 8,
            minute: 0,
        };
        assert_eq!(
            next_run(sunday, None, local(2026, 1, 15, 12, 0)),
            local(2026, 1, 18, 8, 0)
        );
        // On the weekday after the time: a week on.
        assert_eq!(
            next_run(
                monday,
                Some(local(2026, 1, 19, 8, 0)),
                local(2026, 1, 1, 0, 0)
            ),
            local(2026, 1, 26, 8, 0)
        );
    }

    #[test]
    fn a_due_run_starts_once_and_records_its_time() {
        let created = local(2026, 1, 15, 8, 0);
        let t = task(Schedule::Daily { hour: 9, minute: 0 }, created);
        let on_time = local(2026, 1, 15, 9, 0) + TimeDelta::seconds(20);
        assert!(due(&t, on_time));
        assert_eq!(
            step(&t, on_time),
            Step::Run {
                last_run: local(2026, 1, 15, 9, 0)
            }
        );
        // Not yet due.
        assert!(!due(&t, local(2026, 1, 15, 8, 59)));
        // Once run, not due again the same day.
        let mut ran = t.clone();
        ran.last_run = Some(local(2026, 1, 15, 9, 0));
        assert!(!due(&ran, on_time + TimeDelta::minutes(5)));
        assert!(due(&ran, local(2026, 1, 16, 9, 0)));
    }

    #[test]
    fn a_missed_run_is_skipped_unless_catch_up_is_on() {
        let created = local(2026, 1, 10, 8, 0);
        let mut t = task(Schedule::Daily { hour: 9, minute: 0 }, created);
        // The app was closed past 15 January 09:00 and starts at 14:00.
        let startup = local(2026, 1, 15, 14, 0);
        assert!(!due(&t, startup));
        assert_eq!(step(&t, startup), Step::Skip { last_run: startup });
        // With catch-up it runs once, now.
        t.catch_up = true;
        assert_eq!(step(&t, startup), Step::Run { last_run: startup });
    }

    #[test]
    fn disabled_or_empty_tasks_never_run() {
        let created = local(2026, 1, 15, 8, 0);
        let now = local(2026, 1, 15, 9, 0);
        let mut t = task(Schedule::Daily { hour: 9, minute: 0 }, created);
        t.enabled = false;
        assert!(!due(&t, now));
        t.enabled = true;
        t.prompt = "  ".into();
        assert!(!due(&t, now));
        t.prompt = "go".into();
        t.created = None;
        assert!(!due(&t, now));
    }

    #[test]
    fn check_stamps_saves_fires_and_skips() {
        let now = utc(2026, 1, 15, 12, 0);
        let unstamped = ScheduledTask {
            prompt: "x".into(),
            schedule: Schedule::Interval { minutes: 5 },
            ..Default::default()
        };
        let due_now = task(
            Schedule::Interval { minutes: 60 },
            now - TimeDelta::minutes(61),
        );
        let missed = task(
            Schedule::Interval { minutes: 60 },
            now - TimeDelta::hours(5),
        );
        let (a, b, c) = (unstamped.id, due_now.id, missed.id);
        let mut tasks = vec![unstamped, due_now, missed];
        let tick = check(&mut tasks, now);
        assert_eq!(tick.fire, vec![b]);
        assert_eq!(tick.touched, vec![a, b, c]);
        assert_eq!(
            tasks[0].created,
            Some(now),
            "a fresh task is stamped, not run"
        );
        assert_eq!(tasks[0].last_run, None);
        assert_eq!(tasks[1].last_run, Some(now - TimeDelta::minutes(1)));
        assert_eq!(
            tasks[2].last_run,
            Some(now),
            "a missed run is skipped to now"
        );
        // A second check right away changes nothing.
        assert_eq!(check(&mut tasks, now), Tick::default());
    }

    #[test]
    fn changing_the_kind_keeps_the_time_of_day() {
        let daily = Schedule::Daily {
            hour: 7,
            minute: 30,
        };
        assert_eq!(
            with_kind(daily, Kind::Weekly),
            Schedule::Weekly {
                weekday: 0,
                hour: 7,
                minute: 30
            }
        );
        let weekly = Schedule::Weekly {
            weekday: 4,
            hour: 18,
            minute: 5,
        };
        assert_eq!(
            with_kind(weekly, Kind::Daily),
            Schedule::Daily {
                hour: 18,
                minute: 5
            }
        );
        // Kept as it is when the kind does not change.
        assert_eq!(with_kind(weekly, Kind::Weekly), weekly);
        assert_eq!(
            with_kind(daily, Kind::Interval),
            Schedule::Interval { minutes: 60 }
        );
        assert_eq!(Kind::of(weekly), Kind::Weekly);
    }

    #[test]
    fn upcoming_needs_a_save_time() {
        let mut t = task(Schedule::Interval { minutes: 15 }, local(2026, 1, 15, 8, 0));
        assert_eq!(upcoming(&t), Some(local(2026, 1, 15, 8, 15)));
        t.created = None;
        assert_eq!(upcoming(&t), None);
    }

    /// A zone whose clocks go forward an hour on 2026-03-08 at 02:00 (local times 02:00 to
    /// 02:59 do not exist) and were on UTC before that.
    #[derive(Clone, Debug)]
    struct GapZone;

    #[derive(Clone, Copy, Debug)]
    struct GapOffset(FixedOffset);

    impl Offset for GapOffset {
        fn fix(&self) -> FixedOffset {
            self.0
        }
    }

    fn gap_offset(secs: i32) -> GapOffset {
        GapOffset(FixedOffset::east_opt(secs).unwrap())
    }

    impl TimeZone for GapZone {
        type Offset = GapOffset;

        fn from_offset(_: &GapOffset) -> Self {
            GapZone
        }

        fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<GapOffset> {
            MappedLocalTime::Single(gap_offset(if *local < gap_day() { 0 } else { 3600 }))
        }

        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> MappedLocalTime<GapOffset> {
            let gap_start = gap_day().and_hms_opt(2, 0, 0).unwrap();
            let gap_end = gap_day().and_hms_opt(3, 0, 0).unwrap();
            if *local < gap_start {
                MappedLocalTime::Single(gap_offset(0))
            } else if *local < gap_end {
                MappedLocalTime::None
            } else {
                MappedLocalTime::Single(gap_offset(3600))
            }
        }

        fn offset_from_utc_date(&self, utc: &NaiveDate) -> GapOffset {
            gap_offset(if *utc < gap_day() { 0 } else { 3600 })
        }

        fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> GapOffset {
            let switch = gap_day().and_hms_opt(2, 0, 0).unwrap();
            gap_offset(if *utc < switch { 0 } else { 3600 })
        }
    }

    fn gap_day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 8).unwrap()
    }

    #[test]
    fn a_time_the_clocks_skip_falls_forward() {
        let day = gap_day();
        let at = local_at(&GapZone, day, 2, 30);
        assert_eq!(at.naive_local(), day.and_hms_opt(3, 30, 0).unwrap());
        // A time that exists is kept.
        let normal = local_at(&GapZone, day, 1, 30);
        assert_eq!(normal.naive_local(), day.and_hms_opt(1, 30, 0).unwrap());
        // The next daily 02:30 from the evening before is the gap's 03:30.
        let evening = GapZone.with_ymd_and_hms(2026, 3, 7, 12, 0, 0).unwrap();
        let next = next_occurrence(
            Schedule::Daily {
                hour: 2,
                minute: 30,
            },
            &evening,
        );
        assert_eq!(next.naive_local(), day.and_hms_opt(3, 30, 0).unwrap());
    }

    #[test]
    fn a_fixed_zone_gives_exact_times() {
        let zone = FixedOffset::east_opt(2 * 3600).unwrap();
        let after = zone.with_ymd_and_hms(2026, 5, 1, 10, 0, 0).unwrap();
        let next = next_occurrence(Schedule::Interval { minutes: 90 }, &after);
        assert_eq!(next, zone.with_ymd_and_hms(2026, 5, 1, 11, 30, 0).unwrap());
    }

    #[test]
    fn titles_and_notices_name_the_task() {
        let started = Local.with_ymd_and_hms(2026, 2, 3, 7, 5, 0).unwrap();
        assert_eq!(
            chat_title("Morning news", started),
            "Morning news — 2026-02-03 07:05"
        );
        assert!(chat_gone_message("Morning news").contains("\"Morning news\""));
        assert!(chat_busy_message("Morning news").contains("still replying"));
    }

    #[test]
    fn settings_normalise_and_missing_fields_load_defaults() {
        // Older settings without schedules load with the defaults.
        let general: crate::config::GeneralSettings =
            serde_json::from_str(r#"{"system_prompt":"hi"}"#).unwrap();
        assert!(general.schedules.enabled);
        assert!(general.schedules.tasks.is_empty());

        // A task with only a name gets every other default.
        let t: ScheduledTask = serde_json::from_str(r#"{"name":"News"}"#).unwrap();
        assert_eq!(t.name, "News");
        assert_eq!(t.schedule, Schedule::default());
        assert_eq!(t.target, Target::NewChat);
        assert_eq!(t.approval_mode, ApprovalMode::ReadOnly);
        assert!(t.enabled && !t.catch_up);
        assert!(t.created.is_none() && t.last_run.is_none() && t.provider.is_none());

        // Schedules with partial fields fill in the rest.
        let s: Schedule = serde_json::from_str(r#"{"kind":"weekly","weekday":2}"#).unwrap();
        assert_eq!(
            s,
            Schedule::Weekly {
                weekday: 2,
                hour: 9,
                minute: 0
            }
        );

        let mut settings = ScheduleSettings {
            enabled: true,
            tasks: vec![ScheduledTask {
                name: "  ".into(),
                schedule: Schedule::Interval { minutes: 1 },
                provider: Some(" ".into()),
                ..Default::default()
            }],
        };
        settings.normalize();
        let t = &settings.tasks[0];
        assert_eq!(t.name, "Scheduled task");
        assert_eq!(
            t.schedule,
            Schedule::Interval {
                minutes: MIN_INTERVAL_MINS
            }
        );
        assert_eq!(t.provider, None);
    }

    #[test]
    fn settings_round_trip_with_targets_and_last_runs() {
        let chat = Uuid::new_v4();
        let mut t = ScheduledTask {
            name: "Standup".into(),
            prompt: "Summarise yesterday".into(),
            schedule: Schedule::Weekly {
                weekday: 0,
                hour: 8,
                minute: 45,
            },
            target: Target::Chat(chat),
            approval_mode: ApprovalMode::AutoWorkspace,
            workspace: Some(PathBuf::from("/tmp/notes")),
            provider: Some("local".into()),
            catch_up: true,
            created: Some(utc(2026, 1, 1, 0, 0)),
            ..Default::default()
        };
        t.last_run = Some(utc(2026, 1, 5, 8, 45));
        let settings = ScheduleSettings {
            enabled: false,
            tasks: vec![t.clone()],
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: ScheduleSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings);
        assert_eq!(back.tasks[0].target, Target::Chat(chat));
        // Unset optional fields are left out of the file.
        let plain = serde_json::to_string(&ScheduledTask::default()).unwrap();
        assert!(!plain.contains("last_run") && !plain.contains("workspace"));
    }
}
