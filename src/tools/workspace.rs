//! Workspace tools: read, search, edit and run commands in a folder the user picked for the
//! chat. Every path is resolved inside that folder; anything that would leave it (`..`, an
//! absolute path elsewhere, a symlink pointing out) is refused.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use uuid::Uuid;

use crate::config::SandboxNetwork;
use crate::llm::ToolSpec;
use crate::mcp::ToolOutput;
use crate::sandbox::{Place, Sandbox};
use crate::types::SandboxInfo;

pub const READ_FILE: &str = "read_file";
pub const GLOB: &str = "file_glob_search";
pub const GREP: &str = "grep_search";
pub const SHELL: &str = "exec_shell_command";
pub const WRITE_FILE: &str = "write_file";
pub const EDIT_FILE: &str = "edit_file";
pub const INFO: &str = "get_info";

/// The workspace tools in display order: (function name, label).
pub const TOOLS: [(&str, &str); 7] = [
    (READ_FILE, "Read file"),
    (GLOB, "Find files"),
    (GREP, "Search in files"),
    (INFO, "File info"),
    (WRITE_FILE, "Write file"),
    (EDIT_FILE, "Edit file"),
    (SHELL, "Run command"),
];

/// Tools that change files or run programs, and so ask for approval.
pub fn changes_things(function: &str) -> bool {
    matches!(function, WRITE_FILE | EDIT_FILE | SHELL)
}

const MAX_READ_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_READ_LINES: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
const MAX_GLOB_RESULTS: usize = 200;
const DEFAULT_GREP_RESULTS: usize = 100;
const MAX_GREP_RESULTS: usize = 1000;
const MAX_GREP_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 30_000;
const DEFAULT_SHELL_SECS: u64 = 120;
const MAX_SHELL_SECS: u64 = 600;
const MAX_LISTING: usize = 200;

/// A folder the model may work in.
#[derive(Clone, Debug)]
pub struct Workspace {
    /// Canonical path of the folder.
    root: PathBuf,
    /// The Docker sandbox container that runs shell commands.
    sandbox: Option<Place>,
    /// A subagent's workspace: its writes and commands run without asking.
    subagent: bool,
}

impl Workspace {
    pub fn open(path: &Path) -> Result<Self> {
        let root = path
            .canonicalize()
            .with_context(|| format!("the workspace {} doesn't exist", path.display()))?;
        if !root.is_dir() {
            bail!("the workspace {} isn't a folder", root.display());
        }
        Ok(Self {
            root,
            sandbox: None,
            subagent: false,
        })
    }

    /// Run shell commands in the Docker `sandbox`, in the container of `chat`.
    pub fn with_sandbox(mut self, sandbox: Sandbox, chat: Uuid) -> Self {
        self.sandbox = Some(Place::Chat(sandbox, chat));
        self
    }

    /// This workspace for a subagent: shell commands run in the subagent's own container,
    /// which shares the chat's network. `None` without a sandbox (subagents never run
    /// commands on this computer). The container is removed when the last clone is dropped.
    pub fn for_subagent(&self) -> Option<Self> {
        Some(Self {
            root: self.root.clone(),
            sandbox: Some(self.sandbox.as_ref()?.for_subagent()),
            subagent: true,
        })
    }

    /// Whether shell commands run in a sandbox with the folder mounted read-only.
    pub fn read_only(&self) -> bool {
        self.sandbox
            .as_ref()
            .is_some_and(|p| p.sandbox().config().read_only)
    }

    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A note for the system prompt, so the model knows where it is working.
    pub fn prompt_note(&self) -> String {
        let mut note = format!(
            "You are working in the local folder {} (the workspace). Use the workspace tools \
             to look around before changing anything: get_info, file_glob_search, \
             grep_search and read_file. Paths are relative to the workspace. Make changes \
             with edit_file (exact string replacement) or write_file, and run commands with \
             exec_shell_command{}.",
            self.root.display(),
            if self.subagent {
                ""
            } else {
                "; the user approves each of those"
            }
        );
        if let Some(place) = &self.sandbox {
            note.push_str(&format!(
                " Shell commands run in a Linux container (image {}) with this folder at /workspace.",
                place.sandbox().image()
            ));
        }
        note
    }

    /// The spec offered to the model for `name`. The shell tool's description says where its
    /// commands run when this workspace runs them in a sandbox.
    pub fn spec(&self, name: &str) -> ToolSpec {
        let mut spec = base_spec(name);
        if let (SHELL, Some(place)) = (name, &self.sandbox) {
            spec.description = sandbox_shell_description(place.sandbox());
        }
        spec
    }

    /// `rel` (relative to the workspace, or absolute inside it) as a path that must exist.
    fn existing(&self, rel: &str) -> Result<PathBuf> {
        let joined = self.join(rel)?;
        let real = joined
            .canonicalize()
            .with_context(|| format!("{rel} doesn't exist"))?;
        self.check_inside(&real, rel)?;
        Ok(real)
    }

