//! Settings window: providers, MCP servers and general preferences. Edits are made on a
//! draft and only take effect on Apply / Save.

use eframe::egui;

use super::theme::icon;
use super::widgets::{code_block, icon_button, kv_editor, list_editor};
use crate::approval::ApprovalMode;
use crate::commands::SlashCommand;
use crate::config::{
    AccessSettings, DEFAULT_SANDBOX_DOCKERFILE, FULL_SANDBOX_DOCKERFILE, MAX_SANDBOX_NET_RULES,
    McpServerConfig, McpTransportConfig, NetProtocol, ProviderConfig, RemoteBind, RemoteSettings,
    RemoteTls, ReviewScope, ReviewStrictness, SandboxImageSource, SandboxNetRule, SandboxNetwork,
    SandboxPull, SandboxSettings, Settings, SlashCommandSettings, Theme, TunnelMode,
    import_mcp_json,
};
use crate::hooks::{Hook, HookEvent, HookSettings, MAX_TIMEOUT_SECS};
use crate::mcp::manager::{ServerStatus, ServerView};
use crate::memory::{MAX_KB, MIN_KB, Memory, MemoryScope, MemorySettings, Scope};
use crate::ocr::OcrEngine;
use crate::remote::{RemoteStatus, TunnelStatus};
use crate::schedule::ScheduledTask;
use std::collections::HashMap;
use uuid::Uuid;

/// A change to the saved memories, made in Settings → Context & files → Memory. These take
/// effect at once, not with Apply.
pub enum MemoryChange {
    Add { text: String, scope: Scope },
    Edit { id: Uuid, text: String },
    Delete(Uuid),
    DeleteAll,
}

/// What the Memory section keeps between frames: the text fields.
#[derive(Default)]
pub struct MemoryEditor {
    /// The text of each memory being shown, until its field loses focus.
    drafts: HashMap<Uuid, String>,
    new_text: String,
    /// The project a new memory goes to, in per-project scope (`None`: global).
    new_scope: Option<Uuid>,
    confirm_all: bool,
}

/// The rows of a custom sandbox network: host, protocol, ports and a remove button, with
/// each rule's error under it.
fn net_rules_editor(ui: &mut egui::Ui, rules: &mut Vec<SandboxNetRule>) {
    ui.push_id("net-rules", |ui| {
        let mut remove = None;
        for (i, rule) in rules.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.host)
                            .hint_text("pypi.org, 10.0.0.0/8 or *")
                            .desired_width(200.0),
                    );
                    egui::ComboBox::from_id_salt("protocol")
                        .width(70.0)
                        .selected_text(protocol_label(rule.protocol))
                        .show_ui(ui, |ui| {
                            for p in [
                                NetProtocol::Tcp,
                                NetProtocol::Udp,
                                NetProtocol::Icmp,
                                NetProtocol::Any,
                            ] {
                                ui.selectable_value(&mut rule.protocol, p, protocol_label(p));
                            }
                        });
                    if rule.protocol == NetProtocol::Icmp {
                        // ICMP has no ports.
                        rule.ports.clear();
                        ui.add_enabled(
                            false,
                            egui::TextEdit::singleline(&mut rule.ports)
                                .hint_text("no ports")
                                .desired_width(110.0),
                        );
                    } else {
                        ui.add(
                            egui::TextEdit::singleline(&mut rule.ports)
                                .hint_text("all ports")
                                .desired_width(110.0),
                        );
                    }
                    if icon_button(ui, icon::CLOSE, "Remove").clicked() {
                        remove = Some(i);
                    }
                });
                let check = SandboxNetRule {
                    host: rule.host.trim().to_ascii_lowercase(),
                    ..rule.clone()
                };
                if let Err(e) = crate::sandbox::check_rule(&check) {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            });
        }
        if let Some(i) = remove {
            rules.remove(i);
        }
        if rules.len() < MAX_SANDBOX_NET_RULES
            && ui
                .small_button(format!("{}  Add rule", icon::PLUS))
                .clicked()
        {
            rules.push(SandboxNetRule {
                ports: "443".into(),
                ..SandboxNetRule::default()
            });
        }
        ui.weak(
            "Host: a domain (its subdomains too), an IPv4 address or range such as \
             192.168.1.0/24, or * for any address. Ports: such as 80, 443, 8000-8100; empty \
             allows all. ICMP allows ping and takes no ports. Everything else is refused, and \
             only the domains listed resolve.",
        );
    });
}

fn protocol_label(protocol: NetProtocol) -> &'static str {
    match protocol {
        NetProtocol::Tcp => "TCP",
        NetProtocol::Udp => "UDP",
        NetProtocol::Icmp => "ICMP",
        NetProtocol::Any => "Any",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    General,
    Providers,
    Mcp,
    Tools,
    Extensions,
    Context,
    Docker,
    Subagents,
    Background,
    Voice,
}

pub enum SettingsAction {
    Memory(MemoryChange),
    Apply(Box<Settings>),
    Close,
    FetchModels(ProviderConfig),
    DetectKind(ProviderConfig),
    Reconnect(String),
    RefreshTools(String),
    SignIn(String),
    SignOut(String),
    ExportSettings {
        include_secrets: bool,
    },
    ExportConversations,
    ImportSettings,
    ImportConversations,
    /// Check the given "Update from" setting (the draft's, so it works before saving).
    CheckForUpdates(String),
    InstallUpdate,
    /// Test Docker and the sandbox image with these settings (Settings → Docker "Test").
    CheckSandbox(SandboxSettings),
    /// Pull the image, or build the Dockerfile again (Settings → Docker "Pull now" / "Build now").
    PrepareSandbox(SandboxSettings),
    /// Run a scheduled task now, as the draft has it (Settings → Background "Run now").
    RunScheduledTask(Box<ScheduledTask>),
}

pub struct SettingsUi {
    pub draft: Settings,
    tab: Tab,
    selected_provider: usize,
    selected_server: usize,
    show_key: bool,
    show_remote_password: bool,
    show_search_key: bool,
    import_open: bool,
    import_text: String,
    import_error: Option<String>,
    search: String,
    memory_ui: MemoryEditor,
    queued: Vec<SettingsAction>,
    pub fetching_models: bool,
    pub models_error: Option<String>,
    pub detecting_kind: bool,
    pub detect_result: Option<String>,
    export_include_secrets: bool,
    /// Outcome of the last export or import: Ok(message) or Err(error).
    pub export_status: Option<Result<String, String>>,
    /// A settings file waiting for the user to choose Merge or Replace.
    pub pending_import: Option<Box<Settings>>,
    pub checking_update: bool,
    /// Outcome of the last manual update check: Ok(message) or Err(error).
    pub update_status: Option<Result<String, String>>,
    /// The last check found a release this build can install itself.
    pub update_available: bool,
    /// A Docker test, pull or build is running.
    pub sandbox_busy: bool,
    /// Outcome of the Docker test, pull or build: Ok(message) or Err(error).
    pub sandbox_status: Option<Result<String, String>>,
    /// Skills in the app's skills folder, read when the Skills & hooks tab is first shown
    /// (and again after "Refresh").
    user_skills: Option<Vec<crate::skills::Skill>>,
    /// Why the skills folder couldn't be opened.
    skills_folder_error: Option<String>,
}

pub const FIELD_WIDTH: f32 = 420.0;

/// The "Remote access" rows of the General grid (below Secrets), with the Cloudflare Tunnel
/// rows under them.
fn remote_rows(
    ui: &mut egui::Ui,
    r: &mut RemoteSettings,
    show_password: &mut bool,
    status: &RemoteUiStatus,
) {
    let remote = status.server;
    ui.label("Remote access");
    ui.checkbox(
        &mut r.enabled,
        "Serve the chat interface to browsers on other devices",
    );
    ui.end_row();
    if !r.enabled {
        return;
    }

    ui.label("Listen on");
    ui.vertical(|ui| {
        ui.radio_value(&mut r.bind, RemoteBind::Local, "This computer only");
        ui.radio_value(&mut r.bind, RemoteBind::Network, "Local network");
    });
    ui.end_row();

    ui.label("Port");
    ui.add(egui::DragValue::new(&mut r.port).range(1..=65535));
    ui.end_row();

    ui.label("Password");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut r.password)
                .password(!*show_password)
                .hint_text("needed to log in from a browser")
                .desired_width(FIELD_WIDTH - 100.0),
        );
        ui.checkbox(show_password, "Show");
    });
    ui.end_row();

    ui.label("HTTPS");
    let tls_label = |t: RemoteTls| match t {
        RemoteTls::SelfSigned => "Self-signed certificate",
        RemoteTls::Files => "Certificate files",
        RemoteTls::Off => "Off (plain HTTP)",
    };
    egui::ComboBox::from_id_salt("remote-tls")
        .selected_text(tls_label(r.tls))
        .width(220.0)
        .show_ui(ui, |ui| {
            for tls in [RemoteTls::SelfSigned, RemoteTls::Files, RemoteTls::Off] {
                ui.selectable_value(&mut r.tls, tls, tls_label(tls));
            }
        });
    ui.end_row();

    if r.tls == RemoteTls::Files {
        for (label, path, title) in [
            (
                "Certificate file",
                &mut r.cert_path,
                "Choose the certificate file",
            ),
            ("Key file", &mut r.key_path, "Choose the private key file"),
        ] {
            ui.label(label);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(path)
                        .hint_text("PEM file")
                        .desired_width(FIELD_WIDTH - 100.0),
                );
                if ui.button("Browse…").clicked()
                    && let Some(file) = rfd::FileDialog::new()
                        .set_title(title)
                        .add_filter("PEM", &["pem", "crt", "cer", "key"])
                        .pick_file()
                {
                    *path = file.display().to_string();
                }
            });
            ui.end_row();
        }
    }

    ui.label("Status");
    ui.vertical(|ui| {
        match remote {
            RemoteStatus::Running { urls, .. } => {
                for url in urls {
                    ui.horizontal(|ui| {
                        ui.label(format!("Running at {url}"));
                        if super::widgets::icon_button(ui, icon::COPY, "Copy address").clicked() {
                            ui.ctx().copy_text(url.clone());
                        }
                    });
                }
            }
            RemoteStatus::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            RemoteStatus::Off if r.password.is_empty() => {
                ui.label("Set a password to start the server");
            }
            RemoteStatus::Off => {
                ui.label("Stopped");
            }
        }
        if status.server_dirty {
            ui.weak("Apply to restart the server with these settings.");
        }
        match r.tls {
            RemoteTls::Off => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Plain HTTP: the password and chats cross the network unencrypted. Use it only on a network you trust.",
                );
            }
            RemoteTls::SelfSigned => {
                ui.weak("Browsers warn about a self-signed certificate the first time; check the address and continue.");
            }
            RemoteTls::Files => {}
        }
    });
    ui.end_row();

    tunnel_rows(ui, r, show_password, status);
}

/// What the Remote access rows show besides the draft: the applied server and tunnel state,
/// and whether the draft differs from what is applied.
pub struct RemoteUiStatus<'a> {
    pub server: &'a RemoteStatus,
    pub tunnel: &'a TunnelStatus,
    pub server_dirty: bool,
    pub tunnel_dirty: bool,
    /// The last reason Cloudflare Access refused a tunnel request, or a failed key fetch.
    pub access_error: Option<&'a str>,
}

/// The "Cloudflare Tunnel" rows, under the Remote access status.
fn tunnel_rows(
    ui: &mut egui::Ui,
    r: &mut RemoteSettings,
    show_password: &mut bool,
    status: &RemoteUiStatus,
) {
    ui.label("Cloudflare Tunnel");
    ui.checkbox(
        &mut r.tunnel.enabled,
        "Reach this computer from outside the network through a Cloudflare Tunnel",
    );
    ui.end_row();
    if !r.tunnel.enabled {
        return;
    }
    let service = crate::remote::tunnel_service_url(r);
    let t = &mut r.tunnel;

    ui.label("Tunnel type");
    ui.vertical(|ui| {
        ui.radio_value(
            &mut t.mode,
            TunnelMode::Quick,
            "Quick tunnel: a random trycloudflare.com address, no account needed",
        );
        ui.radio_value(
            &mut t.mode,
            TunnelMode::Named,
            "Named tunnel: your own domain, with a Cloudflare account",
        );
    });
    ui.end_row();

    let secret = |ui: &mut egui::Ui, value: &mut String, hint: &str, show: &mut bool| {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(value)
                    .password(!*show)
                    .hint_text(hint)
                    .desired_width(FIELD_WIDTH - 100.0),
            );
            ui.checkbox(show, "Show");
        });
    };
    match t.mode {
        TunnelMode::Quick => {
            ui.label("Allowed emails");
            ui.add(
                egui::TextEdit::singleline(&mut t.allowed_emails)
                    .hint_text("optional: me@example.com, *@example.org")
                    .desired_width(FIELD_WIDTH - 100.0),
            )
            .on_hover_text(
                "Cloudflare asks visitors for a one-time PIN sent to one of these addresses \
                 before the login page. Empty: anyone with the address reaches the login page.",
            );
            ui.end_row();
        }
        TunnelMode::Named => {
            ui.label("Tunnel token");
            secret(
                ui,
                &mut t.token,
                "from the tunnel's page in the Cloudflare dashboard",
                show_password,
            );
            ui.end_row();

            ui.label("Public hostname");
            ui.add(
                egui::TextEdit::singleline(&mut t.hostname)
                    .hint_text("optional: chat.example.com")
                    .desired_width(FIELD_WIDTH - 100.0),
            )
            .on_hover_text(
                "The hostname of the tunnel's route in the Cloudflare dashboard. Optional: the \
                 app also learns it from cloudflared once the tunnel connects.",
            );
            ui.end_row();
        }
    }

    ui.label("cloudflared");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut t.cloudflared)
                .hint_text("found on PATH")
                .desired_width(FIELD_WIDTH - 100.0),
        );
        if ui.button("Browse…").clicked()
            && let Some(file) = rfd::FileDialog::new()
                .set_title("Choose cloudflared")
                .pick_file()
        {
            t.cloudflared = file.display().to_string();
        }
    });
    ui.end_row();

    ui.label("Tunnel status");
    ui.vertical(|ui| {
        let warn = ui.visuals().warn_fg_color;
        let error = ui.visuals().error_fg_color;
        match status.tunnel {
            TunnelStatus::Off => {
                ui.label("Stopped");
            }
            TunnelStatus::Connecting(last_error) => {
                ui.label("Connecting to Cloudflare…");
                if let Some(e) = last_error {
                    ui.colored_label(warn, e);
                }
            }
            TunnelStatus::Connected { connections, hosts } => {
                for host in hosts {
                    let url = format!("https://{host}/");
                    ui.horizontal(|ui| {
                        ui.label(format!("Online at {url}"));
                        if super::widgets::icon_button(ui, icon::COPY, "Copy address").clicked() {
                            ui.ctx().copy_text(url.clone());
                        }
                    });
                }
                if hosts.is_empty() {
                    ui.colored_label(
                        warn,
                        "Connected, but the tunnel has no public hostname yet: add a route \
                         for it in the Cloudflare dashboard.",
                    );
                }
                let plural = if *connections == 1 { "" } else { "s" };
                ui.weak(format!("{connections} connection{plural} to Cloudflare"));
            }
            TunnelStatus::Failed { error: e, retrying } => {
                ui.colored_label(error, e);
                if *retrying {
                    ui.weak("Trying again shortly.");
                }
            }
        }
        if status.tunnel_dirty {
            ui.weak("Apply to start the tunnel with these settings.");
        }
        if r.password.trim().chars().count() < crate::remote::MIN_TUNNEL_PASSWORD_CHARS {
            ui.colored_label(
                warn,
                format!(
                    "Use a password of at least {} characters: the tunnel puts the server on \
                     the internet.",
                    crate::remote::MIN_TUNNEL_PASSWORD_CHARS
                ),
            );
        }
        match r.tunnel.mode {
            TunnelMode::Quick => {
                ui.weak(
                    "The address changes each time the tunnel starts. Quick tunnels are for \
                     trying things out: Cloudflare gives them no uptime guarantee, and the \
                     browser checks for new messages every few seconds instead of live.",
                );
            }
            TunnelMode::Named => {
                let tls_note = if r.tls == RemoteTls::Off {
                    ""
                } else {
                    ", with No TLS Verify on"
                };
                ui.weak(format!(
                    "In the Cloudflare dashboard, give the tunnel a published application \
                     route to {service}{tls_note}."
                ));
            }
        }
    });
    ui.end_row();
    access_rows(ui, &mut r.tunnel.access, r.tunnel.mode, status.access_error);
}

