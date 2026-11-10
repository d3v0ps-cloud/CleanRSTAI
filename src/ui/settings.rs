//! Settings window: sixteen pages in four groups (App, Chats, Models & tools, This computer).
//! Edits are made on a draft and only take effect on Apply / Save. Pages are built from rows
//! (`chrome::setting_row`: label and description on the left, control on the right) under mono
//! section labels; see the helpers below (`section`, `switch`, `choice`, `text_row`, …).

mod appearance;
mod background;
mod context;
mod docker;
mod general;
mod mcp;
mod providers;
mod subagents;
mod tools;
mod voice;

use eframe::egui;

use super::chrome::{setting_row, toggle};
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
    // App
    General,
    Appearance,
    Voice,
    /// Data & privacy.
    Privacy,
    // Chats
    Permissions,
    /// Context & memory.
    Context,
    Workspace,
    /// Artifacts & files.
    Artifacts,
    // Models & tools
    Providers,
    Mcp,
    Tools,
    /// Skills & hooks.
    Extensions,
    Subagents,
    // This computer
    Docker,
    Background,
    /// Remote access.
    Remote,
}

/// A settings page's scrolling body.
fn page(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Room for the scroll bar beside the controls on the right.
            ui.set_max_width(ui.available_width() - 10.0);
            add(ui);
            ui.add_space(12.0);
        });
}

/// A section's label in mono capitals, with room above.
fn section(ui: &mut egui::Ui, title: &str) {
    let p = super::theme::palette(ui.visuals().dark_mode);
    ui.add_space(20.0);
    ui.label(super::theme::caps(title, p.pencil));
    ui.add_space(2.0);
}

/// Muted explanatory text across the page.
fn note(ui: &mut egui::Ui, text: &str) {
    let p = super::theme::palette(ui.visuals().dark_mode);
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(super::theme::size::SMALL)
                .color(p.pencil),
        )
        .wrap(),
    );
}

/// A row with an on/off switch.
fn switch(ui: &mut egui::Ui, label: &str, description: &str, on: &mut bool) {
    setting_row(ui, label, description, |ui| {
        let name = label.to_owned();
        let value = *on;
        toggle(ui, on).widget_info(move || {
            egui::WidgetInfo::selected(egui::Role::CheckBox, true, value, &name)
        });
    });
}

/// A row with a dropdown of `options`.
fn choice<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    description: &str,
    value: &mut T,
    options: &[(T, &str)],
) {
    setting_row(ui, label, description, |ui| {
        let shown = options
            .iter()
            .find(|(v, _)| v == value)
            .map_or("", |(_, l)| *l);
        egui::ComboBox::from_id_salt(id)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (v, l) in options {
                    ui.selectable_value(value, *v, *l);
                }
            });
    });
}

/// A warning line under a row, with the warning glyph.
fn warning(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!("{}  {text}", icon::WARNING))
                .size(super::theme::size::SMALL)
                .color(ui.visuals().warn_fg_color),
        )
        .wrap(),
    );
}

/// A row with a one-line text field.
fn text_row(ui: &mut egui::Ui, label: &str, description: &str, value: &mut String, hint: &str) {
    setting_row(ui, label, description, |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(250.0),
        );
    });
}

/// A label, an optional note and a text area across the page; with `default`, a Reset button
/// while the text differs from it.
fn long_text(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    value: &mut String,
    rows: usize,
    default: Option<&str>,
) {
    let p = super::theme::palette(ui.visuals().dark_mode);
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(p.ink));
        if let Some(default) = default
            && value != default
            && ui.small_button("Reset").clicked()
        {
            *value = default.to_owned();
        }
    });
    if !description.is_empty() {
        note(ui, description);
    }
    ui.add(
        egui::TextEdit::multiline(value)
            .desired_rows(rows)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(8.0);
}

/// A row choosing a provider by id; empty is "The chat's provider".
fn provider_row(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    description: &str,
    value: &mut String,
    providers: &[(String, String, Vec<String>)],
) {
    provider_row_or(
        ui,
        id,
        label,
        description,
        value,
        providers,
        "The chat's provider",
    );
}

/// [`provider_row`] with another name for "no provider chosen".
fn provider_row_or(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    description: &str,
    value: &mut String,
    providers: &[(String, String, Vec<String>)],
    none: &str,
) {
    setting_row(ui, label, description, |ui| {
        let current = providers
            .iter()
            .find(|(pid, ..)| pid == value)
            .map_or(none.to_owned(), |(_, n, _)| n.clone());
        egui::ComboBox::from_id_salt(id)
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(value, String::new(), none);
                for (pid, name, _) in providers {
                    ui.selectable_value(value, pid.clone(), name);
                }
            });
    });
}

