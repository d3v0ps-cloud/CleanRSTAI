//! Projects: chats grouped with shared instructions, reference files and defaults for new
//! chats (the workspace folder, provider, model, tool switches and permission mode).
//!
//! A project is one JSON file in `<data dir>/projects`. Its chats link to it by id
//! (`Conversation.project`). The instructions and the text of the files are added to the
//! system prompt of every request in its chats (see `agent::run`). A project can be exported
//! with its chats to one file and imported again with new ids.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::approval::ApprovalMode;
use crate::attach::Attachment;
use crate::tools::ToolOverrides;
use crate::types::Conversation;

/// Smallest and largest total size of project files sent with each request, in KB.
pub const MIN_FILES_KB: u32 = 16;
pub const MAX_FILES_KB: u32 = 4096;

/// Settings → Context & files → Projects: the defaults for new projects, and the switch.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProjectSettings {
    /// Off hides projects from the sidebar and stops adding project context to requests.
    pub enabled: bool,
    /// Most text of project files sent with each request, in all, in KB.
    pub max_files_kb: u32,
    /// Instructions a new project starts with.
    pub default_instructions: String,
    /// The working folder a new project starts with.
    pub default_workspace: Option<PathBuf>,
    /// The provider profile (by id) a new project starts with; `None` for the active one.
    pub default_provider: Option<String>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_files_kb: 256,
            default_instructions: String::new(),
            default_workspace: None,
            default_provider: None,
        }
    }
}

impl ProjectSettings {
    /// Keeps the size in range.
    pub fn normalize(&mut self) {
        self.max_files_kb = self.max_files_kb.clamp(MIN_FILES_KB, MAX_FILES_KB);
    }
}

/// A project: shared instructions and reference files for its chats, and defaults that a
/// new chat in it takes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    /// Added to the system prompt of every request in the project's chats.
    pub instructions: String,
    /// Reference files; their text is read on each request. The paths are kept as they are,
    /// so on another computer a file may be missing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<PathBuf>,
    /// The working folder a new chat in the project gets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// The provider profile (by id) a new chat gets; `None` for the active one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The model a new chat gets, over its provider's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The tool switches a new chat gets.
    #[serde(skip_serializing_if = "ToolOverrides::is_empty")]
    pub tool_overrides: ToolOverrides,
    /// The permission mode a new chat gets; `None` for Settings' default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

impl Project {
    /// A project named `name`, starting from Settings' defaults.
    pub fn new(name: impl Into<String>, defaults: &ProjectSettings) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            instructions: defaults.default_instructions.clone(),
            workspace: defaults.default_workspace.clone(),
            provider: defaults.default_provider.clone(),
            created: now,
            updated: now,
            ..Default::default()
        }
    }

    fn dir() -> PathBuf {
        crate::config::data_dir().join("projects")
    }

    pub fn load_all() -> Vec<Project> {
        Self::load_all_in(&Self::dir())
    }

    /// The projects saved in `dir`, by name.
    pub fn load_all_in(dir: &Path) -> Vec<Project> {
        let mut out: Vec<Project> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                serde_json::from_str(&text)
                    .inspect_err(|err| log::warn!("skipping {}: {err}", e.path().display()))
                    .ok()
            })
            .collect();
        out.sort_by_key(|p: &Project| p.name.to_lowercase());
        out
    }

    pub fn save(&self) {
        if let Err(e) = self.save_in(&Self::dir()) {
            log::error!("failed to save project {}: {e:#}", self.id);
        }
    }

    pub fn save_in(&self, dir: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        crate::config::write_private(&dir.join(format!("{}.json", self.id)), &bytes)
    }

    /// Delete the project's file. Its chats stay; see [`unlink_chats`].
    pub fn delete(id: Uuid) {
        let _ = std::fs::remove_file(Self::dir().join(format!("{id}.json")));
    }
}