/// The "Cloudflare Access" rows of the tunnel: a login gate in front of a named tunnel's
/// hostname, set up in the Cloudflare dashboard (see `remote::access`).
fn access_rows(
    ui: &mut egui::Ui,
    access: &mut AccessSettings,
    mode: TunnelMode,
    error: Option<&str>,
) {
    ui.label("Cloudflare Access");
    if mode == TunnelMode::Quick {
        ui.weak("Only for named tunnels: a quick tunnel's address can't be put behind an Access application.");
        ui.end_row();
        return;
    }
    ui.checkbox(
        &mut access.enabled,
        "Require Cloudflare Access in front of the tunnel's hostname",
    );
    ui.end_row();
    if !access.enabled {
        return;
    }

    ui.label("Team domain");
    ui.add(
        egui::TextEdit::singleline(&mut access.team_domain)
            .hint_text("myteam.cloudflareaccess.com")
            .desired_width(FIELD_WIDTH - 100.0),
    );
    ui.end_row();

    ui.label("Audience (AUD) tag");
    ui.add(
        egui::TextEdit::singleline(&mut access.audience)
            .hint_text("from the Access application's overview")
            .desired_width(FIELD_WIDTH - 100.0),
    );
    ui.end_row();

    ui.label("Sign in with Access");
    ui.checkbox(
        &mut access.skip_password,
        "A valid Access login also signs the browser in, so the app password is not asked",
    );
    ui.end_row();

    ui.label("Access status");
    ui.vertical(|ui| {
        let warn = ui.visuals().warn_fg_color;
        let team = crate::config::team_host(&access.team_domain);
        if !crate::remote::valid_team_domain(&team) {
            ui.colored_label(
                warn,
                "Enter the team domain, e.g. myteam.cloudflareaccess.com.",
            );
        }
        if access.audience.trim().is_empty() {
            ui.colored_label(warn, "Enter the application's audience (AUD) tag.");
        }
        match error {
            Some(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            None => {
                ui.weak("No Access errors so far.");
            }
        }
        weak_note(
            ui,
            "In the Cloudflare Zero Trust dashboard, add a self-hosted application for the \
             tunnel's public hostname, with a policy for who may sign in. Copy its Application \
             Audience (AUD) tag here, and your team domain (the name.cloudflareaccess.com \
             address your team signs in to). Requests that skip Access are refused.",
        );
    });
    ui.end_row();
}

/// Weak text in a grid cell, wrapped at the width of the fields.
/// A combo box for the provider a voice feature uses: None is the active provider.
fn provider_choice(
    ui: &mut egui::Ui,
    id: &str,
    choice: &mut Option<String>,
    providers: &[(String, String)],
) {
    let shown = choice
        .as_ref()
        .and_then(|id| providers.iter().find(|(pid, _)| pid == id))
        .map_or("Active provider", |(_, name)| name.as_str())
        .to_owned();
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(choice, None, "Active provider");
            for (pid, name) in providers {
                ui.selectable_value(choice, Some(pid.clone()), name);
            }
        });
}

fn weak_note(ui: &mut egui::Ui, text: &str) {
    ui.scope(|ui| {
        ui.set_max_width(FIELD_WIDTH);
        ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
    });
}

/// The web search tool's service and options (Settings → Built-in Tools).
fn web_search_rows(
    ui: &mut egui::Ui,
    s: &mut crate::config::WebSearchSettings,
    show_key: &mut bool,
) {
    use crate::config::{MAX_NATIVE_SEARCH_USES, NativeSearch, SearchBackend};
    ui.strong("Web search");
    // The provider's own search replaces the service below for providers that have one.
    let native = s.native == NativeSearch::PreferProvider;
    egui::Grid::new("web-search-grid")
        .num_columns(2)
        .spacing([12.0, 10.0])
        .show(ui, |ui| {
            ui.label("Search with");
            egui::ComboBox::from_id_salt("search-native")
                .selected_text(s.native.label())
                .width(FIELD_WIDTH)
                .show_ui(ui, |ui| {
                    for n in NativeSearch::ALL {
                        ui.selectable_value(&mut s.native, n, n.label());
                    }
                });
            ui.end_row();
            if native {
                ui.label("");
                weak_note(
                    ui,
                    "Anthropic (Messages API), OpenAI (Responses API) and OpenRouter search \
                     on the provider's side and cite the pages they used. Other providers \
                     keep the service below. The service's settings are not used then.",
                );
                ui.end_row();
                ui.label("Searches");
                ui.add(
                    egui::DragValue::new(&mut s.native_max_uses)
                        .range(1..=MAX_NATIVE_SEARCH_USES)
                        .suffix(" per reply"),
                )
                .on_hover_text("The most searches the model may run for one reply");
                ui.end_row();
            }

            ui.add_enabled_ui(!native, |ui| {
                ui.label("Service");
            });
            ui.add_enabled_ui(!native, |ui| {
                egui::ComboBox::from_id_salt("search-backend")
                    .selected_text(s.backend.label())
                    .width(FIELD_WIDTH)
                    .show_ui(ui, |ui| {
                        for b in SearchBackend::ALL {
                            ui.selectable_value(&mut s.backend, b, b.label());
                        }
                    });
            });
            ui.end_row();

            match s.backend {
                SearchBackend::DuckDuckGo => {
                    ui.label("");
                    weak_note(
                        ui,
                        "No account or key. DuckDuckGo may ask for a human check when it sees \
                         many searches; choose another service then.",
                    );
                    ui.end_row();
                }
                SearchBackend::Searxng => {
                    ui.label("Instance");
                    ui.add(
                        egui::TextEdit::singleline(&mut s.searxng_url)
                            .hint_text("https://searx.example.org or http://localhost:8888")
                            .desired_width(FIELD_WIDTH),
                    )
                    .on_hover_text(
                        "The instance must allow JSON results: add `json` to `search.formats` \
                         in its settings.yml",
                    );
                    ui.end_row();
                }
                SearchBackend::Brave | SearchBackend::Tavily | SearchBackend::Kagi => {
                    ui.label("API key");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut s.api_key)
                                .password(!*show_key)
                                .desired_width(FIELD_WIDTH - 60.0),
                        )
                        .on_hover_text("Kept in the system keychain when that is on (General)");
                        ui.checkbox(show_key, "Show");
                    });
                    ui.end_row();
                }
            }

            ui.label("Results");
            ui.add_enabled(
                !native,
                egui::DragValue::new(&mut s.max_results)
                    .range(1..=crate::config::MAX_SEARCH_RESULTS)
                    .suffix(" per search"),
            )
            .on_hover_text("The model may ask for more, up to 20");
            ui.end_row();

            ui.label("Options");
            ui.vertical(|ui| {
                ui.add_enabled(
                    !native && !matches!(s.backend, SearchBackend::Tavily | SearchBackend::Kagi),
                    egui::Checkbox::new(&mut s.safe_search, "Safe search"),
                );
                ui.checkbox(&mut s.ask, "Ask before each search")
                    .on_hover_text(
                        "The query leaves this computer and can contain parts of the chat. \
                         Subagents and auto-approved calls never ask.",
                    );
                if native {
                    weak_note(
                        ui,
                        "Doesn't apply to the provider's own search: it runs on the provider's \
                         side, so it can't ask before each search.",
                    );
                }
                ui.checkbox(
                    &mut s.subagents,
                    "Subagents may search (when they may use the built-in tools)",
                );
            });
            ui.end_row();
        });
}

const PRESETS: &[(&str, &str)] = &[
    ("OpenAI", "https://api.openai.com/v1"),
    ("OpenRouter", "https://openrouter.ai/api/v1"),
    ("Groq", "https://api.groq.com/openai/v1"),
    ("Mistral", "https://api.mistral.ai/v1"),
    ("DeepSeek", "https://api.deepseek.com/v1"),
    ("Together", "https://api.together.xyz/v1"),
    (
        "Gemini (OpenAI compat)",
        "https://generativelanguage.googleapis.com/v1beta/openai",
    ),
    ("Ollama (local)", "http://localhost:11434/v1"),
    ("LM Studio (local)", "http://localhost:1234/v1"),
    ("llama.cpp / vLLM (local)", "http://localhost:8000/v1"),
];

pub fn status_color(ui: &egui::Ui, status: Option<&ServerStatus>) -> egui::Color32 {
    match status {
        Some(ServerStatus::Connected) => egui::Color32::from_rgb(60, 180, 90),
        Some(ServerStatus::Connecting) => egui::Color32::from_rgb(220, 170, 40),
        Some(ServerStatus::Error(_)) => ui.visuals().error_fg_color,
        _ => ui.visuals().weak_text_color(),
    }
}

