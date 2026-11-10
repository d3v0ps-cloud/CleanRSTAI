//! The fields of one scheduled task, for the Scheduled page's editor: schedule, target,
//! approval mode, folder, provider and catch-up setting.

use eframe::egui;
use uuid::Uuid;

use super::settings::FIELD_WIDTH;
use crate::approval::ApprovalMode;
use crate::schedule::{self, Kind, ScheduledTask, Target};

fn weak(ui: &mut egui::Ui, text: &str) {
    ui.scope(|ui| {
        ui.set_max_width(FIELD_WIDTH);
        ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
    });
}

/// The grid of a task's schedule, target, approval, folder, provider and catch-up settings.
pub fn task_fields(
    ui: &mut egui::Ui,
    task: &mut ScheduledTask,
    providers: &[(String, String)],
    chats: &[(Uuid, String)],
    plan_on: bool,
) {
    egui::Grid::new("task-fields")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Repeats");
            let current = Kind::of(task.schedule);
            egui::ComboBox::from_id_salt("kind")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for kind in Kind::ALL {
                        if ui.selectable_label(current == kind, kind.label()).clicked() {
                            task.schedule = schedule::with_kind(task.schedule, kind);
                        }
                    }
                });
            ui.end_row();

            if let schedule::Schedule::Interval { minutes } = &mut task.schedule {
                ui.label("Every");
                ui.add(
                    egui::DragValue::new(minutes)
                        .range(schedule::MIN_INTERVAL_MINS..=schedule::MAX_INTERVAL_MINS)
                        .suffix(" min"),
                );
                ui.end_row();
            } else {
                if let schedule::Schedule::Weekly { weekday, .. } = &mut task.schedule {
                    ui.label("On");
                    egui::ComboBox::from_id_salt("weekday")
                        .selected_text(weekday_name(*weekday))
                        .show_ui(ui, |ui| {
                            for day in 0..7u8 {
                                ui.selectable_value(weekday, day, weekday_name(day));
                            }
                        });
                    ui.end_row();
                }
                if let Some((hour, minute)) = time_of(&mut task.schedule) {
                    ui.label("At");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(hour)
                                .range(0..=23)
                                .custom_formatter(|n, _| format!("{:02}", n as i64)),
                        );
                        ui.label(":");
                        ui.add(
                            egui::DragValue::new(minute)
                                .range(0..=59)
                                .custom_formatter(|n, _| format!("{:02}", n as i64)),
                        );
                    });
                    ui.end_row();
                }
            }

            ui.label("Runs in");
            let target_label = match task.target {
                Target::NewChat => "A new chat each time".to_owned(),
                Target::Chat(id) => chats
                    .iter()
                    .find(|(c, _)| *c == id)
                    .map_or("A chat that no longer exists".into(), |(_, t)| t.clone()),
            };
            egui::ComboBox::from_id_salt("target")
                .width(FIELD_WIDTH - 140.0)
                .selected_text(target_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut task.target, Target::NewChat, "A new chat each time");
                    for (id, title) in chats {
                        ui.selectable_value(&mut task.target, Target::Chat(*id), title);
                    }
                });
            ui.end_row();
            ui.label("");
            weak(
                ui,
                "Continuing a chat sets its approval mode to this task's, and uses the chat's own \
                 folder and provider settings. A new chat takes the folder below.",
            );
            ui.end_row();

            ui.label("Approvals");
            egui::ComboBox::from_id_salt("approval")
                .selected_text(task.approval_mode.label())
                .show_ui(ui, |ui| {
                    for mode in ApprovalMode::offered(plan_on) {
                        ui.selectable_value(&mut task.approval_mode, mode, mode.label());
                    }
                });
            ui.end_row();
            ui.label("");
            weak(ui, task.approval_mode.description());
            ui.end_row();

            ui.label("Folder");
            ui.horizontal(|ui| {
                let shown = task
                    .workspace
                    .as_ref()
                    .map_or("None".to_owned(), |p| p.display().to_string());
                ui.add(egui::Label::new(shown).truncate());
                if ui.small_button("Choose…").clicked()
                    && let Some(dir) = rfd::FileDialog::new()
                        .set_title("Choose a working folder")
                        .pick_folder()
                {
                    task.workspace = Some(dir);
                }
                if task.workspace.is_some() && ui.small_button("Clear").clicked() {
                    task.workspace = None;
                }
            });
            ui.end_row();

            ui.label("Provider");
            let provider_label = match &task.provider {
                None => "The active provider".to_owned(),
                Some(id) => providers
                    .iter()
                    .find(|(p, _)| p == id)
                    .map_or("A provider that no longer exists".into(), |(_, n)| {
                        n.clone()
                    }),
            };
            egui::ComboBox::from_id_salt("provider")
                .selected_text(provider_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut task.provider, None, "The active provider");
                    for (id, name) in providers {
                        ui.selectable_value(&mut task.provider, Some(id.clone()), name);
                    }
                });
            ui.end_row();

            ui.label("");
            ui.checkbox(
                &mut task.catch_up,
                "Catch up after the app was closed (run once, rather than skip)",
            );
            ui.end_row();
        });
}

/// The hour and minute of a daily or weekly schedule; `None` for an interval.
fn time_of(schedule: &mut schedule::Schedule) -> Option<(&mut u8, &mut u8)> {
    match schedule {
        schedule::Schedule::Daily { hour, minute }
        | schedule::Schedule::Weekly { hour, minute, .. } => Some((hour, minute)),
        schedule::Schedule::Interval { .. } => None,
    }
}

fn weekday_name(day: u8) -> &'static str {
    [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ]
    .get(usize::from(day))
    .copied()
    .unwrap_or("Monday")
}
