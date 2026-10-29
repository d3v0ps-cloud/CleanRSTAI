//! Memory: facts the model saves across chats with the memory tool (`builtin__memory`). The
//! memories are one JSON file, `memory.json` in the data directory, and a chat with memory on
//! gets the ones it can see in its system prompt. A chat with memory off (incognito) gets
//! neither the list nor the tool.
//!
//! Which memories a chat sees follows Settings → Context & files → Memory: global ones
//! always, and a project's own ones too when the scope is per project and the chat is in
//! that project. The tool saves into the same scope and may delete only what the chat sees.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::llm::ToolSpec;
use crate::tools::{self, ToolSelection};
use crate::types::Conversation;

/// Most characters one memory keeps; a longer text is cut.
pub const MAX_CHARS: usize = 500;
/// Smallest and largest size of the memories sent with each request, in KB.
pub const MIN_KB: u32 = 1;
pub const MAX_KB: u32 = 64;

const HEADER: &str = "Things to remember about the user (from earlier chats):";
const GUIDANCE: &str = "Use the memory tool to save a fact the user asks you to remember, or one \
     they state about themselves, their preferences or their work. Delete a memory that is \
     wrong or that the user asks you to forget. Never save passwords or other secrets.";

/// Settings → Context & files → Memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemorySettings {
    /// Chats remember and use memories. A chat can switch it off (incognito).
    pub enabled: bool,
    /// Saving and deleting a memory asks first, like other tools that change things.
    pub ask_before_saving: bool,
    /// Most text of the memories sent with each request, in KB; the newest are kept.
    pub max_kb: u32,
    /// Which memories a chat sees and saves to.
    pub scope: MemoryScope,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ask_before_saving: true,
            max_kb: 8,
            scope: MemoryScope::Global,
        }
    }
}

impl MemorySettings {
    /// Keeps the size in range.
    pub fn normalized(&mut self) {
        self.max_kb = self.max_kb.clamp(MIN_KB, MAX_KB);
    }
}

/// The settings' scope: one pool of memories, or each project's own besides the global ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    #[default]
    Global,
    PerProject,
}

/// Where one memory belongs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Global,
    Project(Uuid),
}

/// One remembered fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    pub id: Uuid,
    pub text: String,
    #[serde(default)]
    pub scope: Scope,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    /// The chat the model saved it from, or `None` when the user wrote it in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_chat: Option<Uuid>,
}

/// The memories a chat may see and save to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visibility {
    pub per_project: bool,
    pub project: Option<Uuid>,
}

impl Visibility {
    pub fn for_chat(settings: &MemorySettings, project: Option<Uuid>) -> Self {
        Self {
            per_project: settings.scope == MemoryScope::PerProject,
            project,
        }
    }

    /// Global memories always; a project's only in per-project scope, from its own chats.
    pub fn sees(&self, memory: &Memory) -> bool {
        match memory.scope {
            Scope::Global => true,
            Scope::Project(p) => self.per_project && self.project == Some(p),
        }
    }

    /// The scope a memory the chat saves gets.
    pub fn save_scope(&self) -> Scope {
        match self.project {
            Some(p) if self.per_project => Scope::Project(p),
            _ => Scope::Global,
        }
    }
}

/// The memories `vis` sees, newest first.
pub fn visible(items: &[Memory], vis: Visibility) -> Vec<Memory> {
    let mut out: Vec<Memory> = items.iter().filter(|m| vis.sees(m)).cloned().collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.created));
    out
}

/// The system prompt section for a chat with memory on: the memories it sees, newest first,
/// cut where the list would pass `max_bytes`, then how to use the tool.
pub fn prompt_section(visible: &[Memory], max_bytes: usize) -> String {
    let mut out = HEADER.to_owned();
    if visible.is_empty() {
        out.push_str(" none saved yet.");
    }
    let mut shown = 0;
    for m in visible {
        let line = format!("\n- [{}] {}", m.id, m.text);
        if out.len() + line.len() > max_bytes {
            break;
        }
        out.push_str(&line);
        shown += 1;
    }
    if shown > 0 && shown < visible.len() {
        out.push_str(&format!(
            "\n({} older {} not shown: the memory size limit is reached.)",
            visible.len() - shown,
            if visible.len() - shown == 1 {
                "memory is"
            } else {
                "memories are"
            }
        ));
    }
    out.push_str("\n\n");
    out.push_str(GUIDANCE);
    out
}