    /// `rel` as a path that may not exist yet (for writing); its nearest existing ancestor
    /// must be inside the workspace.
    fn writable(&self, rel: &str) -> Result<PathBuf> {
        let joined = self.join(rel)?;
        if joined.exists() {
            let real = joined.canonicalize()?;
            self.check_inside(&real, rel)?;
            if real.is_dir() {
                bail!("{rel} is a folder");
            }
            return Ok(real);
        }
        let mut ancestor = joined.parent();
        while let Some(a) = ancestor {
            if a.exists() {
                let real = a.canonicalize()?;
                self.check_inside(&real, rel)?;
                return Ok(real.join(joined.strip_prefix(a).unwrap_or(&joined)));
            }
            ancestor = a.parent();
        }
        bail!("{rel} is outside the workspace")
    }

    fn join(&self, rel: &str) -> Result<PathBuf> {
        let rel = rel.trim();
        let p = Path::new(if rel.is_empty() { "." } else { rel });
        if p.components().any(|c| c == Component::ParentDir) {
            bail!("paths may not contain \"..\"; use a path inside the workspace");
        }
        Ok(if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        })
    }

    fn check_inside(&self, real: &Path, rel: &str) -> Result<()> {
        if !real.starts_with(&self.root) {
            bail!(
                "{rel} is outside the workspace ({}); only files inside it can be used",
                self.root.display()
            );
        }
        Ok(())
    }

    /// `path` relative to the workspace, with `/` separators, for showing to the model.
    fn show(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let s = rel.to_string_lossy().replace('\\', "/");
        if s.is_empty() { ".".into() } else { s }
    }

    /// Files under `dir`, honouring .gitignore and skipping `.git`.
    fn walk(&self, dir: &Path) -> impl Iterator<Item = PathBuf> + use<> {
        ignore::WalkBuilder::new(dir)
            .hidden(false)
            .git_ignore(true)
            .git_global(false)
            .require_git(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| e.into_path())
    }

    /// Run one workspace tool.
    pub async fn call(&self, function: &str, args: &Value, timeout: Duration) -> ToolOutput {
        let result = if function == SHELL {
            self.shell(args, timeout).await
        } else {
            // File-system work can take a while on big trees; keep it off the async runtime.
            let (ws, f, a) = (self.clone(), function.to_owned(), args.clone());
            tokio::task::spawn_blocking(move || ws.call_blocking(&f, &a))
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("the tool crashed: {e}")))
        };
        result.unwrap_or_else(|e| super::error_output(format!("Error: {e:#}")))
    }

    fn call_blocking(&self, function: &str, args: &Value) -> Result<ToolOutput> {
        let text = match function {
            READ_FILE => return self.read_file(args),
            GLOB => self.glob(args)?,
            GREP => self.grep(args)?,
            WRITE_FILE => self.write_file(args)?,
            EDIT_FILE => self.edit_file(args)?,
            INFO => self.info(args)?,
            other => bail!("'{other}' is not a workspace tool"),
        };
        Ok(super::text_output(text))
    }

    fn read_file(&self, args: &Value) -> Result<ToolOutput> {
        let rel = str_arg(args, "path")?;
        let path = self.existing(rel)?;
        if path.is_dir() {
            bail!("{rel} is a folder; use get_info or file_glob_search to list it");
        }
        let size = path.metadata()?.len();
        if size > MAX_READ_BYTES {
            bail!(
                "{rel} is {} MB; files over 10 MB can't be read",
                size / (1024 * 1024)
            );
        }
        let bytes = std::fs::read(&path)?;
        if let Some(mime) = crate::attach::image_mime(&bytes) {
            return Ok(ToolOutput {
                text: format!("{} is an image ({mime}); it is attached.", self.show(&path)),
                is_error: false,
                images: vec![crate::attach::Attachment::image(
                    self.show(&path),
                    mime.into(),
                    &bytes,
                )],
                sandbox: None,
            });
        }
        if is_binary(&bytes) {
            bail!("{rel} is a binary file");
        }
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let offset = usize_arg(args, "offset")?.unwrap_or(1).max(1);
        let limit = usize_arg(args, "limit")?
            .unwrap_or(DEFAULT_READ_LINES)
            .max(1);
        if lines.is_empty() {
            return Ok(super::text_output(format!(
                "{} is empty.",
                self.show(&path)
            )));
        }
        if offset > lines.len() {
            bail!("{rel} has only {} lines", lines.len());
        }
        let end = (offset - 1 + limit).min(lines.len());
        let mut out = String::new();
        for (i, line) in lines[offset - 1..end].iter().enumerate() {
            out.push_str(&format!(
                "{:>6}\t{}\n",
                offset + i,
                clip(line, MAX_LINE_CHARS)
            ));
        }
        if end < lines.len() {
            out.push_str(&format!(
                "[Lines {offset}–{end} of {}. Read on with offset={}.]\n",
                lines.len(),
                end + 1
            ));
        }
        Ok(super::text_output(out))
    }

    fn glob(&self, args: &Value) -> Result<String> {
        let pattern = str_arg(args, "pattern")?;
        let dir = self.existing(opt_str(args, "path").unwrap_or("."))?;
        let matcher = PathMatcher::new(pattern)?;
        let mut hits: Vec<(SystemTime, String)> = self
            .walk(&dir)
            .filter_map(|p| {
                let rel = p
                    .strip_prefix(&dir)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                matcher.is_match(&rel).then(|| {
                    let modified = p.metadata().and_then(|m| m.modified()).ok();
                    (modified.unwrap_or(SystemTime::UNIX_EPOCH), self.show(&p))
                })
            })
            .collect();
        if hits.is_empty() {
            return Ok(format!("No files match {pattern}."));
        }
        // Most recently changed first, like an editor's "recent files".
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let total = hits.len();
        let mut out: String = hits
            .into_iter()
            .take(MAX_GLOB_RESULTS)
            .map(|(_, p)| p + "\n")
            .collect();
        if total > MAX_GLOB_RESULTS {
            out.push_str(&format!(
                "[{total} files match; showing the {MAX_GLOB_RESULTS} most recently changed. \
                 Use a narrower pattern or path.]\n"
            ));
        }
        Ok(out)
    }

    fn grep(&self, args: &Value) -> Result<String> {
        let pattern = str_arg(args, "pattern")?;
        let re = regex::RegexBuilder::new(pattern)
            .case_insensitive(args["case_insensitive"].as_bool().unwrap_or(false))
            .build()
            .with_context(|| format!("{pattern:?} isn't a valid regular expression"))?;
        let start = self.existing(opt_str(args, "path").unwrap_or("."))?;
        let filter = opt_str(args, "glob").map(PathMatcher::new).transpose()?;
        let max = usize_arg(args, "max_results")?
            .unwrap_or(DEFAULT_GREP_RESULTS)
            .clamp(1, MAX_GREP_RESULTS);
        let files: Vec<PathBuf> = if start.is_file() {
            vec![start.clone()]
        } else {
            self.walk(&start).collect()
        };
        let mut out = String::new();
        let mut count = 0;
        let mut more = false;
        'files: for path in files {
            if let Some(f) = &filter {
                let rel = path.strip_prefix(&start).unwrap_or(&path);
                if !f.is_match(&rel.to_string_lossy().replace('\\', "/")) {
                    continue;
                }
            }
            if path.metadata().map(|m| m.len()).unwrap_or(0) > MAX_GREP_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if is_binary(&bytes) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (n, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    if count == max {
                        more = true;
                        break 'files;
                    }
                    out.push_str(&format!(
                        "{}:{}: {}\n",
                        self.show(&path),
                        n + 1,
                        clip(line.trim_end(), 300)
                    ));
                    count += 1;
                }
            }
        }
        if count == 0 {
            return Ok(format!("No matches for {pattern:?}."));
        }
        if more {
            out.push_str(&format!(
                "[Stopped at {max} matches; narrow the search or raise max_results.]\n"
            ));
        }
        Ok(out)
    }

    fn write_file(&self, args: &Value) -> Result<String> {
        let rel = str_arg(args, "path")?;
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .context("missing \"content\"")?;
        let path = self.writable(rel)?;
        let existed = path.exists();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(format!(
            "{} {} ({} bytes, {} lines).",
            if existed { "Overwrote" } else { "Created" },
            self.show(&path),
            content.len(),
            content.lines().count()
        ))
    }

    fn edit_file(&self, args: &Value) -> Result<String> {
        let rel = str_arg(args, "path")?;
        let old = args
            .get("old_string")
            .and_then(Value::as_str)
            .context("missing \"old_string\"")?;
        let new = args
            .get("new_string")
            .and_then(Value::as_str)
            .context("missing \"new_string\"")?;
        let all = args["replace_all"].as_bool().unwrap_or(false);
        if old.is_empty() {
            bail!("old_string is empty; use write_file to create a file");
        }
        if old == new {
            bail!("old_string and new_string are the same");
        }
        let path = self.existing(rel)?;
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("{rel} isn't a text file"))?;
        let n = text.matches(old).count();
        match (n, all) {
            (0, _) => bail!(
                "old_string wasn't found in {rel}; read the file and copy the text exactly, \
                 including whitespace"
            ),
            (n, false) if n > 1 => bail!(
                "old_string appears {n} times in {rel}; include more surrounding text to make \
                 it unique, or set replace_all"
            ),
            _ => {}
        }
        let updated = if all {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        };
        std::fs::write(&path, updated)?;
        Ok(format!(
            "Replaced {} occurrence{} in {}.",
            n,
            if n == 1 { "" } else { "s" },
            self.show(&path)
        ))
    }

    fn info(&self, args: &Value) -> Result<String> {
        let rel = opt_str(args, "path").unwrap_or(".");
        let path = self.existing(rel)?;
        let meta = path.metadata()?;
        let mut out = String::new();
        if path == self.root {
            out.push_str(&format!("Workspace: {}\n", self.root.display()));
            out.push_str(&format!(
                "System: {} {} · shell: {}\n",
                std::env::consts::OS,
                std::env::consts::ARCH,
                shell_name()
            ));
            if let Some(branch) = git_branch(&self.root) {
                out.push_str(&format!("Git branch: {branch}\n"));
            }
        } else {
            out.push_str(&format!("Path: {}\n", self.show(&path)));
        }
        if let Ok(modified) = meta.modified() {
            let t: chrono::DateTime<chrono::Local> = modified.into();
            out.push_str(&format!("Modified: {}\n", t.format("%Y-%m-%d %H:%M")));
        }
        if meta.is_dir() {
            out.push_str("Type: folder\nContents:\n");
            let mut entries: Vec<(bool, String)> = std::fs::read_dir(&path)?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name() != ".git")
                .map(|e| {
                    let dir = e.file_type().is_ok_and(|t| t.is_dir());
                    (!dir, e.file_name().to_string_lossy().into_owned())
                })
                .collect();
            entries.sort();
            for (is_file, name) in entries.iter().take(MAX_LISTING) {
                out.push_str(&format!("  {name}{}\n", if *is_file { "" } else { "/" }));
            }
            if entries.len() > MAX_LISTING {
                out.push_str(&format!("  … and {} more\n", entries.len() - MAX_LISTING));
            }
        } else {
            out.push_str(&format!("Type: file\nSize: {} bytes\n", meta.len()));
            if meta.len() <= MAX_READ_BYTES
                && let Ok(bytes) = std::fs::read(&path)
                && !is_binary(&bytes)
            {
                out.push_str(&format!(
                    "Lines: {}\n",
                    String::from_utf8_lossy(&bytes).lines().count()
                ));
            }
        }
        Ok(out)
    }

    async fn shell(&self, args: &Value, timeout: Duration) -> Result<ToolOutput> {
        let command = str_arg(args, "command")?;
        let secs = args["timeout_secs"]
            .as_u64()
            .unwrap_or(DEFAULT_SHELL_SECS.max(timeout.as_secs()))
            .clamp(1, MAX_SHELL_SECS);
        if let Some(place) = &self.sandbox {
            return self.sandboxed_shell(place, command, secs).await;
        }
        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(command);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(command);
            c
        };
        cmd.current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Its own process group, so a timeout can stop everything the command started.
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd.spawn().context("couldn't start the shell")?;
        let pid = child.id();
        let output =
            match tokio::time::timeout(Duration::from_secs(secs), child.wait_with_output()).await {
                Ok(r) => r.context("the shell failed")?,
                Err(_) => {
                    if let Some(pid) = pid {
                        kill_command(pid);
                    }
                    bail!("the command didn't finish within {secs} s and was stopped")
                }
            };
        Ok(ToolOutput {
            text: shell_output(
                output.status.code(),
                (output.stdout.as_slice(), 0),
                (output.stderr.as_slice(), 0),
            ),
            is_error: !output.status.success(),
            images: Vec::new(),
            sandbox: None,
        })
    }

    /// Runs the command in the chat's sandbox container. A Docker error is returned as is:
    /// the command never runs on this computer instead.
    async fn sandboxed_shell(&self, place: &Place, command: &str, secs: u64) -> Result<ToolOutput> {
        let run = place
            .exec(&self.root, command, Duration::from_secs(secs))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if run.timed_out {
            bail!(
                "the command didn't finish within {secs} s; its container was stopped and removed"
            );
        }
        Ok(ToolOutput {
            text: shell_output(
                run.exit_code,
                (run.stdout.as_slice(), run.stdout_cut),
                (run.stderr.as_slice(), run.stderr_cut),
            ),
            is_error: run.exit_code != Some(0),
            images: Vec::new(),
            sandbox: Some(SandboxInfo {
                image: run.image,
                container: run.container,
            }),
        })
    }
}