/// The project called `key` (ignoring case), or the one whose id is `key`.
pub fn find<'a>(projects: &'a [Project], key: &str) -> Option<&'a Project> {
    let key = key.trim();
    projects
        .iter()
        .find(|p| p.id.to_string() == key)
        .or_else(|| {
            projects
                .iter()
                .find(|p| p.name.to_lowercase() == key.to_lowercase())
        })
}

/// Move a chat into `project`, or out of its project when `None`. Only the link changes; the
/// chat keeps its own folder, provider and switches.
pub fn move_chat(conv: &mut Conversation, project: Option<&Project>) {
    conv.project = project.map(|p| p.id);
}

/// Forget the project in its chats after it was deleted: they stay, outside any project.
pub fn unlink_chats(conversations: &mut [Conversation], id: Uuid) {
    for c in conversations.iter_mut().filter(|c| c.project == Some(id)) {
        c.project = None;
    }
}

/// Give `conv` the project's folder, provider, model, tool switches and permission mode, and
/// link it to the project. `default_mode` is used when the project doesn't set one.
pub fn apply_to(project: &Project, conv: &mut Conversation, default_mode: ApprovalMode) {
    conv.project = Some(project.id);
    conv.workspace = project.workspace.clone();
    conv.provider = project.provider.clone();
    conv.model = project.model.clone().filter(|m| !m.trim().is_empty());
    conv.tool_overrides = project.tool_overrides.clone();
    conv.permissions.mode = project.approval_mode.unwrap_or(default_mode);
}

/// A new chat in `project`, starting with the project's settings (see [`apply_to`]).
pub fn new_chat(project: &Project, default_mode: ApprovalMode) -> Conversation {
    let mut conv = Conversation::with_permission_mode(default_mode);
    apply_to(project, &mut conv, default_mode);
    conv
}

/// A project's instructions and files, as read for one request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectContext {
    pub name: String,
    pub instructions: String,
    /// (file name, text), in the project's order.
    pub files: Vec<(String, String)>,
    /// Files that were left out or cut, and why, for the model to know.
    pub notes: Vec<String>,
}

impl ProjectContext {
    /// The section added to the system prompt.
    pub fn prompt(&self) -> String {
        let mut out = format!(
            "Project: {}\n\nThis chat belongs to the project \"{}\". Follow its instructions \
             unless the user asks otherwise.",
            self.name, self.name
        );
        if !self.instructions.trim().is_empty() {
            out.push_str(&format!(
                "\n\n<project-instructions>\n{}\n</project-instructions>",
                self.instructions.trim()
            ));
        }
        for (name, text) in &self.files {
            out.push_str(&format!(
                "\n\n<project-file name=\"{name}\">\n{}\n</project-file>",
                text.trim_end()
            ));
        }
        if !self.notes.is_empty() {
            out.push_str("\n\nNotes on the project's files:");
            for note in &self.notes {
                out.push_str(&format!("\n- {note}"));
            }
        }
        out
    }
}

/// Read a project's instructions and files for a request. Files are read as text (documents
/// are extracted, see `attach`); images are left out with a note. The text of all files
/// together is cut to `max_kb`. Blocking: call it off the UI thread.
pub fn load_context(project: &Project, max_kb: u32) -> ProjectContext {
    let mut notes = Vec::new();
    let mut texts = Vec::new();
    for path in &project.files {
        let name = file_label(path);
        match Attachment::from_path(path) {
            Ok(a) if a.is_image() => notes.push(format!(
                "{name} is an image and was not sent; project files are sent as text"
            )),
            Ok(a) => match a.text {
                Some(text) => texts.push((name, text)),
                None => notes.push(format!("{name} has no text to send")),
            },
            Err(e) => notes.push(format!("{name} could not be read: {e:#}")),
        }
    }
    let (files, cut) = fit(texts, max_kb as usize * 1024);
    for name in cut {
        notes.push(format!(
            "{name} was cut short to fit the project files size limit"
        ));
    }
    ProjectContext {
        name: project.name.clone(),
        instructions: project.instructions.clone(),
        files,
        notes,
    }
}