impl SettingsUi {
    pub fn new(settings: &Settings, tab: Tab) -> Self {
        Self {
            selected_provider: settings.active_provider,
            draft: settings.clone(),
            tab,
            selected_server: 0,
            show_key: false,
            show_remote_password: false,
            show_search_key: false,
            import_open: false,
            import_text: String::new(),
            import_error: None,
            search: String::new(),
            queued: Vec::new(),
            fetching_models: false,
            models_error: None,
            detecting_kind: false,
            detect_result: None,
            export_include_secrets: false,
            export_status: None,
            pending_import: None,
            checking_update: false,
            update_status: None,
            update_available: false,
            sandbox_busy: false,
            sandbox_status: None,
            user_skills: None,
            skills_folder_error: None,
            memory_ui: MemoryEditor::default(),
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// Run an action on the next frame as if its button had been pressed (used by the
    /// command palette for export/import).
    pub fn queue(&mut self, action: SettingsAction) {
        self.queued.push(action);
    }

    pub fn take_queued(&mut self) -> Vec<SettingsAction> {
        std::mem::take(&mut self.queued)
    }

    /// Render the settings page into `ui` (it takes over the main area while open).
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        servers: &[ServerView],
        remote: &RemoteStatus,
        tunnel: &TunnelStatus,
        access_error: Option<&str>,
        chats: &[(Uuid, String)],
        memories: &[Memory],
        projects: &[(Uuid, String)],
    ) -> Vec<SettingsAction> {
        let mut actions = Vec::new();
        let dark = ui.visuals().dark_mode;

        egui::Panel::left("settings-nav")
            .resizable(false)
            .exact_size(220.0)
            .show(ui, |ui| {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.heading("Settings");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if super::widgets::icon_button(ui, icon::CLOSE, "Close settings (Esc)")
                            .clicked()
                        {
                            actions.push(SettingsAction::Close);
                        }
                    });
                });
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("search settings")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                if self.search.trim().is_empty() {
                    for (tab, label) in [
                        (Tab::General, "General"),
                        (Tab::Providers, "Providers"),
                        (Tab::Mcp, "MCP servers"),
                        (Tab::Tools, "Built-in tools"),
                        (Tab::Extensions, "Skills & hooks"),
                        (Tab::Context, "Context & files"),
                        (Tab::Docker, "Docker sandbox"),
                        (Tab::Subagents, "Subagents"),
                        (Tab::Background, "Background"),
                        (Tab::Voice, "Voice"),
                    ] {
                        if ui
                            .add(
                                egui::Button::selectable(self.tab == tab, label)
                                    .right_text("")
                                    .min_size(egui::vec2(ui.available_width(), 30.0)),
                            )
                            .clicked()
                        {
                            self.tab = tab;
                        }
                    }
                } else {
                    let hits = search_settings(&self.search);
                    if hits.is_empty() {
                        ui.weak("No matching settings");
                    }
                    for (tab, section, label) in hits {
                        let resp = ui.add(
                            egui::Button::selectable(self.tab == tab, label)
                                .right_text(egui::RichText::new(section).small().weak())
                                .min_size(egui::vec2(ui.available_width(), 28.0)),
                        );
                        if resp.clicked() {
                            self.tab = tab;
                            self.search.clear();
                        }
                    }
                }
            });

        egui::Panel::bottom("settings-buttons")
            .show_separator_line(true)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let dirty = &self.draft != live;
                    if ui
                        .add_enabled(dirty, super::theme::primary_button("Save & Close", dark))
                        .clicked()
                    {
                        actions.push(SettingsAction::Apply(Box::new(self.draft.clone())));
                        actions.push(SettingsAction::Close);
                    }
                    if ui.add_enabled(dirty, egui::Button::new("Apply")).clicked() {
                        actions.push(SettingsAction::Apply(Box::new(self.draft.clone())));
                    }
                    if ui
                        .button(if dirty { "Discard changes" } else { "Close" })
                        .clicked()
                    {
                        actions.push(SettingsAction::Close);
                    }
                    if dirty {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("{} Unsaved changes", icon::WARNING),
                        );
                    }
                });
                ui.add_space(4.0);
            });

        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.heading(match self.tab {
                Tab::Providers => "Providers",
                Tab::Mcp => "MCP Servers",
                Tab::Tools => "Built-in Tools",
                Tab::Extensions => "Skills & hooks",
                Tab::Context => "Context & files",
                Tab::Docker => "Docker sandbox",
                Tab::Subagents => "Subagents",
                Tab::Background => "Background",
                Tab::Voice => "Voice",
                Tab::General => "General",
            });
            ui.weak(match self.tab {
                Tab::Providers => "OpenAI-compatible endpoints the app can talk to.",
                Tab::Mcp => "Model Context Protocol servers that give the model tools, resources and prompts.",
                Tab::Tools => "Tools that ship with the app, so they work without an MCP server.",
                Tab::Extensions => "Saved prompts you type as /name, skills the model loads when a task matches, and hooks: commands that run around tool calls and when a reply is done.",
                Tab::Context => "Context compaction, project instructions from the workspace, projects (instructions and files for groups of chats), memory (facts kept across chats), diffs and undo for file edits, code review, and OCR for scanned PDFs.",
                Tab::Docker => "Run the workspace shell tool in a Docker container instead of on this computer.",
                Tab::Subagents => "Let the model delegate independent tasks to agents that work in parallel and report back.",
                Tab::Background => "Replies that keep running while you work in another chat: notifications, how many run at once, and a time limit.",
                Tab::Voice => "Dictate into the message box, and read replies aloud: the speech providers, models and voices.",
                Tab::General => "Appearance, behaviour, secrets and data.",
            });
            ui.add_space(8.0);
            match self.tab {
                Tab::Providers => self.providers_tab(ui, &mut actions),
                Tab::Mcp => self.mcp_tab(ui, live, servers, &mut actions),
                Tab::Tools => self.tools_tab(ui),
                Tab::Extensions => self.extensions_tab(ui),
                Tab::Context => self.context_tab(ui, memories, projects, &mut actions),
                Tab::Docker => self.docker_tab(ui, &mut actions),
                Tab::Subagents => self.subagents_tab(ui),
                Tab::Background => self.background_tab(ui, chats, &mut actions),
                Tab::Voice => self.voice_tab(ui),
                Tab::General => {
                    self.general_tab(ui, live, remote, (tunnel, access_error), &mut actions)
                }
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && &self.draft == live {
            actions.push(SettingsAction::Close);
        }
        actions
    }

    fn providers_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        let draft = &mut self.draft;
        self.selected_provider = self.selected_provider.min(draft.providers.len() - 1);

        egui::Panel::left("provider-list")
            .resizable(false)
            .exact_size(190.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height(ui.available_height() - 36.0)
                    .show(ui, |ui| {
                        for (i, p) in draft.providers.iter().enumerate() {
                            let label = if i == draft.active_provider {
                                format!("● {}", p.name)
                            } else {
                                format!("   {}", p.name)
                            };
                            if ui
                                .selectable_label(i == self.selected_provider, label)
                                .clicked()
                            {
                                self.selected_provider = i;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui
                        .button(icon::PLUS)
                        .on_hover_text("Add provider")
                        .clicked()
                    {
                        draft.providers.push(ProviderConfig {
                            name: format!("Provider {}", draft.providers.len() + 1),
                            ..Default::default()
                        });
                        self.selected_provider = draft.providers.len() - 1;
                    }
                    if ui.button("⧉").on_hover_text("Duplicate").clicked() {
                        let mut copy = draft.providers[self.selected_provider].clone();
                        copy.id = uuid::Uuid::new_v4().to_string();
                        copy.name.push_str(" copy");
                        draft.providers.push(copy);
                        self.selected_provider = draft.providers.len() - 1;
                    }
                    if ui
                        .add_enabled(draft.providers.len() > 1, egui::Button::new("－"))
                        .on_hover_text("Remove provider")
                        .clicked()
                    {
                        draft.providers.remove(self.selected_provider);
                        if draft.active_provider >= self.selected_provider
                            && draft.active_provider > 0
                        {
                            draft.active_provider -= 1;
                        }
                        self.selected_provider = self.selected_provider.saturating_sub(1);
                    }
                });
            });

        let idx = self.selected_provider;
        let is_active = draft.active_provider == idx;
        let p = &mut draft.providers[idx];
        let mut make_active = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("provider-grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .striped(false)
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut p.name);
                        ui.end_row();

                        ui.label("Base URL");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut p.base_url)
                                    .hint_text("https://api.openai.com/v1")
                                    .desired_width(FIELD_WIDTH - 100.0),
                            );
                            egui::ComboBox::from_id_salt("preset")
                                .selected_text("Presets")
                                .width(90.0)
                                .show_ui(ui, |ui| {
                                    for (name, url) in PRESETS {
                                        if ui.selectable_label(false, *name).clicked() {
                                            p.base_url = (*url).to_owned();
                                        }
                                    }
                                });
                        });
                        ui.end_row();

                        ui.label("API key");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut p.api_key)
                                    .password(!self.show_key)
                                    .hint_text("sk-… (leave empty for local servers)")
                                    .desired_width(FIELD_WIDTH - 100.0),
                            );
                            ui.checkbox(&mut self.show_key, "Show");
                        });
                        ui.end_row();

                        ui.label("Model");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut p.model).desired_width(220.0));
                            if !p.models.is_empty() {
                                egui::ComboBox::from_id_salt("model-pick")
                                    .selected_text(format!("{} models", p.models.len()))
                                    .width(110.0)
                                    .height(400.0)
                                    .show_ui(ui, |ui| {
                                        for m in &p.models {
                                            ui.selectable_value(&mut p.model, m.clone(), m);
                                        }
                                    });
                            }
                            let label = if self.fetching_models {
                                "Fetching…"
                            } else {
                                "Fetch models"
                            };
                            if ui
                                .add_enabled(!self.fetching_models, egui::Button::new(label))
                                .clicked()
                            {
                                self.fetching_models = true;
                                self.models_error = None;
                                actions.push(SettingsAction::FetchModels(p.clone()));
                            }
                        });
                        ui.end_row();
                        if let Some(err) = &self.models_error {
                            ui.label("");
                            ui.colored_label(ui.visuals().error_fg_color, err);
                            ui.end_row();
                        }

                        ui.label("Context size").on_hover_text(
                            "Used when the server doesn't report the model's context size; for \
                             compaction and the context-use figure",
                        );
                        super::widgets::inherited_value(
                            ui,
                            &mut p.context_tokens,
                            None,
                            8192,
                            1024..=10_000_000,
                            256.0,
                        );
                        ui.end_row();

                        ui.label("API");
                        ui.vertical(|ui| {
                            egui::ComboBox::from_id_salt("provider-api")
                                .selected_text(p.api.label())
                                .width(260.0)
                                .show_ui(ui, |ui| {
                                    for api in crate::config::ProviderApi::ALL {
                                        ui.selectable_value(&mut p.api, api, api.label())
                                            .on_hover_text(api.description());
                                    }
                                });
                            ui.weak(p.api.description());
                        });
                        ui.end_row();

                        if p.api == crate::config::ProviderApi::Anthropic {
                            ui.label("Prompt caching");
                            ui.checkbox(
                                &mut p.prompt_caching,
                                "Cache the system prompt, tools and conversation so far",
                            )
                            .on_hover_text(
                                "Later requests read the cached part at a tenth of the price; \
                                 writing it costs a quarter more once.",
                            );
                            ui.end_row();
                        }

                        ui.label("Server type");
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("provider-kind")
                                    .selected_text(p.kind.label())
                                    .width(220.0)
                                    .show_ui(ui, |ui| {
                                        for kind in crate::config::ProviderKind::ALL {
                                            ui.selectable_value(&mut p.kind, kind, kind.label())
                                                .on_hover_text(kind.description());
                                        }
                                    });
                                let label = if self.detecting_kind { "Detecting…" } else { "Detect" };
                                if ui
                                    .add_enabled(!self.detecting_kind, egui::Button::new(label))
                                    .on_hover_text("Probe the server to find out what it is")
                                    .clicked()
                                {
                                    self.detecting_kind = true;
                                    self.detect_result = None;
                                    actions.push(SettingsAction::DetectKind(p.clone()));
                                }
                            });
                            ui.weak(p.kind.description());
                            if let Some(r) = &self.detect_result {
                                ui.weak(r);
                            }
                        });
                        ui.end_row();

                        ui.label("Token usage");
                        ui.checkbox(
                            &mut p.request_usage,
                            "Request usage in streamed responses (stream_options)",
                        )
                        .on_hover_text(
                            "Needed for exact token counts and speeds. Turn off if the server rejects the request.",
                        );
                        ui.end_row();

                        ui.label("Streaming");
                        ui.checkbox(
                            &mut p.stream,
                            "Stream responses (disable if the server doesn't support SSE)",
                        );
                        ui.end_row();

                        ui.label("Timeout");
                        ui.add(
                            egui::DragValue::new(&mut p.timeout_secs)
                                .range(10..=3600)
                                .suffix(" s"),
                        );
                        ui.end_row();

                        ui.label("Extra headers");
                        ui.vertical(|ui| {
                            kv_editor(
                                ui,
                                "provider-headers",
                                &mut p.extra_headers,
                                "Header",
                                "Value",
                                false,
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(12.0);
                ui.label(egui::RichText::new("Reasoning and sampling").strong());
                ui.weak(
                    "Unset options leave the server's default. A chat can change them under \
                     the message box.",
                );
                ui.add_space(4.0);
                crate::ui::widgets::model_options_grid(
                    ui,
                    "provider-options",
                    &mut p.options,
                    &Default::default(),
                    p.api,
                );
                let model = p.model.trim().to_owned();
                if !model.is_empty() {
                    ui.add_space(6.0);
                    let mut own = p.model_options.get(&model).cloned().unwrap_or_default();
                    let title = if own.is_empty() {
                        format!("Only for {model}")
                    } else {
                        format!("Only for {model} (changed)")
                    };
                    egui::CollapsingHeader::new(title)
                        .id_salt(("model-options", idx))
                        .show(ui, |ui| {
                            ui.weak("These win over the options above for this model.");
                            crate::ui::widgets::model_options_grid(
                                ui,
                                "model-options-grid",
                                &mut own,
                                &p.options,
                                p.api,
                            );
                        });
                    if own.is_empty() {
                        p.model_options.remove(&model);
                    } else {
                        p.model_options.insert(model, own);
                    }
                }

                ui.add_space(8.0);
                if is_active {
                    ui.weak("● This is the active provider.");
                } else if ui.button("Use this provider").clicked() {
                    make_active = true;
                }
            });
        if make_active {
            draft.active_provider = idx;
        }
    }

    fn mcp_tab(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        servers: &[ServerView],
        actions: &mut Vec<SettingsAction>,
    ) {
        let draft = &mut self.draft;
        let view_of = |id: &str| servers.iter().find(|v| v.id == id);

        egui::Panel::left("server-list")
            .resizable(false)
            .exact_size(200.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height(ui.available_height() - 64.0)
                    .show(ui, |ui| {
                        if draft.mcp_servers.is_empty() {
                            ui.weak("No servers yet.");
                        }
                        for (i, s) in draft.mcp_servers.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let color = if s.enabled {
                                    status_color(ui, view_of(&s.id).map(|v| &v.status))
                                } else {
                                    ui.visuals().weak_text_color()
                                };
                                ui.colored_label(color, "●");
                                if ui
                                    .selectable_label(i == self.selected_server, &s.name)
                                    .clicked()
                                {
                                    self.selected_server = i;
                                    self.import_open = false;
                                }
                            });
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button(format!("{}  Add", icon::PLUS)).clicked() {
                        draft.mcp_servers.push(McpServerConfig::default());
                        self.selected_server = draft.mcp_servers.len() - 1;
                        self.import_open = false;
                    }
                    if ui
                        .add_enabled(
                            !draft.mcp_servers.is_empty(),
                            egui::Button::new("－ Remove"),
                        )
                        .clicked()
                    {
                        draft.mcp_servers.remove(self.selected_server);
                        self.selected_server = self.selected_server.saturating_sub(1);
                    }
                });
                if ui
                    .button("Import JSON…")
                    .on_hover_text("Paste a Claude Desktop / Cursor style mcpServers config")
                    .clicked()
                {
                    self.import_open = !self.import_open;
                }
            });

        if self.import_open {
            ui.heading("Import servers");
            ui.label("Paste JSON in the common `mcpServers` format:");
            ui.add(
                egui::TextEdit::multiline(&mut self.import_text)
                    .code_editor()
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .hint_text("{\n  \"mcpServers\": {\n    \"filesystem\": {\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/Users/me\"]\n    },\n    \"remote\": { \"url\": \"https://example.com/mcp\" }\n  }\n}"),
            );
            ui.horizontal(|ui| {
                if ui.button("Import").clicked() {
                    match import_mcp_json(&self.import_text) {
                        Ok(list) => {
                            let n = list.len();
                            draft.mcp_servers.extend(list);
                            self.selected_server = draft.mcp_servers.len().saturating_sub(n);
                            self.import_open = false;
                            self.import_text.clear();
                            self.import_error = None;
                        }
                        Err(e) => self.import_error = Some(format!("{e:#}")),
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.import_open = false;
                }
            });
            if let Some(e) = &self.import_error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            return;
        }

        if draft.mcp_servers.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label("MCP servers give the model tools: files, databases, web, and more.");
                ui.weak("Add one manually, or import an existing mcpServers JSON config.");
            });
            return;
        }
        self.selected_server = self.selected_server.min(draft.mcp_servers.len() - 1);
        let s = &mut draft.mcp_servers[self.selected_server];
        let view = view_of(&s.id);
        let live_cfg = live.mcp_servers.iter().find(|c| c.id == s.id);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("mcp-grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut s.name)
                            .on_hover_text("Used to prefix tool names shown to the model");
                        ui.end_row();

                        ui.label("Enabled");
                        ui.checkbox(&mut s.enabled, "");
                        ui.end_row();

                        ui.label("Transport");
                        ui.horizontal(|ui| {
                            let is_stdio = matches!(s.transport, McpTransportConfig::Stdio { .. });
                            if ui.radio(is_stdio, "stdio (local process)").clicked() && !is_stdio {
                                s.transport = McpTransportConfig::default();
                            }
                            if ui.radio(!is_stdio, "Streamable HTTP").clicked() && is_stdio {
                                s.transport = McpTransportConfig::Http {
                                    url: String::new(),
                                    headers: Vec::new(),
                                    oauth_client_id: String::new(),
                                };
                            }
                        });
                        ui.end_row();

                        match &mut s.transport {
                            McpTransportConfig::Stdio {
                                command,
                                args,
                                env,
                                cwd,
                            } => {
                                ui.label("Command");
                                ui.add(
                                    egui::TextEdit::singleline(command)
                                        .hint_text("npx, uvx, docker, /path/to/server")
                                        .desired_width(FIELD_WIDTH),
                                );
                                ui.end_row();
                                ui.label("Arguments");
                                ui.vertical(|ui| list_editor(ui, "args", args, "argument"));
                                ui.end_row();
                                ui.label("Environment");
                                ui.vertical(|ui| kv_editor(ui, "env", env, "NAME", "value", false));
                                ui.end_row();
                                ui.label("Working dir");
                                ui.add(
                                    egui::TextEdit::singleline(cwd)
                                        .hint_text("(optional)")
                                        .desired_width(FIELD_WIDTH),
                                );
                                ui.end_row();
                            }
                            McpTransportConfig::Http {
                                url,
                                headers,
                                oauth_client_id,
                            } => {
                                ui.label("URL");
                                ui.add(
                                    egui::TextEdit::singleline(url)
                                        .hint_text("https://example.com/mcp")
                                        .desired_width(FIELD_WIDTH),
                                );
                                ui.end_row();
                                ui.label("Headers");
                                ui.vertical(|ui| {
                                    kv_editor(
                                        ui,
                                        "headers",
                                        headers,
                                        "Authorization",
                                        "Bearer …",
                                        true,
                                    )
                                });
                                ui.end_row();
                                ui.label("OAuth client ID");
                                ui.add(
                                    egui::TextEdit::singleline(oauth_client_id)
                                        .hint_text("(optional) only if the server can't register CleanRSTAI automatically")
                                        .desired_width(FIELD_WIDTH),
                                )
                                .on_hover_text(
                                    "Servers that require sign-in usually register CleanRSTAI automatically. Set this only if the server's documentation gives you a client ID.",
                                );
                                ui.end_row();
                            }
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                let unsaved = live_cfg.is_none_or(|l| l.needs_reconnect(s));
                ui.horizontal(|ui| {
                    ui.strong("Status:");
                    match (live_cfg, view) {
                        _ if unsaved => {
                            ui.weak("Apply settings to connect with this configuration.");
                        }
                        (_, Some(v)) => {
                            let color = status_color(ui, Some(&v.status));
                            let text = match &v.status {
                                ServerStatus::Disabled => "Disabled".to_owned(),
                                ServerStatus::Connecting => "Connecting…".to_owned(),
                                ServerStatus::Connected => format!(
                                    "Connected · {} tools · {}",
                                    v.tools.len(),
                                    v.server_info
                                ),
                                ServerStatus::NeedsSignIn => "Sign-in required".to_owned(),
                                ServerStatus::SigningIn => {
                                    "Waiting for you to sign in in your browser…".to_owned()
                                }
                                ServerStatus::Error(_) => "Error".to_owned(),
                            };
                            ui.colored_label(color, text);
                            if matches!(
                                v.status,
                                ServerStatus::Connecting | ServerStatus::SigningIn
                            ) {
                                ui.spinner();
                            }
                        }
                        _ => {
                            ui.weak("Not connected");
                        }
                    }
                    if !unsaved && s.enabled {
                        let is_http = matches!(s.transport, McpTransportConfig::Http { .. });
                        let status = view.map(|v| &v.status);
                        if is_http
                            && matches!(
                                status,
                                Some(ServerStatus::NeedsSignIn | ServerStatus::Error(_))
                            )
                            && ui
                                .button(format!("{}  Sign in…", icon::SIGN_IN))
                                .on_hover_text("Opens your browser to sign in to this server")
                                .clicked()
                        {
                            actions.push(SettingsAction::SignIn(s.id.clone()));
                        }
                        if is_http
                            && view.is_some_and(|v| v.signed_in)
                            && ui.button("Sign out").clicked()
                        {
                            actions.push(SettingsAction::SignOut(s.id.clone()));
                        }
                        if ui.button("Reconnect").clicked() {
                            actions.push(SettingsAction::Reconnect(s.id.clone()));
                        }
                        if view.is_some_and(|v| v.status == ServerStatus::Connected)
                            && ui.button("Refresh").clicked()
                        {
                            actions.push(SettingsAction::RefreshTools(s.id.clone()));
                        }
                    }
                });

                if let Some(v) = view.filter(|_| !unsaved) {
                    if let ServerStatus::Error(e) = &v.status {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    if !v.tools.is_empty() {
                        ui.add_space(6.0);
                        ui.strong("Tools");
                        ui.weak("Unchecked tools are not offered to the model.");
                        for t in &v.tools {
                            let mut on = !s.disabled_tools.contains(&t.name);
                            let resp = ui.checkbox(&mut on, &t.name);
                            let resp = if t.description.is_empty() {
                                resp
                            } else {
                                resp.on_hover_text(&t.description)
                            };
                            if resp.changed() {
                                if on {
                                    s.disabled_tools.retain(|n| n != &t.name);
                                } else {
                                    s.disabled_tools.push(t.name.clone());
                                }
                            }
                        }
                    }
                    if !v.resources.is_empty() {
                        egui::CollapsingHeader::new(format!("Resources ({})", v.resources.len()))
                            .id_salt(("resources", &s.id))
                            .show(ui, |ui| {
                                ui.weak(format!("Attach these to a message from the {} menu next to the input.", icon::BOOKS));
                                for r in &v.resources {
                                    ui.label(&r.name).on_hover_text(if r.description.is_empty() {
                                        r.uri.clone()
                                    } else {
                                        format!("{}\n{}", r.description, r.uri)
                                    });
                                }
                            });
                    }
                    if !v.prompts.is_empty() {
                        egui::CollapsingHeader::new(format!("Prompts ({})", v.prompts.len()))
                            .id_salt(("prompts", &s.id))
                            .show(ui, |ui| {
                                ui.weak(format!("Insert these from the {} menu next to the input.", icon::BOOKS));
                                for p in &v.prompts {
                                    let args: Vec<&str> =
                                        p.arguments.iter().map(|a| a.name.as_str()).collect();
                                    let label = if args.is_empty() {
                                        p.name.clone()
                                    } else {
                                        format!("{} ({})", p.name, args.join(", "))
                                    };
                                    ui.label(label).on_hover_text(&p.description);
                                }
                            });
                    }
                    if !v.stderr.is_empty() {
                        egui::CollapsingHeader::new("Server log (stderr)")
                            .id_salt(("stderr", &s.id))
                            .show(ui, |ui| {
                                let start = v.stderr.len().saturating_sub(60);
                                code_block(ui, &v.stderr[start..].join("\n"));
                            });
                    }
                }
            });
    }

    fn tools_tab(&mut self, ui: &mut egui::Ui) {
        let b = &mut self.draft.general.builtin_tools;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    "Built-in tools that run on this computer don't ask for approval; web fetch \
                     reaches the internet, so it asks first, like MCP tools. These are the \
                     defaults for new chats; switch them for one chat from the tools menu next \
                     to the message box.",
                );
                ui.add_space(10.0);
                for (on, ic, name, desc) in [
                    (
                        &mut b.time,
                        icon::CLOCK,
                        "Time & date",
                        "The current date, time, weekday and time zone, and the time at other UTC offsets.",
                    ),
                    (
                        &mut b.calculator,
                        icon::CALCULATOR,
                        "Calculator",
                        "Exact arithmetic and functions (powers, roots, logarithms, trigonometry, rounding, min/max/avg) instead of mental maths.",
                    ),
                    (
                        &mut b.tasks,
                        icon::LIST_CHECKS,
                        "Task list",
                        "For multi-step requests the model breaks the work into actionable tasks, shows them as a checklist in the chat, and ticks them off as it goes.",
                    ),
                    (
                        &mut b.fetch,
                        icon::GLOBE,
                        "Web fetch",
                        "Reads a web URL: pages as text with their links, plain text and JSON as they are, images as images. Asks for approval before each fetch.",
                    ),
                    (
                        &mut b.web_search,
                        icon::SEARCH,
                        "Web search",
                        "Searches the web with the service below (or the provider's own search, when Search with says so) and lists the results (title, address, snippet), so the model can read the useful pages with web fetch.",
                    ),
                    (
                        &mut b.workspace,
                        icon::FOLDER,
                        "Workspace tools",
                        "When a chat has a working folder (set below the message box): read_file, file_glob_search, grep_search, get_info, git_status and git_diff run freely inside it; write_file, edit_file and exec_shell_command ask for approval. Paths can't leave the folder.",
                    ),
                ] {
                    ui.horizontal(|ui| {
                        ui.checkbox(on, egui::RichText::new(format!("{ic}  {name}")).strong());
                    });
                    ui.indent(name, |ui| {
                        ui.weak(desc);
                    });
                    ui.add_space(8.0);
                }
                ui.add_enabled_ui(b.workspace, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut b.git_commit,
                            egui::RichText::new(format!("{}  Commit (git_commit)", icon::CHECK_SQUARE))
                                .strong(),
                        );
                    });
                    ui.indent("git-commit", |ui| {
                        ui.weak(
                            "Lets the model commit the workspace's changes to its git repository as \
                             one local commit. Every commit asks for approval, even in Auto in \
                             workspace mode. Nothing is pushed, and git hooks don't run. Off by \
                             default.",
                        );
                        ui.add_enabled_ui(b.git_commit, |ui| {
                            ui.checkbox(&mut b.allow_signing, "Allow signed commits");
                            ui.weak(
                                "Off: commits are never signed. On: a repository set to sign its \
                                 commits signs them, and gpg may ask for a passphrase.",
                            );
                        });
                    });
                    ui.add_space(8.0);
                });
                ui.add_enabled_ui(b.fetch, |ui| {
                    ui.checkbox(
                        &mut b.fetch_private_networks,
                        "Let web fetch reach this computer and private networks",
                    );
                    ui.indent("fetch-private", |ui| {
                        ui.weak(
                            "Off: localhost, private (10.x, 192.168.x, …) and link-local addresses, \
                             such as cloud metadata endpoints, are refused, also after a redirect. \
                             Subagents and auto-approved calls fetch without asking, so leave \
                             this off unless you need it.",
                        );
                    });
                });
                ui.add_space(12.0);
                ui.add_enabled_ui(b.web_search, |ui| {
                    web_search_rows(ui, &mut b.search, &mut self.show_search_key);
                });
                ui.add_space(8.0);
                ui.weak(format!(
                    "{}  The subagent tool has its own page: Subagents. Context compaction, \
                     project instructions and undo for file edits are on Context & files. \
                     Slash commands and skills are on Skills & hooks.",
                    icon::ROBOT
                ));
            });
    }

    fn extensions_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                slash_command_rows(ui, &mut self.draft.general.slash_commands);
                ui.add_space(16.0);
                self.skill_rows(ui);
                ui.add_space(16.0);
                hook_rows(ui, &mut self.draft.general.hooks);
            });
    }

    /// Skills: the on/off switch, the workspace switch, the size limit, the skills folder and
    /// the skills it holds, each with its own switch.
    fn skill_rows(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.draft.general.skills;
        ui.label(egui::RichText::new(format!("{}  Skills", icon::BOOKS)).strong());
        ui.indent("skills-help", |ui| {
            ui.weak(
                "A skill is a folder with a SKILL.md file: instructions the model reads with \
                 its load_skill tool when a task matches the skill's description. Only the \
                 names and descriptions are sent with each message; the text is sent when the \
                 model loads it.",
            );
        });
        ui.add_space(4.0);
        ui.checkbox(&mut s.enabled, "Offer skills to the model");
        ui.add_enabled_ui(s.enabled, |ui| {
            ui.checkbox(
                &mut s.workspace_skills,
                "Load skills from the workspace's .cleanrstai/skills folder",
            )
            .on_hover_text(
                "Each .cleanrstai/skills/<name>/SKILL.md in a chat's working folder is a skill. \
                 It replaces a skill from your skills folder with the same name.",
            );
            ui.horizontal(|ui| {
                ui.label("Size limit per load");
                ui.add(
                    egui::DragValue::new(&mut s.max_kb)
                        .range(1..=1024)
                        .suffix(" KB"),
                )
                .on_hover_text("Text beyond this is left out when the model loads a skill");
            });
        });
        ui.add_space(8.0);

        let folder = crate::skills::user_root();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Open skills folder", icon::FOLDER))
                .on_hover_text("Create the folder if it's missing and show it")
                .clicked()
            {
                self.skills_folder_error = match std::fs::create_dir_all(&folder)
                    .map_err(|e| e.to_string())
                    .and_then(|()| open::that_detached(&folder).map_err(|e| e.to_string()))
                {
                    Ok(()) => None,
                    Err(e) => Some(format!("Couldn't open {}: {e}", folder.display())),
                };
                self.user_skills = None;
            }
            if ui.button("Refresh").clicked() {
                self.user_skills = None;
            }
        });
        ui.weak(format!("Your skills folder: {}", folder.display()));
        if let Some(e) = &self.skills_folder_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.add_space(6.0);

        let found = self
            .user_skills
            .get_or_insert_with(|| {
                // Listed without the disabled names, so switched-off skills can be switched on.
                let all = crate::config::SkillSettings {
                    disabled: Vec::new(),
                    ..self.draft.general.skills.clone()
                };
                crate::skills::discover(&[(folder.clone(), crate::skills::SkillSource::User)], &all)
            })
            .clone();
        if found.is_empty() {
            ui.weak(
                "No skills yet. Add a folder with a SKILL.md file to the skills folder above, \
                 then press Refresh.",
            );
        }
        let disabled = &mut self.draft.general.skills.disabled;
        for skill in &found {
            ui.push_id(("skill", &skill.dir), |ui| {
                let mut on = !disabled.contains(&skill.name);
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut on, egui::RichText::new(&skill.name).strong())
                        .changed()
                    {
                        if on {
                            disabled.retain(|n| *n != skill.name);
                        } else if !disabled.contains(&skill.name) {
                            disabled.push(skill.name.clone());
                        }
                    }
                    ui.weak(skill.source.label());
                });
                ui.indent("skill-desc", |ui| {
                    if skill.description.is_empty() {
                        ui.weak("No description");
                    } else {
                        ui.weak(skill.description.as_str());
                    }
                });
            });
            ui.add_space(4.0);
        }
    }

    fn context_tab(
        &mut self,
        ui: &mut egui::Ui,
        memories: &[Memory],
        projects: &[(Uuid, String)],
        actions: &mut Vec<SettingsAction>,
    ) {
        let providers: Vec<(String, String, Vec<String>)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.models.clone()))
            .collect();
        let g = &mut self.draft.general;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    "These are the defaults for new chats; switch each one for one chat under \
                     Chat features in the tools menu next to the message box.",
                );
                ui.add_space(10.0);

                // ---- compaction
                let c = &mut g.compaction;
                ui.checkbox(
                    &mut c.enabled,
                    egui::RichText::new(format!(
                        "{}  Compact long chats automatically",
                        icon::STACK
                    ))
                    .strong(),
                );
                ui.indent("compaction-help", |ui| {
                    ui.weak(
                        "When the next request would fill most of the model's context, the older \
                         messages are summarised by a model and only the summary and the latest \
                         messages are sent from then on. The chat keeps every message, with a \
                         marker where the summary starts. \"Compact now\" in the tools menu and \
                         the command palette works even when this is off.",
                    );
                });
                ui.add_space(6.0);
                egui::Grid::new("compaction-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Compact at");
                        ui.add(
                            egui::Slider::new(&mut c.threshold_percent, 50..=95)
                                .suffix("% of the context"),
                        );
                        ui.end_row();

                        ui.label("Keep word for word");
                        ui.add(
                            egui::DragValue::new(&mut c.keep_recent)
                                .range(2..=50)
                                .suffix(" latest messages"),
                        )
                        .on_hover_text(
                            "Tool results stay with the call that asked for them, so a few more \
                             may be kept",
                        );
                        ui.end_row();

                        ui.label("Context size");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut c.context_tokens)
                                    .range(0..=10_000_000)
                                    .speed(256.0)
                                    .suffix(" tokens"),
                            );
                            ui.weak(if c.context_tokens == 0 {
                                "0: only when the server reports its size"
                            } else {
                                "used when the server doesn't report one"
                            });
                        })
                        .response
                        .on_hover_text(
                            "llama.cpp, LiteLLM, Ollama, LM Studio, vLLM and OpenRouter report \
                             the context size; for other servers set the model's size here",
                        );
                        ui.end_row();

                        ui.label("Summary by");
                        let current = providers
                            .iter()
                            .find(|(id, ..)| *id == c.provider_id)
                            .map_or("The chat's provider".to_owned(), |(_, n, _)| n.clone());
                        egui::ComboBox::from_id_salt("compaction-provider")
                            .selected_text(current)
                            .width(FIELD_WIDTH)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut c.provider_id,
                                    String::new(),
                                    "The chat's provider",
                                );
                                for (id, name, _) in &providers {
                                    ui.selectable_value(&mut c.provider_id, id.clone(), name);
                                }
                            });
                        ui.end_row();

                        ui.label("Summary model");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut c.model)
                                    .hint_text("the provider's selected model")
                                    .desired_width(FIELD_WIDTH - 40.0),
                            );
                            let models = providers
                                .iter()
                                .find(|(id, ..)| *id == c.provider_id)
                                .map(|(.., m)| m.clone())
                                .unwrap_or_default();
                            if !models.is_empty() {
                                ui.menu_button(icon::CARET_DOWN, |ui| {
                                    ui.set_max_height(320.0);
                                    egui::ScrollArea::vertical().show(ui, |ui| {
                                        for m in models {
                                            if ui.button(&m).clicked() {
                                                c.model = m;
                                                ui.close();
                                            }
                                        }
                                    });
                                });
                            }
                        });
                        ui.end_row();

                        ui.label("Older tool results");
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut c.trim_tool_results,
                                "Shorten long results of older tool calls in what is sent",
                            )
                            .on_hover_text(
                                "A cheaper step before a summary: results older than the kept \
                                 messages are cut to the length below. The chat keeps them whole.",
                            );
                            ui.add_enabled(
                                c.trim_tool_results,
                                egui::DragValue::new(&mut c.trim_chars)
                                    .range(200..=100_000)
                                    .speed(50.0)
                                    .suffix(" characters"),
                            );
                        });
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("Summary instructions");
                    if c.prompt != crate::config::DEFAULT_COMPACTION_PROMPT
                        && ui.small_button("Reset").clicked()
                    {
                        c.prompt = crate::config::DEFAULT_COMPACTION_PROMPT.into();
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut c.prompt)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(16.0);

                // ---- project instructions
                let ins = &mut g.instructions;
                ui.checkbox(
                    &mut ins.enabled,
                    egui::RichText::new(format!(
                        "{}  Read project instructions from the workspace",
                        icon::FILE
                    ))
                    .strong(),
                );
                ui.indent("instructions-help", |ui| {
                    ui.weak(
                        "When a chat has a working folder, files such as AGENTS.md (used by Codex \
                         and others) or CLAUDE.md in it are added to the system prompt, so the \
                         model follows the project's own conventions. The tools menu shows which \
                         files the latest request read.",
                    );
                });
                ui.add_space(6.0);
                egui::Grid::new("instructions-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("File names");
                        let mut names = ins.file_names.join(", ");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut names)
                                    .hint_text("AGENTS.md, CLAUDE.md")
                                    .desired_width(FIELD_WIDTH),
                            )
                            .on_hover_text("Looked for in this order, separated by commas")
                            .changed()
                        {
                            // Empty names stay while typing (so a comma can be typed);
                            // saving drops them.
                            ins.file_names =
                                names.split(',').map(|n| n.trim().to_owned()).collect();
                        }
                        ui.end_row();

                        ui.label("Which files");
                        ui.vertical(|ui| {
                            ui.radio_value(&mut ins.all_files, false, "The first one found");
                            ui.radio_value(&mut ins.all_files, true, "Every one found, in order");
                        });
                        ui.end_row();

                        ui.label("Parent folders");
                        ui.checkbox(
                            &mut ins.parent_folders,
                            "Also read them from the folders above the workspace",
                        )
                        .on_hover_text(
                            "Outermost first, so the workspace's own file reads as the most \
                             specific",
                        );
                        ui.end_row();

                        ui.label("Size limit");
                        ui.add(
                            egui::DragValue::new(&mut ins.max_kb)
                                .range(1..=1024)
                                .suffix(" KB"),
                        )
                        .on_hover_text("Text beyond this, in all files together, is left out");
                        ui.end_row();
                    });
                ui.add_space(16.0);

                // ---- projects
                let pr = &mut g.projects;
                ui.checkbox(
                    &mut pr.enabled,
                    egui::RichText::new(format!("{}  Projects", icon::FOLDER)).strong(),
                );
                ui.indent("projects-help", |ui| {
                    ui.weak(
                        "Group chats into projects in the sidebar. A project's instructions and \
                         files are added to the system prompt of each of its chats, and its \
                         folder, provider, model, tool switches and permission mode are what a \
                         new chat in it starts with. Off hides projects and stops adding their \
                         context. Each chat can switch the project context off too.",
                    );
                });
                ui.add_space(6.0);
                egui::Grid::new("projects-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Files size limit");
                        ui.add(
                            egui::DragValue::new(&mut pr.max_files_kb)
                                .range(
                                    crate::projects::MIN_FILES_KB..=crate::projects::MAX_FILES_KB,
                                )
                                .suffix(" KB"),
                        )
                        .on_hover_text(
                            "The text of a project's files sent with each request, in all. \
                             Longer files are cut.",
                        );
                        ui.end_row();

                        ui.label("New projects: folder");
                        ui.horizontal(|ui| {
                            let shown = pr
                                .default_workspace
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| "None".into());
                            ui.label(shown);
                            if ui.small_button("Choose…").clicked()
                                && let Some(dir) = rfd::FileDialog::new()
                                    .set_title("Choose a default working folder")
                                    .pick_folder()
                            {
                                pr.default_workspace = Some(dir);
                            }
                            if pr.default_workspace.is_some() && ui.small_button("Clear").clicked()
                            {
                                pr.default_workspace = None;
                            }
                        });
                        ui.end_row();

                        ui.label("New projects: provider");
                        let selected = match &pr.default_provider {
                            None => "Settings' active provider".to_owned(),
                            Some(id) => providers.iter().find(|(pid, ..)| pid == id).map_or(
                                "A provider that no longer exists".into(),
                                |(_, name, _)| name.clone(),
                            ),
                        };
                        egui::ComboBox::from_id_salt("projects-default-provider")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut pr.default_provider,
                                    None,
                                    "Settings' active provider",
                                );
                                for (id, name, _) in &providers {
                                    ui.selectable_value(
                                        &mut pr.default_provider,
                                        Some(id.clone()),
                                        name,
                                    );
                                }
                            });
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("New projects: instructions");
                    if !pr.default_instructions.is_empty() && ui.small_button("Clear").clicked() {
                        pr.default_instructions.clear();
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut pr.default_instructions)
                        .hint_text("Nothing: a new project starts empty")
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(16.0);

                // ---- memory
                memory_section(
                    ui,
                    &mut g.memory,
                    &mut self.memory_ui,
                    memories,
                    projects,
                    actions,
                );
                ui.add_space(16.0);

                // ---- file edits
                let fe = &mut g.file_edits;
                ui.label(
                    egui::RichText::new(format!("{}  Workspace file edits", icon::UNDO)).strong(),
                );
                ui.add_space(4.0);
                ui.checkbox(
                    &mut fe.diff_preview,
                    "Show the change as a diff on the approval prompt and in the tool card",
                );
                ui.checkbox(
                    &mut fe.checkpoints,
                    "Keep checkpoints so a turn's write_file and edit_file changes can be undone",
                )
                .on_hover_text(
                    "Each edit keeps the file's previous content in the chat. \"Undo file edits\" \
                     under the turn's reply puts the files back, newest first; a file that \
                     changed again since is left alone. Shell commands aren't undone.",
                );
                ui.add_enabled_ui(fe.checkpoints, |ui| {
                    egui::Grid::new("file-edits-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Largest file");
                            ui.add(
                                egui::DragValue::new(&mut fe.max_file_kb)
                                    .range(16..=65_536)
                                    .speed(16.0)
                                    .suffix(" KB"),
                            )
                            .on_hover_text("Bigger files are changed without a checkpoint");
                            ui.end_row();

                            ui.label("Keep for");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::DragValue::new(&mut fe.keep_turns)
                                        .range(0..=1000)
                                        .suffix(" turns"),
                                );
                                ui.weak("0 keeps them all");
                            });
                            ui.end_row();
                        });
                });
                ui.add_space(16.0);

                // ---- code review
                let r = &mut g.review;
                ui.checkbox(
                    &mut r.enabled,
                    egui::RichText::new(format!("{}  Code review", icon::CODE)).strong(),
                )
                .on_hover_text("Adds Review changes to the tools menu and the command palette");
                ui.indent("review-help", |ui| {
                    ui.weak(
                        "Review changes sends the working folder's git diff to a model with the \
                         instructions below, as a new message in the chat. The diff leaves your \
                         computer for the provider chosen here.",
                    );
                });
                ui.add_enabled_ui(r.enabled, |ui| {
                    ui.add_space(6.0);
                    ui.label("Changes to review");
                    ui.radio_value(
                        &mut r.scope,
                        ReviewScope::Uncommitted,
                        "Uncommitted changes (staged, unstaged and new files)",
                    );
                    ui.radio_value(
                        &mut r.scope,
                        ReviewScope::Branch,
                        "This branch against its base",
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("How strict");
                        ui.radio_value(&mut r.strictness, ReviewStrictness::Lenient, "Lenient")
                            .on_hover_text("Only bugs, data loss and security issues");
                        ui.radio_value(&mut r.strictness, ReviewStrictness::Normal, "Normal")
                            .on_hover_text("The review instructions below, as written");
                        ui.radio_value(&mut r.strictness, ReviewStrictness::Strict, "Strict")
                            .on_hover_text(
                                "Also style, naming, readability, missing tests and \
                                 documentation, and risky assumptions",
                            );
                    });
                    ui.add_space(6.0);
                    egui::Grid::new("review-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Base branch");
                            ui.add(
                                egui::TextEdit::singleline(&mut r.base_branch)
                                    .hint_text("auto: origin/HEAD, main or master")
                                    .desired_width(FIELD_WIDTH),
                            )
                            .on_hover_text("Used by \"This branch against its base\"");
                            ui.end_row();

                            ui.label("Review by");
                            let current = providers
                                .iter()
                                .find(|(id, ..)| *id == r.provider_id)
                                .map_or("The chat's provider".to_owned(), |(_, n, _)| n.clone());
                            egui::ComboBox::from_id_salt("review-provider")
                                .selected_text(current)
                                .width(FIELD_WIDTH)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut r.provider_id,
                                        String::new(),
                                        "The chat's provider",
                                    );
                                    for (id, name, _) in &providers {
                                        ui.selectable_value(&mut r.provider_id, id.clone(), name);
                                    }
                                });
                            ui.end_row();

                            ui.label("Review model");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut r.model)
                                        .hint_text("the provider's selected model")
                                        .desired_width(FIELD_WIDTH - 40.0),
                                );
                                let models = providers
                                    .iter()
                                    .find(|(id, ..)| *id == r.provider_id)
                                    .map(|(.., m)| m.clone())
                                    .unwrap_or_default();
                                if !models.is_empty() {
                                    ui.menu_button(icon::CARET_DOWN, |ui| {
                                        ui.set_max_height(320.0);
                                        egui::ScrollArea::vertical().show(ui, |ui| {
                                            for m in models {
                                                if ui.button(&m).clicked() {
                                                    r.model = m;
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                }
                            });
                            ui.end_row();

                            ui.label("Largest diff");
                            ui.add(
                                egui::DragValue::new(&mut r.max_diff_kb)
                                    .range(16..=4096)
                                    .suffix(" KB"),
                            )
                            .on_hover_text(
                                "A longer diff is cut to this size, and the model is told so",
                            );
                            ui.end_row();
                        });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.strong("Review instructions");
                        if r.prompt != crate::config::DEFAULT_REVIEW_PROMPT
                            && ui.small_button("Reset").clicked()
                        {
                            r.prompt = crate::config::DEFAULT_REVIEW_PROMPT.into();
                        }
                    });
                    ui.add(
                        egui::TextEdit::multiline(&mut r.prompt)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                });

                ui.add_space(16.0);

                // ---- OCR for scanned PDFs
                let o = &mut g.ocr;
                ui.checkbox(
                    &mut o.enabled,
                    egui::RichText::new(format!("{}  Read scanned PDFs (OCR)", icon::FILE))
                        .strong(),
                );
                ui.indent("ocr-help", |ui| {
                    ui.weak(
                        "A PDF with no text layer (a scan or a photo of pages) has its page \
                         images read. A vision model reads each page, and the page image is sent \
                         to the provider chosen here; Tesseract runs on this computer. Pages past \
                         the limit are not read. Without OCR, such a PDF is attached with a note.",
                    );
                });
                ui.add_enabled_ui(o.enabled, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Read with");
                        ui.radio_value(&mut o.engine, OcrEngine::VisionModel, "A vision model");
                        ui.radio_value(&mut o.engine, OcrEngine::Tesseract, "Tesseract");
                    });
                    ui.add_space(6.0);
                    egui::Grid::new("ocr-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            if o.engine == OcrEngine::VisionModel {
                                ui.label("Vision model by");
                                let current = o
                                    .provider
                                    .as_ref()
                                    .and_then(|id| providers.iter().find(|(pid, ..)| pid == id))
                                    .map_or("The chat's provider".to_owned(), |(_, n, _)| {
                                        n.clone()
                                    });
                                egui::ComboBox::from_id_salt("ocr-provider")
                                    .selected_text(current)
                                    .width(FIELD_WIDTH)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut o.provider,
                                            None,
                                            "The chat's provider",
                                        );
                                        for (id, name, _) in &providers {
                                            ui.selectable_value(
                                                &mut o.provider,
                                                Some(id.clone()),
                                                name,
                                            );
                                        }
                                    });
                                ui.end_row();
                            } else {
                                ui.label("Tesseract program");
                                ui.add(
                                    egui::TextEdit::singleline(&mut o.tesseract_path)
                                        .hint_text("tesseract")
                                        .desired_width(FIELD_WIDTH),
                                )
                                .on_hover_text("A name on PATH, or the full path of the program");
                                ui.end_row();

                                ui.label("Languages");
                                ui.add(
                                    egui::TextEdit::singleline(&mut o.languages)
                                        .hint_text("eng")
                                        .desired_width(FIELD_WIDTH),
                                )
                                .on_hover_text(
                                    "Tesseract language codes joined with +, e.g. eng+deu; \
                                     each must be installed with Tesseract",
                                );
                                ui.end_row();
                            }

                            ui.label("Pages per PDF");
                            ui.add(
                                egui::DragValue::new(&mut o.max_pages)
                                    .range(1..=200)
                                    .suffix(" pages"),
                            )
                            .on_hover_text(
                                "Pages past this are not read, and the model is told so",
                            );
                            ui.end_row();
                        });
                    if o.engine == OcrEngine::VisionModel {
                        ui.add_space(4.0);
                        weak_note(
                            ui,
                            "The model must accept images. Pages are sent one at a time, so a \
                             long scan takes a while.",
                        );
                    }
                });
            });
    }

    fn docker_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        let busy = self.sandbox_busy;
        let sb = &mut self.draft.sandbox;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("docker-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Sandbox");
                        ui.checkbox(&mut sb.enabled, "Run shell commands in a Docker container");
                        ui.end_row();

                        ui.label("Image");
                        ui.vertical(|ui| {
                            ui.radio_value(
                                &mut sb.source,
                                SandboxImageSource::Image,
                                "Docker image",
                            );
                            ui.radio_value(
                                &mut sb.source,
                                SandboxImageSource::Dockerfile,
                                "Custom Dockerfile",
                            );
                        });
                        ui.end_row();

                        match sb.source {
                            SandboxImageSource::Image => {
                                ui.label("Image name");
                                ui.add(
                                    egui::TextEdit::singleline(&mut sb.image)
                                        .hint_text("python:3.13-slim")
                                        .desired_width(FIELD_WIDTH),
                                );
                                ui.end_row();

                                ui.label("Get the image");
                                ui.vertical(|ui| {
                                    ui.radio_value(
                                        &mut sb.pull,
                                        SandboxPull::Missing,
                                        "Pull when missing",
                                    );
                                    ui.radio_value(
                                        &mut sb.pull,
                                        SandboxPull::Always,
                                        "Pull every time the app starts",
                                    );
                                    ui.radio_value(
                                        &mut sb.pull,
                                        SandboxPull::Never,
                                        "Only use an image already on this computer",
                                    );
                                });
                                ui.end_row();
                            }
                            SandboxImageSource::Dockerfile => {
                                ui.label("Dockerfile");
                                ui.vertical(|ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut sb.dockerfile)
                                            .code_editor()
                                            .desired_rows(10)
                                            .desired_width(FIELD_WIDTH),
                                    );
                                    ui.weak(format!(
                                        "Built by the app as {} when it changes. There is no \
                                         build context, so COPY and ADD of local files don't work.",
                                        crate::sandbox::image_ref(sb)
                                    ));
                                    ui.horizontal(|ui| {
                                        if ui
                                            .button("Small example")
                                            .on_hover_text("python:3.13-slim with git and curl")
                                            .clicked()
                                        {
                                            sb.dockerfile = DEFAULT_SANDBOX_DOCKERFILE.into();
                                        }
                                        if ui
                                            .button("Full example")
                                            .on_hover_text(
                                                "Ubuntu with Python, Node.js, build tools and \
                                                 network tools (ping, nmap, dig, traceroute); \
                                                 Python data, document, PDF, image, chart \
                                                 and NLP packages; about 3 GB",
                                            )
                                            .clicked()
                                        {
                                            sb.dockerfile = FULL_SANDBOX_DOCKERFILE.into();
                                        }
                                    });
                                });
                                ui.end_row();
                            }
                        }

                        ui.label("Network");
                        ui.vertical(|ui| {
                            ui.radio_value(
                                &mut sb.network,
                                SandboxNetwork::None,
                                "None (no network)",
                            );
                            ui.radio_value(
                                &mut sb.network,
                                SandboxNetwork::Bridge,
                                "Normal network (can reach this computer, your local network and the internet)",
                            );
                            ui.radio_value(
                                &mut sb.network,
                                SandboxNetwork::Custom,
                                "Only the hosts and ports below",
                            );
                        });
                        ui.end_row();

                        if sb.network == SandboxNetwork::Custom {
                            ui.label("Allowed outbound");
                            ui.vertical(|ui| net_rules_editor(ui, &mut sb.rules));
                            ui.end_row();
                        }

                        ui.label("CPUs");
                        ui.add(
                            egui::DragValue::new(&mut sb.cpus)
                                .range(0.25..=64.0)
                                .speed(0.25),
                        );
                        ui.end_row();

                        ui.label("Memory");
                        ui.add(
                            egui::DragValue::new(&mut sb.memory_mb)
                                .range(256..=262_144)
                                .speed(64)
                                .suffix(" MB"),
                        );
                        ui.end_row();

                        ui.label("Processes");
                        ui.add(
                            egui::DragValue::new(&mut sb.pids)
                                .range(32..=65_536)
                                .speed(16),
                        );
                        ui.end_row();

                        ui.label("Workspace folder");
                        ui.checkbox(
                            &mut sb.read_only,
                            "Mount read-only (commands can't change files)",
                        );
                        ui.end_row();

                        ui.label("docker");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut sb.docker)
                                    .hint_text("found on PATH")
                                    .desired_width(FIELD_WIDTH - 100.0),
                            );
                            if ui.button("Browse…").clicked()
                                && let Some(file) = rfd::FileDialog::new()
                                    .set_title("Choose the docker program")
                                    .pick_file()
                            {
                                sb.docker = file.display().to_string();
                            }
                        });
                        ui.end_row();

                        ui.label("Status");
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                if ui.add_enabled(!busy, egui::Button::new("Test")).clicked() {
                                    self.sandbox_status = None;
                                    actions.push(SettingsAction::CheckSandbox(sb.normalized()));
                                }
                                let prepare = match sb.source {
                                    SandboxImageSource::Image => "Pull now",
                                    SandboxImageSource::Dockerfile => "Build now",
                                };
                                if ui.add_enabled(!busy, egui::Button::new(prepare)).clicked() {
                                    self.sandbox_status = None;
                                    actions.push(SettingsAction::PrepareSandbox(sb.normalized()));
                                }
                                if busy {
                                    ui.spinner();
                                }
                            });
                            match &self.sandbox_status {
                                Some(Ok(m)) => {
                                    ui.weak(m);
                                }
                                Some(Err(e)) => {
                                    ui.colored_label(ui.visuals().error_fg_color, e);
                                }
                                None => {}
                            }
                            ui.weak(
                                "The sandbox applies to new commands; settings changes remove the \
                                 running containers.",
                            );
                        });
                        ui.end_row();
                    });
            });
    }

    fn voice_tab(&mut self, ui: &mut egui::Ui) {
        let providers: Vec<(String, String)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let voice = &mut self.draft.general.voice;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.checkbox(
                    &mut voice.input.enabled,
                    egui::RichText::new("Voice input").strong(),
                );
                ui.indent("voice-in", |ui| {
                    ui.weak(
                        "A microphone button under the message box records a clip, and the \
                         transcript goes into the message box to read and edit before you \
                         send it. Ctrl/⌘+Shift+M starts and stops a recording. The clip goes to \
                         the transcription endpoint below: a cloud service, or a local server \
                         such as whisper.cpp, faster-whisper-server, speaches or LocalAI.",
                    );
                    ui.add_enabled_ui(voice.input.enabled, |ui| {
                        egui::Grid::new("voice-in-grid")
                            .num_columns(2)
                            .spacing([12.0, 10.0])
                            .show(ui, |ui| {
                                ui.label("Provider");
                                provider_choice(
                                    ui,
                                    "voice-in-provider",
                                    &mut voice.input.provider,
                                    &providers,
                                );
                                ui.end_row();
                                ui.label("Model");
                                ui.add(
                                    egui::TextEdit::singleline(&mut voice.input.model)
                                        .desired_width(220.0),
                                );
                                ui.end_row();
                                ui.label("Language");
                                ui.add(
                                    egui::TextEdit::singleline(&mut voice.input.language)
                                        .hint_text("Detect")
                                        .desired_width(220.0),
                                );
                                ui.end_row();
                                ui.label("Longest recording (seconds)");
                                ui.add(
                                    egui::DragValue::new(&mut voice.input.max_secs)
                                        .range(5..=crate::voice::MAX_RECORD_SECS),
                                );
                                ui.end_row();
                            });
                    });
                });
                ui.add_space(12.0);
                ui.checkbox(
                    &mut voice.output.enabled,
                    egui::RichText::new("Read replies aloud").strong(),
                );
                ui.indent("voice-out", |ui| {
                    ui.weak(
                        "A speaker button under each reply reads it aloud, and Ctrl/⌘+Shift+S \
                         reads the latest reply or stops the reading. Code blocks and tables are \
                         announced rather than read, and links are read as their text.",
                    );
                    ui.add_enabled_ui(voice.output.enabled, |ui| {
                        ui.horizontal(|ui| {
                            ui.radio_value(
                                &mut voice.output.engine,
                                crate::voice::SpeechEngine::OsVoice,
                                crate::voice::SpeechEngine::OsVoice.label(),
                            );
                            ui.radio_value(
                                &mut voice.output.engine,
                                crate::voice::SpeechEngine::Endpoint,
                                crate::voice::SpeechEngine::Endpoint.label(),
                            );
                        });
                        let endpoint = voice.output.engine == crate::voice::SpeechEngine::Endpoint;
                        ui.weak(if endpoint {
                            "Sent to the speech endpoint below as WAV audio, then played on this computer."
                        } else {
                            "Spoken by this computer: say on macOS, System.Speech on Windows, spd-say or espeak-ng on Linux. Nothing leaves the computer."
                        });
                        egui::Grid::new("voice-out-grid")
                            .num_columns(2)
                            .spacing([12.0, 10.0])
                            .show(ui, |ui| {
                                if endpoint {
                                    ui.label("Provider");
                                    provider_choice(
                                        ui,
                                        "voice-out-provider",
                                        &mut voice.output.provider,
                                        &providers,
                                    );
                                    ui.end_row();
                                    ui.label("Model");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut voice.output.model)
                                            .desired_width(220.0),
                                    );
                                    ui.end_row();
                                    ui.label("Voice");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut voice.output.voice)
                                            .desired_width(220.0),
                                    );
                                    ui.end_row();
                                }
                                ui.label("Speed");
                                ui.add(
                                    egui::Slider::new(&mut voice.output.speed, 0.5..=2.0)
                                        .step_by(0.05),
                                );
                                ui.end_row();
                                ui.label("Longest reply read (characters)");
                                ui.add(
                                    egui::DragValue::new(&mut voice.output.max_chars).range(
                                        200..=crate::voice::MAX_SPEAK_CHARS,
                                    ),
                                );
                                ui.end_row();
                            });
                    });
                });
            });
    }

    fn background_tab(
        &mut self,
        ui: &mut egui::Ui,
        chats: &[(Uuid, String)],
        actions: &mut Vec<SettingsAction>,
    ) {
        let providers: Vec<(String, String)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let plan_on = self.draft.general.plan.enabled;
        let schedules = &mut self.draft.general.schedules;
        let bg = &mut self.draft.general.background;
        let ex = &mut self.draft.general.exec;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled_ui(crate::tray::SUPPORTED, |ui| {
                    ui.checkbox(&mut bg.keep_running, crate::tray::setting_label())
                });
                if crate::tray::SUPPORTED {
                    ui.indent("bg-keep-running", |ui| {
                        ui.weak(
                            "Closing the window hides it, and replies and MCP servers keep \
                             running. Show the window or quit from the icon's menu. On macOS, \
                             Command-Q quits as usual.",
                        );
                    });
                } else {
                    ui.indent("bg-keep-running", |ui| {
                        ui.weak("Not available on Linux yet.");
                    });
                }
                ui.add_space(10.0);
                ui.checkbox(
                    &mut bg.notify,
                    egui::RichText::new("Notify when a reply finishes, fails or needs approval")
                        .strong(),
                );
                ui.indent("bg-notify", |ui| {
                    let on = bg.notify;
                    ui.add_enabled(
                        on,
                        egui::Checkbox::new(
                            &mut bg.notify_only_unfocused,
                            "Only while the CleanRSTAI window is in the background",
                        ),
                    );
                    ui.weak(
                        "Sent through the system's notifications. Each chat can also switch \
                         its notifications off from the tools menu.",
                    );
                });
                ui.add_space(10.0);
                egui::Grid::new("background-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Replies at once");
                        ui.add(
                            egui::DragValue::new(&mut bg.max_parallel)
                                .range(1..=crate::config::MAX_PARALLEL_RUNS),
                        );
                        ui.end_row();
                        ui.label("");
                        weak_note(
                            ui,
                            "Replies beyond this wait in a queue, marked Queued, and start in \
                             the order they were sent as others finish. Stop on a queued chat \
                             takes it out of the queue.",
                        );
                        ui.end_row();

                        ui.label("Time limit");
                        ui.add(
                            egui::DragValue::new(&mut bg.time_limit_mins)
                                .range(0..=crate::config::MAX_TIME_LIMIT_MINS)
                                .suffix(" min"),
                        );
                        ui.end_row();
                        ui.label("");
                        weak_note(
                            ui,
                            "0 means no limit. A reply that runs longer is stopped and says \
                             so. The time counts from when the reply starts, including any \
                             wait for approval.",
                        );
                        ui.end_row();
                    });
                ui.add_space(14.0);
                ui.label(egui::RichText::new("Command line").strong());
                ui.checkbox(
                    &mut ex.enabled,
                    "Allow `cleanrstai exec` to run requests from a terminal or script",
                );
                ui.indent("exec-settings", |ui| {
                    let on = ex.enabled;
                    egui::Grid::new("exec-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.add_enabled(on, egui::Label::new("Approvals"));
                            ui.add_enabled_ui(on, |ui| {
                                egui::ComboBox::from_id_salt("exec-approval-mode")
                                    .selected_text(ex.approval_mode.label())
                                    .show_ui(ui, |ui| {
                                        for m in ApprovalMode::offered(plan_on) {
                                            ui.selectable_value(
                                                &mut ex.approval_mode,
                                                m,
                                                m.label(),
                                            )
                                            .on_hover_text(m.description());
                                        }
                                    });
                            });
                            ui.end_row();
                            ui.label("");
                            weak_note(
                                ui,
                                "The mode of a command-line request; --mode overrides it. Calls \
                                 it doesn't allow are denied, not asked, and the model is told.",
                            );
                            ui.end_row();

                            ui.add_enabled(on, egui::Label::new("MCP servers"));
                            ui.add_enabled(
                                on,
                                egui::Checkbox::new(&mut ex.mcp, "Start them for each request"),
                            );
                            ui.end_row();

                            ui.add_enabled(on, egui::Label::new("Wait for them"));
                            ui.add_enabled(
                                on,
                                egui::DragValue::new(&mut ex.mcp_wait_secs)
                                    .range(0..=crate::exec::MAX_MCP_WAIT_SECS)
                                    .suffix(" s"),
                            );
                            ui.end_row();

                            ui.label("");
                            weak_note(
                                ui,
                                "How long a request waits for servers to connect before it \
                                 starts. --no-mcp skips them.",
                            );
                            ui.end_row();

                            ui.add_enabled(on, egui::Label::new("Chats"));
                            ui.add_enabled(
                                on,
                                egui::Checkbox::new(&mut ex.save_chats, "Save each run's chat"),
                            );
                            ui.end_row();
                        });
                    ui.add_space(4.0);
                    weak_note(
                        ui,
                        "Example: git diff | cleanrstai exec --mode read-only \"review this\" -\n\
                         Replies go to stdout and tool calls to stderr. Run \
                         `cleanrstai exec --help` for every option.",
                    );
                });
                super::schedule_settings::section(
                    ui, schedules, &providers, chats, plan_on, actions,
                );
            });
    }

    fn subagents_tab(&mut self, ui: &mut egui::Ui) {
        let providers: Vec<(String, String, Vec<String>)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.models.clone()))
            .collect();
        let servers: Vec<(String, String)> = self
            .draft
            .mcp_servers
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let sa = &mut self.draft.subagents;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.checkbox(
                    &mut sa.enabled,
                    egui::RichText::new("Let the model delegate tasks to subagents").strong(),
                );
                ui.indent("sa-enabled", |ui| {
                    ui.weak(
                        "Adds a \"delegate\" tool. Each subagent works on one task on its own, \
                         in parallel with the others, and its result goes back to the main chat. \
                         Delegating asks for approval unless tool calls are auto-approved \
                         (General). It can also be switched per chat from the tools menu.",
                    );
                });
                ui.add_space(10.0);
                ui.add_enabled_ui(sa.enabled, |ui| {
                    egui::Grid::new("subagent-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Provider");
                            let current = providers
                                .iter()
                                .find(|(id, ..)| *id == sa.provider_id)
                                .map_or("Same as the chat".to_owned(), |(_, n, _)| n.clone());
                            egui::ComboBox::from_id_salt("subagent-provider")
                                .selected_text(current)
                                .width(FIELD_WIDTH)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut sa.provider_id,
                                        String::new(),
                                        "Same as the chat",
                                    );
                                    for (id, name, _) in &providers {
                                        ui.selectable_value(&mut sa.provider_id, id.clone(), name);
                                    }
                                });
                            ui.end_row();

                            ui.label("Model");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut sa.model)
                                        .hint_text("the provider's selected model")
                                        .desired_width(FIELD_WIDTH - 40.0),
                                );
                                let models = providers
                                    .iter()
                                    .find(|(id, ..)| *id == sa.provider_id)
                                    .map(|(.., m)| m.clone())
                                    .unwrap_or_default();
                                if !models.is_empty() {
                                    ui.menu_button(icon::CARET_DOWN, |ui| {
                                        ui.set_max_height(320.0);
                                        egui::ScrollArea::vertical().show(ui, |ui| {
                                            for m in models {
                                                if ui.button(&m).clicked() {
                                                    sa.model = m;
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                }
                            });
                            ui.end_row();

                            ui.label("Per-task choice");
                            ui.checkbox(
                                &mut sa.allow_provider_choice,
                                "Let the model pick another provider profile or model for a task",
                            )
                            .on_hover_text(
                                "The delegate tool lists your provider profiles, so the model can, \
                                 for example, send simple tasks to a fast local model.",
                            );
                            ui.end_row();

                            ui.label("Tools");
                            ui.vertical(|ui| {
                                ui.checkbox(
                                    &mut sa.builtin_tools,
                                    "Time, calculator and web fetch (when on in the chat)",
                                );
                                ui.checkbox(&mut sa.mcp_tools, "The chat's MCP tools");
                                if sa.mcp_tools && !servers.is_empty() {
                                    ui.indent("sa-servers", |ui| {
                                        for (id, name) in &servers {
                                            let mut on = !sa.excluded_servers.contains(id);
                                            if ui.checkbox(&mut on, name).changed() {
                                                if on {
                                                    sa.excluded_servers.retain(|x| x != id);
                                                } else {
                                                    sa.excluded_servers.push(id.clone());
                                                }
                                            }
                                        }
                                    });
                                }
                                if sa.mcp_tools {
                                    ui.colored_label(
                                        ui.visuals().warn_fg_color,
                                        format!(
                                            "{}  Subagents call MCP tools without asking for approval.",
                                            icon::WARNING
                                        ),
                                    );
                                }
                                ui.checkbox(
                                    &mut sa.workspace,
                                    "The workspace tools, with commands in their own sandbox container",
                                )
                                .on_hover_text(
                                    "Only while the Docker sandbox is on. Each subagent gets its \
                                     own container with the chat's folder at /workspace, sharing \
                                     the chat's network rules. It is removed when the subagent \
                                     finishes.",
                                );
                                if sa.workspace {
                                    ui.colored_label(
                                        ui.visuals().warn_fg_color,
                                        format!(
                                            "{}  Subagents read, write and run commands without asking for approval; approving the delegate call allows it.",
                                            icon::WARNING
                                        ),
                                    );
                                }
                            });
                            ui.end_row();

                            ui.label("Run at once");
                            ui.add(egui::DragValue::new(&mut sa.max_parallel).range(1..=16))
                                .on_hover_text("How many subagents work at the same time");
                            ui.end_row();

                            ui.label("Tasks per call");
                            ui.add(egui::DragValue::new(&mut sa.max_tasks).range(1..=32))
                                .on_hover_text("Most tasks one delegate call may hand out");
                            ui.end_row();

                            ui.label("Max tool rounds");
                            ui.add(egui::DragValue::new(&mut sa.max_rounds).range(1..=100))
                                .on_hover_text("Tool-call rounds each subagent may make");
                            ui.end_row();

                            ui.label("Time limit");
                            ui.add(
                                egui::DragValue::new(&mut sa.timeout_secs)
                                    .range(10..=3600)
                                    .suffix(" s"),
                            )
                            .on_hover_text("Per subagent");
                            ui.end_row();
                        });
                    ui.add_space(12.0);
                    ui.strong("Subagent system prompt");
                    ui.add(
                        egui::TextEdit::multiline(&mut sa.system_prompt)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
    }

    fn general_tab(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        remote: &RemoteStatus,
        (tunnel, access_error): (&TunnelStatus, Option<&str>),
        actions: &mut Vec<SettingsAction>,
    ) {
        let dirty = &self.draft != live;
        let (draft_remote, live_remote) = (&self.draft.general.remote, &live.general.remote);
        let remote_status = RemoteUiStatus {
            server: remote,
            tunnel,
            server_dirty: crate::remote::server_settings(draft_remote)
                != crate::remote::server_settings(live_remote),
            tunnel_dirty: draft_remote.tunnel != live_remote.tunnel,
            access_error,
        };
        // Applied after the scroll area, which holds a borrow of the draft.
        let mut import_choice: Option<Option<crate::export::ImportMode>> = None;
        let g = &mut self.draft.general;
        let plan_on = g.plan.enabled;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.strong("System prompt");
                ui.add(
                    egui::TextEdit::multiline(&mut g.system_prompt)
                        .desired_rows(5)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(10.0);

                egui::Grid::new("general-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Theme");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut g.theme, Theme::System, "System");
                            ui.radio_value(&mut g.theme, Theme::Light, "Light");
                            ui.radio_value(&mut g.theme, Theme::Dark, "Dark");
                        });
                        ui.end_row();

                        ui.label("UI scale");
                        ui.add(
                            egui::Slider::new(&mut g.zoom, 0.5..=2.5)
                                .step_by(0.05)
                                .suffix("×"),
                        );
                        ui.end_row();

                        ui.label("Sending");
                        ui.vertical(|ui| {
                            ui.radio_value(
                                &mut g.send_with_enter,
                                true,
                                "Enter sends, Shift+Enter for newline",
                            );
                            ui.radio_value(
                                &mut g.send_with_enter,
                                false,
                                "Ctrl/⌘+Enter sends, Enter for newline",
                            );
                        });
                        ui.end_row();

                        ui.label("Secrets");
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut g.use_keychain,
                                "Store API keys and secrets in the system keychain",
                            )
                            .on_hover_text(
                                "API keys, header values and MCP environment variables are kept in the macOS Keychain, Windows Credential Manager or Secret Service instead of settings.json.",
                            );
                            if g.use_keychain
                                && let Err(e) = crate::secrets::OsKeychain::status()
                            {
                                ui.colored_label(
                                    ui.visuals().warn_fg_color,
                                    format!("Keychain unavailable ({e}); secrets will stay in settings.json."),
                                );
                            }
                        });
                        ui.end_row();

                        remote_rows(ui, &mut g.remote, &mut self.show_remote_password, &remote_status);

                        ui.label("Updates");
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut g.check_for_updates,
                                "Check GitHub for a new release at startup",
                            );
                            ui.horizontal(|ui| {
                                ui.label("Update from");
                                ui.add(
                                    egui::TextEdit::singleline(&mut g.update_url)
                                        .hint_text("owner/repo or a releases API URL")
                                        .desired_width(380.0),
                                )
                                .on_hover_text(
                                    "A GitHub owner/repo, a github.com link, or a URL that \
                                     answers like GitHub's releases/latest API",
                                );
                                if g.update_url != crate::update::LATEST_RELEASE_URL
                                    && ui.button("Reset").clicked()
                                {
                                    g.update_url = crate::update::LATEST_RELEASE_URL.into();
                                }
                            });
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        !self.checking_update,
                                        egui::Button::new("Check for updates now"),
                                    )
                                    .clicked()
                                {
                                    self.checking_update = true;
                                    self.update_status = None;
                                    actions.push(SettingsAction::CheckForUpdates(
                                        g.update_url.clone(),
                                    ));
                                }
                                if self.checking_update {
                                    ui.spinner();
                                }
                                ui.weak(concat!("Installed: v", env!("CARGO_PKG_VERSION")));
                            });
                            match &self.update_status {
                                Some(Ok(m)) => {
                                    ui.horizontal(|ui| {
                                        ui.label(m);
                                        if self.update_available
                                            && ui.button("Install update").clicked()
                                        {
                                            self.update_available = false;
                                            actions.push(SettingsAction::InstallUpdate);
                                        }
                                    });
                                }
                                Some(Err(e)) => {
                                    ui.colored_label(ui.visuals().error_fg_color, e);
                                }
                                None => {}
                            }
                        });
                        ui.end_row();

                        ui.label("Reasoning");
                        ui.vertical(|ui| {
                            for mode in crate::config::ReasoningDisplay::ALL {
                                ui.radio_value(&mut g.reasoning_display, mode, mode.label());
                            }
                        })
                        .response
                        .on_hover_text("How the model's \"Thinking\" section starts out");
                        ui.end_row();

                        ui.label("Stats");
                        ui.checkbox(
                            &mut g.show_stats,
                            "Show prefill/decode speed and token counts under responses",
                        );
                        ui.end_row();

                        ui.label("Images");
                        ui.checkbox(
                            &mut g.load_remote_images,
                            "Load web images in replies",
                        )
                        .on_hover_text(
                            "When off, images from the web show as links. Loading them tells \
                             the image's server that this chat was opened, and a reply could \
                             put chat contents in the image address.",
                        );
                        ui.end_row();

                        ui.label("Artifacts");
                        ui.checkbox(
                            &mut g.open_artifacts,
                            "Open web pages, drawings and long code in a side panel",
                        )
                        .on_hover_text(
                            "When a reply makes one, the panel opens beside the chat. Off: \
                             click the card under the reply to open it.",
                        );
                        ui.end_row();

                        ui.label("Web page preview");
                        let hp = &mut g.html_preview;
                        ui.vertical(|ui| {
                            ui.add_enabled_ui(
                                crate::ui::webview::SUPPORTED || crate::ui::webview::OWN_WINDOW,
                                |ui| {
                                ui.checkbox(&mut hp.enabled, "Show web pages live in the panel")
                                    .on_hover_text(
                                        "Off: the panel shows the code and \"Open in browser\" \
                                         opens the page",
                                    );
                                ui.add_enabled_ui(hp.enabled, |ui| {
                                    ui.checkbox(&mut hp.scripts, "Let pages run their scripts");
                                    ui.checkbox(
                                        &mut hp.network,
                                        "Let pages load from the network",
                                    )
                                    .on_hover_text(
                                        "Off: scripts, styles, images and requests from the web \
                                         are blocked, so a page can't send chat content anywhere. \
                                         Pages that use a CDN library need this on.",
                                    );
                                });
                            });
                            #[cfg(target_os = "linux")]
                            weak_note(ui, &crate::ui::linux_preview::status());
                            #[cfg(not(target_os = "linux"))]
                            if !crate::ui::webview::SUPPORTED {
                                weak_note(
                                    ui,
                                    "Needs the macOS or Windows app; on Linux web pages open in \
                                     your browser.",
                                );
                            }
                        });
                        ui.end_row();

                        ui.label("Save as files");
                        ui.vertical(|ui| {
                            ui.checkbox(&mut g.save_as.docx, "Word document (.docx)");
                            ui.checkbox(&mut g.save_as.pdf, "PDF (.pdf)");
                            ui.checkbox(&mut g.save_as.xlsx, "Excel workbook (.xlsx)")
                                .on_hover_text(
                                    "Offered for replies with a table and for CSV or TSV \
                                     artifacts",
                                );
                            weak_note(
                                ui,
                                "The Save as menu on a reply or artifact offers the checked \
                                 formats.",
                            );
                        });
                        ui.end_row();

                        ui.label("Tool calls");
                        ui.checkbox(
                            &mut g.auto_approve_tools,
                            "Run MCP tools and subagents without asking for approval",
                        );
                        ui.end_row();

                        ui.label("New chats start in");
                        egui::ComboBox::from_id_salt("default-approval-mode")
                            .selected_text(g.default_approval_mode.label())
                            .show_ui(ui, |ui| {
                                for m in ApprovalMode::offered(plan_on) {
                                    ui.selectable_value(&mut g.default_approval_mode, m, m.label())
                                        .on_hover_text(m.description());
                                }
                            });
                        ui.end_row();

                        ui.label("Max tool rounds");
                        ui.add(egui::DragValue::new(&mut g.max_tool_rounds).range(1..=200))
                            .on_hover_text(
                                "How many times the model may call tools before it must answer",
                            );
                        ui.end_row();

                        ui.label("Tool timeout");
                        ui.add(
                            egui::DragValue::new(&mut g.tool_timeout_secs)
                                .range(5..=3600)
                                .suffix(" s"),
                        );
                        ui.end_row();
                    });

                ui.add_space(16.0);
                ui.separator();
                ui.strong("Plan mode");
                ui.checkbox(&mut g.plan.enabled, "Offer Plan in the permission menus")
                    .on_hover_text(
                        "A Plan chat investigates with read-only tools and proposes a numbered \
                         plan. Off, Plan is hidden, and a chat set to it is read only.",
                    );
                ui.add_enabled_ui(g.plan.enabled, |ui| {
                    egui::Grid::new("plan-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Mode after approval");
                            egui::ComboBox::from_id_salt("plan-after-approval")
                                .selected_text(g.plan.after_mode().label())
                                .show_ui(ui, |ui| {
                                    for m in [
                                        ApprovalMode::Default,
                                        ApprovalMode::AutoWorkspace,
                                        ApprovalMode::AllowAll,
                                    ] {
                                        ui.selectable_value(&mut g.plan.after_approval, m, m.label())
                                            .on_hover_text(m.description());
                                    }
                                });
                            ui.end_row();
                            ui.label("Approval message");
                            ui.add(
                                egui::TextEdit::singleline(&mut g.plan.approve_message)
                                    .desired_width(FIELD_WIDTH),
                            )
                            .on_hover_text("Sent as the user's message when a plan is approved");
                            ui.end_row();
                        });
                    ui.add_space(4.0);
                    weak_note(
                        ui,
                        "Under a finished plan, Approve plan switches the chat to the mode \
                         above and sends the approval message. Plan and Read only can't be the \
                         mode after approval.",
                    );
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("Plan instructions");
                    if g.plan.prompt != crate::plan::PLAN_PROMPT
                        && ui.small_button("Reset").clicked()
                    {
                        g.plan.prompt = crate::plan::PLAN_PROMPT.into();
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut g.plan.prompt)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(16.0);
                ui.separator();
                ui.strong("Branches");
                ui.checkbox(&mut g.branching.enabled, "Keep the version a reply replaces")
                    .on_hover_text(
                        "Regenerating a reply, or editing a message, keeps the version it \
                         replaces as a branch, with ‹ k/N › arrows to switch between them. Off, \
                         the replaced version is dropped. Versions already kept stay switchable. \
                         A chat's Branches switch overrides this.",
                    );
                ui.add_enabled_ui(g.branching.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Versions kept per message");
                        ui.add(
                            egui::DragValue::new(&mut g.branching.max_per_message).range(1..=50),
                        );
                    });
                    weak_note(
                        ui,
                        "Past this many, the oldest version of a message is dropped. Files \
                         changed by a version are only undone while it is the one shown.",
                    );
                });
                ui.add_space(16.0);
                ui.separator();
                ui.strong("Export & import");
                ui.horizontal(|ui| {
                    if ui
                        .button("Export settings…")
                        .on_hover_text("Providers, MCP servers and preferences, as saved")
                        .clicked()
                    {
                        actions.push(SettingsAction::ExportSettings {
                            include_secrets: self.export_include_secrets,
                        });
                    }
                    ui.checkbox(&mut self.export_include_secrets, "Include API keys and secrets");
                });
                if self.export_include_secrets {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "The file will contain API keys, header values and MCP environment variables in plain text.",
                    );
                } else {
                    ui.weak("API keys, header values and MCP environment variable values are left blank.");
                }
                if dirty {
                    ui.weak("Unsaved changes are not included; Apply first to export them.");
                }
                if ui
                    .button("Export conversations…")
                    .on_hover_text("All chats, including tool calls and results")
                    .clicked()
                {
                    actions.push(SettingsAction::ExportConversations);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Import settings…")
                        .on_hover_text("Load a settings export into this window for review")
                        .clicked()
                    {
                        actions.push(SettingsAction::ImportSettings);
                    }
                    if ui
                        .button("Import conversations…")
                        .on_hover_text("Add chats from a conversations export; existing chats are kept")
                        .clicked()
                    {
                        actions.push(SettingsAction::ImportConversations);
                    }
                });
                if let Some(incoming) = &self.pending_import {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.label(format!(
                            "Import {} provider(s) and {} MCP server(s)?",
                            incoming.providers.len(),
                            incoming.mcp_servers.len()
                        ));
                        ui.weak("Merge adds them, updating entries that already exist, and keeps your other settings. Replace uses the imported settings as they are.");
                        ui.horizontal(|ui| {
                            if ui.button("Merge").clicked() {
                                import_choice = Some(Some(crate::export::ImportMode::Merge));
                            }
                            if ui.button("Replace").clicked() {
                                import_choice = Some(Some(crate::export::ImportMode::Replace));
                            }
                            if ui.button("Cancel").clicked() {
                                import_choice = Some(None);
                            }
                        });
                    });
                }
                match &self.export_status {
                    Some(Ok(msg)) => {
                        ui.colored_label(egui::Color32::from_rgb(60, 170, 90), msg);
                    }
                    Some(Err(err)) => {
                        ui.colored_label(ui.visuals().error_fg_color, err);
                    }
                    None => {}
                }

                ui.add_space(16.0);
                ui.separator();
                ui.weak(format!("Settings file: {}", Settings::path().display()));
                ui.weak(format!(
                    "Conversations: {}",
                    crate::config::data_dir().join("conversations").display()
                ));
            });

        if let Some(choice) = import_choice
            && let Some(incoming) = self.pending_import.take()
        {
            if let Some(mode) = choice {
                self.draft = crate::export::apply_settings_import(&self.draft, *incoming, mode);
                let missing = crate::export::missing_secrets(&self.draft);
                let mut msg = "Imported. Review the settings, then Apply or Save.".to_owned();
                if !missing.is_empty() {
                    msg.push_str(&format!(" Still blank: {}.", missing.join(", ")));
                }
                self.export_status = Some(Ok(msg));
            } else {
                self.export_status = None;
            }
        }
    }
}

