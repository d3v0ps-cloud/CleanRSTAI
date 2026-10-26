//! The HTTP side of browser remote access: routes, login, the bridge from requests to
//! `RemoteCommand`s, server-sent events, the embedded browser client and security headers.

use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::serve::{IncomingStream, Listener};
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use uuid::Uuid;

use super::auth::Auth;
use super::{RemoteCommand, RemoteEvent, TunnelHosts};
use crate::agent::{AppEvent, EventSender};
use crate::approval::Approval;
use crate::config::{RemoteSettings, RemoteTls};

const CSRF_HEADER: HeaderName = HeaderName::from_static("x-cleanrstai");
const CSP: &str = "default-src 'self'; img-src 'self' data:; style-src 'self'; \
    script-src 'self'; connect-src 'self'; font-src 'self'; frame-ancestors 'none'; \
    base-uri 'none'; form-action 'self'";
const BODY_LIMIT: usize = 1024 * 1024;
const MAX_TEXT_CHARS: usize = 200_000;
const APP_TIMEOUT: Duration = Duration::from_secs(10);
/// Every failed password check waits this long before the answer.
const FAILURE_DELAY: Duration = Duration::from_millis(400);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// TLS handshakes in progress at once; further connections wait in the accept queue.
const MAX_HANDSHAKES: usize = 64;
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How often an open event stream checks that its session still exists.
#[cfg(not(test))]
const SESSION_CHECK: Duration = Duration::from_secs(15);
#[cfg(test)]
const SESSION_CHECK: Duration = Duration::from_millis(200);
const FONT_CACHE: &str = "public, max-age=604800";

/// What every handler needs. Shared by all connections.
pub(crate) struct AppState {
    auth: Auth,
    tx: EventSender,
    repaint: Arc<dyn Fn() + Send + Sync>,
    events: broadcast::Sender<RemoteEvent>,
    /// The connections are TLS.
    tls: bool,
    /// Send `Strict-Transport-Security`: only for a certificate the user supplied.
    hsts: bool,
    /// This computer's host name, lowercase, read once at start.
    machine_host: Option<String>,
    /// Public hostnames of the running Cloudflare Tunnel; empty without one.
    tunnel_hosts: TunnelHosts,
}

/// Whether responses carry `Strict-Transport-Security`. Only for certificate files the user
/// chose: the self-signed certificate is for localhost and LAN addresses, so pinning HSTS to it
/// would lock browsers out once the certificate changes.
pub(crate) fn hsts_enabled(cfg: &RemoteSettings) -> bool {
    cfg.tls == RemoteTls::Files
}

/// Serves the browser client and API on `listener` until the task is aborted. With `tls`,
/// every connection is wrapped in TLS first.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    listener: TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    auth: Auth,
    tx: EventSender,
    repaint: Arc<dyn Fn() + Send + Sync>,
    events: broadcast::Sender<RemoteEvent>,
    hsts: bool,
    tunnel_hosts: TunnelHosts,
) {
    let state = Arc::new(AppState {
        auth,
        tx,
        repaint,
        events,
        tls: tls.is_some(),
        hsts,
        machine_host: machine_host(),
        tunnel_hosts,
    });
    let app = router(state).into_make_service_with_connect_info::<Peer>();
    let result = match tls {
        Some(config) => axum::serve(TlsListener::new(listener, config), app).await,
        None => axum::serve(listener, app).await,
    };
    if let Err(e) = result {
        log::error!("The remote access server stopped: {e}");
    }
}

fn router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/api/state", get(state_view))
        .route("/api/chats", post(new_chat))
        .route("/api/chats/{id}", get(chat_view).delete(delete_chat))
        .route("/api/chats/{id}/messages", post(send_message))
        .route("/api/chats/{id}/stop", post(stop_chat))
        .route("/api/chats/{id}/regenerate", post(regenerate))
        .route("/api/chats/{id}/approvals/{call_id}", post(approve))
        .route("/api/chats/{id}/rename", post(rename))
        .route("/api/chats/{id}/pin", post(pin))
        .route("/api/model", post(set_model))
        .route("/api/events", get(events))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_login));

    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/app.js", get(app_js))
        .route("/markdown.js", get(markdown_js))
        .route("/style.css", get(style_css))
        .route("/favicon.png", get(favicon))
        .route("/fonts/{file}", get(font))
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        // Not behind the login: a stale or unknown cookie must still be cleared.
        .route("/api/logout", post(logout))
        .merge(protected)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn(csrf_guard))
        .layer(middleware::from_fn_with_state(state.clone(), host_guard))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers,
        ))
        .with_state(state)
}

/// Every non-GET request must carry `X-CleanRSTAI: 1`. A browser sends that header only from
/// this page's script, so a form on another site cannot make a request that gets through.
async fn csrf_guard(req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD);
    let marked = req
        .headers()
        .get(&CSRF_HEADER)
        .is_some_and(|v| v.as_bytes() == b"1");
    if safe || marked {
        next.run(req).await
    } else {
        ApiError::new(StatusCode::FORBIDDEN, "missing X-CleanRSTAI header").into_response()
    }
}

/// Rejects requests whose `Host` is not this computer or the tunnel's public hostname, which
/// stops DNS rebinding: a page on another site that resolves to 127.0.0.1 still sends its own
/// host name.
async fn host_guard(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let allowed = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|host| {
            host_allowed(host, state.machine_host.as_deref()) || tunnel_host(&state, host)
        });
    if allowed {
        next.run(req).await
    } else {
        ApiError::new(StatusCode::MISDIRECTED_REQUEST, "unknown host").into_response()
    }
}