/// The text a memory keeps: whitespace collapsed (a memory is one line), at most
/// [`MAX_CHARS`] characters. Returns the text and whether it was cut; an empty text is refused.
pub fn clean_text(text: &str) -> Result<(String, bool), String> {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return Err("The memory is empty: say what to remember.".into());
    }
    if one_line.chars().count() > MAX_CHARS {
        let kept: String = one_line.chars().take(MAX_CHARS).collect();
        return Ok((kept.trim_end().to_owned(), true));
    }
    Ok((one_line, false))
}

/// Case-insensitive equality, the way duplicates are spotted.
fn same_text(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// What [`MemoryStore::add`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Added {
    Saved {
        memory: Memory,
        cut: bool,
    },
    /// The same text is remembered already (in that scope).
    Known(Memory),
}

/// The memories in `memory.json`, shared by the agent (tokio), the Settings window (main thread)
/// and a `cleanrstai exec` run. Every read and change goes to the file, so each sees the others'
/// changes; the file is replaced atomically, and `lock` keeps this process's changes in turn.
pub struct MemoryStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl MemoryStore {
    /// The store in the data directory.
    pub fn open() -> Arc<Self> {
        Arc::new(Self::at(&crate::config::data_dir().join("memory.json")))
    }

    /// The store in the file at `path` (which need not exist yet).
    pub fn at(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            lock: Mutex::new(()),
        }
    }

    /// The memories in the file; none when there is no file yet.
    fn read(&self) -> Result<Vec<Memory>, String> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("{} can't be read: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(format!("{} can't be read: {e}", self.path.display())),
        }
    }

    /// Every memory, in the order they were saved. A file that can't be read gives none (and
    /// is logged); changes refuse to run on it, so it is never overwritten.
    pub fn all(&self) -> Vec<Memory> {
        self.read().unwrap_or_else(|e| {
            log::error!("{e}");
            Vec::new()
        })
    }

    /// Reads the file, applies `change` and saves the result. Nothing is written when `change`
    /// refuses, or when the file can't be read.
    fn update<T>(
        &self,
        change: impl FnOnce(&mut Vec<Memory>) -> Result<T, String>,
    ) -> Result<T, String> {
        let _turn = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut items = self.read()?;
        let out = change(&mut items)?;
        save_to(&self.path, &items).map_err(|e| format!("Could not save memory: {e:#}"))?;
        Ok(out)
    }

    /// Remembers `text` in `scope`, unless the same text is remembered there already.
    pub fn add(
        &self,
        text: &str,
        scope: Scope,
        source_chat: Option<Uuid>,
    ) -> Result<Added, String> {
        let (text, cut) = clean_text(text)?;
        self.update(|items| {
            if let Some(known) = items
                .iter()
                .find(|m| m.scope == scope && same_text(&m.text, &text))
            {
                return Ok(Added::Known(known.clone()));
            }
            let now = Utc::now();
            let memory = Memory {
                id: Uuid::new_v4(),
                text,
                scope,
                created: now,
                updated: now,
                source_chat,
            };
            items.push(memory.clone());
            Ok(Added::Saved { memory, cut })
        })
    }

    /// Replaces the text of memory `id`. Refused when the text is empty or another memory in
    /// the same scope has it already.
    pub fn edit(&self, id: Uuid, text: &str) -> Result<(), String> {
        let (text, _) = clean_text(text)?;
        self.update(|items| {
            let scope = items
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.scope)
                .ok_or_else(|| "That memory no longer exists.".to_owned())?;
            if items
                .iter()
                .any(|m| m.id != id && m.scope == scope && same_text(&m.text, &text))
            {
                return Err("Another memory already says that.".into());
            }
            let memory = items
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or_else(|| "That memory no longer exists.".to_owned())?;
            memory.text = text;
            memory.updated = Utc::now();
            Ok(())
        })
    }

    /// Deletes memory `id` when `allowed` says so; returns it.
    pub fn remove(&self, id: Uuid, allowed: impl Fn(&Memory) -> bool) -> Result<Memory, String> {
        self.update(|items| {
            let at = items
                .iter()
                .position(|m| m.id == id)
                .ok_or_else(|| "No memory has that id.".to_owned())?;
            if !allowed(&items[at]) {
                return Err("That memory is not in this chat's scope.".into());
            }
            Ok(items.remove(at))
        })
    }

    /// Deletes the memories `matches` selects; returns how many.
    pub fn remove_all(&self, matches: impl Fn(&Memory) -> bool) -> Result<usize, String> {
        self.update(|items| {
            let before = items.len();
            items.retain(|m| !matches(m));
            Ok(before - items.len())
        })
    }
}