/// Slash commands: the on/off switches and the editable list of saved prompts.
/// Settings → Context & files → Memory: the switches, then the saved memories. Their text and
/// deletes are sent as [`MemoryChange`]s at once, not kept in the draft.
fn memory_section(
    ui: &mut egui::Ui,
    m: &mut MemorySettings,
    editor: &mut MemoryEditor,
    memories: &[Memory],
    projects: &[(Uuid, String)],
    actions: &mut Vec<SettingsAction>,
) {
    let project_name = |id: Uuid| -> String {
        projects
            .iter()
            .find(|(pid, _)| *pid == id)
            .map_or_else(|| "Deleted project".to_owned(), |(_, name)| name.clone())
    };
    ui.label(egui::RichText::new(format!("{}  Memory", icon::EDIT)).strong());
    ui.indent("memory-help", |ui| {
        ui.weak(
            "Facts the model saves when you ask it to remember something, or that you state about \
             yourself. A chat with memory on gets them in its system prompt with every request, so \
             they are sent to your provider. Off, or a chat switched to incognito, uses none of \
             them and the model can't save any.",
        );
    });
    ui.add_space(6.0);
    ui.checkbox(&mut m.enabled, "New chats use memory")
        .on_hover_text("Each chat can switch it off for itself (Chat features in the tools menu)");
    ui.checkbox(
        &mut m.ask_before_saving,
        "Ask before saving or deleting a memory",
    )
    .on_hover_text("Off, the model saves and deletes memories without asking");
    egui::Grid::new("memory-grid")
        .num_columns(2)
        .spacing([12.0, 10.0])
        .show(ui, |ui| {
            ui.label("Size limit");
            ui.add(
                egui::DragValue::new(&mut m.max_kb)
                    .range(MIN_KB..=MAX_KB)
                    .suffix(" KB"),
            )
            .on_hover_text(
                "The memories sent with each request, newest first; older ones are left out \
                 past this",
            );
            ui.end_row();

            ui.label("Scope");
            egui::ComboBox::from_id_salt("memory-scope")
                .selected_text(match m.scope {
                    MemoryScope::Global => "Global",
                    MemoryScope::PerProject => "Per project",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut m.scope, MemoryScope::Global, "Global")
                        .on_hover_text("One set of memories for every chat");
                    ui.selectable_value(&mut m.scope, MemoryScope::PerProject, "Per project")
                        .on_hover_text(
                            "A project's chats also see and save that project's memories, \
                             besides the global ones; other chats see only the global ones",
                        );
                });
            ui.end_row();
        });

    ui.add_space(10.0);
    editor
        .drafts
        .retain(|id, _| memories.iter().any(|m| m.id == *id));
    ui.strong(format!("Saved memories ({})", memories.len()));
    if memories.is_empty() {
        ui.weak("None yet.");
    }
    for memory in memories.iter().rev() {
        ui.horizontal(|ui| {
            let scope = match memory.scope {
                Scope::Global => "Global".to_owned(),
                Scope::Project(p) => format!("Project: {}", project_name(p)),
            };
            let saved = memory
                .created
                .with_timezone(&chrono::Local)
                .format("%-d %b %Y");
            ui.weak(format!("{scope} · saved {saved}"));
            if super::widgets::icon_button(ui, icon::TRASH, "Delete this memory").clicked() {
                actions.push(SettingsAction::Memory(MemoryChange::Delete(memory.id)));
            }
        });
        let draft = editor
            .drafts
            .entry(memory.id)
            .or_insert_with(|| memory.text.clone());
        let resp = ui.add(
            egui::TextEdit::singleline(draft)
                .desired_width(f32::INFINITY)
                .hint_text("Empty: forget it"),
        );
        if resp.lost_focus() {
            let text = draft.trim().to_owned();
            editor.drafts.remove(&memory.id);
            if text.is_empty() {
                actions.push(SettingsAction::Memory(MemoryChange::Delete(memory.id)));
            } else if text != memory.text {
                actions.push(SettingsAction::Memory(MemoryChange::Edit {
                    id: memory.id,
                    text,
                }));
            }
        }
        ui.add_space(4.0);
    }

    if !memories.is_empty() {
        ui.horizontal(|ui| {
            if !editor.confirm_all {
                if ui.button("Delete all memories").clicked() {
                    editor.confirm_all = true;
                }
            } else {
                ui.label(format!("Delete all {} memories?", memories.len()));
                if ui.button("Delete all").clicked() {
                    actions.push(SettingsAction::Memory(MemoryChange::DeleteAll));
                    editor.confirm_all = false;
                }
                if ui.button("Cancel").clicked() {
                    editor.confirm_all = false;
                }
            }
        });
    }

    ui.add_space(8.0);
    ui.strong("Add a memory");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut editor.new_text)
                .hint_text("A fact to remember")
                .desired_width(260.0),
        );
        let per_project = m.scope == MemoryScope::PerProject;
        if per_project {
            let shown = editor
                .new_scope
                .map_or_else(|| "Global".to_owned(), project_name);
            egui::ComboBox::from_id_salt("memory-new-scope")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut editor.new_scope, None, "Global");
                    for (id, name) in projects {
                        ui.selectable_value(&mut editor.new_scope, Some(*id), name);
                    }
                });
        }
        if ui
            .add_enabled(
                !editor.new_text.trim().is_empty(),
                egui::Button::new("Add memory"),
            )
            .clicked()
        {
            let scope = match editor.new_scope.filter(|_| per_project) {
                Some(p) => Scope::Project(p),
                None => Scope::Global,
            };
            let text = std::mem::take(&mut editor.new_text);
            actions.push(SettingsAction::Memory(MemoryChange::Add { text, scope }));
        }
    });
}