/// The context a request in `project` gets: its files read now, or `None` while Projects are
/// off in Settings. Blocking, like [`load_context`].
pub fn context_for(project: &Project, settings: &ProjectSettings) -> Option<ProjectContext> {
    settings
        .enabled
        .then(|| load_context(project, settings.max_files_kb))
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Cut texts so that all of them fit in `budget` bytes. Each gets an equal share of what is
/// left, starting with the shortest, so a short file keeps its whole text and the longer ones
/// share the rest. Returns the texts in their order, and the names of the ones that were cut.
pub fn fit(files: Vec<(String, String)>, budget: usize) -> (Vec<(String, String)>, Vec<String>) {
    let mut limit = vec![0usize; files.len()];
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by_key(|&i| files[i].1.len());
    let mut left = budget;
    for (k, &i) in order.iter().enumerate() {
        let share = left / (files.len() - k);
        limit[i] = share.min(files[i].1.len());
        left -= limit[i];
    }
    let mut cut = Vec::new();
    let out = files
        .into_iter()
        .enumerate()
        .map(|(i, (name, text))| {
            if text.len() <= limit[i] {
                (name, text)
            } else {
                cut.push(name.clone());
                (name, truncate_at(&text, limit[i]).to_owned())
            }
        })
        .collect();
    (out, cut)
}

/// The start of `text`, at most `n` bytes long, on a character boundary.
fn truncate_at(text: &str, n: usize) -> &str {
    let mut end = n.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

// ---------------------------------------------------------------- export and import

const BUNDLE_APP: &str = "CleanRSTAI";
const BUNDLE_FORMAT: &str = "project";
const BUNDLE_VERSION: u32 = 1;
/// The file name ending of an exported project.
pub const BUNDLE_EXTENSION: &str = "cleanrstai-project.json";

/// A project and its chats, as one export file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectBundle {
    pub app: String,
    pub format: String,
    pub format_version: u32,
    pub exported_at: DateTime<Utc>,
    pub project: Project,
    pub chats: Vec<Conversation>,
}

/// The export of `project` with the chats in `chats` that belong to it.
pub fn bundle(project: &Project, chats: &[Conversation]) -> ProjectBundle {
    ProjectBundle {
        app: BUNDLE_APP.into(),
        format: BUNDLE_FORMAT.into(),
        format_version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        project: project.clone(),
        chats: chats
            .iter()
            .filter(|c| c.project == Some(project.id))
            .cloned()
            .collect(),
    }
}

/// The project and chats of an export, with new ids so importing twice doesn't clash. The
/// chats are linked to the new project. File edits can no longer be undone from the import
/// (their checkpoints came from another computer), but their diffs are kept.
pub fn unbundle(bundle: ProjectBundle) -> (Project, Vec<Conversation>) {
    let mut project = bundle.project;
    project.id = Uuid::new_v4();
    let chats = bundle
        .chats
        .into_iter()
        .map(|mut c| {
            c.id = Uuid::new_v4();
            c.project = Some(project.id);
            for m in &mut c.messages {
                if let Some(edit) = &mut m.edit {
                    edit.checkpoint = false;
                    edit.before = None;
                }
            }
            c
        })
        .collect();
    (project, chats)
}

/// Read an export file, checking what it is.
pub fn parse_bundle(bytes: &[u8]) -> Result<ProjectBundle> {
    let bundle: ProjectBundle =
        serde_json::from_slice(bytes).context("not a valid project export")?;
    if bundle.app != BUNDLE_APP || bundle.format != BUNDLE_FORMAT {
        bail!("not a CleanRSTAI project export");
    }
    if bundle.format_version == 0 || bundle.format_version > BUNDLE_VERSION {
        bail!(
            "unsupported project export version {}; this app reads up to {BUNDLE_VERSION} \
             (was it made by a newer CleanRSTAI?)",
            bundle.format_version
        );
    }
    Ok(bundle)
}

/// Ask where to save an export of the project called `name`. `None` if cancelled.
pub fn pick_export_path(name: &str) -> Option<PathBuf> {
    let file = crate::export::safe_file_name(name, BUNDLE_EXTENSION);
    let path = rfd::FileDialog::new()
        .set_title("Export project")
        .set_file_name(file)
        .add_filter("CleanRSTAI project", &["json"])
        .save_file()?;
    Some(if path.to_string_lossy().ends_with(BUNDLE_EXTENSION) {
        path
    } else {
        PathBuf::from(format!("{}.{BUNDLE_EXTENSION}", path.display()))
    })
}

/// Ask for a project export to import. `None` if cancelled.
pub fn pick_import_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Import project")
        .add_filter("CleanRSTAI project", &["json"])
        .pick_file()
}

