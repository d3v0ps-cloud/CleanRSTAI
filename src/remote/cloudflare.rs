//! Publishes a named Cloudflare Tunnel's hostname through Cloudflare's API: adds the route
//! (an ingress rule from the hostname to this server) to the tunnel's configuration, keeping
//! any other routes, and creates the proxied DNS record that points the hostname at the
//! tunnel. A token-run tunnel takes its routes from Cloudflare, so cloudflared can't do this.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use reqwest::Method;
use serde_json::{Value, json};

pub const API: &str = "https://api.cloudflare.com/client/v4";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// What to publish, and with which credentials.
pub struct Publish {
    /// The API's base URL (tests point it at a mock server).
    pub api: String,
    pub api_token: String,
    pub tunnel_token: String,
    pub hostname: String,
    /// Where cloudflared sends the hostname's requests, e.g. `https://127.0.0.1:8484`.
    pub service: String,
}

/// Publishes `p.hostname` on the tunnel. Returns a short description of what it did.
pub async fn publish(http: &reqwest::Client, p: &Publish) -> Result<String, String> {
    let (account, tunnel) = tunnel_ids(&p.tunnel_token)?;
    let api = Api {
        http,
        base: p.api.trim_end_matches('/'),
        token: p.api_token.trim(),
    };

    let config_url = format!("/accounts/{account}/cfd_tunnel/{tunnel}/configurations");
    let current = api.call(Method::GET, &config_url, None).await?;
    let config = current.get("config").cloned().unwrap_or(Value::Null);
    let (config, changed) = with_route(config, &p.hostname, &p.service);
    if changed {
        api.call(Method::PUT, &config_url, Some(json!({ "config": config })))
            .await?;
    }

    let zone = find_zone(&api, &p.hostname).await?;
    let target = format!("{tunnel}.cfargotunnel.com");
    let records = api
        .call(
            Method::GET,
            &format!("/zones/{zone}/dns_records?name={}", p.hostname),
            None,
        )
        .await?;
    match records.as_array().and_then(|r| r.first()) {
        Some(r) if r["type"] == "CNAME" && r["content"] == target.as_str() => {}
        Some(r) => {
            return Err(format!(
                "{} already has a DNS record ({} {}): remove it in the Cloudflare dashboard \
                 or choose another hostname",
                p.hostname,
                r["type"].as_str().unwrap_or("?"),
                r["content"].as_str().unwrap_or("?"),
            ));
        }
        None => {
            api.call(
                Method::POST,
                &format!("/zones/{zone}/dns_records"),
                Some(json!({
                    "type": "CNAME",
                    "name": p.hostname,
                    "content": target,
                    "proxied": true,
                    "comment": "CleanRSTAI remote access",
                })),
            )
            .await?;
        }
    }
    Ok(format!("https://{}/ → {}", p.hostname, p.service))
}

/// The account and tunnel IDs inside a tunnel token: base64 of `{"a":…,"t":…,"s":…}`.
fn tunnel_ids(token: &str) -> Result<(String, String), String> {
    let token = token.trim();
    let bytes = STANDARD
        .decode(token)
        .or_else(|_| URL_SAFE_NO_PAD.decode(token.trim_end_matches('=')))
        .map_err(|_| "The tunnel token isn't valid: copy it again from the dashboard")?;
    let v: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "The tunnel token isn't valid: copy it again from the dashboard")?;
    match (v["a"].as_str(), v["t"].as_str()) {
        (Some(a), Some(t)) if !a.is_empty() && !t.is_empty() => Ok((a.into(), t.into())),
        _ => Err("The tunnel token isn't valid: copy it again from the dashboard".into()),
    }
}