fn slash_command_rows(ui: &mut egui::Ui, s: &mut SlashCommandSettings) {
    ui.checkbox(
        &mut s.enabled,
        egui::RichText::new(format!(
            "{}  Slash commands (saved prompts)",
            icon::LIGHTNING
        ))
        .strong(),
    );
    ui.indent("slash-help", |ui| {
        ui.weak(
            "Type / and a name in the message box to send a saved prompt. {{args}} in a prompt \
             is replaced by what you type after the name; without it, that text is added after \
             a blank line. Names use letters, digits, - and _.",
        );
    });
    ui.add_enabled_ui(s.enabled, |ui| {
        ui.checkbox(
            &mut s.workspace_commands,
            "Also load commands from the chat's working folder",
        )
        .on_hover_text(
            "Each .cleanrstai/commands/<name>.md file in the folder is a command. A first line \
             description: ... gives its description; a command there replaces one here with the \
             same name.",
        );
        ui.add_space(6.0);
        let mut remove = None;
        for (i, c) in s.commands.iter_mut().enumerate() {
            ui.push_id(("slash-command", i), |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut c.name)
                            .hint_text("name")
                            .desired_width(140.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut c.description)
                            .hint_text("description")
                            .desired_width(FIELD_WIDTH - 60.0),
                    );
                    if icon_button(ui, icon::TRASH, "Delete command").clicked() {
                        remove = Some(i);
                    }
                });
                if !c.name.trim().is_empty() && !crate::commands::valid_name(c.name.trim()) {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        "Names use letters, digits, - and _ only; this command is dropped on save",
                    );
                }
                ui.add(
                    egui::TextEdit::multiline(&mut c.prompt)
                        .hint_text("The prompt; {{args}} is replaced by what follows the name")
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
            });
            ui.add_space(6.0);
        }
        if let Some(i) = remove {
            s.commands.remove(i);
        }
        if ui
            .small_button(format!("{}  Add command", icon::PLUS))
            .clicked()
        {
            s.commands.push(SlashCommand::default());
        }
    });
}

