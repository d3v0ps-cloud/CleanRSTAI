//! Skills: folders of instructions the model loads with the `load_skill` tool when a task
//! matches. A skill is a folder with a `SKILL.md` file, optionally starting with front matter:
//!
//! ```text
//! ---
//! name: release-notes
//! description: Write release notes from the merged pull requests
//! ---
//! Steps the model follows…
//! ```
//!
//! Skills come from `skills/` in the app's config folder and, when a chat has a workspace,
//! from `.cleanrstai/skills/` in it. The model only sees names and descriptions in the system
//! prompt; it loads a skill's text (or another file in its folder) through the tool.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::config::SkillSettings;

/// The file in a skill's folder that holds its instructions.
pub const SKILL_FILE: &str = "SKILL.md";
/// Folder under a workspace that holds workspace skills.
const WORKSPACE_DIR: &[&str] = &[".cleanrstai", "skills"];
/// Longest description kept, in characters.
const MAX_DESCRIPTION: usize = 200;
/// Most other files named after a skill's text.
const MAX_LISTED_FILES: usize = 50;
/// Most folder entries looked at while listing a skill's files.
const MAX_SCANNED_ENTRIES: usize = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSource {
    /// The app's skills folder in the config directory.
    User,
    /// `.cleanrstai/skills/` in the chat's working folder.
    Workspace,
}

impl SkillSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "Your skills folder",
            Self::Workspace => "Workspace",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    /// One line, shown to the model in the system prompt.
    pub description: String,
    pub source: SkillSource,
    /// The skill's folder (holding `SKILL.md`).
    pub dir: PathBuf,
}

/// The app's skills folder.
pub fn user_root() -> PathBuf {
    crate::config::config_dir().join("skills")
}

/// The workspace's skills folder, `.cleanrstai/skills`.
pub fn workspace_root(workspace: &Path) -> PathBuf {
    WORKSPACE_DIR
        .iter()
        .fold(workspace.to_path_buf(), |p, s| p.join(s))
}

/// The skills a chat may use: none when skills are off, else those from the folders for
/// `workspace` (if any), with the workspace's skills last so they replace same-named ones.
pub fn for_chat(settings: &SkillSettings, workspace: Option<&Path>) -> Vec<Skill> {
    if !settings.enabled {
        return Vec::new();
    }
    let mut dirs = vec![(user_root(), SkillSource::User)];
    if let Some(ws) = workspace.filter(|_| settings.workspace_skills) {
        dirs.push((workspace_root(ws), SkillSource::Workspace));
    }
    discover(&dirs, settings)
}

/// Find the skills in `dirs`, each a folder of skill folders (`<dir>/<name>/SKILL.md`). `dirs`
/// go from lowest to highest priority: a skill in a later folder replaces one of the same name.
/// Skills named in `settings.disabled` are left out. Sorted by name.
pub fn discover(dirs: &[(PathBuf, SkillSource)], settings: &SkillSettings) -> Vec<Skill> {
    let mut found: BTreeMap<String, Skill> = BTreeMap::new();
    for (root, source) in dirs {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.join(SKILL_FILE).is_file())
            .collect();
        folders.sort();
        for dir in folders {
            let Some(folder) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(dir.join(SKILL_FILE)) else {
                continue;
            };
            let skill = parse(folder, &text, *source, dir.clone());
            found.insert(skill.name.clone(), skill);
        }
    }
    found
        .into_values()
        .filter(|s| !settings.disabled.contains(&s.name))
        .collect()
}

/// The skill described by a `SKILL.md` text in the folder `folder`.
fn parse(folder: &str, text: &str, source: SkillSource, dir: PathBuf) -> Skill {
    let (fields, body) = front_matter(text);
    let field = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let name = field("name").unwrap_or_else(|| folder.to_owned());
    let description = match field("description") {
        Some(d) => one_line(&d),
        None => body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#') && *l != "---")
            .map(one_line)
            .unwrap_or_default(),
    };
    Skill {
        name,
        description,
        source,
        dir,
    }
}

/// Splits off the front matter: the `key: value` lines between a leading `---` line and the
/// next `---` line. Text without a closed front matter has no fields and is all body.
fn front_matter(text: &str) -> (Vec<(String, String)>, &str) {
    let text = text.trim_start_matches('\u{feff}');
    let Some(rest) = text.strip_prefix("---") else {
        return (Vec::new(), text);
    };
    let Some(rest) = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
    else {
        return (Vec::new(), text);
    };
    let mut fields = Vec::new();
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        offset += line.len();
        let trimmed = line.trim();
        if trimmed == "---" {
            return (fields, &rest[offset..]);
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            fields.push((
                key.trim().to_ascii_lowercase(),
                unquote(value.trim()).to_owned(),
            ));
        }
    }
    (Vec::new(), text)
}