/// The shell tool's result text: the exit code, then stdout and stderr when not empty. Each
/// stream is given with the number of bytes the sandbox dropped from its middle; that count
/// goes after the kept text, because [`keep_ends`] would cut a note placed in the middle.
fn shell_output(code: Option<i32>, stdout: (&[u8], usize), stderr: (&[u8], usize)) -> String {
    let code = code.map_or_else(|| "killed by a signal".to_owned(), |c| c.to_string());
    let mut out = format!("Exit code: {code}\n");
    for (name, (bytes, cut)) in [("stdout", stdout), ("stderr", stderr)] {
        let text = String::from_utf8_lossy(bytes);
        if !text.trim().is_empty() {
            let mut body = keep_ends(&text);
            if cut > 0 {
                body.push_str(&format!("\n[… {cut} bytes of output dropped …]"));
            }
            out.push_str(&format!("--- {name} ---\n{body}\n"));
        }
    }
    out
}

/// The shell tool's description when its commands run in the sandbox `sandbox`.
fn sandbox_shell_description(sandbox: &Sandbox) -> String {
    let cfg = sandbox.config();
    let read_only = if cfg.read_only { " (read-only)" } else { "" };
    let network = match cfg.network {
        SandboxNetwork::None => "The container has no network access.".to_owned(),
        SandboxNetwork::Bridge => "The container has network access.".to_owned(),
        SandboxNetwork::Custom => custom_network_note(&cfg.rules),
    };
    format!(
        "Run a shell command (sh) in a Linux container from the image {}, with the workspace \
         folder at /workspace{read_only}. {network} Packages you install stay for later \
         commands in this chat until the container is removed. Return its exit code, stdout \
         and stderr. Commands run without a terminal or input; use non-interactive flags.",
        sandbox.image()
    )
}