/// Write an export file, readable only by the user.
pub fn write_bundle(path: &Path, bundle: &ProjectBundle) -> Result<()> {
    crate::config::write_private(path, &serde_json::to_vec_pretty(bundle)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, FileEdit, Role};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cleanrstai-proj-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(name: &str) -> Project {
        let mut p = Project::new(name, &ProjectSettings::default());
        p.instructions = "Answer in French.".into();
        p.workspace = Some(PathBuf::from("/work/site"));
        p.provider = Some("prov-1".into());
        p.model = Some("big-model".into());
        p.approval_mode = Some(ApprovalMode::AutoWorkspace);
        p.tool_overrides
            .set("builtin:calculator".into(), false, true);
        p
    }

    #[test]
    fn a_project_saves_and_loads_and_an_older_file_gets_defaults() {
        let dir = temp_dir("roundtrip");
        let p = sample("Website");
        p.save_in(&dir).unwrap();
        assert_eq!(Project::load_all_in(&dir), vec![p.clone()]);

        // A file written before the optional fields existed loads with their defaults.
        let old = format!(
            r#"{{"id":"{}","name":"Old","instructions":"","created":"2026-01-01T00:00:00Z","updated":"2026-01-01T00:00:00Z"}}"#,
            Uuid::new_v4()
        );
        std::fs::write(dir.join("old.json"), old).unwrap();
        let all = Project::load_all_in(&dir);
        let old = all.iter().find(|p| p.name == "Old").unwrap();
        assert!(old.files.is_empty() && old.provider.is_none() && old.approval_mode.is_none());
        assert_eq!(old.tool_overrides, ToolOverrides::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_defaults_come_from_settings() {
        let defaults = ProjectSettings {
            default_instructions: "Be brief.".into(),
            default_workspace: Some(PathBuf::from("/notes")),
            default_provider: Some("p".into()),
            ..Default::default()
        };
        let p = Project::new("Notes", &defaults);
        assert_eq!(p.instructions, "Be brief.");
        assert_eq!(p.workspace, Some(PathBuf::from("/notes")));
        assert_eq!(p.provider.as_deref(), Some("p"));
        assert_eq!(p.model, None);
    }

    #[test]
    fn project_settings_default_and_clamp() {
        let s = ProjectSettings::default();
        assert!(s.enabled);
        assert_eq!(s.max_files_kb, 256);
        let mut s = ProjectSettings {
            max_files_kb: 1,
            ..Default::default()
        };
        s.normalize();
        assert_eq!(s.max_files_kb, MIN_FILES_KB);
        s.max_files_kb = 100_000;
        s.normalize();
        assert_eq!(s.max_files_kb, MAX_FILES_KB);
    }

    #[test]
    fn a_new_chat_inherits_the_project_settings() {
        let p = sample("Site");
        let c = new_chat(&p, ApprovalMode::ReadOnly);
        assert_eq!(c.project, Some(p.id));
        assert_eq!(c.workspace, p.workspace);
        assert_eq!(c.provider, p.provider);
        assert_eq!(c.model.as_deref(), Some("big-model"));
        assert_eq!(c.tool_overrides, p.tool_overrides);
        assert_eq!(c.permissions.mode, ApprovalMode::AutoWorkspace);

        // Without a mode on the project, Settings' default applies.
        let mut plain = sample("Plain");
        plain.approval_mode = None;
        assert_eq!(
            new_chat(&plain, ApprovalMode::ReadOnly).permissions.mode,
            ApprovalMode::ReadOnly
        );
    }

    #[test]
    fn projects_off_means_no_project_context() {
        let dir = temp_dir("context-for");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "Keep it short.").unwrap();
        let mut p = sample("Off");
        p.files = vec![file];
        let mut settings = ProjectSettings::default();
        let ctx = context_for(&p, &settings).expect("on by default");
        assert!(ctx.prompt().contains("Keep it short."));
        settings.enabled = false;
        assert_eq!(context_for(&p, &settings), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_is_found_by_name_or_id() {
        let list = vec![sample("Website"), sample("Notes")];
        assert_eq!(find(&list, " notes ").map(|p| p.id), Some(list[1].id));
        assert_eq!(
            find(&list, &list[0].id.to_string()).map(|p| p.id),
            Some(list[0].id)
        );
        assert!(find(&list, "Missing").is_none());
    }

    #[test]
    fn chats_move_in_and_out_of_a_project() {
        let p = sample("Site");
        let mut c = Conversation::new();
        move_chat(&mut c, Some(&p));
        assert_eq!(c.project, Some(p.id));
        move_chat(&mut c, None);
        assert_eq!(c.project, None);
        // Moving in keeps the chat's own folder and switches.
        c.workspace = Some(PathBuf::from("/mine"));
        move_chat(&mut c, Some(&p));
        assert_eq!(c.workspace, Some(PathBuf::from("/mine")));
    }

    #[test]
    fn deleting_a_project_keeps_its_chats_outside_it() {
        let p = sample("Site");
        let mut inside = new_chat(&p, ApprovalMode::Default);
        inside.messages.push(ChatMessage::new(Role::User, "hi"));
        let mut other = Conversation::new();
        other.project = Some(Uuid::new_v4());
        let mut all = vec![inside, other.clone()];
        unlink_chats(&mut all, p.id);
        assert_eq!(all[0].project, None);
        assert_eq!(all[0].messages.len(), 1);
        assert_eq!(all[1].project, other.project);
    }

    #[test]
    fn fit_keeps_short_texts_whole_and_shares_the_rest() {
        let files = vec![
            ("big".to_owned(), "x".repeat(300)),
            ("small".to_owned(), "y".repeat(10)),
            ("mid".to_owned(), "z".repeat(100)),
        ];
        let (out, cut) = fit(files, 200);
        // The short file keeps its text; the 190 bytes left split between the other two.
        assert_eq!(out[1].1.len(), 10);
        assert_eq!(out[0].1.len(), 95);
        assert_eq!(out[2].1.len(), 95);
        assert_eq!(cut, ["big", "mid"]);
        assert_eq!(out[0].0, "big");
        let total: usize = out.iter().map(|(_, t)| t.len()).sum();
        assert!(total <= 200);

        // Enough room: nothing is cut.
        let (out, cut) = fit(vec![("a".into(), "abc".into())], 10);
        assert_eq!(out[0].1, "abc");
        assert!(cut.is_empty());
    }

    #[test]
    fn fit_cuts_on_a_character_boundary() {
        let (out, cut) = fit(vec![("u".into(), "é".repeat(10))], 5);
        assert_eq!(out[0].1, "éé");
        assert_eq!(cut, ["u"]);
    }

    #[test]
    fn project_files_are_read_as_text_and_images_are_left_out() {
        let dir = temp_dir("files");
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "Use metric units.\n").unwrap();
        let image = dir.join("photo.png");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0u8; 16]);
        std::fs::write(&image, png).unwrap();
        let missing = dir.join("gone.md");

        let mut p = sample("Files");
        p.files = vec![notes, image, missing];
        let ctx = load_context(&p, 256);
        assert_eq!(
            ctx.files,
            [("notes.txt".to_owned(), "Use metric units.\n".to_owned())]
        );
        assert_eq!(ctx.notes.len(), 2, "{:?}", ctx.notes);
        assert!(ctx.notes[0].starts_with("photo.png is an image"));
        assert!(ctx.notes[1].starts_with("gone.md could not be read"));

        let prompt = ctx.prompt();
        assert!(prompt.starts_with("Project: Files"), "{prompt}");
        assert!(prompt.contains("Answer in French."), "{prompt}");
        assert!(
            prompt.contains("<project-file name=\"notes.txt\">"),
            "{prompt}"
        );
        assert!(prompt.contains("Use metric units."), "{prompt}");
        assert!(prompt.contains("- photo.png is an image"), "{prompt}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bundle_gets_new_ids_and_keeps_the_titles_and_messages() {
        let p = sample("Export me");
        let mut chat = new_chat(&p, ApprovalMode::Default);
        chat.title = "First chat".into();
        chat.messages.push(ChatMessage::new(Role::User, "hello"));
        let mut edit_call = ChatMessage::new(Role::Tool, "Wrote a.txt");
        edit_call.edit = Some(FileEdit {
            path: "a.txt".into(),
            abs: PathBuf::from("/work/site/a.txt"),
            diff: "+x\n".into(),
            created: false,
            before: Some("old".into()),
            checkpoint: true,
            after_sha256: "abc".into(),
            undone: false,
        });
        chat.messages.push(edit_call);
        let original_chat_id = chat.id;

        let doc = bundle(&p, &[chat.clone(), Conversation::new()]);
        assert_eq!(doc.chats.len(), 1, "only the project's chats are exported");
        let parsed = parse_bundle(&serde_json::to_vec(&doc).unwrap()).unwrap();
        let (project, chats) = unbundle(parsed.clone());
        let (project2, chats2) = unbundle(parsed);

        assert_ne!(project.id, p.id);
        assert_ne!(
            project.id, project2.id,
            "importing twice gives two projects"
        );
        assert_eq!(project.name, "Export me");
        assert_eq!(project.workspace, p.workspace);
        assert_eq!(chats.len(), 1);
        let imported = &chats[0];
        assert_ne!(imported.id, original_chat_id);
        assert_ne!(imported.id, chats2[0].id);
        assert_eq!(imported.project, Some(project.id));
        assert_eq!(imported.title, "First chat");
        assert_eq!(imported.messages[0].content, "hello");
        let edit = imported.messages[1].edit.as_ref().unwrap();
        assert!(!edit.checkpoint && edit.before.is_none());
        assert_eq!(edit.diff, "+x\n");
        // The original is untouched.
        assert!(chat.messages[1].edit.as_ref().unwrap().checkpoint);
    }

    #[test]
    fn a_bundle_must_be_a_project_export() {
        let doc = bundle(&sample("P"), &[]);
        let mut value = serde_json::to_value(&doc).unwrap();
        value["app"] = serde_json::json!("Other");
        assert!(parse_bundle(value.to_string().as_bytes()).is_err());

        let mut value = serde_json::to_value(&doc).unwrap();
        value["format"] = serde_json::json!("conversations");
        assert!(parse_bundle(value.to_string().as_bytes()).is_err());

        let mut value = serde_json::to_value(&doc).unwrap();
        value["format_version"] = serde_json::json!(99);
        let err = parse_bundle(value.to_string().as_bytes()).unwrap_err();
        assert!(err.to_string().contains("unsupported"), "{err}");

        assert!(parse_bundle(b"not json").is_err());
    }

    #[test]
    fn a_bundle_round_trips_through_a_file() {
        let dir = temp_dir("bundle");
        let p = sample("File");
        let path = dir.join("file.cleanrstai-project.json");
        write_bundle(&path, &bundle(&p, &[])).unwrap();
        let back = parse_bundle(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.project, p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
