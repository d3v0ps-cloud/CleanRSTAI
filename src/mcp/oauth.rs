//! OAuth 2.1 sign-in for remote (Streamable HTTP) MCP servers, following the MCP
//! authorization spec: protected-resource metadata (RFC 9728) → authorization-server
//! metadata (RFC 8414) → dynamic client registration (RFC 7591) → authorization code
//! with PKCE over a loopback redirect (RFC 8252) → tokens, with the `resource`
//! indicator (RFC 8707) on every request.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::secrets::SecretStore;

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// Returned (inside an `anyhow::Error`) when a server needs the user to sign in.
#[derive(Debug, Clone)]
pub struct AuthRequired {
    pub www_authenticate: Option<String>,
}

impl std::fmt::Display for AuthRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this server requires you to sign in")
    }
}

impl std::error::Error for AuthRequired {}

pub fn is_auth_required(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<AuthRequired>())
}

/// Tokens and the client they were issued to, as persisted in the keychain.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub expires_at: Option<i64>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    pub resource: String,
}

impl Tokens {
    /// Expired, or about to be within a minute.
    pub fn is_stale(&self) -> bool {
        self.expires_at
            .is_some_and(|t| t <= chrono::Utc::now().timestamp() + 60)
    }
}

#[derive(Debug, Clone)]
struct ServerMeta {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    scopes: Vec<String>,
    resource: String,
}

/// The canonical resource URI for an MCP server URL: no fragment, no query.
fn canonical_resource(server_url: &str) -> String {
    let mut u = reqwest::Url::parse(server_url)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| server_url.into());
    if let Some(i) = u.find(['#', '?']) {
        u.truncate(i);
    }
    u.trim_end_matches('/').to_owned()
}

/// `resource_metadata="…"` from a `WWW-Authenticate: Bearer …` header.
fn resource_metadata_url(www_authenticate: &str) -> Option<String> {
    let i = www_authenticate.find("resource_metadata=")? + "resource_metadata=".len();
    let rest = &www_authenticate[i..];
    let value = match rest.strip_prefix('"') {
        Some(q) => &q[..q.find('"')?],
        None => rest.split([',', ' ']).next()?,
    };
    Some(value.to_owned())
}

/// `/.well-known/{name}` URLs to try for `url`, path-aware first (RFC 8414 §3.1).
fn well_known(url: &reqwest::Url, name: &str) -> Vec<String> {
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_end_matches('/');
    let mut out = Vec::new();
    if !path.is_empty() {
        out.push(format!("{origin}/.well-known/{name}{path}"));
    }
    out.push(format!("{origin}/.well-known/{name}"));
    out
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<Value> {
    let resp = http
        .get(url)
        .header("accept", "application/json")
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("GET {url}: HTTP {}", resp.status());
    }
    Ok(resp.json().await?)
}

async fn discover(
    http: &reqwest::Client,
    server_url: &str,
    www_authenticate: Option<&str>,
) -> Result<ServerMeta> {
    let server = reqwest::Url::parse(server_url).context("invalid server URL")?;

    // 1. Protected resource metadata names the authorization server.
    let mut candidates: Vec<String> = www_authenticate
        .and_then(resource_metadata_url)
        .into_iter()
        .collect();
    candidates.extend(well_known(&server, "oauth-protected-resource"));
    let mut resource = canonical_resource(server_url);
    let mut auth_server = None;
    let mut scopes = Vec::new();
    for url in &candidates {
        if let Ok(v) = get_json(http, url).await {
            if let Some(r) = v.get("resource").and_then(Value::as_str) {
                resource = r.to_owned();
            }
            scopes = string_list(&v, "scopes_supported");
            auth_server = v
                .get("authorization_servers")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .map(str::to_owned);
            break;
        }
    }
    // Older servers act as their own authorization server.
    let issuer = auth_server.unwrap_or_else(|| server.origin().ascii_serialization());
    let issuer_url = reqwest::Url::parse(&issuer).context("invalid authorization server URL")?;

    // 2. Authorization server metadata.
    let mut urls = well_known(&issuer_url, "oauth-authorization-server");
    urls.extend(well_known(&issuer_url, "openid-configuration"));
    urls.push(format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    ));
    let mut meta = None;
    for url in &urls {
        if let Ok(v) = get_json(http, url).await
            && v.get("authorization_endpoint").is_some()
        {
            meta = Some(v);
            break;
        }
    }
    let meta = meta.with_context(|| format!("couldn't find OAuth metadata for {issuer}"))?;
    let field = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_owned);
    let methods = string_list(&meta, "code_challenge_methods_supported");
    if !methods.is_empty() && !methods.iter().any(|m| m == "S256") {
        bail!("the authorization server doesn't support PKCE (S256)");
    }
    if scopes.is_empty() {
        scopes = string_list(&meta, "scopes_supported");
    }
    Ok(ServerMeta {
        authorization_endpoint: field("authorization_endpoint")
            .context("no authorization_endpoint")?,
        token_endpoint: field("token_endpoint").context("no token_endpoint")?,
        registration_endpoint: field("registration_endpoint"),
        scopes,
        resource,
    })
}