/// `"x"` and `'x'` give `x`; other values are returned as they are.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// The text on one line, with runs of whitespace collapsed and at most
/// [`MAX_DESCRIPTION`] characters.
fn one_line(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= MAX_DESCRIPTION {
        return joined;
    }
    let mut short: String = joined.chars().take(MAX_DESCRIPTION).collect();
    short.push('…');
    short
}

/// The system prompt section that lists `skills` for the model.
pub fn prompt_section(skills: &[Skill]) -> String {
    let mut out =
        String::from("Skills available (load one with the load_skill tool before following it):");
    for s in skills {
        if s.description.is_empty() {
            out.push_str(&format!("\n- {}", s.name));
        } else {
            out.push_str(&format!("\n- {}: {}", s.name, s.description));
        }
    }
    out
}

/// Runs the `load_skill` tool: `name` picks the skill, `file` (optional) a file in its folder
/// instead of `SKILL.md`. Text longer than `max_bytes` is cut.
pub fn call(skills: &[Skill], args: &Value, max_bytes: usize) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or("missing \"name\": give a skill's name from the list.")?;
    let Some(skill) = skills.iter().find(|s| s.name == name) else {
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        return Err(format!(
            "no skill named '{name}'. Skills: {}.",
            if names.is_empty() {
                "none".to_owned()
            } else {
                names.join(", ")
            }
        ));
    };
    let file = args
        .get("file")
        .and_then(Value::as_str)
        .filter(|f| !f.trim().is_empty());
    load(skill, file, max_bytes)
}

/// The text of a skill: its `SKILL.md` (followed by the names of the other files in its
/// folder), or the file `file` inside the folder. Text longer than `max_bytes` is cut.
pub fn load(skill: &Skill, file: Option<&str>, max_bytes: usize) -> Result<String, String> {
    match file {
        None => {
            let path = resolve(&skill.dir, SKILL_FILE)?;
            let mut text = read_capped(&path, max_bytes)?;
            let (others, more) = other_files(&skill.dir);
            if !others.is_empty() {
                text.push_str(
                    "\n\nOther files in this skill's folder (load one with the file argument):",
                );
                for f in &others {
                    text.push_str(&format!("\n- {f}"));
                }
                if more {
                    text.push_str("\n- …");
                }
            }
            Ok(text)
        }
        Some(rel) => {
            let path = resolve(&skill.dir, rel)?;
            read_capped(&path, max_bytes)
        }
    }
}

/// The file `rel` inside the skill folder `dir`. `rel` must be relative and must not go up
/// with `..`; after symlinks are followed it must still be inside the folder.
fn resolve(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("the file name is empty.".into());
    }
    let rel_path = Path::new(rel);
    if rel_path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "'{rel}' must be a path inside the skill's folder (no '..' and no absolute paths)."
        ));
    }
    let root = dir
        .canonicalize()
        .map_err(|e| format!("the skill's folder can't be read: {e}"))?;
    let path = root
        .join(rel_path)
        .canonicalize()
        .map_err(|_| format!("there is no file '{rel}' in this skill."))?;
    if !path.starts_with(&root) {
        return Err(format!("'{rel}' is outside the skill's folder."));
    }
    if !path.is_file() {
        return Err(format!("'{rel}' is not a file."));
    }
    Ok(path)
}

/// The text of `path`, cut at `max_bytes` (on a character boundary) with a note.
fn read_capped(path: &Path, max_bytes: usize) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("can't read the file: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text.".to_owned())?;
    if text.len() <= max_bytes {
        return Ok(text);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(format!(
        "{}\n\n(Cut short: the file is larger than the {} KB limit.)",
        &text[..end],
        (max_bytes / 1024).max(1)
    ))
}