/// Hooks (see `hooks`): the master switch, the warning about where they run, and each hook's
/// switch, name, event, tool patterns, command and timeout.
fn hook_rows(ui: &mut egui::Ui, h: &mut HookSettings) {
    let HookSettings { enabled, hooks } = h;
    ui.label(egui::RichText::new(format!("{}  Hooks", icon::TERMINAL)).strong());
    ui.indent("hooks-help", |ui| {
        ui.weak(
            "Commands that run around the model's tool calls: before a tool runs (a non-zero \
             exit or a timeout blocks the call), after it (the output is added to the result), \
             and when a reply is done (the output is shown as a notice). Each command gets the \
             call in CLEANRSTAI_* environment variables. Each chat can switch hooks off from \
             the tools menu.",
        );
        weak_note(
            ui,
            "Hooks run commands with your user's rights on this computer, or in the Docker \
             sandbox when it's on. They are read only from these settings, never from a \
             workspace or from the model.",
        );
    });
    ui.checkbox(enabled, "Run hooks").on_hover_text(
        "The default for each chat. A chat's own Hooks switch in the tools menu overrides it.",
    );
    ui.add_enabled_ui(*enabled, |ui| {
        let mut remove = None;
        for (i, hook) in hooks.iter_mut().enumerate() {
            ui.push_id(("hook", i), |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut hook.enabled, "")
                        .on_hover_text("Run this hook");
                    ui.add(
                        egui::TextEdit::singleline(&mut hook.name)
                            .hint_text("name")
                            .desired_width(140.0),
                    );
                    egui::ComboBox::from_id_salt("hook-event")
                        .selected_text(hook.event.label())
                        .show_ui(ui, |ui| {
                            for event in HookEvent::ALL {
                                ui.selectable_value(&mut hook.event, event, event.label());
                            }
                        });
                    if icon_button(ui, icon::TRASH, "Delete hook").clicked() {
                        remove = Some(i);
                    }
                });
                if hook.event != HookEvent::ReplyDone {
                    ui.horizontal(|ui| {
                        ui.label("Tools");
                        ui.add(
                            egui::TextEdit::singleline(&mut hook.tools)
                                .hint_text("all tools; or write_file, edit_file, github__*")
                                .desired_width(FIELD_WIDTH),
                        );
                    });
                }
                ui.add(
                    egui::TextEdit::multiline(&mut hook.command)
                        .hint_text("The shell command, e.g. cargo fmt")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.label("Timeout");
                    ui.add(
                        egui::DragValue::new(&mut hook.timeout_secs)
                            .range(1..=MAX_TIMEOUT_SECS)
                            .suffix(" s"),
                    );
                });
            });
            ui.add_space(6.0);
        }
        if let Some(i) = remove {
            hooks.remove(i);
        }
        if ui
            .small_button(format!("{}  Add hook", icon::PLUS))
            .clicked()
        {
            hooks.push(Hook::default());
        }
    });
}