/// True when `host` is one of the running tunnel's public hostnames.
fn tunnel_host(state: &AppState, host: &str) -> bool {
    let Some(name) = host_name(host) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    state.tunnel_hosts.read().unwrap().contains(&name)
}

/// Who is asking, for the login throttle.
struct Client {
    ip: IpAddr,
    /// The request came through the Cloudflare Tunnel.
    tunnel: bool,
}

/// Requests through the tunnel reach the server from cloudflared on this computer, so for
/// those the visitor's address is Cloudflare's `Cf-Connecting-IP`. The header is trusted only
/// on a loopback connection addressed to the tunnel's hostname, where only cloudflared (or
/// another program on this computer) can have set it.
fn client(state: &AppState, peer: SocketAddr, headers: &HeaderMap) -> Client {
    let via_tunnel = peer.ip().is_loopback()
        && headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|host| tunnel_host(state, host));
    if !via_tunnel {
        return Client {
            ip: peer.ip(),
            tunnel: false,
        };
    }
    let visitor = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok());
    Client {
        ip: visitor.unwrap_or(peer.ip()),
        tunnel: true,
    }
}

impl AppState {
    fn throttled(&self, client: &Client) -> bool {
        self.auth.throttled(client.ip) || (client.tunnel && self.auth.tunnel_throttled())
    }

    fn record_failure(&self, client: &Client) {
        self.auth.record_failure(client.ip);
        if client.tunnel {
            self.auth.record_tunnel_failure();
        }
    }
}

/// True for `localhost`, any IP literal, names ending in `.local` (mDNS) and this computer's
/// own host name. The port is ignored; names compare case-insensitively.
fn host_allowed(host: &str, machine: Option<&str>) -> bool {
    let Some(name) = host_name(host) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    if name.parse::<IpAddr>().is_ok() {
        return true;
    }
    name == "localhost" || name.ends_with(".local") || machine.is_some_and(|m| m == name)
}

/// The name part of a `Host` value: `example.com:8484` gives `example.com`, `[::1]:8484` gives
/// `::1`. None for a value that is not host[:port].
fn host_name(host: &str) -> Option<&str> {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        let (addr, after) = rest.split_once(']')?;
        return (after.is_empty() || after.starts_with(':')).then_some(addr);
    }
    match host.matches(':').count() {
        0 => Some(host),
        1 => host.split_once(':').map(|(name, _)| name),
        // A bare IPv6 address has no brackets, so it cannot be a valid Host.
        _ => None,
    }
}

/// This computer's host name, lowercase. None when it cannot be read.
#[cfg(unix)]
fn machine_host() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for `buf.len()` bytes and gethostname NUL-terminates on success.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    if !ok {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8(buf[..end].to_vec()).ok()?;
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

#[cfg(not(unix))]
fn machine_host() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_ascii_lowercase())
}

async fn security_headers(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let api = req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    if api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if state.hsts {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    res
}

/// The API needs a session cookie or the password as a bearer token. A wrong bearer token
/// counts as a failed login: it is throttled and slowed down the same way.
async fn require_login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(Peer(addr)): ConnectInfo<Peer>,
    req: Request,
    next: Next,
) -> Response {
    let headers = req.headers();
    if session_valid(&state, headers) {
        return next.run(req).await;
    }
    let Some(bearer) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return ApiError::new(StatusCode::UNAUTHORIZED, "login required").into_response();
    };
    let who = client(&state, addr, headers);
    if state.throttled(&who) {
        return too_many_failures();
    }
    if state.auth.check_bearer(bearer) {
        return next.run(req).await;
    }
    state.record_failure(&who);
    tokio::time::sleep(FAILURE_DELAY).await;
    ApiError::new(StatusCode::UNAUTHORIZED, "login required").into_response()
}

fn too_many_failures() -> Response {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too many failed logins; wait a minute and try again",
    )
    .into_response()
}

/// The session token from the `Cookie` headers, if any.
fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(Auth::token_from_cookie_header)
}

fn session_valid(state: &AppState, headers: &HeaderMap) -> bool {
    cookie_token(headers).is_some_and(|token| state.auth.valid_session(token))
}

async fn index() -> Response {
    text_asset("text/html; charset=utf-8", include_str!("web/index.html"))
}

async fn app_js() -> Response {
    text_asset("text/javascript; charset=utf-8", include_str!("web/app.js"))
}

async fn markdown_js() -> Response {
    text_asset(
        "text/javascript; charset=utf-8",
        include_str!("web/markdown.js"),
    )
}

async fn style_css() -> Response {
    text_asset("text/css; charset=utf-8", include_str!("web/style.css"))
}

fn text_asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

async fn favicon() -> Response {
    (
        [(header::CONTENT_TYPE, "image/png")],
        &include_bytes!("../../assets/icon/window-light.png")[..],
    )
        .into_response()
}