/// The shell tool's sentence about a custom sandbox network: what it may reach.
fn custom_network_note(rules: &[crate::config::SandboxNetRule]) -> String {
    use crate::config::NetProtocol;
    let allowed: Vec<String> = rules
        .iter()
        .map(|r| {
            let protocol = match r.protocol {
                NetProtocol::Tcp => "TCP",
                NetProtocol::Udp => "UDP",
                NetProtocol::Any => "any protocol",
            };
            let host = if r.host == "*" {
                "any address"
            } else {
                &r.host
            };
            if r.ports.is_empty() {
                format!("{host} ({protocol})")
            } else {
                format!("{host} port {} ({protocol})", r.ports)
            }
        })
        .collect();
    if allowed.is_empty() {
        "The container has no network access.".to_owned()
    } else {
        format!(
            "The container's network allows only outbound connections to: {}; everything else \
             is refused, and other domains don't resolve.",
            allowed.join(", ")
        )
    }
}

/// Stop a timed-out shell command and every process it started.
fn kill_command(pid: u32) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: a plain syscall. The shell leads its own process group (`process_group(0)`),
        // and that group id can't be reused while any of its processes is still alive.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    #[cfg(windows)]
    crate::mcp::stdio::kill_process_tree(pid);
}

/// Glob matching on a relative path. A pattern without `/` matches file names at any depth
/// (`*.rs`); one with `/` matches the whole relative path (`src/**/*.rs`).
struct PathMatcher {
    glob: globset::GlobMatcher,
    by_name: bool,
}