/// Settings the search box can find: (tab, section, label).
const SEARCH_INDEX: &[(Tab, &str, &str)] = &[
    (Tab::Voice, "Voice", "Voice input"),
    (Tab::Voice, "Voice", "Voice input provider"),
    (Tab::Voice, "Voice", "Transcription model"),
    (Tab::Voice, "Voice", "Dictation language"),
    (Tab::Voice, "Voice", "Longest recording"),
    (Tab::Voice, "Voice", "Read replies aloud"),
    (
        Tab::Voice,
        "Voice",
        "Speech engine (system voice or endpoint)",
    ),
    (Tab::Voice, "Voice", "Speech provider"),
    (Tab::Voice, "Voice", "Speech model and voice"),
    (Tab::Voice, "Voice", "Speech speed"),
    (Tab::Voice, "Voice", "Longest reply read aloud"),
    (Tab::General, "General", "System prompt"),
    (Tab::General, "General", "Theme (light / dark)"),
    (Tab::General, "General", "UI scale / zoom"),
    (Tab::General, "General", "Send with Enter"),
    (Tab::General, "General", "Keychain / secrets"),
    (Tab::General, "General", "Remote access (browser)"),
    (Tab::General, "General", "Remote access password"),
    (Tab::General, "General", "Remote access port"),
    (Tab::General, "General", "Remote access HTTPS certificate"),
    (Tab::General, "General", "Cloudflare Tunnel"),
    (Tab::General, "General", "Cloudflare Tunnel token"),
    (Tab::General, "General", "Cloudflare Tunnel public hostname"),
    (
        Tab::General,
        "General",
        "Cloudflare quick tunnel (trycloudflare.com)",
    ),
    (Tab::General, "General", "cloudflared path"),
    (Tab::General, "General", "Cloudflare Access"),
    (Tab::General, "General", "Cloudflare Access team domain"),
    (
        Tab::General,
        "General",
        "Cloudflare Access audience (AUD) tag",
    ),
    (
        Tab::General,
        "General",
        "Cloudflare Access sign in without the app password",
    ),
    (Tab::General, "General", "Show speed stats"),
    (Tab::General, "General", "Save as Word document (.docx)"),
    (Tab::General, "General", "Save as PDF"),
    (Tab::General, "General", "Save as Excel workbook (.xlsx)"),
    (Tab::General, "General", "Save reply as file"),
    (Tab::General, "General", "Load web images in replies"),
    (
        Tab::General,
        "General",
        "Reasoning / thinking display (collapsed by default)",
    ),
    (Tab::General, "General", "Auto-approve tool calls"),
    (
        Tab::General,
        "General",
        "Default permission mode for new chats",
    ),
    (Tab::General, "General", "Plan mode"),
    (Tab::General, "General", "Plan mode after approval"),
    (Tab::General, "General", "Plan approval message"),
    (Tab::General, "General", "Plan instructions"),
    (Tab::General, "General", "Branches"),
    (Tab::General, "General", "Keep the version a reply replaces"),
    (Tab::General, "General", "Versions kept per message"),
    (Tab::General, "General", "Max tool rounds"),
    (Tab::General, "General", "Tool timeout"),
    (Tab::General, "General", "Export settings"),
    (Tab::General, "General", "Export conversations"),
    (Tab::General, "General", "Import settings"),
    (Tab::General, "General", "Import conversations"),
    (Tab::General, "General", "Check for updates"),
    (Tab::General, "General", "Update from (release source)"),
    (Tab::Providers, "Providers", "Base URL / endpoint"),
    (Tab::Providers, "Providers", "API key"),
    (Tab::Providers, "Providers", "Model"),
    (Tab::Providers, "Providers", "Fetch models"),
    (
        Tab::Providers,
        "Providers",
        "Server type (llama.cpp, LiteLLM, OpenAI)",
    ),
    (Tab::Providers, "Providers", "Temperature"),
    (Tab::Providers, "Providers", "Max tokens"),
    (Tab::Providers, "Providers", "Streaming"),
    (Tab::Providers, "Providers", "Token usage (stream_options)"),
    (Tab::Providers, "Providers", "Timeout"),
    (Tab::Providers, "Providers", "Extra headers"),
    (Tab::Mcp, "MCP", "Add server"),
    (Tab::Mcp, "MCP", "Import mcpServers JSON"),
    (Tab::Mcp, "MCP", "stdio command and arguments"),
    (Tab::Mcp, "MCP", "Environment variables"),
    (Tab::Mcp, "MCP", "Streamable HTTP URL and headers"),
    (Tab::Mcp, "MCP", "OAuth sign-in / client ID"),
    (Tab::Mcp, "MCP", "Enable or disable tools"),
    (Tab::Mcp, "MCP", "Resources and prompts"),
    (Tab::Tools, "Tools", "Time & date tool"),
    (Tab::Tools, "Tools", "Calculator tool"),
    (Tab::Tools, "Tools", "Task list / planning tool"),
    (Tab::Tools, "Tools", "Web fetch tool (read URLs)"),
    (
        Tab::Tools,
        "Tools",
        "Web search tool (DuckDuckGo, SearXNG, Brave, Tavily, Kagi)",
    ),
    (Tab::Tools, "Tools", "Web search API key"),
    (
        Tab::Tools,
        "Tools",
        "Web search with the provider's own search (Anthropic, OpenAI, OpenRouter)",
    ),
    (Tab::Tools, "Tools", "Provider search: searches per reply"),
    (
        Tab::Extensions,
        "Skills & hooks",
        "Slash commands (saved prompts)",
    ),
    (Tab::Extensions, "Skills & hooks", "Skills (load on demand)"),
    (
        Tab::Extensions,
        "Skills & hooks",
        "Load skills from the workspace folder",
    ),
    (Tab::Extensions, "Skills & hooks", "Skill size limit"),
    (Tab::Extensions, "Skills & hooks", "Open skills folder"),
    (Tab::Extensions, "Skills & hooks", "Disable a skill"),
    (
        Tab::Extensions,
        "Skills & hooks",
        "Hooks (commands around tool calls)",
    ),
    (
        Tab::Extensions,
        "Skills & hooks",
        "Hook before a tool runs, block it",
    ),
    (Tab::Extensions, "Skills & hooks", "Hook timeout"),
    (
        Tab::Context,
        "Context & files",
        "Context compaction (summarise long chats)",
    ),
    (
        Tab::Context,
        "Context & files",
        "Compaction threshold and context size",
    ),
    (Tab::Context, "Context & files", "Compaction summary model"),
    (
        Tab::Context,
        "Context & files",
        "Code review (review changes)",
    ),
    (Tab::Context, "Context & files", "Review prompt and model"),
    (Tab::Context, "Context & files", "Code review strictness"),
    (
        Tab::Context,
        "Context & files",
        "Project instructions (AGENTS.md, CLAUDE.md)",
    ),
    (Tab::Context, "Context & files", "Projects (on / off)"),
    (Tab::Context, "Context & files", "Memory (on / off)"),
    (Tab::Context, "Context & files", "Memory: ask before saving"),
    (Tab::Context, "Context & files", "Memory: size limit"),
    (
        Tab::Context,
        "Context & files",
        "Memory: global or per project",
    ),
    (Tab::Context, "Context & files", "Memory: saved memories"),
    (
        Tab::Context,
        "Context & files",
        "Memory: delete all memories",
    ),
    (
        Tab::Context,
        "Context & files",
        "Projects: files size limit",
    ),
    (
        Tab::Context,
        "Context & files",
        "Projects: default instructions",
    ),
    (Tab::Context, "Context & files", "Projects: default folder"),
    (
        Tab::Context,
        "Context & files",
        "Projects: default provider",
    ),
    (
        Tab::Context,
        "Context & files",
        "Diff preview for file edits",
    ),
    (
        Tab::Context,
        "Context & files",
        "Undo file edits (checkpoints)",
    ),
    (Tab::Context, "Context & files", "Read scanned PDFs (OCR)"),
    (
        Tab::Context,
        "Context & files",
        "OCR engine and vision model",
    ),
    (
        Tab::Context,
        "Context & files",
        "Tesseract program and languages",
    ),
    (
        Tab::General,
        "General",
        "Live web page preview (scripts, network)",
    ),
    (
        Tab::Tools,
        "Tools",
        "Workspace tools (files, search, shell)",
    ),
    (Tab::Tools, "Tools", "Commit changes (git_commit)"),
    (Tab::Tools, "Tools", "Allow signed commits"),
    (
        Tab::Subagents,
        "Subagents",
        "Enable subagents (delegate tool)",
    ),
    (Tab::Docker, "Docker sandbox", "Docker sandbox"),
    (Tab::Docker, "Docker sandbox", "Sandbox image"),
    (Tab::Docker, "Docker sandbox", "Custom Dockerfile"),
    (Tab::Docker, "Docker sandbox", "Sandbox network"),
    (
        Tab::Docker,
        "Docker sandbox",
        "Allowed hosts, ports and protocols (firewall)",
    ),
    (
        Tab::Docker,
        "Docker sandbox",
        "Sandbox CPU and memory limits",
    ),
    (Tab::Subagents, "Subagents", "Subagent provider and model"),
    (
        Tab::Subagents,
        "Subagents",
        "Subagent tools, MCP and workspace access",
    ),
    (
        Tab::Subagents,
        "Subagents",
        "Parallel subagents, rounds and time limit",
    ),
    (Tab::Subagents, "Subagents", "Subagent system prompt"),
    (
        Tab::Background,
        "Background",
        "Notify when a reply finishes or needs approval",
    ),
    (
        Tab::Background,
        "Background",
        "Only notify while the window is in the background",
    ),
    (Tab::Background, "Background", "Replies at once (queue)"),
    (Tab::Background, "Background", "Reply time limit (minutes)"),
    (
        Tab::Background,
        "Background",
        "Keep running in the menu bar or system tray when the window is closed",
    ),
    (Tab::Background, "Background", "Run scheduled tasks"),
    (Tab::Background, "Background", "Scheduled task prompt"),
    (
        Tab::Background,
        "Background",
        "Scheduled task repeats every (minutes)",
    ),
    (Tab::Background, "Background", "Scheduled task approvals"),
    (Tab::Background, "Background", "Scheduled task folder"),
    (Tab::Background, "Background", "Scheduled task catch up"),
    (
        Tab::Background,
        "Background",
        "Command line (cleanrstai exec)",
    ),
    (Tab::Background, "Background", "Command line approval mode"),
    (Tab::Background, "Background", "Command line MCP servers"),
    (
        Tab::Background,
        "Background",
        "Command line MCP wait (seconds)",
    ),
    (Tab::Background, "Background", "Command line save chats"),
];

