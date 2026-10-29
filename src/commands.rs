//! Slash commands: saved prompts typed as `/name` in the message box. They come from Settings
//! and from `.cleanrstai/commands/*.md` in the chat's working folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::SlashCommandSettings;

/// Replaced by what the user types after the command name.
pub const ARGS: &str = "{{args}}";
/// Most suggestions listed under the message box.
const MAX_SUGGESTIONS: usize = 8;
/// Folder under the working folder that holds workspace commands.
const WORKSPACE_DIR: &[&str] = &[".cleanrstai", "commands"];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SlashCommand {
    /// Typed after the slash: letters, digits, `-` and `_`.
    pub name: String,
    /// Shown next to the name in the suggestions.
    pub description: String,
    /// The text sent in place of the command; `{{args}}` is replaced by the arguments.
    pub prompt: String,
}

/// The commands the app starts with.
pub fn default_commands() -> Vec<SlashCommand> {
    vec![
        SlashCommand {
            name: "explain".into(),
            description: "Explain this code step by step".into(),
            prompt: "Explain this code step by step:\n\n{{args}}".into(),
        },
        SlashCommand {
            name: "summarize".into(),
            description: "Summarise in a few bullet points".into(),
            prompt: "Summarise the following in a few bullet points:\n\n{{args}}".into(),
        },
    ]
}

/// Whether `name` can be typed after a slash.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The text to send for `input` when it is a known command (`/name args`), or `None` when the
/// input is sent as typed.
pub fn expand(input: &str, commands: &[SlashCommand]) -> Option<String> {
    let rest = input.strip_prefix('/')?;
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (name, after) = rest.split_at(name_end);
    let command = commands.iter().find(|c| c.name == name)?;
    let args = after.trim();
    let text = if command.prompt.contains(ARGS) {
        command.prompt.replace(ARGS, args)
    } else if args.is_empty() {
        command.prompt.clone()
    } else {
        format!("{}\n\n{}", command.prompt, args)
    };
    Some(text.trim().to_owned())
}

/// The commands a message box offers while `input` is a command name being typed (`/ex`).
pub fn suggestions(input: &str, commands: &[SlashCommand]) -> Vec<SlashCommand> {
    let Some(typed) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if typed.contains(char::is_whitespace) {
        return Vec::new();
    }
    commands
        .iter()
        .filter(|c| c.name.starts_with(typed))
        .take(MAX_SUGGESTIONS)
        .cloned()
        .collect()
}