/// The tunnel configuration with a route from `hostname` to `service` in place of any route
/// for that hostname, before the catch-all rule (added when missing, as Cloudflare requires
/// one last). Other routes and settings are kept. True when that changed anything.
fn with_route(config: Value, hostname: &str, service: &str) -> (Value, bool) {
    let mut config = match config {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let old: Vec<Value> = config
        .get("ingress")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ours = |rule: &Value| {
        rule["hostname"]
            .as_str()
            .is_some_and(|h| h.eq_ignore_ascii_case(hostname))
            && rule.get("path").is_none()
    };
    let catch_all = |rule: &Value| rule.get("hostname").is_none();

    let mut route = json!({ "hostname": hostname, "service": service });
    if service.starts_with("https://") {
        // The server's certificate is self-signed, or made for another name than 127.0.0.1.
        route["originRequest"] = json!({ "noTLSVerify": true });
    }
    let mut ingress: Vec<Value> = old
        .iter()
        .filter(|r| !ours(r) && !catch_all(r))
        .cloned()
        .collect();
    ingress.push(route);
    let last = old
        .iter()
        .rev()
        .find(|r| catch_all(r))
        .cloned()
        .unwrap_or_else(|| json!({ "service": "http_status:404" }));
    ingress.push(last);

    let changed = ingress != old;
    config.insert("ingress".into(), Value::Array(ingress));
    (Value::Object(config), changed)
}

/// The zone (domain) the hostname belongs to: the longest suffix Cloudflare knows.
async fn find_zone(api: &Api<'_>, hostname: &str) -> Result<String, String> {
    let labels: Vec<&str> = hostname.split('.').collect();
    for start in 0..labels.len().saturating_sub(1) {
        let name = labels[start..].join(".");
        let zones = api
            .call(Method::GET, &format!("/zones?name={name}"), None)
            .await?;
        if let Some(id) = zones
            .as_array()
            .and_then(|z| z.first())
            .and_then(|z| z["id"].as_str())
        {
            return Ok(id.to_owned());
        }
    }
    Err(format!(
        "No domain on this Cloudflare account matches {hostname}, or the API token can't read \
         it (it needs Zone: Read)"
    ))
}

struct Api<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    token: &'a str,
}