fn string_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

async fn register(
    http: &reqwest::Client,
    endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>)> {
    let resp = http
        .post(endpoint)
        .json(&serde_json::json!({
            "client_name": "CleanRSTAI",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        bail!(
            "client registration failed: HTTP {status}: {}",
            resp.text().await.unwrap_or_default()
        );
    }
    let v: Value = resp.json().await?;
    let id = v
        .get("client_id")
        .and_then(Value::as_str)
        .context("registration returned no client_id")?;
    let secret = v
        .get("client_secret")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((id.to_owned(), secret))
}

fn random_token() -> String {
    // 256 bits from the OS RNG (uuid v4 uses getrandom).
    let bytes: Vec<u8> = (0..2)
        .flat_map(|_| *uuid::Uuid::new_v4().as_bytes())
        .collect();
    B64URL.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    B64URL.encode(Sha256::digest(verifier.as_bytes()))
}

fn parse_token_response(
    v: &Value,
    meta_token_endpoint: &str,
    client_id: &str,
    client_secret: Option<String>,
    resource: &str,
    old_refresh: Option<String>,
) -> Result<Tokens> {
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        let desc = v
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or("");
        bail!("token request failed: {err} {desc}");
    }
    Ok(Tokens {
        access_token: v
            .get("access_token")
            .and_then(Value::as_str)
            .context("token response has no access_token")?
            .to_owned(),
        refresh_token: v
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or(old_refresh),
        expires_at: v
            .get("expires_in")
            .and_then(Value::as_i64)
            .map(|s| chrono::Utc::now().timestamp() + s),
        token_endpoint: meta_token_endpoint.to_owned(),
        client_id: client_id.to_owned(),
        client_secret,
        resource: resource.to_owned(),
    })
}

async fn token_request(
    http: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<Value> {
    let resp = http
        .post(endpoint)
        .header("accept", "application/json")
        .form(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() && v.get("error").is_none() {
        bail!("token endpoint returned HTTP {status}");
    }
    Ok(v)
}

/// Wait for the browser to hit the loopback redirect; returns the query parameters.
async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    state: &str,
) -> Result<Vec<(String, String)>> {
    loop {
        let (mut sock, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]).to_string();
        let target = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("");
        let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
            continue;
        };
        if url.path() != "/callback" {
            let _ = sock
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
            continue;
        }
        let params: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let message = if get("state") != Some(state) {
            "Sign-in failed: the response didn't match this request."
        } else if get("error").is_some() {
            "Sign-in was cancelled or refused. You can close this window."
        } else {
            "Signed in to the MCP server. You can close this window and return to CleanRSTAI."
        };
        let body = format!(
            "<!doctype html><meta charset=utf-8><title>CleanRSTAI</title>\
             <body style=\"font-family:system-ui;padding:3em\"><h2>{message}</h2></body>"
        );
        let _ = sock
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
        if get("state") != Some(state) {
            continue;
        }
        if let Some(err) = get("error") {
            let desc = get("error_description").unwrap_or("");
            bail!("authorization denied: {err} {desc}");
        }
        return Ok(params);
    }
}

/// Run the interactive sign-in. `open_url` should open the user's browser.
pub async fn sign_in(
    http: &reqwest::Client,
    server_url: &str,
    www_authenticate: Option<&str>,
    configured_client_id: Option<&str>,
    open_url: impl Fn(&str),
) -> Result<Tokens> {
    let meta = discover(http, server_url, www_authenticate).await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );

    let (client_id, client_secret) = match configured_client_id.filter(|c| !c.trim().is_empty()) {
        Some(id) => (id.trim().to_owned(), None),
        None => {
            let endpoint = meta.registration_endpoint.as_deref().ok_or_else(|| {
                anyhow!("this server doesn't support automatic client registration; set an OAuth client ID for it in Settings")
            })?;
            register(http, endpoint, &redirect_uri).await?
        }
    };

    let verifier = random_token();
    let state = random_token();
    let mut url = reqwest::Url::parse(&meta.authorization_endpoint)
        .context("invalid authorization_endpoint")?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &meta.resource);
        if !meta.scopes.is_empty() {
            q.append_pair("scope", &meta.scopes.join(" "));
        }
    }
    open_url(url.as_str());

    let params = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_callback(listener, &state))
        .await
        .map_err(|_| {
            anyhow!(
                "sign-in timed out after {} minutes",
                SIGN_IN_TIMEOUT.as_secs() / 60
            )
        })??;
    let code = params
        .iter()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.clone())
        .context("the authorization server returned no code")?;

    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", client_id.as_str()),
        ("code_verifier", verifier.as_str()),
        ("resource", meta.resource.as_str()),
    ];
    if let Some(s) = &client_secret {
        form.push(("client_secret", s.as_str()));
    }
    let v = token_request(http, &meta.token_endpoint, &form).await?;
    parse_token_response(
        &v,
        &meta.token_endpoint,
        &client_id,
        client_secret.clone(),
        &meta.resource,
        None,
    )
}