/// The commands in `<dir>/.cleanrstai/commands/`: each `<name>.md` file is one command. A first
/// line `description: ...` gives its description and is not part of the prompt. Files whose
/// names aren't valid command names are ignored, and so are unreadable ones.
pub fn workspace_commands(dir: &Path) -> Vec<SlashCommand> {
    let folder = WORKSPACE_DIR
        .iter()
        .fold(dir.to_path_buf(), |p, s| p.join(s));
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut out: Vec<SlashCommand> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md") && p.is_file())
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_owned();
            if !valid_name(&name) {
                return None;
            }
            let text = std::fs::read_to_string(&p).ok()?;
            Some(parse_file(name, &text))
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn parse_file(name: String, text: &str) -> SlashCommand {
    let text = text.trim_start_matches('\u{feff}');
    let first_line = text.lines().next().unwrap_or("");
    let (description, body) = match first_line.trim().strip_prefix("description:") {
        Some(d) => (d.trim().to_owned(), &text[first_line.len()..]),
        None => (String::new(), text),
    };
    SlashCommand {
        name,
        description,
        prompt: body.trim().to_owned(),
    }
}

/// The commands that apply in a chat: the working folder's, when enabled, then the ones from
/// Settings whose names the folder doesn't use (a folder command wins). Sorted by name.
pub fn available(settings: &SlashCommandSettings, workspace: Option<&Path>) -> Vec<SlashCommand> {
    if !settings.enabled {
        return Vec::new();
    }
    let mut out = match workspace {
        Some(dir) if settings.workspace_commands => workspace_commands(dir),
        _ => Vec::new(),
    };
    for c in &settings.commands {
        if !out.iter().any(|o| o.name == c.name) {
            out.push(c.clone());
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The commands for the message box, read again when the input or the working folder changes
/// (so a folder is not read on every frame).
#[derive(Default)]
pub struct SuggestionCache {
    key: Option<(Option<PathBuf>, String)>,
    commands: Vec<SlashCommand>,
}

impl SuggestionCache {
    /// Forget what was read, e.g. after the settings change.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn get(
        &mut self,
        settings: &SlashCommandSettings,
        workspace: Option<&Path>,
        input: &str,
    ) -> &[SlashCommand] {
        let key = (workspace.map(Path::to_path_buf), input.to_owned());
        if self.key.as_ref() != Some(&key) {
            self.commands = available(settings, workspace);
            self.key = Some(key);
        }
        &self.commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str, prompt: &str) -> SlashCommand {
        SlashCommand {
            name: name.into(),
            description: String::new(),
            prompt: prompt.into(),
        }
    }

    #[test]
    fn expands_args_in_place() {
        let cmds = [cmd("explain", "Explain:\n\n{{args}}\n\nEnd {{args}}")];
        assert_eq!(
            expand("/explain  fn main() {}", &cmds).as_deref(),
            Some("Explain:\n\nfn main() {}\n\nEnd fn main() {}")
        );
    }

    #[test]
    fn appends_args_without_placeholder() {
        let cmds = [cmd("tidy", "Tidy this up.")];
        assert_eq!(
            expand("/tidy the  code ", &cmds).as_deref(),
            Some("Tidy this up.\n\nthe  code")
        );
        assert_eq!(expand("/tidy", &cmds).as_deref(), Some("Tidy this up."));
        assert_eq!(expand("/tidy\n", &cmds).as_deref(), Some("Tidy this up."));
    }

    #[test]
    fn unknown_and_prefix_names_are_not_expanded() {
        let cmds = [cmd("explain", "Explain {{args}}")];
        assert_eq!(expand("/explainx please", &cmds), None);
        assert_eq!(expand("/EXPLAIN", &cmds), None);
        assert_eq!(expand("explain", &cmds), None);
        assert_eq!(expand("/", &cmds), None);
        assert_eq!(expand("hello /explain", &cmds), None);
    }

    #[test]
    fn suggestions_match_the_typed_name() {
        let cmds = [cmd("explain", ""), cmd("exit", ""), cmd("summarize", "")];
        let names = |v: Vec<SlashCommand>| v.into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names(suggestions("/ex", &cmds)), ["explain", "exit"]);
        assert_eq!(
            names(suggestions("/", &cmds)),
            ["explain", "exit", "summarize"]
        );
        assert!(suggestions("/explain ", &cmds).is_empty());
        assert!(suggestions("explain", &cmds).is_empty());
    }

    #[test]
    fn valid_names() {
        assert!(valid_name("my-cmd_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("has space"));
        assert!(!valid_name("dot.name"));
        assert!(!valid_name("slash/name"));
    }

    /// A unique temp folder holding an empty `.cleanrstai/commands` folder.
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleanrstai-commands-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join(".cleanrstai").join("commands")).unwrap();
        dir
    }

    #[test]
    fn loads_workspace_files() {
        let dir = temp_dir("load");
        let folder = dir.join(".cleanrstai").join("commands");
        std::fs::write(
            folder.join("review.md"),
            "description: Review a diff\nReview the changes below:\n\n{{args}}\n",
        )
        .unwrap();
        std::fs::write(folder.join("plain.md"), "Just a prompt.").unwrap();
        std::fs::write(folder.join("bad name.md"), "Ignored.").unwrap();
        std::fs::write(folder.join("notes.txt"), "Ignored.").unwrap();

        let cmds = workspace_commands(&dir);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].name, "plain");
        assert_eq!(cmds[0].description, "");
        assert_eq!(cmds[0].prompt, "Just a prompt.");
        assert_eq!(cmds[1].name, "review");
        assert_eq!(cmds[1].description, "Review a diff");
        assert_eq!(cmds[1].prompt, "Review the changes below:\n\n{{args}}");
        let empty = temp_dir("empty");
        assert!(workspace_commands(&empty).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn workspace_command_overrides_settings_command() {
        let dir = temp_dir("override");
        let folder = dir.join(".cleanrstai").join("commands");
        std::fs::write(folder.join("explain.md"), "Folder version {{args}}").unwrap();
        let settings = SlashCommandSettings {
            enabled: true,
            workspace_commands: true,
            commands: vec![cmd("explain", "Settings version"), cmd("zeta", "Z")],
        };

        let cmds = available(&settings, Some(&dir));
        let names: Vec<_> = cmds.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["explain", "zeta"]);
        assert_eq!(cmds[0].prompt, "Folder version {{args}}");
        assert_eq!(
            expand("/explain hi", &cmds).as_deref(),
            Some("Folder version hi")
        );

        // Off, or no folder: the folder's files are not read.
        let off = SlashCommandSettings {
            workspace_commands: false,
            ..settings.clone()
        };
        assert_eq!(available(&off, Some(&dir))[0].prompt, "Settings version");
        assert_eq!(available(&settings, None)[0].prompt, "Settings version");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn disabled_offers_nothing() {
        let settings = SlashCommandSettings {
            enabled: false,
            ..SlashCommandSettings::default()
        };
        assert!(available(&settings, None).is_empty());
    }

    #[test]
    fn normalize_trims_and_drops_bad_and_repeated_names() {
        let mut s = crate::config::Settings::default();
        s.general.slash_commands.commands = vec![
            cmd(" explain ", "first"),
            cmd("explain", "repeat"),
            cmd("", "empty"),
            cmd("bad name", "spaces"),
            cmd("dot.name", "dot"),
            cmd("ok_1", "kept"),
        ];
        s.normalize();
        let kept = &s.general.slash_commands.commands;
        let names: Vec<_> = kept.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["explain", "ok_1"]);
        assert_eq!(kept[0].prompt, "first");
    }

    #[test]
    fn defaults_are_valid() {
        let cmds = default_commands();
        assert!(cmds.iter().all(|c| valid_name(&c.name)));
        assert_eq!(
            expand("/explain x", &cmds).as_deref(),
            Some("Explain this code step by step:\n\nx")
        );
    }
}