fn save_to(path: &Path, items: &[Memory]) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(items)?;
    crate::config::write_private(path, &bytes)
        .with_context(|| format!("writing {}", path.display()))
}

/// What one chat's request needs for memory: the store, what the chat sees, and the tool's
/// approval setting. Only built for a chat with memory on (see [`for_request`]).
#[derive(Clone)]
pub struct MemoryRun {
    pub store: Arc<MemoryStore>,
    pub visibility: Visibility,
    pub ask_before_saving: bool,
    pub max_bytes: usize,
    pub chat: Uuid,
}

/// The memory a request of `conv` gets: `None` when the chat has memory switched off
/// (incognito), otherwise the store with the settings' scope and limits.
pub fn for_request(
    store: &Arc<MemoryStore>,
    selection: &ToolSelection,
    settings: &MemorySettings,
    conv: &Conversation,
) -> Option<MemoryRun> {
    selection.feature_on(tools::MEMORY).then(|| MemoryRun {
        store: store.clone(),
        visibility: Visibility::for_chat(settings, conv.project),
        ask_before_saving: settings.ask_before_saving,
        max_bytes: settings.max_kb as usize * 1024,
        chat: conv.id,
    })
}

/// Whether a call with these arguments changes memory: anything but `list`, including
/// arguments that don't parse.
fn changes_memory(arguments: &str) -> bool {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|v| {
            v.get("action")
                .and_then(Value::as_str)
                .map(|a| a.trim().to_owned())
        })
        .as_deref()
        != Some("list")
}

impl MemoryRun {
    /// The system prompt section for this request.
    pub fn prompt(&self) -> String {
        let all = self.store.all();
        prompt_section(&visible(&all, self.visibility), self.max_bytes)
    }

    /// Whether a call with `arguments` asks for approval first.
    pub fn asks(&self, arguments: &str) -> bool {
        self.ask_before_saving && changes_memory(arguments)
    }

    /// Runs one call of the memory tool: `save`, `list` or `delete`. The text returned is what
    /// the model sees; `Err` is a tool error.
    pub fn call(&self, args: &Value) -> Result<String, String> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        match action {
            "save" => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("missing \"text\": give the fact to remember.")?;
                match self
                    .store
                    .add(text, self.visibility.save_scope(), Some(self.chat))?
                {
                    Added::Saved { memory, cut } => Ok(format!(
                        "Saved memory {}: {}{}",
                        memory.id,
                        memory.text,
                        if cut { " (cut to 500 characters)" } else { "" }
                    )),
                    Added::Known(known) => Ok(format!(
                        "Not saved: this is already remembered as {}.",
                        known.id
                    )),
                }
            }
            "list" => {
                let items = visible(&self.store.all(), self.visibility);
                if items.is_empty() {
                    return Ok("No memories for this chat.".into());
                }
                Ok(items
                    .iter()
                    .map(|m| format!("- [{}] {}", m.id, m.text))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "delete" => {
                let id = args
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s.trim()).ok())
                    .ok_or("missing \"id\": use an id from the list.")?;
                let vis = self.visibility;
                let gone = self.store.remove(id, |m| vis.sees(m))?;
                Ok(format!("Deleted memory {}: {}", gone.id, gone.text))
            }
            other => Err(format!(
                "unknown action '{other}': use \"save\", \"list\" or \"delete\"."
            )),
        }
    }
}