/// The fonts style.css refers to, from `assets/fonts`. Other names are 404.
async fn font(Path(file): Path<String>) -> Response {
    let bytes: Option<&'static [u8]> = match file.as_str() {
        "InstrumentSans.ttf" => Some(&include_bytes!("../../assets/fonts/InstrumentSans.ttf")[..]),
        "IBMPlexMono-Regular.ttf" => {
            Some(&include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf")[..])
        }
        "Phosphor.ttf" => Some(&include_bytes!("../../assets/fonts/Phosphor.ttf")[..]),
        _ => None,
    };
    match bytes {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, "font/ttf"),
                (header::CACHE_CONTROL, FONT_CACHE),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Only the session cookie counts here. A bearer password is checked by `require_login`, which
/// throttles failures; answering it here would be an unthrottled password oracle.
async fn session(
    State(state): State<Arc<AppState>>,
    ConnectInfo(Peer(addr)): ConnectInfo<Peer>,
    headers: HeaderMap,
) -> Json<Value> {
    // Visitors through the tunnel reach Cloudflare over HTTPS whatever this server speaks.
    let tunnel = client(&state, addr, &headers).tunnel;
    Json(json!({
        "logged_in": session_valid(&state, &headers),
        "plain_http": !state.tls && !tunnel,
    }))
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(Peer(addr)): ConnectInfo<Peer>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let who = client(&state, addr, &headers);
    if state.throttled(&who) {
        return Ok(too_many_failures());
    }
    let body: LoginBody = parse_body(&body)?;
    if !state.auth.check_password(&body.password) {
        state.record_failure(&who);
        tokio::time::sleep(FAILURE_DELAY).await;
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "wrong password"));
    }
    state.auth.clear_failures(who.ip);
    let token = state.auth.create_session();
    Ok((
        [(header::SET_COOKIE, state.auth.session_cookie(&token))],
        Json(json!({})),
    )
        .into_response())
}

/// Ends the session the cookie names, if any, and clears the cookie. Always 200.
async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_token(&headers) {
        state.auth.remove_session(token);
    }
    (
        [(header::SET_COOKIE, state.auth.clear_cookie())],
        Json(json!({})),
    )
        .into_response()
}

async fn state_view(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let view = ask(&state, |reply| RemoteCommand::State { reply }).await?;
    Ok(Json(json!(view)))
}

async fn chat_view(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = chat_id(&id)?;
    match ask(&state, |reply| RemoteCommand::Chat { id, reply }).await? {
        Some(view) => Ok(Json(json!(view))),
        None => Err(not_found()),
    }
}

async fn new_chat(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let id = ask(&state, |reply| RemoteCommand::NewChat { reply }).await?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

/// `id` is `new` to start a chat with the message.
async fn send_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let body: TextBody = parse_body(&body)?;
    if body.text.chars().count() > MAX_TEXT_CHARS {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "the message is longer than 200000 characters",
        ));
    }
    let chat = if id == "new" {
        None
    } else {
        Some(chat_id(&id)?)
    };
    let text = body.text;
    let id = ask(&state, |reply| RemoteCommand::Send { chat, text, reply })
        .await?
        .map_err(command_failed)?;
    Ok(Json(json!({ "id": id })))
}

async fn stop_chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let chat = chat_id(&id)?;
    ask(&state, |reply| RemoteCommand::Stop { chat, reply }).await?;
    Ok(Json(json!({})))
}

async fn regenerate(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let chat = chat_id(&id)?;
    ask(&state, |reply| RemoteCommand::Regenerate { chat, reply })
        .await?
        .map_err(command_failed)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct ApprovalBody {
    answer: Approval,
}

async fn approve(
    State(state): State<Arc<AppState>>,
    Path((id, call_id)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let chat = chat_id(&id)?;
    let body: ApprovalBody = parse_body(&body)?;
    ask(&state, |reply| RemoteCommand::Approve {
        chat,
        call_id,
        answer: body.answer,
        reply,
    })
    .await?
    .map_err(|message| ApiError::new(StatusCode::NOT_FOUND, message))?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

async fn rename(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let chat = chat_id(&id)?;
    let body: RenameBody = parse_body(&body)?;
    ask(&state, |reply| RemoteCommand::Rename {
        chat,
        title: body.title,
        reply,
    })
    .await?
    .map_err(command_failed)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct PinBody {
    pinned: bool,
}

async fn pin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let chat = chat_id(&id)?;
    let body: PinBody = parse_body(&body)?;
    ask(&state, |reply| RemoteCommand::Pin {
        chat,
        pinned: body.pinned,
        reply,
    })
    .await?
    .map_err(command_failed)?;
    Ok(Json(json!({})))
}

async fn delete_chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let chat = chat_id(&id)?;
    ask(&state, |reply| RemoteCommand::Delete { chat, reply }).await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct ModelBody {
    provider: usize,
    model: String,
}

async fn set_model(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let body: ModelBody = parse_body(&body)?;
    ask(&state, |reply| RemoteCommand::SetModel {
        provider: body.provider,
        model: body.model,
        reply,
    })
    .await?
    .map_err(command_failed)?;
    Ok(Json(json!({})))
}

/// The state of one open event stream.
struct Watch {
    /// The `hello` event is still to be sent.
    hello: bool,
    rx: broadcast::Receiver<RemoteEvent>,
    ticker: tokio::time::Interval,
    state: Arc<AppState>,
    /// The session cookie the stream was opened with. None when the request used the bearer
    /// password, which cannot change without a restart, so the stream keeps going.
    token: Option<String>,
}

/// Server-sent events: one `data:` line per `RemoteEvent`. A client that fell behind gets a
/// `list` event so it refetches everything. The stream ends when its session is logged out or
/// expires, so the browser stops receiving chats it can no longer see.
async fn events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let token = cookie_token(&headers)
        .filter(|token| state.auth.session_alive(token))
        .map(str::to_owned);
    let watch = Watch {
        hello: true,
        rx: state.events.subscribe(),
        ticker: tokio::time::interval(SESSION_CHECK),
        state: state.clone(),
        token,
    };
    let stream = futures_util::stream::unfold(watch, |mut w| async move {
        if std::mem::take(&mut w.hello) {
            // Sent at once, so a browser can tell when a proxy holds the stream back (quick
            // tunnels buffer event streams) and poll instead.
            let event = Event::default().data(r#"{"type":"hello"}"#);
            return Some((Ok::<Event, Infallible>(event), w));
        }
        loop {
            let received = tokio::select! {
                received = w.rx.recv() => Some(received),
                _ = w.ticker.tick() => None,
            };
            match received {
                Some(Ok(event)) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    return Some((Ok::<Event, Infallible>(Event::default().data(data)), w));
                }
                Some(Err(broadcast::error::RecvError::Lagged(_))) => {
                    let event = Event::default().data(r#"{"type":"list"}"#);
                    return Some((Ok(event), w));
                }
                Some(Err(broadcast::error::RecvError::Closed)) => return None,
                None => {
                    let gone = w
                        .token
                        .as_deref()
                        .is_some_and(|token| !w.state.auth.session_alive(token));
                    if gone {
                        return None;
                    }
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}

/// Sends a command to the app and waits for its answer. The app is woken with `repaint`.
async fn ask<T>(
    state: &AppState,
    make: impl FnOnce(oneshot::Sender<T>) -> RemoteCommand,
) -> Result<T, ApiError> {
    let (reply, answer) = oneshot::channel();
    state
        .tx
        .send(AppEvent::Remote(make(reply)))
        .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "the app is not running"))?;
    (state.repaint)();
    match tokio::time::timeout(APP_TIMEOUT, answer).await {
        Ok(Ok(value)) => Ok(value),
        _ => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "the app did not answer",
        )),
    }
}

/// An app-side refusal: 404 for a missing chat, 409 while a reply is running, otherwise 400.
fn command_failed(message: String) -> ApiError {
    let lower = message.to_lowercase();
    let status = if lower.contains("not found") {
        StatusCode::NOT_FOUND
    } else if lower.contains("running") || lower.contains("still") {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    ApiError::new(status, message)
}

fn chat_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| not_found())
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "chat not found")
}

fn parse_body<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("bad request body: {e}")))
}