/// A row with a model name: a field, and a menu of the provider's known models.
fn model_row(ui: &mut egui::Ui, label: &str, value: &mut String, models: &[String]) {
    setting_row(ui, label, "Empty: the provider's selected model", |ui| {
        if !models.is_empty() {
            ui.menu_button(icon::CARET_DOWN, |ui| {
                ui.set_max_height(320.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for m in models {
                        if ui.button(m).clicked() {
                            *value = m.clone();
                            ui.close();
                        }
                    }
                });
            });
        }
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text("the provider's selected model")
                .desired_width(220.0),
        );
    });
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
    /// List every release at this "Update from" setting, for the version picker.
    ListReleases(String),
    /// Install this release (any version, dev builds included).
    InstallRelease(Box<crate::update::Release>),
    /// Test Docker and the sandbox image with these settings (Settings → Docker "Test").
    CheckSandbox(SandboxSettings),
    /// Pull the image, or build the Dockerfile again (Settings → Docker "Pull now" / "Build now").
    PrepareSandbox(SandboxSettings),
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
    /// The microphones found when the Voice tab was first shown.
    mic_devices: Option<Vec<String>>,
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
    /// The releases for the version picker: listed, failed, or not asked for yet.
    pub releases: Option<Result<Vec<crate::update::Release>, String>>,
    pub listing_releases: bool,
    /// The version picked in the list.
    pub chosen_release: Option<String>,
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

/// A row with a path field and a Browse… button. `filter` limits the dialog to these
/// extensions (none: any file).
fn file_row(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    path: &mut String,
    hint: &str,
    title: &str,
    filter: &[&str],
) {
    setting_row(ui, label, description, |ui| {
        if ui.button("Browse…").clicked() {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if !filter.is_empty() {
                dialog = dialog.add_filter("Files", filter);
            }
            if let Some(file) = dialog.pick_file() {
                *path = file.display().to_string();
            }
        }
        ui.add(
            egui::TextEdit::singleline(path)
                .hint_text(hint)
                .desired_width(190.0),
        );
    });
}

/// A row with a secret field and a Show switch beside it.
fn secret_row(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    value: &mut String,
    hint: &str,
    show: &mut bool,
) {
    setting_row(ui, label, description, |ui| {
        ui.checkbox(show, "Show");
        ui.add(
            egui::TextEdit::singleline(value)
                .password(!*show)
                .hint_text(hint)
                .desired_width(190.0),
        );
    });
}

/// The Remote access page: the server's rows, then the Cloudflare Tunnel and Access sections.
fn remote_rows(
    ui: &mut egui::Ui,
    r: &mut RemoteSettings,
    show_password: &mut bool,
    status: &RemoteUiStatus,
) {
    let (warn, error) = (ui.visuals().warn_fg_color, ui.visuals().error_fg_color);
    switch(
        ui,
        "Remote access",
        "Serve the chat interface to browsers on other devices",
        &mut r.enabled,
    );
    if !r.enabled {
        return;
    }
    choice(
        ui,
        "remote-bind",
        "Listen on",
        "",
        &mut r.bind,
        &[
            (RemoteBind::Local, "This computer only"),
            (RemoteBind::Network, "Local network"),
        ],
    );
    setting_row(ui, "Port", "", |ui| {
        ui.add(egui::DragValue::new(&mut r.port).range(1..=65535));
    });
    secret_row(
        ui,
        "Password",
        "Needed to log in from a browser",
        &mut r.password,
        "",
        show_password,
    );
    choice(
        ui,
        "remote-tls",
        "HTTPS",
        match r.tls {
            RemoteTls::Off => "",
            RemoteTls::SelfSigned => {
                "Browsers warn about a self-signed certificate the first time; check the \
                 address and continue"
            }
            RemoteTls::Files => "",
        },
        &mut r.tls,
        &[
            (RemoteTls::SelfSigned, "Self-signed certificate"),
            (RemoteTls::Files, "Certificate files"),
            (RemoteTls::Off, "Off (plain HTTP)"),
        ],
    );
    if r.tls == RemoteTls::Off {
        warning(
            ui,
            "Plain HTTP: the password and chats cross the network unencrypted. Use it only on \
             a network you trust.",
        );
    }
    if r.tls == RemoteTls::Files {
        let pem = ["pem", "crt", "cer", "key"];
        file_row(
            ui,
            "Certificate file",
            "",
            &mut r.cert_path,
            "PEM file",
            "Choose the certificate file",
            &pem,
        );
        file_row(
            ui,
            "Key file",
            "",
            &mut r.key_path,
            "PEM file",
            "Choose the private key file",
            &pem,
        );
    }

    section(ui, "Status");
    ui.add_space(4.0);
    match status.server {
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
            ui.colored_label(error, e);
        }
        RemoteStatus::Off if r.password.is_empty() => {
            ui.label("Set a password to start the server");
        }
        RemoteStatus::Off => {
            ui.label("Stopped");
        }
    }
    if status.server_dirty {
        note(ui, "Apply to restart the server with these settings.");
    }

    section(ui, "Cloudflare Tunnel");
    switch(
        ui,
        "Cloudflare Tunnel",
        "Reach this computer from outside the network through a Cloudflare Tunnel",
        &mut r.tunnel.enabled,
    );
    if !r.tunnel.enabled {
        return;
    }
    let service = crate::remote::tunnel_service_url(r);
    let t = &mut r.tunnel;
    choice(
        ui,
        "tunnel-mode",
        "Tunnel type",
        match t.mode {
            TunnelMode::Quick => {
                "A random trycloudflare.com address, no account needed. The address changes \
                 each time the tunnel starts. Quick tunnels are for trying things out: \
                 Cloudflare gives them no uptime guarantee, and the browser checks for new \
                 messages every few seconds instead of live"
            }
            TunnelMode::Named => "Your own domain, with a Cloudflare account",
        },
        &mut t.mode,
        &[
            (TunnelMode::Quick, "Quick tunnel"),
            (TunnelMode::Named, "Named tunnel"),
        ],
    );
    match t.mode {
        TunnelMode::Quick => text_row(
            ui,
            "Allowed emails",
            "Cloudflare asks visitors for a one-time PIN sent to one of these addresses before \
             the login page. Empty: anyone with the address reaches the login page",
            &mut t.allowed_emails,
            "me@example.com, *@example.org",
        ),
        TunnelMode::Named => {
            secret_row(
                ui,
                "Tunnel token",
                "From the tunnel's page in the Cloudflare dashboard",
                &mut t.token,
                "",
                show_password,
            );
            text_row(
                ui,
                "Public hostname",
                "The hostname of the tunnel's route in the Cloudflare dashboard. Optional: the \
                 app also learns it from cloudflared once the tunnel connects",
                &mut t.hostname,
                "chat.example.com",
            );
        }
    }
    file_row(
        ui,
        "cloudflared program",
        "Empty: found on PATH",
        &mut t.cloudflared,
        "found on PATH",
        "Choose cloudflared",
        &[],
    );

    ui.add_space(8.0);
    match status.tunnel {
        TunnelStatus::Off => {
            ui.label("Tunnel stopped");
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
                warning(
                    ui,
                    "Connected, but the tunnel has no public hostname yet: add a route for it \
                     in the Cloudflare dashboard.",
                );
            }
            let plural = if *connections == 1 { "" } else { "s" };
            note(
                ui,
                &format!("{connections} connection{plural} to Cloudflare"),
            );
        }
        TunnelStatus::Failed { error: e, retrying } => {
            ui.colored_label(error, e);
            if *retrying {
                note(ui, "Trying again shortly.");
            }
        }
    }
    if status.tunnel_dirty {
        note(ui, "Apply to start the tunnel with these settings.");
    }
    if r.password.trim().chars().count() < crate::remote::MIN_TUNNEL_PASSWORD_CHARS {
        warning(
            ui,
            &format!(
                "Use a password of at least {} characters: the tunnel puts the server on the \
                 internet.",
                crate::remote::MIN_TUNNEL_PASSWORD_CHARS
            ),
        );
    }
    if r.tunnel.mode == TunnelMode::Named {
        let tls_note = if r.tls == RemoteTls::Off {
            ""
        } else {
            ", with No TLS Verify on"
        };
        note(
            ui,
            &format!(
                "In the Cloudflare dashboard, give the tunnel a published application route to \
                 {service}{tls_note}."
            ),
        );
    }
    access_rows(ui, &mut r.tunnel.access, r.tunnel.mode, status.access_error);
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