fn search_settings(query: &str) -> Vec<(Tab, &'static str, &'static str)> {
    let mut hits: Vec<(i32, Tab, &str, &str)> = SEARCH_INDEX
        .iter()
        .filter_map(|&(tab, section, label)| {
            super::palette::score(query, label).map(|s| (s, tab, section, label))
        })
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter().map(|(_, t, s, l)| (t, s, l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_settings() {
        let hits = search_settings("api key");
        assert_eq!(hits[0], (Tab::Providers, "Providers", "API key"));
        assert_eq!(search_settings("theme")[0].0, Tab::General);
        assert_eq!(search_settings("oauth")[0].0, Tab::Mcp);
        assert_eq!(search_settings("calculator")[0].0, Tab::Tools);
        assert_eq!(search_settings("subagent model")[0].0, Tab::Subagents);
        assert_eq!(search_settings("scanned pdf")[0].0, Tab::Context);
        assert_eq!(search_settings("tesseract")[0].0, Tab::Context);
        assert_eq!(search_settings("agents.md")[0].0, Tab::Context);
        assert_eq!(search_settings("projects")[0].0, Tab::Context);
        assert_eq!(search_settings("memory")[0].0, Tab::Context);
        assert_eq!(search_settings("memory size")[0].0, Tab::Context);
        assert_eq!(search_settings("memory per project")[0].0, Tab::Context);
        assert_eq!(search_settings("delete all memories")[0].0, Tab::Context);
        assert_eq!(search_settings("project files size")[0].0, Tab::Context);
        assert_eq!(search_settings("compaction")[0].0, Tab::Context);
        assert_eq!(search_settings("review changes")[0].0, Tab::Context);
        assert_eq!(search_settings("review prompt")[0].0, Tab::Context);
        assert_eq!(search_settings("searxng")[0].0, Tab::Tools);
        assert_eq!(search_settings("git commit")[0].0, Tab::Tools);
        assert_eq!(search_settings("signed commits")[0].0, Tab::Tools);
        assert_eq!(search_settings("provider search")[0].0, Tab::Tools);
        assert_eq!(search_settings("searches per reply")[0].0, Tab::Tools);
        assert_eq!(search_settings("slash command")[0].0, Tab::Extensions);
        assert_eq!(search_settings("saved prompts")[0].0, Tab::Extensions);
        assert_eq!(search_settings("skills")[0].0, Tab::Extensions);
        assert_eq!(search_settings("skill folder")[0].0, Tab::Extensions);
        assert_eq!(search_settings("disable a skill")[0].0, Tab::Extensions);
        assert_eq!(search_settings("hooks")[0].0, Tab::Extensions);
        assert_eq!(search_settings("dictation")[0].0, Tab::Voice);
        assert_eq!(search_settings("transcription")[0].0, Tab::Voice);
        assert_eq!(search_settings("read aloud")[0].0, Tab::Voice);
        assert_eq!(search_settings("speech speed")[0].0, Tab::Voice);
        assert_eq!(search_settings("hook timeout")[0].0, Tab::Extensions);
        assert_eq!(search_settings("web page preview")[0].0, Tab::General);
        assert_eq!(search_settings("save as pdf")[0].0, Tab::General);
        assert_eq!(search_settings("excel workbook")[0].0, Tab::General);
        assert_eq!(search_settings("default permission")[0].0, Tab::General);
        assert_eq!(search_settings("plan mode")[0].0, Tab::General);
        assert_eq!(search_settings("plan instructions")[0].0, Tab::General);
        assert_eq!(search_settings("branches")[0].0, Tab::General);
        assert_eq!(search_settings("versions kept")[0].0, Tab::General);
        assert_eq!(search_settings("notify")[0].0, Tab::Background);
        assert_eq!(search_settings("replies at once")[0].0, Tab::Background);
        assert_eq!(search_settings("reply time limit")[0].0, Tab::Background);
        assert_eq!(search_settings("command line")[0].0, Tab::Background);
        assert_eq!(
            search_settings("command line approval")[0].0,
            Tab::Background
        );
        assert_eq!(search_settings("menu bar")[0].0, Tab::Background);
        assert_eq!(search_settings("system tray")[0].0, Tab::Background);
        assert_eq!(search_settings("scheduled tasks")[0].0, Tab::Background);
        assert_eq!(search_settings("cloudflare access")[0].0, Tab::General);
        assert_eq!(search_settings("team domain")[0].0, Tab::General);
        assert_eq!(search_settings("AUD tag")[0].0, Tab::General);
        assert_eq!(search_settings("catch up")[0].0, Tab::Background);
        assert!(search_settings("zzzz").is_empty());
    }
}
