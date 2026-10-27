//! Project instructions: `AGENTS.md`, `CLAUDE.md` or other files the user lists in Settings,
//! read from a chat's workspace folder (and optionally the folders above it) and added to the
//! system prompt.

use std::path::{Path, PathBuf};

use crate::config::InstructionSettings;

/// The instruction files found for a workspace, and their text for the system prompt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    /// The files read, as shown to the user: relative to the workspace for files inside it.
    pub files: Vec<String>,
    /// The note for the system prompt.
    pub prompt: String,
    /// Some text was left out to stay under the size limit.
    pub truncated: bool,
}

/// Read the instruction files for the workspace at `root`. `None` when there are none.
pub fn load(root: &Path, s: &InstructionSettings) -> Option<Loaded> {
    let folders: Vec<PathBuf> = if s.parent_folders {
        // Outermost first, so the workspace's own file comes last and reads as the most specific.
        let mut up: Vec<PathBuf> = root.ancestors().map(Path::to_path_buf).collect();
        up.reverse();
        up
    } else {
        vec![root.to_path_buf()]
    };
    let mut budget = s.max_kb as usize * 1024;
    let mut out = Loaded::default();
    let mut sections = String::new();
    for folder in folders {
        for name in &s.file_names {
            let path = folder.join(name);
            if !path.is_file() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let shown = match path.strip_prefix(root) {
                Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                Err(_) => path.display().to_string(),
            };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let text = if text.len() > budget {
                out.truncated = true;
                let mut end = budget;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                &text[..end]
            } else {
                text
            };
            budget -= text.len();
            if !text.is_empty() {
                sections.push_str(&format!(
                    "\n\n<instructions file=\"{shown}\">\n{text}\n</instructions>"
                ));
                out.files.push(shown);
            }
            if !s.all_files || budget == 0 {
                break;
            }
        }
        if budget == 0 {
            break;
        }
    }
    if out.files.is_empty() {
        return None;
    }
    out.prompt = format!(
        "The workspace has project instructions. Follow them unless the user asks otherwise; \
         where they disagree, files listed later are more specific.{sections}{}",
        if out.truncated {
            "\n\n(Some of the instructions were left out because they are long.)"
        } else {
            ""
        }
    );
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cleanrstai-ins-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn reads_the_first_listed_file_or_all_of_them() {
        let dir = temp();
        let ws = dir.join("ws");
        let s = InstructionSettings::default();
        assert_eq!(load(&ws, &s), None);

        std::fs::write(ws.join("CLAUDE.md"), "Use tabs.").unwrap();
        let got = load(&ws, &s).unwrap();
        assert_eq!(got.files, ["CLAUDE.md"]);
        assert!(
            got.prompt
                .contains("<instructions file=\"CLAUDE.md\">\nUse tabs.\n</instructions>")
        );

        std::fs::write(ws.join("AGENTS.md"), "Run the tests.").unwrap();
        assert_eq!(load(&ws, &s).unwrap().files, ["AGENTS.md"]);
        let all = InstructionSettings {
            all_files: true,
            ..s.clone()
        };
        assert_eq!(load(&ws, &all).unwrap().files, ["AGENTS.md", "CLAUDE.md"]);

        // Parent folders come first; the workspace's own file last.
        std::fs::write(dir.join("AGENTS.md"), "Be brief.").unwrap();
        let parents = InstructionSettings {
            parent_folders: true,
            ..s.clone()
        };
        let got = load(&ws, &parents).unwrap();
        assert_eq!(got.files.len(), 2);
        assert!(got.files[0].ends_with("AGENTS.md") && got.files[0] != "AGENTS.md");
        assert_eq!(got.files[1], "AGENTS.md");
        assert!(got.prompt.find("Be brief").unwrap() < got.prompt.find("Run the tests").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stops_at_the_size_limit() {
        let dir = temp();
        let ws = dir.join("ws");
        std::fs::write(ws.join("AGENTS.md"), "é".repeat(2000)).unwrap();
        let s = InstructionSettings {
            max_kb: 1,
            ..Default::default()
        };
        let got = load(&ws, &s).unwrap();
        assert!(got.truncated);
        assert!(got.prompt.contains("left out"));
        assert!(got.prompt.len() < 1024 + 400, "{}", got.prompt.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