/// The "Cloudflare Access" section: a login gate in front of a named tunnel's hostname, set
/// up in the Cloudflare dashboard (see `remote::access`).
fn access_rows(
    ui: &mut egui::Ui,
    access: &mut AccessSettings,
    mode: TunnelMode,
    error: Option<&str>,
) {
    section(ui, "Cloudflare Access");
    if mode == TunnelMode::Quick {
        ui.add_space(4.0);
        note(
            ui,
            "Only for named tunnels: a quick tunnel's address can't be put behind an Access \
             application.",
        );
        return;
    }
    switch(
        ui,
        "Require Cloudflare Access",
        "In front of the tunnel's hostname. Requests that skip Access are refused",
        &mut access.enabled,
    );
    if !access.enabled {
        return;
    }
    text_row(
        ui,
        "Team domain",
        "The name.cloudflareaccess.com address your team signs in to",
        &mut access.team_domain,
        "myteam.cloudflareaccess.com",
    );
    text_row(
        ui,
        "Audience (AUD) tag",
        "From the Access application's overview",
        &mut access.audience,
        "",
    );
    switch(
        ui,
        "Sign in with Access",
        "A valid Access login also signs the browser in, so the app password is not asked",
        &mut access.skip_password,
    );
    ui.add_space(8.0);
    let team = crate::config::team_host(&access.team_domain);
    if !crate::remote::valid_team_domain(&team) {
        warning(
            ui,
            "Enter the team domain, e.g. myteam.cloudflareaccess.com.",
        );
    }
    if access.audience.trim().is_empty() {
        warning(ui, "Enter the application's audience (AUD) tag.");
    }
    match error {
        Some(e) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        None => note(ui, "No Access errors so far."),
    }
    note(
        ui,
        "In the Cloudflare Zero Trust dashboard, add a self-hosted application for the tunnel's \
         public hostname, with a policy for who may sign in, and copy its Application Audience \
         (AUD) tag here.",
    );
}

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