impl Api<'_> {
    /// One API call. Returns its `result`, or Cloudflare's own error message.
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.token)
            .timeout(TIMEOUT);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Can't reach Cloudflare's API: {e}"))?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if v["success"] == true {
            return Ok(v["result"].clone());
        }
        let message = v["errors"]
            .as_array()
            .and_then(|e| e.first())
            .and_then(|e| e["message"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("HTTP {status}"));
        Err(match status.as_u16() {
            401 | 403 => format!(
                "Cloudflare refused the API token ({message}): it needs Cloudflare Tunnel: \
                 Edit, DNS: Edit and Zone: Read"
            ),
            _ => format!("Cloudflare's API answered: {message}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Response, serve};

    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
    const TUNNEL: &str = "11111111-2222-3333-4444-555555555555";

    fn tunnel_token() -> String {
        STANDARD.encode(format!(
            r#"{{"a":"{ACCOUNT}","t":"{TUNNEL}","s":"c2VjcmV0"}}"#
        ))
    }

    fn ok(result: Value) -> Response {
        Response::json(json!({ "success": true, "errors": [], "result": result }))
    }

    #[test]
    fn tunnel_ids_come_from_the_token() {
        assert_eq!(
            tunnel_ids(&tunnel_token()).unwrap(),
            (ACCOUNT.to_string(), TUNNEL.to_string())
        );
        assert!(tunnel_ids("not a token").is_err());
        assert!(tunnel_ids(&STANDARD.encode(r#"{"a":"x"}"#)).is_err());
    }

    #[test]
    fn the_route_replaces_ours_and_keeps_the_rest() {
        let config = json!({
            "ingress": [
                { "hostname": "wiki.example.com", "service": "http://localhost:3000" },
                { "hostname": "Chat.example.com", "service": "http://old:1" },
                { "service": "http_status:418" }
            ],
            "warp-routing": { "enabled": false }
        });
        let (new, changed) = with_route(config, "chat.example.com", "https://127.0.0.1:8484");
        assert!(changed);
        assert_eq!(
            new,
            json!({
                "ingress": [
                    { "hostname": "wiki.example.com", "service": "http://localhost:3000" },
                    {
                        "hostname": "chat.example.com",
                        "service": "https://127.0.0.1:8484",
                        "originRequest": { "noTLSVerify": true }
                    },
                    { "service": "http_status:418" }
                ],
                "warp-routing": { "enabled": false }
            })
        );
        // Publishing again changes nothing.
        let (_, again) = with_route(new, "chat.example.com", "https://127.0.0.1:8484");
        assert!(!again);
    }

    #[test]
    fn an_empty_configuration_gets_the_route_and_a_catch_all() {
        let (new, changed) = with_route(Value::Null, "chat.example.com", "http://127.0.0.1:8484");
        assert!(changed);
        assert_eq!(
            new["ingress"],
            json!([
                { "hostname": "chat.example.com", "service": "http://127.0.0.1:8484" },
                { "service": "http_status:404" }
            ])
        );
    }

    fn job(api: String) -> Publish {
        Publish {
            api,
            api_token: "api-token".into(),
            tunnel_token: tunnel_token(),
            hostname: "chat.example.com".into(),
            service: "https://127.0.0.1:8484".into(),
        }
    }

    #[tokio::test]
    async fn publishing_sets_the_route_and_creates_the_dns_record() {
        let (api, log) = serve(|req| match (req.method.as_str(), req.path.as_str()) {
            ("GET", p) if p.ends_with("/configurations") => {
                ok(json!({ "config": { "ingress": [{ "service": "http_status:404" }] } }))
            }
            ("PUT", p) if p.ends_with("/configurations") => ok(json!({})),
            ("GET", "/zones?name=chat.example.com") => ok(json!([])),
            ("GET", "/zones?name=example.com") => ok(json!([{ "id": "zone1" }])),
            ("GET", p) if p.starts_with("/zones/zone1/dns_records") => ok(json!([])),
            ("POST", "/zones/zone1/dns_records") => ok(json!({ "id": "rec1" })),
            _ => Response::status(404),
        })
        .await;
        let done = publish(&reqwest::Client::new(), &job(api)).await.unwrap();
        assert_eq!(done, "https://chat.example.com/ → https://127.0.0.1:8484");

        let log = log.lock().unwrap();
        assert!(
            log.iter()
                .all(|r| r.header("authorization") == Some("Bearer api-token"))
        );
        let put = log.iter().find(|r| r.method == "PUT").unwrap();
        assert_eq!(
            put.path,
            format!("/accounts/{ACCOUNT}/cfd_tunnel/{TUNNEL}/configurations")
        );
        assert_eq!(
            put.json()["config"]["ingress"][0]["hostname"],
            "chat.example.com"
        );
        let post = log.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(
            post.json(),
            json!({
                "type": "CNAME",
                "name": "chat.example.com",
                "content": format!("{TUNNEL}.cfargotunnel.com"),
                "proxied": true,
                "comment": "CleanRSTAI remote access",
            })
        );
    }

    #[tokio::test]
    async fn an_existing_record_elsewhere_is_left_alone() {
        let (api, log) = serve(|req| match (req.method.as_str(), req.path.as_str()) {
            ("GET", p) if p.ends_with("/configurations") => ok(json!({ "config": null })),
            ("PUT", _) => ok(json!({})),
            ("GET", p) if p.starts_with("/zones?") => ok(json!([{ "id": "zone1" }])),
            ("GET", _) => ok(json!([{ "type": "A", "content": "192.0.2.1" }])),
            _ => Response::status(404),
        })
        .await;
        let err = publish(&reqwest::Client::new(), &job(api))
            .await
            .unwrap_err();
        assert!(
            err.contains("already has a DNS record (A 192.0.2.1)"),
            "{err}"
        );
        assert!(!log.lock().unwrap().iter().any(|r| r.method == "POST"));
    }

    #[tokio::test]
    async fn a_refused_api_token_names_the_permissions() {
        let (api, _) = serve(|_| {
            let body = json!({
                "success": false,
                "errors": [{ "code": 10000, "message": "Authentication error" }]
            });
            Response::with_type(403, "application/json", body.to_string())
        })
        .await;
        let err = publish(&reqwest::Client::new(), &job(api))
            .await
            .unwrap_err();
        assert!(err.contains("Authentication error"), "{err}");
        assert!(err.contains("DNS: Edit"), "{err}");
    }
}