/// An error answer: `{"error": "..."}` with the status.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

/// Accepts TCP connections and runs each TLS handshake in its own task, so a slow or
/// silent client cannot hold up the others. Only finished handshakes reach `accept`.
struct TlsListener {
    local: SocketAddr,
    ready: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    /// The accept loop owns the TCP listener. It must end with this listener, or the port
    /// stays bound after the server is stopped.
    accept: JoinHandle<()>,
}

impl TlsListener {
    fn new(listener: TcpListener, config: Arc<rustls::ServerConfig>) -> Self {
        let local = listener
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let (tx, ready) = mpsc::channel(64);
        let accept = tokio::spawn(accept_loop(listener, TlsAcceptor::from(config), tx));
        Self {
            local,
            ready,
            accept,
        }
    }
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ready: mpsc::Sender<(TlsStream<TcpStream>, SocketAddr)>,
) {
    let permits = Arc::new(Semaphore::new(MAX_HANDSHAKES));
    loop {
        // A permit is taken before accepting, so at most MAX_HANDSHAKES connections are in
        // their handshake at once; the rest wait in the kernel's accept queue.
        let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
            return;
        };
        let (stream, addr) = match listener.accept().await {
            Ok(connection) => connection,
            Err(e) => {
                log::warn!("Remote access: accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let ready = ready.clone();
        tokio::spawn(async move {
            // Held until the handshake ends, then released.
            let _permit = permit;
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                Ok(Ok(tls)) => {
                    let _ = ready.send((tls, addr)).await;
                }
                Ok(Err(e)) => log::debug!("Remote access: TLS handshake from {addr} failed: {e}"),
                Err(_) => log::debug!("Remote access: TLS handshake from {addr} timed out"),
            }
        });
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(connection) => connection,
            // The accept loop only ends with the task; nothing more will arrive.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// The client address a handler sees through `ConnectInfo`.
#[derive(Clone, Copy, Debug)]
struct Peer(SocketAddr);

impl Connected<IncomingStream<'_, TcpListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        Peer(*stream.remote_addr())
    }
}

impl Connected<IncomingStream<'_, TlsListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        Peer(*stream.remote_addr())
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use chrono::Utc;
    use tokio::runtime::Handle;

    use super::super::{
        ApprovalView, ChatSummary, ChatView, ProviderView, RemoteHandle, StateView, start,
    };
    use super::*;
    use crate::config::{RemoteSettings, RemoteTls};

    const PASSWORD: &str = "correct horse";
    const CHAT: Uuid = Uuid::from_u128(0x5eed);

    fn state_view() -> StateView {
        StateView {
            chats: vec![ChatSummary {
                id: CHAT,
                title: "Test chat".into(),
                pinned: false,
                updated_at: Utc::now(),
                running: false,
                approvals: 0,
            }],
            providers: vec![ProviderView {
                name: "Local".into(),
                model: "m1".into(),
                models: vec!["m1".into()],
            }],
            active_provider: 0,
            send_with_enter: true,
            theme: "system".into(),
            user_name: None,
        }
    }

    fn chat_view() -> ChatView {
        ChatView {
            id: CHAT,
            title: "Test chat".into(),
            pinned: false,
            messages: Vec::new(),
            running: false,
            running_tool: None,
            approvals: Vec::<ApprovalView>::new(),
            error: None,
        }
    }

    /// Answers the commands the way the app would for this test data.
    fn answer(cmd: RemoteCommand) {
        match cmd {
            RemoteCommand::State { reply } => {
                let _ = reply.send(state_view());
            }
            RemoteCommand::Chat { id, reply } => {
                let _ = reply.send((id == CHAT).then(chat_view));
            }
            RemoteCommand::NewChat { reply } => {
                let _ = reply.send(Uuid::new_v4());
            }
            RemoteCommand::Send { text, reply, .. } => {
                let _ = reply.send(if text == "busy" {
                    Err("A reply is still running".to_string())
                } else {
                    Ok(CHAT)
                });
            }
            RemoteCommand::Stop { reply, .. } | RemoteCommand::Delete { reply, .. } => {
                let _ = reply.send(());
            }
            RemoteCommand::Regenerate { reply, .. }
            | RemoteCommand::Pin { reply, .. }
            | RemoteCommand::SetModel { reply, .. } => {
                let _ = reply.send(Ok(()));
            }
            RemoteCommand::Rename { title, reply, .. } => {
                let _ = reply.send(match title.as_str() {
                    "missing" => Err("Chat not found".to_string()),
                    "busy" => Err("A reply is still running".to_string()),
                    _ => Ok(()),
                });
            }
            RemoteCommand::Approve {
                call_id,
                answer,
                reply,
                ..
            } => {
                let _ = reply.send(if call_id == "call_1" && answer == Approval::Once {
                    Ok(())
                } else {
                    Err(format!("No pending call {call_id}"))
                });
            }
        }
    }

    fn settings(tls: RemoteTls) -> RemoteSettings {
        RemoteSettings {
            enabled: true,
            port: 0,
            password: PASSWORD.into(),
            tls,
            ..RemoteSettings::default()
        }
    }

    /// Starts the server with a fake app loop that answers its commands.
    fn start_test(cfg: &RemoteSettings) -> Result<RemoteHandle, String> {
        start_test_with_tunnel(cfg, TunnelHosts::default())
    }

    fn start_test_with_tunnel(
        cfg: &RemoteSettings,
        tunnel_hosts: TunnelHosts,
    ) -> Result<RemoteHandle, String> {
        let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let AppEvent::Remote(cmd) = event {
                    answer(cmd);
                }
            }
        });
        start(&Handle::current(), cfg, tx, Arc::new(|| {}), tunnel_hosts)
    }

    fn expect_err(result: Result<RemoteHandle, String>) -> String {
        match result {
            Ok(_) => panic!("the server should not start"),
            Err(message) => message,
        }
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    /// A client for a self-signed certificate.
    fn tls_client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    /// The `cleanrstai_session=...` pair from a response's `Set-Cookie`.
    fn session_pair(resp: &reqwest::Response) -> String {
        resp.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("cleanrstai_session="))
            .expect("a session cookie")
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    async fn login(client: &reqwest::Client, base: &str) -> String {
        let resp = client
            .post(format!("{base}api/login"))
            .header("X-CleanRSTAI", "1")
            .json(&json!({ "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        session_pair(&resp)
    }

    async fn post_json(
        client: &reqwest::Client,
        url: String,
        cookie: &str,
        body: Value,
    ) -> reqwest::Response {
        client
            .post(url)
            .header("X-CleanRSTAI", "1")
            .header(header::COOKIE, cookie)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn serves_the_client_with_security_headers() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let resp = client().get(&base).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(resp.headers()[header::CONTENT_SECURITY_POLICY], CSP);
        assert_eq!(resp.headers()[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(resp.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(resp.headers()[header::REFERRER_POLICY], "no-referrer");
        assert!(
            resp.headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .is_none()
        );
        assert!(resp.headers().get(header::CACHE_CONTROL).is_none());

        let css = client()
            .get(format!("{base}style.css"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            css.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        let font = client()
            .get(format!("{base}fonts/Phosphor.ttf"))
            .send()
            .await
            .unwrap();
        assert_eq!(font.status(), 200);
        assert_eq!(font.headers()[header::CONTENT_TYPE], "font/ttf");
        assert_eq!(font.headers()[header::CACHE_CONTROL], FONT_CACHE);
        let missing = client()
            .get(format!("{base}fonts/nope.ttf"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), 404);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn markdown_test_script_is_not_served() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let resp = client()
            .get(format!("{}markdown.test.js", handle.urls()[0]))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_requires_login() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let resp = client()
            .get(format!("{base}api/state"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body, json!({ "error": "login required" }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn post_without_csrf_header_is_forbidden() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let resp = client()
            .post(format!("{base}api/login"))
            .json(&json!({ "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body, json!({ "error": "missing X-CleanRSTAI header" }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn login_cookie_is_http_only_and_not_secure_on_plain_http() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();

        let wrong = client
            .post(format!("{base}api/login"))
            .header("X-CleanRSTAI", "1")
            .json(&json!({ "password": "nope" }))
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 401);

        let resp = client
            .post(format!("{base}api/login"))
            .header("X-CleanRSTAI", "1")
            .json(&json!({ "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let cookie = resp.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.starts_with("cleanrstai_session="), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Strict"), "{cookie}");
        assert!(!cookie.contains("Secure"), "{cookie}");

        let session: Value = client
            .get(format!("{base}api/session"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(session, json!({ "logged_in": false, "plain_http": true }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn session_cookie_authorizes_the_api() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let cookie = login(&client, &base).await;

        let resp = client
            .get(format!("{base}api/state"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let state: Value = resp.json().await.unwrap();
        assert_eq!(state["chats"][0]["title"], "Test chat");
        assert_eq!(state["providers"][0]["model"], "m1");

        let session: Value = client
            .get(format!("{base}api/session"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(session["logged_in"], true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bearer_password_authorizes_the_api() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let ok = client
            .get(format!("{base}api/state"))
            .header(header::AUTHORIZATION, format!("Bearer {PASSWORD}"))
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), 200);
        let bad = client
            .get(format!("{base}api/state"))
            .header(header::AUTHORIZATION, "Bearer wrong")
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), 401);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sixth_failed_login_is_throttled() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        for _ in 0..5 {
            let resp = client
                .post(format!("{base}api/login"))
                .header("X-CleanRSTAI", "1")
                .json(&json!({ "password": "wrong" }))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 401);
        }
        let blocked = client
            .post(format!("{base}api/login"))
            .header("X-CleanRSTAI", "1")
            .json(&json!({ "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(blocked.status(), 429);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unknown_or_invalid_chat_ids_are_not_found() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let cookie = login(&client, &base).await;

        let unknown = format!("{}", Uuid::from_u128(7));
        for path in [
            format!("api/chats/{unknown}"),
            "api/chats/not-a-uuid".to_string(),
        ] {
            let resp = client
                .get(format!("{base}{path}"))
                .header(header::COOKIE, &cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 404, "{path}");
        }

        let known = client
            .get(format!("{base}api/chats/{CHAT}"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(known.status(), 200);
        let view: Value = known.json().await.unwrap();
        assert_eq!(view["title"], "Test chat");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn send_maps_app_refusals_and_limits() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let cookie = login(&client, &base).await;
        let messages = format!("{base}api/chats/{CHAT}/messages");

        let busy = post_json(
            &client,
            messages.clone(),
            &cookie,
            json!({ "text": "busy" }),
        )
        .await;
        assert_eq!(busy.status(), 409);
        let body: Value = busy.json().await.unwrap();
        assert_eq!(body, json!({ "error": "A reply is still running" }));

        let ok = post_json(&client, messages.clone(), &cookie, json!({ "text": "hi" })).await;
        assert_eq!(ok.status(), 200);
        assert_eq!(ok.json::<Value>().await.unwrap(), json!({ "id": CHAT }));

        let fresh = post_json(
            &client,
            format!("{base}api/chats/new/messages"),
            &cookie,
            json!({ "text": "hello" }),
        )
        .await;
        assert_eq!(fresh.status(), 200);

        let long = "x".repeat(MAX_TEXT_CHARS + 1);
        let too_long = post_json(&client, messages.clone(), &cookie, json!({ "text": long })).await;
        assert_eq!(too_long.status(), 400);

        let not_json = client
            .post(&messages)
            .header("X-CleanRSTAI", "1")
            .header(header::COOKIE, &cookie)
            .body("{")
            .send()
            .await
            .unwrap();
        assert_eq!(not_json.status(), 400);

        let oversized = client
            .post(&messages)
            .header("X-CleanRSTAI", "1")
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(vec![b' '; BODY_LIMIT + 1])
            .send()
            .await
            .unwrap();
        assert_eq!(oversized.status(), 413);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn approval_route_answers_the_call() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let cookie = login(&client, &base).await;

        let once = post_json(
            &client,
            format!("{base}api/chats/{CHAT}/approvals/call_1"),
            &cookie,
            json!({ "answer": "once" }),
        )
        .await;
        assert_eq!(once.status(), 200);
        assert_eq!(once.json::<Value>().await.unwrap(), json!({}));

        let missing = post_json(
            &client,
            format!("{base}api/chats/{CHAT}/approvals/call_2"),
            &cookie,
            json!({ "answer": "once" }),
        )
        .await;
        assert_eq!(missing.status(), 404);

        let bad_answer = post_json(
            &client,
            format!("{base}api/chats/{CHAT}/approvals/call_1"),
            &cookie,
            json!({ "answer": "maybe" }),
        )
        .await;
        assert_eq!(bad_answer.status(), 400);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn logout_ends_the_session() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let cookie = login(&client, &base).await;

        let resp = post_json(&client, format!("{base}api/logout"), &cookie, json!({})).await;
        assert_eq!(resp.status(), 200);
        let clear = resp.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_string();
        assert!(clear.contains("Max-Age=0"), "{clear}");

        let after = client
            .get(format!("{base}api/state"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(after.status(), 401);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn events_stream_pushes_notifications() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let cookie = login(&client, &base).await;

        let mut resp = client
            .get(format!("{base}api/events"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "text/event-stream");
        handle.notify(RemoteEvent::Chat { id: CHAT });

        let mut seen = String::new();
        let found = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let Some(chunk) = resp.chunk().await.unwrap() else {
                    return false;
                };
                seen.push_str(&String::from_utf8_lossy(&chunk));
                if seen.contains(r#"data: {"type":"chat""#) {
                    return true;
                }
            }
        })
        .await;
        assert_eq!(found, Ok(true), "{seen}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_password_is_refused() {
        let cfg = RemoteSettings {
            password: "   ".into(),
            ..settings(RemoteTls::Off)
        };
        assert_eq!(
            expect_err(start_test(&cfg)),
            "Set a password to start the server"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn second_server_on_the_same_port_fails() {
        let first = start_test(&settings(RemoteTls::Off)).unwrap();
        // The address reports the port actually bound, e.g. `http://127.0.0.1:43123/`.
        let port: u16 = first.urls()[0]
            .trim_end_matches('/')
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let cfg = RemoteSettings {
            port,
            ..settings(RemoteTls::Off)
        };
        let message = expect_err(start_test(&cfg));
        assert!(message.contains(&port.to_string()), "{message}");
        assert!(message.starts_with("Can't listen on port"), "{message}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restart_on_the_same_port_right_after_drop() {
        // The app drops the old server before it starts the new one with the same settings.
        let first = start_test(&settings(RemoteTls::Off)).unwrap();
        let port: u16 = first.urls()[0]
            .trim_end_matches('/')
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        drop(first);
        let cfg = RemoteSettings {
            port,
            ..settings(RemoteTls::Off)
        };
        assert!(start_test(&cfg).is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tls_serves_https_with_secure_cookie_and_hsts() {
        let dir = std::env::temp_dir().join(format!("cleanrstai-remote-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let certified = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, certified.cert.pem()).unwrap();
        std::fs::write(&key_path, certified.key_pair.serialize_pem()).unwrap();
        let cfg = RemoteSettings {
            tls: RemoteTls::Files,
            cert_path: cert_path.display().to_string(),
            key_path: key_path.display().to_string(),
            ..settings(RemoteTls::Files)
        };

        let handle = start_test(&cfg).unwrap();
        let base = handle.urls()[0].clone();
        assert!(base.starts_with("https://"), "{base}");
        let client = tls_client();

        let page = client.get(&base).send().await.unwrap();
        assert_eq!(page.status(), 200);
        assert_eq!(
            page.headers()[header::STRICT_TRANSPORT_SECURITY],
            "max-age=31536000"
        );

        let resp = client
            .post(format!("{base}api/login"))
            .header("X-CleanRSTAI", "1")
            .json(&json!({ "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let cookie = resp.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.contains("; Secure"), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");

        let session: Value = client
            .get(format!("{base}api/session"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(session["plain_http"], false);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The port in a server address such as `http://127.0.0.1:43123/`.
    fn port_of(base: &str) -> u16 {
        base.trim_end_matches('/')
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// A certificate and key for localhost in a new folder: (cert path, key path).
    fn write_test_cert(dir: &Path) -> (PathBuf, PathBuf) {
        std::fs::create_dir_all(dir).unwrap();
        let certified = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, certified.cert.pem()).unwrap();
        std::fs::write(&key_path, certified.key_pair.serialize_pem()).unwrap();
        (cert_path, key_path)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stopped_tls_server_releases_its_port() {
        let dir = std::env::temp_dir().join(format!("cleanrstai-remote-{}", Uuid::new_v4()));
        let (cert_path, key_path) = write_test_cert(&dir);
        let cfg = RemoteSettings {
            cert_path: cert_path.display().to_string(),
            key_path: key_path.display().to_string(),
            ..settings(RemoteTls::Files)
        };

        let first = start_test(&cfg).unwrap();
        let port = port_of(&first.urls()[0]);
        drop(first);
        // The accept task is aborted with the server; once it is gone nothing listens.
        let released = tokio::time::timeout(Duration::from_secs(1), async {
            while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            released.is_ok(),
            "port {port} is still accepting connections"
        );

        let again = RemoteSettings {
            port,
            ..cfg.clone()
        };
        assert!(start_test(&again).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn host_header_names() {
        let machine = Some("box");
        for host in [
            "localhost",
            "LOCALHOST:8484",
            "127.0.0.1",
            "127.0.0.1:8484",
            "192.168.1.5:8484",
            "[::1]:8484",
            "[::1]",
            "myhost.local",
            "Myhost.LOCAL:8484",
            "box",
            "BOX:8484",
        ] {
            assert!(host_allowed(host, machine), "{host}");
        }
        for host in [
            "evil.example",
            "evil.example:8484",
            "localhost.evil.example",
            "127.0.0.1.evil.example",
            "box.example",
            "local",
            "",
            "[::1",
            "::1",
        ] {
            assert!(!host_allowed(host, machine), "{host}");
        }
        assert!(!host_allowed("box", None));
    }

    #[test]
    fn hsts_only_for_certificate_files() {
        assert!(hsts_enabled(&settings(RemoteTls::Files)));
        assert!(!hsts_enabled(&settings(RemoteTls::SelfSigned)));
        assert!(!hsts_enabled(&settings(RemoteTls::Off)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unknown_host_names_get_421() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let port = port_of(&base);

        let evil = client()
            .get(format!("{base}api/session"))
            .header(header::HOST, "evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(evil.status(), 421);
        assert_eq!(evil.headers()[header::X_FRAME_OPTIONS], "DENY");
        let body: Value = evil.json().await.unwrap();
        assert_eq!(body, json!({ "error": "unknown host" }));

        for host in [
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("myhost.local:{port}"),
            format!("[::1]:{port}"),
        ] {
            let resp = client()
                .get(format!("{base}api/session"))
                .header(header::HOST, &host)
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200, "{host}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tunnel_hostnames_are_served_only_while_the_tunnel_runs() {
        let hosts = TunnelHosts::default();
        let handle = start_test_with_tunnel(&settings(RemoteTls::Off), hosts.clone()).unwrap();
        let base = handle.urls()[0].clone();
        let session = |host: &'static str| {
            client()
                .get(format!("{base}api/session"))
                .header(header::HOST, host)
                .send()
        };
        assert_eq!(session("chat.example.com").await.unwrap().status(), 421);

        hosts.write().unwrap().push("chat.example.com".into());
        let resp = session("Chat.Example.com").await.unwrap();
        assert_eq!(resp.status(), 200);
        // Cloudflare serves visitors over HTTPS, so no plain-HTTP warning for them.
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body, json!({ "logged_in": false, "plain_http": false }));

        hosts.write().unwrap().clear();
        assert_eq!(session("chat.example.com").await.unwrap().status(), 421);
    }

    /// Five wrong passwords from one visitor through the tunnel block that visitor only.
    #[tokio::test(flavor = "multi_thread")]
    async fn tunnel_logins_are_throttled_by_the_visitor_address() {
        let hosts: TunnelHosts = Arc::new(std::sync::RwLock::new(vec!["chat.example.com".into()]));
        let handle = start_test_with_tunnel(&settings(RemoteTls::Off), hosts).unwrap();
        let base = handle.urls()[0].clone();
        let attempt = |visitor: &'static str, password: &'static str| {
            client()
                .post(format!("{base}api/login"))
                .header(header::HOST, "chat.example.com")
                .header("Cf-Connecting-IP", visitor)
                .header("X-CleanRSTAI", "1")
                .json(&json!({ "password": password }))
                .send()
        };
        for _ in 0..5 {
            assert_eq!(attempt("203.0.113.7", "wrong").await.unwrap().status(), 401);
        }
        assert_eq!(
            attempt("203.0.113.7", PASSWORD).await.unwrap().status(),
            429
        );
        assert_eq!(
            attempt("198.51.100.9", PASSWORD).await.unwrap().status(),
            200
        );
    }

    /// Off the tunnel, `Cf-Connecting-IP` is just a header anyone can send: it is ignored.
    #[tokio::test(flavor = "multi_thread")]
    async fn cf_connecting_ip_is_ignored_off_the_tunnel() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        for i in 0..5 {
            let resp = client()
                .post(format!("{base}api/login"))
                .header("Cf-Connecting-IP", format!("203.0.113.{i}"))
                .header("X-CleanRSTAI", "1")
                .json(&json!({ "password": "wrong" }))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 401);
        }
        let blocked = client()
            .post(format!("{base}api/login"))
            .header("Cf-Connecting-IP", "198.51.100.9")
            .header("X-CleanRSTAI", "1")
            .json(&json!({ "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(blocked.status(), 429);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn session_ignores_the_bearer_password() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let session: Value = client
            .get(format!("{base}api/session"))
            .header(header::AUTHORIZATION, format!("Bearer {PASSWORD}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(session, json!({ "logged_in": false, "plain_http": true }));

        // The bearer password still opens the API itself.
        let state = client
            .get(format!("{base}api/state"))
            .header(header::AUTHORIZATION, format!("Bearer {PASSWORD}"))
            .send()
            .await
            .unwrap();
        assert_eq!(state.status(), 200);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn logout_clears_an_unknown_or_missing_cookie() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();

        let stale = post_json(
            &client,
            format!("{base}api/logout"),
            "cleanrstai_session=expired-or-unknown",
            json!({}),
        )
        .await;
        assert_eq!(stale.status(), 200);
        let clear = stale.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(clear.contains("Max-Age=0"), "{clear}");

        let bare = client
            .post(format!("{base}api/logout"))
            .header("X-CleanRSTAI", "1")
            .send()
            .await
            .unwrap();
        assert_eq!(bare.status(), 200);
        assert!(bare.headers().get(header::SET_COOKIE).is_some());

        let no_csrf = client
            .post(format!("{base}api/logout"))
            .send()
            .await
            .unwrap();
        assert_eq!(no_csrf.status(), 403);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_event_stream_ends_after_logout() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let cookie = login(&client, &base).await;

        let mut resp = client
            .get(format!("{base}api/events"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let logout = post_json(&client, format!("{base}api/logout"), &cookie, json!({})).await;
        assert_eq!(logout.status(), 200);

        let ended = tokio::time::timeout(Duration::from_secs(20), async {
            while resp.chunk().await.unwrap().is_some() {}
        })
        .await;
        assert!(ended.is_ok(), "the event stream is still open after logout");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn command_refusals_map_to_status_codes() {
        let handle = start_test(&settings(RemoteTls::Off)).unwrap();
        let base = handle.urls()[0].clone();
        let client = client();
        let cookie = login(&client, &base).await;
        let rename = format!("{base}api/chats/{CHAT}/rename");

        let missing = post_json(
            &client,
            rename.clone(),
            &cookie,
            json!({ "title": "missing" }),
        )
        .await;
        assert_eq!(missing.status(), 404);
        let body: Value = missing.json().await.unwrap();
        assert_eq!(body, json!({ "error": "Chat not found" }));

        let busy = post_json(&client, rename.clone(), &cookie, json!({ "title": "busy" })).await;
        assert_eq!(busy.status(), 409);

        let other = post_json(&client, rename, &cookie, json!({ "title": "Fine" })).await;
        assert_eq!(other.status(), 200);
    }
}