/// The files under a skill folder, relative with `/` separators, except `SKILL.md`: up to
/// [`MAX_LISTED_FILES`] of them, sorted, and whether there were more. Hidden folders are
/// skipped, and so is everything past [`MAX_SCANNED_ENTRIES`] entries.
fn other_files(dir: &Path) -> (Vec<String>, bool) {
    let mut found = Vec::new();
    let mut scanned = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            scanned += 1;
            if scanned > MAX_SCANNED_ENTRIES {
                stack.clear();
                break;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    stack.push(path);
                }
            } else if (kind.is_file() || (kind.is_symlink() && path.is_file()))
                && let Ok(rel) = path.strip_prefix(dir)
            {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel != SKILL_FILE {
                    found.push(rel);
                }
            }
        }
    }
    found.sort();
    let more = found.len() > MAX_LISTED_FILES;
    found.truncate(MAX_LISTED_FILES);
    (found, more)
}

/// The skills for a chat, read once per working folder, as [`crate::commands::SuggestionCache`]
/// does for slash commands: they are kept until the folder changes or [`SkillCache::clear`] is
/// called (when the settings are applied).
#[derive(Default)]
pub struct SkillCache {
    key: Option<Option<PathBuf>>,
    skills: Vec<Skill>,
}

impl SkillCache {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn get(&mut self, settings: &SkillSettings, workspace: Option<&Path>) -> &[Skill] {
        if self.key.as_ref().map(|k| k.as_deref()) != Some(workspace) {
            self.skills = for_chat(settings, workspace);
            self.key = Some(workspace.map(Path::to_path_buf));
        }
        &self.skills
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cleanrstai-skills-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn write_skill(root: &Path, folder: &str, text: &str) -> PathBuf {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), text).unwrap();
        dir
    }

    fn all() -> SkillSettings {
        SkillSettings::default()
    }

    #[test]
    fn reads_front_matter_and_falls_back_to_the_folder_and_body() {
        let root = temp("parse");
        write_skill(
            &root,
            "notes",
            "---\nname: release-notes\ndescription: \"Write the release notes\"\n---\n# Title\n\nBody.\n",
        );
        write_skill(&root, "plain", "# Heading\n\n  First   real\nline here.\n");
        write_skill(
            &root,
            "empty-front",
            "---\nname:\ndescription:\n---\nFrom the body.",
        );
        let found = discover(&[(root.clone(), SkillSource::User)], &all());
        let got: Vec<(&str, &str)> = found
            .iter()
            .map(|s| (s.name.as_str(), s.description.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("empty-front", "From the body."),
                ("plain", "First real"),
                ("release-notes", "Write the release notes"),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_descriptions_are_shortened() {
        let root = temp("long");
        write_skill(&root, "long", &format!("# T\n\n{}\n", "word ".repeat(100)));
        let found = discover(&[(root.clone(), SkillSource::User)], &all());
        assert_eq!(found.len(), 1);
        assert!(found[0].description.chars().count() <= MAX_DESCRIPTION + 1);
        assert!(found[0].description.ends_with('…'));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_skills_replace_user_skills_and_disabled_ones_are_left_out() {
        let user = temp("user");
        let ws = temp("ws");
        write_skill(&user, "alpha", "---\ndescription: user alpha\n---\n");
        write_skill(&user, "beta", "---\ndescription: user beta\n---\n");
        write_skill(&user, "gamma", "---\ndescription: user gamma\n---\n");
        write_skill(&ws, "alpha", "---\ndescription: workspace alpha\n---\n");
        let dirs = [
            (user.clone(), SkillSource::User),
            (ws.clone(), SkillSource::Workspace),
        ];
        let found = discover(&dirs, &all());
        let names: Vec<(&str, &str, SkillSource)> = found
            .iter()
            .map(|s| (s.name.as_str(), s.description.as_str(), s.source))
            .collect();
        assert_eq!(
            names,
            [
                ("alpha", "workspace alpha", SkillSource::Workspace),
                ("beta", "user beta", SkillSource::User),
                ("gamma", "user gamma", SkillSource::User),
            ]
        );

        let off = SkillSettings {
            disabled: vec!["beta".into(), "alpha".into(), "missing".into()],
            ..all()
        };
        let left: Vec<String> = discover(&dirs, &off).into_iter().map(|s| s.name).collect();
        assert_eq!(left, ["gamma"]);

        // A folder without SKILL.md is not a skill, and a missing root is fine.
        std::fs::create_dir_all(user.join("no-file")).unwrap();
        assert!(discover(&[(user.join("nowhere"), SkillSource::User)], &all()).is_empty());
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn for_chat_respects_the_switches() {
        let off = SkillSettings {
            enabled: false,
            ..all()
        };
        assert!(for_chat(&off, None).is_empty());
        let ws = temp("switch");
        let no_ws = SkillSettings {
            workspace_skills: false,
            ..all()
        };
        write_skill(
            &workspace_root(&ws),
            "local",
            "---\ndescription: here\n---\n",
        );
        assert!(
            for_chat(&no_ws, Some(&ws))
                .iter()
                .all(|s| s.source != SkillSource::Workspace)
        );
        assert!(
            for_chat(&all(), Some(&ws))
                .iter()
                .any(|s| s.name == "local" && s.source == SkillSource::Workspace)
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_returns_skill_text_and_lists_its_other_files() {
        let root = temp("load");
        let dir = write_skill(
            &root,
            "tool",
            "---\ndescription: a tool\n---\nDo the thing.",
        );
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts/run.sh"), "echo hi").unwrap();
        std::fs::write(dir.join("notes.md"), "more").unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join(".hidden/x"), "x").unwrap();
        let skill = discover(&[(root.clone(), SkillSource::User)], &all()).remove(0);
        let text = load(&skill, None, 1024).unwrap();
        assert!(text.contains("Do the thing."), "{text}");
        assert!(text.contains("- notes.md\n- scripts/run.sh"), "{text}");
        assert!(!text.contains(".hidden"));
        assert!(!text.contains("- SKILL.md"));
        assert_eq!(
            load(&skill, Some("scripts/run.sh"), 1024).unwrap(),
            "echo hi"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_stays_inside_the_skill_folder() {
        let root = temp("confine");
        let dir = write_skill(&root, "safe", "---\ndescription: s\n---\nbody");
        std::fs::write(root.join("secret.txt"), "outside").unwrap();
        let skill = discover(&[(root.clone(), SkillSource::User)], &all()).remove(0);

        for bad in ["../secret.txt", "a/../../secret.txt", "/etc/passwd", ""] {
            assert!(load(&skill, Some(bad), 1024).is_err(), "{bad} was allowed");
        }
        assert!(load(&skill, Some("missing.md"), 1024).is_err());
        std::fs::write(dir.join("inside.txt"), "in").unwrap();
        assert_eq!(load(&skill, Some("inside.txt"), 1024).unwrap(), "in");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("secret.txt"), dir.join("link.txt")).unwrap();
            let err = load(&skill, Some("link.txt"), 1024).unwrap_err();
            assert!(err.contains("outside"), "{err}");
            // A link that stays inside the folder is fine.
            std::os::unix::fs::symlink(dir.join("inside.txt"), dir.join("alias.txt")).unwrap();
            assert_eq!(load(&skill, Some("alias.txt"), 1024).unwrap(), "in");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_text_is_cut_with_a_note() {
        let root = temp("cut");
        let dir = write_skill(&root, "big", "---\ndescription: b\n---\n");
        std::fs::write(dir.join("big.txt"), "é".repeat(2000)).unwrap();
        let skill = discover(&[(root.clone(), SkillSource::User)], &all()).remove(0);
        let text = load(&skill, Some("big.txt"), 1001).unwrap();
        assert!(text.contains("Cut short"), "{text}");
        assert!(text.len() < 1001 + 100);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_tool_names_skills_it_does_not_know() {
        let root = temp("tool");
        write_skill(&root, "one", "---\ndescription: first\n---\nText.");
        let skills = discover(&[(root.clone(), SkillSource::User)], &all());
        let got = call(&skills, &json!({"name": "one"}), 1024).unwrap();
        assert!(got.contains("Text."));
        let err = call(&skills, &json!({"name": "two"}), 1024).unwrap_err();
        assert!(err.contains("Skills: one."), "{err}");
        assert!(call(&skills, &json!({}), 1024).is_err());
        let prompt = prompt_section(&skills);
        assert!(prompt.starts_with("Skills available"));
        assert!(prompt.ends_with("\n- one: first"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_reads_once_per_folder_until_cleared() {
        let ws = temp("cache");
        write_skill(
            &workspace_root(&ws),
            "one",
            "---\ndescription: first\n---\n",
        );
        let mut cache = SkillCache::default();
        assert_eq!(cache.get(&all(), Some(&ws)).len(), 1);
        write_skill(
            &workspace_root(&ws),
            "two",
            "---\ndescription: second\n---\n",
        );
        assert_eq!(
            cache.get(&all(), Some(&ws)).len(),
            1,
            "kept for the same folder"
        );
        cache.clear();
        assert_eq!(cache.get(&all(), Some(&ws)).len(), 2);
        let _ = std::fs::remove_dir_all(&ws);
    }
}