impl PathMatcher {
    fn new(pattern: &str) -> Result<Self> {
        let by_name = !pattern.contains('/');
        let glob = globset::GlobBuilder::new(pattern)
            .literal_separator(!by_name)
            .build()
            .with_context(|| format!("{pattern:?} isn't a valid glob pattern"))?
            .compile_matcher();
        Ok(Self { glob, by_name })
    }

    fn is_match(&self, rel: &str) -> bool {
        if self.by_name {
            self.glob.is_match(rel.rsplit('/').next().unwrap_or(rel))
        } else {
            self.glob.is_match(rel)
        }
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing \"{key}\""))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn usize_arg(args: &Value, key: &str) -> Result<Option<usize>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(|n| Some(n as usize))
            .with_context(|| format!("\"{key}\" must be a whole number")),
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8192)].contains(&0)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Long command output: the start and the end, which is where errors usually are.
fn keep_ends(text: &str) -> String {
    let n = text.chars().count();
    if n <= MAX_OUTPUT_CHARS {
        return text.trim_end().to_owned();
    }
    let half = MAX_OUTPUT_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(n - half).collect();
    format!(
        "{head}\n[… {} characters cut …]\n{}",
        n - 2 * half,
        tail.trim_end()
    )
}

fn shell_name() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}

fn git_branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git/HEAD")).ok()?;
    Some(head.trim().strip_prefix("ref: refs/heads/").map_or_else(
        || format!("detached at {}", &head.trim()[..head.trim().len().min(12)]),
        str::to_owned,
    ))
}