/// Use the refresh token to get a new access token.
pub async fn refresh(http: &reqwest::Client, t: &Tokens) -> Result<Tokens> {
    let rt = t.refresh_token.as_deref().context("no refresh token")?;
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", rt),
        ("client_id", t.client_id.as_str()),
        ("resource", t.resource.as_str()),
    ];
    if let Some(s) = &t.client_secret {
        form.push(("client_secret", s.as_str()));
    }
    let v = token_request(http, &t.token_endpoint, &form).await?;
    parse_token_response(
        &v,
        &t.token_endpoint,
        &t.client_id,
        t.client_secret.clone(),
        &t.resource,
        t.refresh_token.clone(),
    )
}

fn account(server_id: &str) -> String {
    format!("mcp:{server_id}:oauth")
}

pub fn load_tokens(store: &dyn SecretStore, server_id: &str) -> Option<Tokens> {
    let json = store.get(&account(server_id)).ok()??;
    serde_json::from_str(&json).ok()
}

pub fn save_tokens(store: &dyn SecretStore, server_id: &str, t: &Tokens) -> Result<()> {
    store
        .set(&account(server_id), &serde_json::to_string(t)?)
        .map_err(|e| anyhow!("saving sign-in tokens: {e}"))
}

pub fn delete_tokens(store: &dyn SecretStore, server_id: &str) {
    let _ = store.delete(&account(server_id));
}

pub type Store = Arc<dyn SecretStore + Send + Sync>;

/// Per-server token state used by the HTTP transport.
pub struct Auth {
    server_id: String,
    store: Store,
    http: reqwest::Client,
    tokens: tokio::sync::Mutex<Option<Tokens>>,
    /// Whether tokens were stored when this connection started.
    pub signed_in: bool,
}

impl Auth {
    pub fn new(server_id: &str, store: Store, http: reqwest::Client) -> Self {
        let tokens = load_tokens(store.as_ref(), server_id);
        Self {
            server_id: server_id.to_owned(),
            store,
            http,
            signed_in: tokens.is_some(),
            tokens: tokio::sync::Mutex::new(tokens),
        }
    }

    /// The access token to send, refreshing it first if it is about to expire.
    pub async fn bearer(&self) -> Option<String> {
        let mut guard = self.tokens.lock().await;
        let t = guard.as_ref()?;
        if t.is_stale() && t.refresh_token.is_some() {
            match refresh(&self.http, t).await {
                Ok(new) => {
                    let _ = save_tokens(self.store.as_ref(), &self.server_id, &new);
                    *guard = Some(new);
                }
                Err(e) => log::warn!("refreshing MCP token: {e:#}"),
            }
        }
        guard.as_ref().map(|t| t.access_token.clone())
    }

    /// After a 401: try the refresh token once. Returns true if there is a new token.
    pub async fn refresh_after_rejection(&self, rejected: Option<&str>) -> bool {
        let mut guard = self.tokens.lock().await;
        let Some(t) = guard.as_ref() else {
            return false;
        };
        if rejected.is_some_and(|r| r != t.access_token) {
            return true; // another request already refreshed it
        }
        match refresh(&self.http, t).await {
            Ok(new) => {
                let _ = save_tokens(self.store.as_ref(), &self.server_id, &new);
                *guard = Some(new);
                true
            }
            Err(e) => {
                log::warn!("MCP token rejected and refresh failed: {e:#}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_www_authenticate() {
        assert_eq!(
            resource_metadata_url(r#"Bearer realm="x", resource_metadata="https://a.b/.well-known/oauth-protected-resource""#).as_deref(),
            Some("https://a.b/.well-known/oauth-protected-resource")
        );
        assert_eq!(
            resource_metadata_url("Bearer resource_metadata=https://a/m, x=y").as_deref(),
            Some("https://a/m")
        );
        assert_eq!(resource_metadata_url("Bearer realm=\"x\""), None);
    }

    #[test]
    fn well_known_and_resource() {
        let u = reqwest::Url::parse("https://auth.example.com/tenant1").unwrap();
        assert_eq!(
            well_known(&u, "oauth-authorization-server"),
            vec![
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/oauth-authorization-server"
            ]
        );
        assert_eq!(
            canonical_resource("https://Mcp.Example.com/mcp/?x=1#f"),
            "https://mcp.example.com/mcp"
        );
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 43);
    }
}