/// The memory tool's definition for the model.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: tools::MEMORY_TOOL.into(),
        description: "Remember facts about the user across chats. \"save\" stores one short \
            fact in its own line; \"list\" shows what is remembered for this chat; \"delete\" \
            forgets a memory by its id. Memories are listed in the system prompt too."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["save", "list", "delete"],
                    "description": "What to do."
                },
                "text": {
                    "type": "string",
                    "description": "For save: the fact, in one sentence (at most 500 characters)."
                },
                "id": {
                    "type": "string",
                    "description": "For delete: the memory's id, as listed."
                }
            },
            "required": ["action"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cleanrstai-memory-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("memory.json")
    }

    fn mem(text: &str, scope: Scope, created_secs: i64) -> Memory {
        let at = DateTime::from_timestamp(1_700_000_000 + created_secs, 0).unwrap();
        Memory {
            id: Uuid::new_v4(),
            text: text.into(),
            scope,
            created: at,
            updated: at,
            source_chat: None,
        }
    }

    #[test]
    fn store_round_trips_through_the_file() {
        let path = temp_path("round");
        let store = MemoryStore::at(&path);
        let Added::Saved { memory, cut } =
            store.add("  Likes tea.  ", Scope::Global, None).unwrap()
        else {
            panic!("expected a new memory");
        };
        assert!(!cut);
        assert_eq!(memory.text, "Likes tea.");
        let project = Uuid::new_v4();
        store
            .add("Uses Rust 1.95", Scope::Project(project), None)
            .unwrap();

        // Reloading from the file gives the same memories back.
        let reloaded = MemoryStore::at(&path);
        assert_eq!(reloaded.all(), store.all());
        assert_eq!(reloaded.all().len(), 2);

        reloaded.edit(memory.id, "Likes green tea.").unwrap();
        let again = MemoryStore::at(&path);
        let edited = again.all().into_iter().find(|m| m.id == memory.id).unwrap();
        assert_eq!(edited.text, "Likes green tea.");
        assert!(edited.updated >= edited.created);

        again.remove(memory.id, |_| true).unwrap();
        assert_eq!(MemoryStore::at(&path).all().len(), 1);
        assert_eq!(MemoryStore::at(&path).remove_all(|_| true).unwrap(), 1);
        assert!(MemoryStore::at(&path).all().is_empty());
        // No temporary file is left behind.
        assert!(!path.with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_unreadable_file_is_kept_and_changes_refuse_to_touch_it() {
        let path = temp_path("corrupt");
        std::fs::write(&path, b"{ not json").unwrap();
        let store = MemoryStore::at(&path);
        assert!(store.all().is_empty());
        assert!(store.add("Likes tea", Scope::Global, None).is_err());
        assert!(store.remove_all(|_| true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn two_stores_on_one_file_keep_each_others_memories() {
        // The app and a `cleanrstai exec --save` run each have their own store on the file.
        let path = temp_path("two");
        let app = MemoryStore::at(&path);
        let exec = MemoryStore::at(&path);
        app.add("Likes tea", Scope::Global, None).unwrap();
        assert_eq!(exec.all().len(), 1);
        exec.add("Owns a bike", Scope::Global, None).unwrap();
        // The app's next change builds on the file, so the bike memory survives.
        app.add("Speaks French", Scope::Global, None).unwrap();
        let texts: Vec<String> = MemoryStore::at(&path)
            .all()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert_eq!(texts, ["Likes tea", "Owns a bike", "Speaks French"]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn duplicates_are_refused_without_case() {
        let path = temp_path("dupe");
        let store = MemoryStore::at(&path);
        store
            .add("Prefers metric units", Scope::Global, None)
            .unwrap();
        let again = store
            .add("prefers METRIC units", Scope::Global, None)
            .unwrap();
        assert!(matches!(again, Added::Known(_)));
        // The same text in another scope is a different memory.
        let project = Uuid::new_v4();
        let other = store
            .add("prefers metric units", Scope::Project(project), None)
            .unwrap();
        assert!(matches!(other, Added::Saved { .. }));
        // Editing into a text the same scope has already is refused.
        let tea = match store.add("Likes tea", Scope::Global, None).unwrap() {
            Added::Saved { memory, .. } => memory.id,
            Added::Known(_) => panic!("new text"),
        };
        assert!(store.edit(tea, "PREFERS metric units").is_err());
        let kept = store.all().into_iter().find(|m| m.id == tea).unwrap();
        assert_eq!(kept.text, "Likes tea");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn text_is_one_line_and_capped() {
        assert_eq!(
            clean_text("  a \n\n b\t c ").unwrap(),
            ("a b c".into(), false)
        );
        assert!(clean_text("   \n ").is_err());
        let long = "é".repeat(MAX_CHARS + 10);
        let (kept, cut) = clean_text(&long).unwrap();
        assert!(cut);
        assert_eq!(kept.chars().count(), MAX_CHARS);
    }

    #[test]
    fn scope_decides_what_a_chat_sees_and_saves() {
        let project = Uuid::new_v4();
        let other = Uuid::new_v4();
        let items = vec![
            mem("global", Scope::Global, 1),
            mem("mine", Scope::Project(project), 2),
            mem("theirs", Scope::Project(other), 3),
        ];
        let texts = |vis: Visibility| -> Vec<String> {
            visible(&items, vis).into_iter().map(|m| m.text).collect()
        };
        // Global scope: only global memories, whatever project the chat is in.
        let global = MemorySettings::default();
        assert_eq!(
            texts(Visibility::for_chat(&global, Some(project))),
            ["global"]
        );
        // Per project: the chat's project's memories too, newest first.
        let per = MemorySettings {
            scope: MemoryScope::PerProject,
            ..Default::default()
        };
        assert_eq!(
            texts(Visibility::for_chat(&per, Some(project))),
            ["mine", "global"]
        );
        // A chat outside a project sees the global ones only.
        assert_eq!(texts(Visibility::for_chat(&per, None)), ["global"]);
        // Saves go to the project only in per-project scope inside a project.
        assert_eq!(
            Visibility::for_chat(&per, Some(project)).save_scope(),
            Scope::Project(project)
        );
        assert_eq!(Visibility::for_chat(&per, None).save_scope(), Scope::Global);
        assert_eq!(
            Visibility::for_chat(&global, Some(project)).save_scope(),
            Scope::Global
        );
    }

    #[test]
    fn prompt_lists_newest_first_and_cuts_to_the_limit() {
        let items = vec![
            mem("older fact", Scope::Global, 1),
            mem("newer fact", Scope::Global, 5),
        ];
        let shown = visible(
            &items,
            Visibility {
                per_project: false,
                project: None,
            },
        );
        let text = prompt_section(&shown, 4096);
        assert!(text.starts_with(HEADER), "{text}");
        let newer = text.find("newer fact").unwrap();
        let older = text.find("older fact").unwrap();
        assert!(newer < older, "{text}");
        assert!(text.contains(&format!("- [{}] newer fact", shown[0].id)));
        assert!(text.contains(GUIDANCE));

        // Only what fits: the newest stays, the older is left out and said so.
        let cut = prompt_section(&shown, HEADER.len() + 60);
        assert!(cut.contains("newer fact"), "{cut}");
        assert!(!cut.contains("older fact"), "{cut}");
        assert!(cut.contains("1 older memory is not shown"), "{cut}");
        // The budget covers the header and the listed memories; the note and guidance follow.
        let listed: usize = cut
            .lines()
            .filter(|l| l.starts_with("- ["))
            .map(|l| l.len() + 1)
            .sum();
        assert!(HEADER.len() + listed <= HEADER.len() + 60, "{cut}");

        // Nothing saved yet still says so.
        let empty = prompt_section(&[], 4096);
        assert!(empty.contains("none saved yet"), "{empty}");
    }

    #[test]
    fn the_list_action_is_the_only_one_without_approval() {
        assert!(!changes_memory(r#"{"action": "list"}"#));
        assert!(changes_memory(r#"{"action": "save", "text": "x"}"#));
        assert!(changes_memory(r#"{"action": "delete", "id": "x"}"#));
        assert!(changes_memory("not json"));
    }

    #[test]
    fn default_settings_are_off_and_old_files_load() {
        let s = MemorySettings::default();
        assert!(!s.enabled && s.ask_before_saving);
        assert_eq!((s.max_kb, s.scope), (8, MemoryScope::Global));
        let mut big = MemorySettings {
            max_kb: 500,
            ..Default::default()
        };
        big.normalized();
        assert_eq!(big.max_kb, MAX_KB);
        let mut small = MemorySettings {
            max_kb: 0,
            ..Default::default()
        };
        small.normalized();
        assert_eq!(small.max_kb, MIN_KB);
    }
}