/// The spec of a workspace tool as it runs on this computer.
fn base_spec(name: &str) -> ToolSpec {
    let (description, parameters) = match name {
        READ_FILE => (
            "Read a text file in the workspace, with line numbers. Long files are read in \
             parts: use offset and limit. Images are returned as images.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the workspace."},
                    "offset": {"type": "integer", "description": "First line to read, from 1 (default 1)."},
                    "limit": {"type": "integer", "description": format!("Most lines to read (default {DEFAULT_READ_LINES}).")}
                },
                "required": ["path"]
            }),
        ),
        GLOB => (
            "Find files in the workspace by glob pattern, most recently changed first. \
             \"*.rs\" matches file names at any depth; \"src/**/*.rs\" matches paths. \
             Files ignored by .gitignore are skipped.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern, e.g. \"*.toml\" or \"src/**/*.rs\"."},
                    "path": {"type": "string", "description": "Folder to search in (default: the whole workspace)."}
                },
                "required": ["pattern"]
            }),
        ),
        GREP => (
            "Search the contents of files in the workspace with a regular expression. Returns \
             path:line: text for each matching line. Skips binary files and files ignored by \
             .gitignore.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression (Rust regex syntax)."},
                    "path": {"type": "string", "description": "File or folder to search (default: the whole workspace)."},
                    "glob": {"type": "string", "description": "Only search files matching this glob, e.g. \"*.py\"."},
                    "case_insensitive": {"type": "boolean"},
                    "max_results": {"type": "integer", "description": format!("Most matching lines to return (default {DEFAULT_GREP_RESULTS}).")}
                },
                "required": ["pattern"]
            }),
        ),
        INFO => (
            "Information about the workspace (its path, operating system, shell, git branch \
             and top-level contents), or about a file or folder in it (type, size, modified \
             time, line count, or a folder's contents).",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File or folder (default: the workspace itself)."}
                }
            }),
        ),
        WRITE_FILE => (
            "Create a file in the workspace, or replace a file's whole content. Creates \
             missing folders. Prefer edit_file for changing part of an existing file.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the workspace."},
                    "content": {"type": "string", "description": "The complete new content."}
                },
                "required": ["path", "content"]
            }),
        ),
        EDIT_FILE => (
            "Edit a file in the workspace by replacing an exact string. old_string must match \
             the file exactly (including whitespace) and be unique unless replace_all is set. \
             Read the file first.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the workspace."},
                    "old_string": {"type": "string", "description": "The exact text to replace."},
                    "new_string": {"type": "string", "description": "The text to put in its place."},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence (default false)."}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        ),
        SHELL => (
            "Run a shell command in the workspace folder (sh on macOS and Linux, cmd on \
             Windows) and return its exit code, stdout and stderr. Commands run without a \
             terminal or input; use non-interactive flags.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command line to run."},
                    "timeout_secs": {"type": "integer", "description": format!("Stop the command after this many seconds (default {DEFAULT_SHELL_SECS}, at most {MAX_SHELL_SECS}).")}
                },
                "required": ["command"]
            }),
        ),
        other => unreachable!("not a workspace tool: {other}"),
    };
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> (tempdir::Dir, Workspace) {
        let dir = tempdir::Dir::new();
        std::fs::create_dir_all(dir.0.join("src/nested")).unwrap();
        std::fs::write(
            dir.0.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.0.join("src/nested/lib.rs"),
            "pub fn add() {}\n// TODO: more\n",
        )
        .unwrap();
        std::fs::write(dir.0.join("README.md"), "# Project\nTODO: write docs\n").unwrap();
        std::fs::write(dir.0.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(dir.0.join("target")).unwrap();
        std::fs::write(dir.0.join("target/out.rs"), "// TODO generated\n").unwrap();
        let ws = Workspace::open(&dir.0).unwrap();
        (dir, ws)
    }

    /// A temporary folder removed on drop (keeps the dev-dependency list empty).
    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let p =
                    std::env::temp_dir().join(format!("cleanrstai-ws-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    async fn call(ws: &Workspace, f: &str, args: Value) -> ToolOutput {
        ws.call(f, &args, Duration::from_secs(30)).await
    }

    #[tokio::test]
    async fn reads_with_line_numbers_and_pages() {
        let (_d, ws) = temp_workspace();
        let out = call(&ws, READ_FILE, json!({"path": "src/main.rs"})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.starts_with("     1\tfn main() {\n"),
            "{}",
            out.text
        );
        let part = call(
            &ws,
            READ_FILE,
            json!({"path": "src/main.rs", "offset": 2, "limit": 1}),
        )
        .await;
        assert!(
            part.text.starts_with("     2\t    println!"),
            "{}",
            part.text
        );
        assert!(part.text.contains("offset=3"), "{}", part.text);
        let dir = call(&ws, READ_FILE, json!({"path": "src"})).await;
        assert!(
            dir.is_error && dir.text.contains("is a folder"),
            "{}",
            dir.text
        );
    }

    #[tokio::test]
    async fn refuses_paths_outside_the_workspace() {
        let (_d, ws) = temp_workspace();
        for path in ["../secret", "/etc/hosts", "src/../../x"] {
            let out = call(&ws, READ_FILE, json!({"path": path})).await;
            assert!(out.is_error, "{path}: {}", out.text);
        }
        let out = call(
            &ws,
            WRITE_FILE,
            json!({"path": "/tmp/escape.txt", "content": "x"}),
        )
        .await;
        assert!(
            out.is_error && out.text.contains("outside the workspace"),
            "{}",
            out.text
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", ws.root().join("link")).unwrap();
            let out = call(&ws, READ_FILE, json!({"path": "link/hosts"})).await;
            assert!(out.is_error && out.text.contains("outside"), "{}", out.text);
            let out = call(
                &ws,
                WRITE_FILE,
                json!({"path": "link/new.txt", "content": "x"}),
            )
            .await;
            assert!(out.is_error && out.text.contains("outside"), "{}", out.text);
        }
    }

    #[tokio::test]
    async fn globs_and_greps_honour_gitignore() {
        let (_d, ws) = temp_workspace();
        let out = call(&ws, GLOB, json!({"pattern": "*.rs"})).await;
        assert!(
            out.text.contains("src/main.rs") && out.text.contains("src/nested/lib.rs"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("target/"), "ignored: {}", out.text);
        let out = call(&ws, GLOB, json!({"pattern": "src/*.rs"})).await;
        assert_eq!(out.text.trim(), "src/main.rs");

        let out = call(
            &ws,
            GREP,
            json!({"pattern": "todo", "case_insensitive": true}),
        )
        .await;
        assert!(
            out.text.contains("README.md:2: TODO: write docs"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("src/nested/lib.rs:2: // TODO: more"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("target/"), "{}", out.text);
        let out = call(&ws, GREP, json!({"pattern": "TODO", "glob": "*.md"})).await;
        assert_eq!(out.text.lines().count(), 1, "{}", out.text);
        let out = call(&ws, GREP, json!({"pattern": "("})).await;
        assert!(out.is_error, "{}", out.text);
    }

    #[tokio::test]
    async fn writes_and_edits_files() {
        let (_d, ws) = temp_workspace();
        let out = call(
            &ws,
            WRITE_FILE,
            json!({"path": "docs/new.md", "content": "a\nb\n"}),
        )
        .await;
        assert!(out.text.starts_with("Created docs/new.md"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(ws.root().join("docs/new.md")).unwrap(),
            "a\nb\n"
        );

        let edit = |old: &str, new: &str, all: bool| json!({"path": "README.md", "old_string": old, "new_string": new, "replace_all": all});
        let out = call(&ws, EDIT_FILE, edit("write docs", "write the docs", false)).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(
            std::fs::read_to_string(ws.root().join("README.md"))
                .unwrap()
                .contains("write the docs")
        );
        let out = call(&ws, EDIT_FILE, edit("missing", "x", false)).await;
        assert!(
            out.is_error && out.text.contains("wasn't found"),
            "{}",
            out.text
        );
        std::fs::write(ws.root().join("README.md"), "x x x").unwrap();
        let out = call(&ws, EDIT_FILE, edit("x", "y", false)).await;
        assert!(
            out.is_error && out.text.contains("appears 3 times"),
            "{}",
            out.text
        );
        let out = call(&ws, EDIT_FILE, edit("x", "y", true)).await;
        assert!(out.text.contains("Replaced 3 occurrences"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(ws.root().join("README.md")).unwrap(),
            "y y y"
        );
    }

    #[tokio::test]
    async fn describes_the_workspace_and_files() {
        let (_d, ws) = temp_workspace();
        let out = call(&ws, INFO, json!({})).await;
        assert!(out.text.contains("Workspace: "), "{}", out.text);
        assert!(
            out.text.contains("  src/\n") && out.text.contains("  README.md\n"),
            "{}",
            out.text
        );
        assert!(!out.text.contains(".git/"), "{}", out.text);
        let out = call(&ws, INFO, json!({"path": "src/main.rs"})).await;
        assert!(
            out.text.contains("Type: file") && out.text.contains("Lines: 3"),
            "{}",
            out.text
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_commands_in_the_workspace() {
        let (_d, ws) = temp_workspace();
        let out = call(&ws, SHELL, json!({"command": "ls src && echo oops >&2"})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Exit code: 0"), "{}", out.text);
        assert!(
            out.text.contains("main.rs") && out.text.contains("--- stderr ---\noops"),
            "{}",
            out.text
        );
        let out = call(&ws, SHELL, json!({"command": "exit 3"})).await;
        assert!(
            out.is_error && out.text.starts_with("Exit code: 3"),
            "{}",
            out.text
        );
        let out = call(&ws, SHELL, json!({"command": "sleep 5", "timeout_secs": 1})).await;
        assert!(
            out.is_error && out.text.contains("didn't finish within 1 s"),
            "{}",
            out.text
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_stops_the_whole_pipeline() {
        let (_d, ws) = temp_workspace();
        let cmd = "(sleep 2; touch late) | cat";
        let out = call(&ws, SHELL, json!({"command": cmd, "timeout_secs": 1})).await;
        assert!(out.is_error, "{}", out.text);
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(!ws.root().join("late").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_sandboxed_output_over_the_cap_says_how_much_was_dropped() {
        use crate::sandbox::fake_docker::Fake;
        let fake = Fake::new();
        let (_d, ws) = temp_workspace();
        let ws = sandboxed(ws, &fake);
        // 4.5 MB against the 4 MiB cap (4 194 304 bytes kept): 305 696 bytes are dropped.
        let cmd = "head -c 4500000 /dev/zero | tr '\\0' a";
        let out = call(&ws, SHELL, json!({"command": cmd})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("--- stdout ---\n"), "{}", out.text);
        assert!(
            out.text.contains("\n[… 305696 bytes of output dropped …]"),
            "{}",
            out.text
        );
    }

    #[test]
    fn long_output_keeps_both_ends() {
        let text = format!("START{}END", "x".repeat(MAX_OUTPUT_CHARS * 2));
        let kept = keep_ends(&text);
        assert!(kept.starts_with("START") && kept.ends_with("END"));
        assert!(kept.contains("characters cut"));
    }

    #[cfg(unix)]
    fn sandboxed(ws: Workspace, fake: &crate::sandbox::fake_docker::Fake) -> Workspace {
        let sandbox = Sandbox::new(fake.settings());
        ws.with_sandbox(sandbox, Uuid::new_v4())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_shell_runs_in_the_container_and_reports_it() {
        use crate::sandbox::fake_docker::Fake;
        let fake = Fake::new();
        let (_d, ws) = temp_workspace();
        let ws = sandboxed(ws, &fake);
        let out = call(&ws, SHELL, json!({"command": "echo inside"})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Exit code: 0\n"), "{}", out.text);
        assert!(out.text.contains("--- stdout ---\ninside"), "{}", out.text);
        let info = out.sandbox.expect("the sandbox is reported");
        assert_eq!(info.image, "python:3.13-slim");
        assert!(info.container.starts_with("cleanrstai-"), "{info:?}");
        assert!(fake.count_prefix("exec ") >= 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_sandboxed_command_is_an_error_with_its_exit_code() {
        use crate::sandbox::fake_docker::Fake;
        let fake = Fake::new();
        let (_d, ws) = temp_workspace();
        let ws = sandboxed(ws, &fake);
        let out = call(&ws, SHELL, json!({"command": "exit 3"})).await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Exit code: 3"), "{}", out.text);
        assert!(out.sandbox.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_shell_never_runs_on_the_host_when_docker_is_down() {
        use crate::sandbox::fake_docker::Fake;
        let fake = Fake::new();
        fake.mark("down");
        let (_d, ws) = temp_workspace();
        let ws = sandboxed(ws, &fake);
        let out = call(&ws, SHELL, json!({"command": "touch host_ran"})).await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("Docker isn't running"), "{}", out.text);
        assert!(out.sandbox.is_none());
        assert!(!ws.root().join("host_ran").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_sandboxed_timeout_says_the_container_was_removed() {
        use crate::sandbox::fake_docker::Fake;
        let fake = Fake::new();
        let (_d, ws) = temp_workspace();
        let ws = sandboxed(ws, &fake);
        let out = call(&ws, SHELL, json!({"command": "sleep 5", "timeout_secs": 1})).await;
        assert!(out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("didn't finish within 1 s; its container was stopped and removed"),
            "{}",
            out.text
        );
    }

    #[test]
    fn sandboxed_shell_spec_names_the_image_and_the_container_rules() {
        let (_d, ws) = temp_workspace();
        let sandbox = Sandbox::new(crate::config::SandboxSettings::default());
        let ws = ws.with_sandbox(sandbox, Uuid::new_v4());
        let desc = ws.spec(SHELL).description;
        assert!(desc.contains("python:3.13-slim"), "{desc}");
        assert!(desc.contains("/workspace"), "{desc}");
        assert!(desc.contains("no network access"), "{desc}");
        assert!(!desc.contains("(read-only)"), "{desc}");
        assert!(
            ws.prompt_note()
                .contains("Shell commands run in a Linux container (image python:3.13-slim) with this folder at /workspace."),
            "{}",
            ws.prompt_note()
        );
    }

    #[test]
    fn sandboxed_shell_spec_follows_read_only_and_network_settings() {
        let (_d, ws) = temp_workspace();
        let cfg = crate::config::SandboxSettings {
            read_only: true,
            network: SandboxNetwork::Bridge,
            ..Default::default()
        };
        let ws = ws.with_sandbox(Sandbox::new(cfg), Uuid::new_v4());
        let desc = ws.spec(SHELL).description;
        assert!(desc.contains("/workspace (read-only)."), "{desc}");
        assert!(desc.contains("has network access"), "{desc}");
    }

    #[test]
    fn unsandboxed_shell_spec_is_unchanged() {
        let (_d, ws) = temp_workspace();
        assert_eq!(
            ws.spec(SHELL).description,
            "Run a shell command in the workspace folder (sh on macOS and Linux, cmd on \
             Windows) and return its exit code, stdout and stderr. Commands run without a \
             terminal or input; use non-interactive flags."
        );
        assert!(!ws.prompt_note().contains("Linux container"));
    }
}