/// The web search tool's service and options (Settings → Built-in Tools).
fn web_search_rows(
    ui: &mut egui::Ui,
    s: &mut crate::config::WebSearchSettings,
    show_key: &mut bool,
) {
    use crate::config::{MAX_NATIVE_SEARCH_USES, NativeSearch, SearchBackend};
    // The provider's own search replaces the service below for providers that have one.
    let native = s.native == NativeSearch::PreferProvider;
    section(ui, "Web search");
    setting_row(
        ui,
        "Search with",
        if native {
            "Anthropic (Messages API), OpenAI (Responses API) and OpenRouter search on the \
             provider's side and cite the pages they used. Other providers keep the service \
             below; its settings are not used otherwise"
        } else {
            ""
        },
        |ui| {
            egui::ComboBox::from_id_salt("search-native")
                .selected_text(s.native.label())
                .show_ui(ui, |ui| {
                    for n in NativeSearch::ALL {
                        ui.selectable_value(&mut s.native, n, n.label());
                    }
                });
        },
    );
    if native {
        setting_row(
            ui,
            "Searches per reply",
            "The most searches the model may run for one reply",
            |ui| {
                ui.add(
                    egui::DragValue::new(&mut s.native_max_uses).range(1..=MAX_NATIVE_SEARCH_USES),
                );
            },
        );
    }
    ui.add_enabled_ui(!native, |ui| {
        setting_row(
            ui,
            "Service",
            if s.backend == SearchBackend::DuckDuckGo {
                "No account or key. DuckDuckGo may ask for a human check when it sees many \
                 searches; choose another service then"
            } else {
                ""
            },
            |ui| {
                egui::ComboBox::from_id_salt("search-backend")
                    .selected_text(s.backend.label())
                    .show_ui(ui, |ui| {
                        for b in SearchBackend::ALL {
                            ui.selectable_value(&mut s.backend, b, b.label());
                        }
                    });
            },
        );
    });
    match s.backend {
        SearchBackend::DuckDuckGo => {}
        SearchBackend::Searxng => text_row(
            ui,
            "Instance",
            "The instance must allow JSON results: add `json` to `search.formats` in its \
             settings.yml",
            &mut s.searxng_url,
            "https://searx.example.org",
        ),
        SearchBackend::Brave | SearchBackend::Tavily | SearchBackend::Kagi => secret_row(
            ui,
            "API key",
            "Kept in the system keychain when that is on (Data & privacy)",
            &mut s.api_key,
            "",
            show_key,
        ),
    }
    ui.add_enabled_ui(!native, |ui| {
        setting_row(
            ui,
            "Results per search",
            "The model may ask for more, up to 20",
            |ui| {
                ui.add(
                    egui::DragValue::new(&mut s.max_results)
                        .range(1..=crate::config::MAX_SEARCH_RESULTS),
                );
            },
        );
    });
    ui.add_enabled_ui(
        !native && !matches!(s.backend, SearchBackend::Tavily | SearchBackend::Kagi),
        |ui| switch(ui, "Safe search", "", &mut s.safe_search),
    );
    switch(
        ui,
        "Ask before each search",
        if native {
            "Doesn't apply to the provider's own search: it runs on the provider's side, so it \
             can't ask before each search"
        } else {
            "The query leaves this computer and can contain parts of the chat. Subagents and \
             auto-approved calls never ask"
        },
        &mut s.ask,
    );
    switch(
        ui,
        "Subagents may search",
        "When they may use the built-in tools",
        &mut s.subagents,
    );
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
            mic_devices: None,
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
            releases: None,
            listing_releases: false,
            chosen_release: None,
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
        memories: &[Memory],
        projects: &[(Uuid, String)],
    ) -> Vec<SettingsAction> {
        let mut actions = Vec::new();
        let dark = ui.visuals().dark_mode;

        let p = super::theme::palette(dark);
        egui::Panel::top("settings-header")
            .show_separator_line(true)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 20,
                right: 12,
                top: 12,
                bottom: 10,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Settings")
                            .size(16.0)
                            .strong()
                            .color(p.ink),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if super::widgets::icon_button(ui, icon::CLOSE, "Close settings (Esc)")
                            .clicked()
                        {
                            actions.push(SettingsAction::Close);
                        }
                    });
                });
            });

        egui::Panel::left("settings-nav")
            .resizable(false)
            .exact_size(195.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 10)))
            .show(ui, |ui| {
                search_pill(ui, &mut self.search);
                ui.add_space(6.0);
                let query = self.search.trim().to_owned();
                let query = query.as_str();
                let hits = if query.is_empty() {
                    Vec::new()
                } else {
                    search_settings(query)
                };
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = super::chrome::ROW_GAP;
                        let mut shown = 0;
                        for (group, tabs) in NAV_GROUPS {
                            // While searching, only the pages whose name or settings match.
                            let tabs: Vec<_> = tabs
                                .iter()
                                .filter(|(tab, _, label)| {
                                    query.is_empty()
                                        || super::palette::score(query, label).is_some()
                                        || hits.iter().any(|h| h.0 == *tab)
                                })
                                .collect();
                            if tabs.is_empty() {
                                continue;
                            }
                            super::chrome::section_label(ui, group);
                            for (tab, glyph, label) in tabs {
                                shown += 1;
                                if super::chrome::nav_row_sized(
                                    ui,
                                    glyph,
                                    label,
                                    self.tab == *tab,
                                    false,
                                    26.0,
                                )
                                .clicked()
                                {
                                    self.tab = *tab;
                                }
                            }
                        }
                        if !query.is_empty() {
                            if hits.is_empty() && shown == 0 {
                                ui.add_space(6.0);
                                ui.weak("No matching settings");
                            }
                            if !hits.is_empty() {
                                super::chrome::section_label(ui, "Matching settings");
                            }
                            for (tab, section, label) in hits {
                                let resp = ui.add(
                                    egui::Button::selectable(self.tab == tab, label)
                                        .right_text(egui::RichText::new(section).small().weak())
                                        .truncate()
                                        .min_size(egui::vec2(ui.available_width(), 28.0)),
                                );
                                if resp.clicked() {
                                    self.tab = tab;
                                    self.search.clear();
                                }
                            }
                        }
                    });
            });

        egui::Panel::bottom("settings-buttons")
            .show_separator_line(true)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let dirty = &self.draft != live;
                    if ui
                        .add_enabled(dirty, super::theme::primary_button("Save", dark))
                        .clicked()
                    {
                        actions.push(SettingsAction::Apply(Box::new(self.draft.clone())));
                        actions.push(SettingsAction::Close);
                    }
                    if ui.add_enabled(dirty, egui::Button::new("Apply")).clicked() {
                        actions.push(SettingsAction::Apply(Box::new(self.draft.clone())));
                    }
                    if ui.button(if dirty { "Cancel" } else { "Close" }).clicked() {
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

        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 24,
                right: 16,
                top: 16,
                bottom: 8,
            }))
            .show(ui, |ui| {
                let (title, about) = match self.tab {
                    Tab::General => (
                        "General",
                        "The system prompt, your name, the send key and updates.",
                    ),
                    Tab::Appearance => ("Appearance", "Theme, size, motion and how replies look."),
                    Tab::Voice => (
                        "Voice",
                        "Dictate into the message box, and read replies aloud: the speech \
                         providers, models and voices.",
                    ),
                    Tab::Privacy => (
                        "Data & privacy",
                        "Where secrets are kept, what replies may load from the web, export and \
                         import, and where the app keeps its files.",
                    ),
                    Tab::Permissions => (
                        "Permissions",
                        "What runs without asking, how far one reply may go, and plan mode.",
                    ),
                    Tab::Context => (
                        "Context & memory",
                        "What a chat carries along: compaction of long chats, memory across \
                         chats, projects, and the versions a regenerated reply keeps.",
                    ),
                    Tab::Workspace => (
                        "Workspace",
                        "What the model may do in a chat's working folder: the file and shell \
                         tools, commits, project instructions, undo for file edits and code review.",
                    ),
                    Tab::Artifacts => (
                        "Artifacts & files",
                        "The artifact panel, live web pages, the Save as formats, and reading \
                         scanned PDFs.",
                    ),
                    Tab::Providers => (
                        "Providers",
                        "OpenAI-compatible endpoints the app can talk to.",
                    ),
                    Tab::Mcp => (
                        "MCP servers",
                        "Model Context Protocol servers that give the model tools, resources and \
                         prompts.",
                    ),
                    Tab::Tools => (
                        "Tools",
                        "Tools that ship with the app, so they work without an MCP server.",
                    ),
                    Tab::Extensions => (
                        "Skills & hooks",
                        "Saved prompts you type as /name, skills the model loads when a task \
                         matches, and hooks: commands that run around tool calls and when a \
                         reply is done.",
                    ),
                    Tab::Subagents => (
                        "Subagents",
                        "Let the model delegate independent tasks to agents that work in \
                         parallel and report back.",
                    ),
                    Tab::Docker => (
                        "Docker sandbox",
                        "Run the workspace shell tool in a Docker container instead of on this \
                         computer.",
                    ),
                    Tab::Background => (
                        "Background",
                        "Keeping the app running, notifications, how many replies run at once, \
                         and the command line.",
                    ),
                    Tab::Remote => (
                        "Remote access",
                        "Use your chats from a browser on another device: the built-in server, \
                         a Cloudflare Tunnel and Cloudflare Access.",
                    ),
                };
                ui.label(super::theme::display(title, super::theme::size::TITLE).color(p.ink));
                note(ui, about);
                ui.add_space(8.0);
                match self.tab {
                    Tab::General => self.general_tab(ui, &mut actions),
                    Tab::Appearance => self.appearance_tab(ui),
                    Tab::Voice => self.voice_tab(ui),
                    Tab::Privacy => self.privacy_tab(ui, live, &mut actions),
                    Tab::Permissions => self.permissions_tab(ui),
                    Tab::Context => self.context_tab(ui, memories, projects, &mut actions),
                    Tab::Workspace => self.workspace_tab(ui),
                    Tab::Artifacts => self.artifacts_tab(ui),
                    Tab::Providers => self.providers_tab(ui, &mut actions),
                    Tab::Mcp => self.mcp_tab(ui, live, servers, &mut actions),
                    Tab::Tools => self.tools_tab(ui),
                    Tab::Extensions => self.extensions_tab(ui),
                    Tab::Subagents => self.subagents_tab(ui),
                    Tab::Docker => self.docker_tab(ui, &mut actions),
                    Tab::Background => self.background_tab(ui),
                    Tab::Remote => self.remote_tab(ui, live, remote, (tunnel, access_error)),
                }
            });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && &self.draft == live {
            actions.push(SettingsAction::Close);
        }
        actions
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
    switch(
        ui,
        "New chats use memory",
        "Facts the model saves when you ask it to remember something, or that you state about \
         yourself. A chat with memory on gets them in its system prompt with every request, so \
         they are sent to your provider. Off, or a chat switched to incognito under Chat \
         options, uses none of them and the model can't save any",
        &mut m.enabled,
    );
    switch(
        ui,
        "Ask before saving or deleting a memory",
        "Off, the model saves and deletes memories without asking",
        &mut m.ask_before_saving,
    );
    setting_row(
        ui,
        "Size limit",
        "The memories sent with each request, newest first; older ones are left out past this",
        |ui| {
            ui.add(
                egui::DragValue::new(&mut m.max_kb)
                    .range(MIN_KB..=MAX_KB)
                    .suffix(" KB"),
            );
        },
    );
    choice(
        ui,
        "memory-scope",
        "Scope",
        "Global: one set of memories for every chat. Per project: a project's chats also see \
         and save that project's memories, besides the global ones",
        &mut m.scope,
        &[
            (MemoryScope::Global, "Global"),
            (MemoryScope::PerProject, "Per project"),
        ],
    );

    ui.add_space(10.0);
    editor
        .drafts
        .retain(|id, _| memories.iter().any(|m| m.id == *id));
    let ink = super::theme::palette(ui.visuals().dark_mode).ink;
    ui.label(egui::RichText::new(format!("Saved memories ({})", memories.len())).color(ink));
    if memories.is_empty() {
        note(ui, "None yet.");
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
    ui.label(egui::RichText::new("Add a memory").color(ink));
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
    switch(
        ui,
        "Slash commands",
        "Type / and a name in the message box to send a saved prompt. {{args}} in a prompt is \
         replaced by what you type after the name; without it, that text is added after a \
         blank line. Names use letters, digits, - and _",
        &mut s.enabled,
    );
    ui.add_enabled_ui(s.enabled, |ui| {
        switch(
            ui,
            "Also load commands from the working folder",
            "Each .cleanrstai/commands/<name>.md file in a chat's folder is a command. A first \
             line description: ... gives its description; a command there replaces one here \
             with the same name",
            &mut s.workspace_commands,
        );
        ui.add_space(10.0);
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
    switch(
        ui,
        "Run hooks",
        "Commands that run around the model's tool calls: before a tool runs (a non-zero exit \
         or a timeout blocks the call), after it (the output is added to the result), and when \
         a reply is done (the output is shown as a notice). Each command gets the call in \
         CLEANRSTAI_* environment variables. This is the default for each chat; a chat's own \
         Hooks switch under Chat options overrides it",
        enabled,
    );
    ui.add_space(6.0);
    warning(
        ui,
        "Hooks run commands with your user's rights on this computer, or in the Docker sandbox \
         when it's on. They are read only from these settings, never from a workspace or from \
         the model.",
    );
    ui.add_space(6.0);
    ui.add_enabled_ui(*enabled, |ui| {
        let mut remove = None;
        for (i, hook) in hooks.iter_mut().enumerate() {
            ui.push_id(("hook", i), |ui| {
                ui.horizontal(|ui| {
                    toggle(ui, &mut hook.enabled).on_hover_text("Run this hook");
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
/// The settings pages in the navigation, by group: page, icon, label.
type NavPage = (Tab, &'static str, &'static str);

const NAV_GROUPS: &[(&str, &[NavPage])] = &[
    (
        "App",
        &[
            (Tab::General, icon::GEAR, "General"),
            (Tab::Appearance, icon::PALETTE, "Appearance"),
            (Tab::Voice, icon::MICROPHONE, "Voice"),
            (Tab::Privacy, icon::SHIELD, "Data & privacy"),
        ],
    ),
    (
        "Chats",
        &[
            (Tab::Permissions, icon::SHIELD_CHECK, "Permissions"),
            (Tab::Context, icon::STACK, "Context & memory"),
            (Tab::Workspace, icon::FOLDER, "Workspace"),
            (Tab::Artifacts, icon::SHAPES, "Artifacts & files"),
        ],
    ),
    (
        "Models & tools",
        &[
            (Tab::Providers, icon::CLOUD, "Providers"),
            (Tab::Mcp, icon::PLUGS, "MCP servers"),
            (Tab::Tools, icon::TOOLBOX, "Tools"),
            (Tab::Extensions, icon::BOOKS, "Skills & hooks"),
            (Tab::Subagents, icon::ROBOT, "Subagents"),
        ],
    ),
    (
        "This computer",
        &[
            (Tab::Docker, icon::TERMINAL, "Docker sandbox"),
            (Tab::Background, icon::CLOCK, "Background"),
            (Tab::Remote, icon::LAPTOP, "Remote access"),
        ],
    ),
];

/// A dropdown of the microphones `record::input_devices` found, "System default" first.
pub fn mic_picker(ui: &mut egui::Ui, device: &mut String, devices: &[String]) -> bool {
    let before = device.clone();
    egui::ComboBox::from_id_salt("mic-device")
        .selected_text(if device.is_empty() {
            "System default"
        } else {
            device.as_str()
        })
        .truncate()
        .width(240.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(device, String::new(), "System default");
            for d in devices {
                ui.selectable_value(device, d.clone(), d);
            }
            // A saved microphone that isn't plugged in stays chosen until another is picked.
            if !device.is_empty() && !devices.contains(device) {
                ui.label(egui::RichText::new(format!("{device} (not found)")).weak());
            }
        });
    *device != before
}

/// The UI scale choices.
const ZOOM_STEPS: [f32; 8] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// The settings search field: a rounded pill with a magnifier, ringed while focused.
fn search_pill(ui: &mut egui::Ui, text: &mut String) {
    let p = super::theme::palette(ui.visuals().dark_mode);
    let id = egui::Id::unique("settings-search");
    let focused = ui.memory(|m| m.has_focus(id));
    egui::Frame::new()
        .fill(p.raised)
        .stroke(egui::Stroke::new(
            1.0,
            if focused { p.pencil } else { p.hairline },
        ))
        .corner_radius(super::theme::radius::MD)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(icon::SEARCH).size(14.0).color(p.pencil));
                ui.add(
                    egui::TextEdit::singleline(text)
                        .id(id)
                        .frame(egui::Frame::NONE)
                        .hint_text(egui::RichText::new("Search").color(p.pencil))
                        .desired_width(f32::INFINITY),
                );
            });
        });
}

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
    (Tab::Appearance, "Appearance", "Theme (light / dark)"),
    (Tab::Appearance, "Appearance", "UI scale / zoom"),
    (Tab::General, "General", "Send with Enter"),
    (Tab::Privacy, "Data & privacy", "Keychain / secrets"),
    (Tab::Remote, "Remote access", "Remote access (browser)"),
    (Tab::Remote, "Remote access", "Remote access password"),
    (Tab::Remote, "Remote access", "Remote access port"),
    (
        Tab::Remote,
        "Remote access",
        "Remote access HTTPS certificate",
    ),
    (Tab::Remote, "Remote access", "Cloudflare Tunnel"),
    (Tab::Remote, "Remote access", "Cloudflare Tunnel token"),
    (
        Tab::Remote,
        "Remote access",
        "Cloudflare Tunnel public hostname",
    ),
    (
        Tab::Remote,
        "Remote access",
        "Cloudflare quick tunnel (trycloudflare.com)",
    ),
    (Tab::Remote, "Remote access", "cloudflared path"),
    (Tab::Remote, "Remote access", "Cloudflare Access"),
    (
        Tab::Remote,
        "Remote access",
        "Cloudflare Access team domain",
    ),
    (
        Tab::Remote,
        "Remote access",
        "Cloudflare Access audience (AUD) tag",
    ),
    (
        Tab::Remote,
        "Remote access",
        "Cloudflare Access sign in without the app password",
    ),
    (Tab::Appearance, "Appearance", "Show speed stats"),
    (
        Tab::Artifacts,
        "Artifacts & files",
        "Save as Word document (.docx)",
    ),
    (Tab::Artifacts, "Artifacts & files", "Save as PDF"),
    (
        Tab::Artifacts,
        "Artifacts & files",
        "Save as Excel workbook (.xlsx)",
    ),
    (Tab::Artifacts, "Artifacts & files", "Save reply as file"),
    (Tab::Privacy, "Data & privacy", "Load web images in replies"),
    (
        Tab::Appearance,
        "Appearance",
        "Reasoning / thinking display (collapsed by default)",
    ),
    (Tab::General, "General", "Your name (greeting)"),
    (Tab::Appearance, "Appearance", "Reduce motion / animations"),
    (
        Tab::Artifacts,
        "Artifacts & files",
        "Artifacts in a side panel",
    ),
    (Tab::Voice, "Voice", "Microphone (input device)"),
    (Tab::Permissions, "Permissions", "Auto-approve tool calls"),
    (
        Tab::Permissions,
        "Permissions",
        "Default permission mode for new chats",
    ),
    (Tab::Permissions, "Permissions", "Plan mode"),
    (Tab::Permissions, "Permissions", "Plan mode after approval"),
    (Tab::Permissions, "Permissions", "Plan approval message"),
    (Tab::Permissions, "Permissions", "Plan instructions"),
    (Tab::Context, "Context & memory", "Branches"),
    (
        Tab::Context,
        "Context & memory",
        "Keep the version a reply replaces",
    ),
    (
        Tab::Context,
        "Context & memory",
        "Versions kept per message",
    ),
    (Tab::Permissions, "Permissions", "Max tool rounds"),
    (Tab::Permissions, "Permissions", "Tool timeout"),
    (Tab::Privacy, "Data & privacy", "Export settings"),
    (Tab::Privacy, "Data & privacy", "Export conversations"),
    (Tab::Privacy, "Data & privacy", "Import settings"),
    (Tab::Privacy, "Data & privacy", "Import conversations"),
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
        "Context & memory",
        "Context compaction (summarise long chats)",
    ),
    (
        Tab::Context,
        "Context & memory",
        "Compaction threshold and context size",
    ),
    (Tab::Context, "Context & memory", "Compaction summary model"),
    (Tab::Workspace, "Workspace", "Code review (review changes)"),
    (Tab::Workspace, "Workspace", "Review prompt and model"),
    (Tab::Workspace, "Workspace", "Code review strictness"),
    (
        Tab::Workspace,
        "Workspace",
        "Project instructions (AGENTS.md, CLAUDE.md)",
    ),
    (Tab::Context, "Context & memory", "Projects (on / off)"),
    (Tab::Context, "Context & memory", "Memory (on / off)"),
    (
        Tab::Context,
        "Context & memory",
        "Memory: ask before saving",
    ),
    (Tab::Context, "Context & memory", "Memory: size limit"),
    (
        Tab::Context,
        "Context & memory",
        "Memory: global or per project",
    ),
    (Tab::Context, "Context & memory", "Memory: saved memories"),
    (
        Tab::Context,
        "Context & memory",
        "Memory: delete all memories",
    ),
    (
        Tab::Context,
        "Context & memory",
        "Projects: files size limit",
    ),
    (
        Tab::Context,
        "Context & memory",
        "Projects: default instructions",
    ),
    (Tab::Context, "Context & memory", "Projects: default folder"),
    (
        Tab::Context,
        "Context & memory",
        "Projects: default provider",
    ),
    (Tab::Workspace, "Workspace", "Diff preview for file edits"),
    (Tab::Workspace, "Workspace", "Undo file edits (checkpoints)"),
    (
        Tab::Artifacts,
        "Artifacts & files",
        "Read scanned PDFs (OCR)",
    ),
    (
        Tab::Artifacts,
        "Artifacts & files",
        "OCR engine and vision model",
    ),
    (
        Tab::Artifacts,
        "Artifacts & files",
        "Tesseract program and languages",
    ),
    (
        Tab::Artifacts,
        "Artifacts & files",
        "Live web page preview (scripts, network)",
    ),
    (
        Tab::Workspace,
        "Workspace",
        "Workspace tools (files, search, shell)",
    ),
    (Tab::Workspace, "Workspace", "Commit changes (git_commit)"),
    (Tab::Workspace, "Workspace", "Allow signed commits"),
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
    (
        Tab::Permissions,
        "Permissions",
        "Reply limits (tokens, cost)",
    ),
    (
        Tab::Privacy,
        "Data & privacy",
        "Settings and conversations file locations",
    ),
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
        assert_eq!(search_settings("theme")[0].0, Tab::Appearance);
        assert_eq!(search_settings("reduce motion")[0].0, Tab::Appearance);
        assert_eq!(search_settings("oauth")[0].0, Tab::Mcp);
        assert_eq!(search_settings("calculator")[0].0, Tab::Tools);
        assert_eq!(search_settings("subagent model")[0].0, Tab::Subagents);
        assert_eq!(search_settings("scanned pdf")[0].0, Tab::Artifacts);
        assert_eq!(search_settings("tesseract")[0].0, Tab::Artifacts);
        assert_eq!(search_settings("agents.md")[0].0, Tab::Workspace);
        assert_eq!(search_settings("projects")[0].0, Tab::Context);
        assert_eq!(search_settings("memory")[0].0, Tab::Context);
        assert_eq!(search_settings("memory size")[0].0, Tab::Context);
        assert_eq!(search_settings("memory per project")[0].0, Tab::Context);
        assert_eq!(search_settings("delete all memories")[0].0, Tab::Context);
        assert_eq!(search_settings("project files size")[0].0, Tab::Context);
        assert_eq!(search_settings("compaction")[0].0, Tab::Context);
        assert_eq!(search_settings("review changes")[0].0, Tab::Workspace);
        assert_eq!(search_settings("review prompt")[0].0, Tab::Workspace);
        assert_eq!(search_settings("searxng")[0].0, Tab::Tools);
        assert_eq!(search_settings("git commit")[0].0, Tab::Workspace);
        assert_eq!(search_settings("signed commits")[0].0, Tab::Workspace);
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
        assert_eq!(search_settings("web page preview")[0].0, Tab::Artifacts);
        assert_eq!(search_settings("save as pdf")[0].0, Tab::Artifacts);
        assert_eq!(search_settings("excel workbook")[0].0, Tab::Artifacts);
        assert_eq!(search_settings("default permission")[0].0, Tab::Permissions);
        assert_eq!(search_settings("plan mode")[0].0, Tab::Permissions);
        assert_eq!(search_settings("plan instructions")[0].0, Tab::Permissions);
        assert_eq!(search_settings("branches")[0].0, Tab::Context);
        assert_eq!(search_settings("versions kept")[0].0, Tab::Context);
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
        assert_eq!(search_settings("keychain")[0].0, Tab::Privacy);
        assert_eq!(search_settings("export settings")[0].0, Tab::Privacy);
        assert_eq!(search_settings("reply limits")[0].0, Tab::Permissions);
        assert_eq!(search_settings("cloudflare access")[0].0, Tab::Remote);
        assert_eq!(search_settings("team domain")[0].0, Tab::Remote);
        assert_eq!(search_settings("AUD tag")[0].0, Tab::Remote);
        assert!(search_settings("zzzz").is_empty());
    }
}
