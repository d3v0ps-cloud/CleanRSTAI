//! Cloudflare Access in front of a named tunnel. Access signs every request it lets through
//! with a JWT, sent as `Cf-Access-Jwt-Assertion` (and in the `CF_Authorization` cookie). This
//! module checks that token against the team's published keys, plus the audience, issuer and
//! times, so a request that reaches the server without going through Access is refused.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, HeaderName, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature;
use serde_json::Value;

use crate::config::{TunnelMode, TunnelSettings, team_host};

const ASSERTION: HeaderName = HeaderName::from_static("cf-access-jwt-assertion");
const COOKIE_PREFIX: &str = "CF_Authorization=";
/// A token this many seconds past its expiry, or early for `nbf`/`iat`, is still accepted.
const SKEW_SECS: u64 = 60;
/// Keys are fetched again for an unknown key id at most this often.
const REFETCH_EVERY: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// What the server does with requests for the tunnel's hostname.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    /// Access is not in use: requests are not checked.
    #[default]
    Off,
    /// A valid Access token is needed; the app password is still asked for.
    Require,
    /// A valid Access token also signs the browser in.
    SignIn,
}

/// Why a request was not let through. The messages are shown in the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessError {
    NotConfigured,
    Missing,
    Malformed,
    UnsupportedAlg,
    UnknownKey,
    BadSignature,
    Audience,
    Issuer,
    Expired,
    NotYetValid,
    Keys(String),
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => {
                f.write_str("Cloudflare Access needs the team domain and the audience tag")
            }
            Self::Missing => f.write_str("the request has no Cloudflare Access token"),
            Self::Malformed => f.write_str("the Cloudflare Access token is malformed"),
            Self::UnsupportedAlg => {
                f.write_str("the Cloudflare Access token is not signed with RS256")
            }
            Self::UnknownKey => f.write_str("no Cloudflare Access key has the token's key id"),
            Self::BadSignature => f.write_str("the Cloudflare Access token's signature is wrong"),
            Self::Audience => f.write_str("the Cloudflare Access token is for another application"),
            Self::Issuer => f.write_str("the Cloudflare Access token is from another team"),
            Self::Expired => f.write_str("the Cloudflare Access token has expired"),
            Self::NotYetValid => f.write_str("the Cloudflare Access token is not valid yet"),
            Self::Keys(e) => write!(f, "couldn't get the Cloudflare Access keys: {e}"),
        }
    }
}

/// What an accepted token says about its user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claims {
    /// The user's email, for logs. None for a service token.
    pub email: Option<String>,
}

/// An RSA public key from a JWK: the modulus and exponent, big-endian, without leading zeros.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RsaKey {
    n: Vec<u8>,
    e: Vec<u8>,
}

impl RsaKey {
    fn verify(&self, message: &[u8], signature_bytes: &[u8]) -> bool {
        signature::RsaPublicKeyComponents {
            n: &self.n,
            e: &self.e,
        }
        .verify(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            message,
            signature_bytes,
        )
        .is_ok()
    }
}

/// Reads a JWK set. Keys that are not RSA signing keys are skipped.
pub fn parse_keys(json: &str) -> Result<HashMap<String, RsaKey>, String> {
    let doc: Value =
        serde_json::from_str(json).map_err(|e| format!("the key list isn't JSON: {e}"))?;
    let keys = doc
        .get("keys")
        .and_then(Value::as_array)
        .ok_or("the key list has no keys")?;
    let mut out = HashMap::new();
    for key in keys {
        let text = |name: &str| key.get(name).and_then(Value::as_str);
        if text("kty") != Some("RSA") || text("use").is_some_and(|u| u != "sig") {
            continue;
        }
        let (Some(kid), Some(n), Some(e)) = (text("kid"), text("n"), text("e")) else {
            continue;
        };
        let (Some(n), Some(e)) = (decode(n), decode(e)) else {
            continue;
        };
        out.insert(
            kid.to_owned(),
            RsaKey {
                n: strip_zeros(&n),
                e: strip_zeros(&e),
            },
        );
    }
    Ok(out)
}

fn decode(part: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(part).ok()
}

fn strip_zeros(bytes: &[u8]) -> Vec<u8> {
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
    bytes[first..].to_vec()
}

/// A token whose header passed: RS256 with a key id. Its signature and claims are not checked
/// yet, so the key can be looked up first.
#[derive(Debug)]
pub struct Parsed<'a> {
    pub kid: String,
    signing_input: &'a str,
    payload: &'a str,
    signature: Vec<u8>,
}

/// Splits a compact JWT and checks its header. Only `RS256` is accepted: `none` and `HS256`
/// (a shared secret the attacker could be given) are refused.
pub fn parse(token: &str) -> Result<Parsed<'_>, AccessError> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(AccessError::Malformed);
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err(AccessError::Malformed);
    }
    let header_json: Value = decode_json(header)?;
    if header_json.get("alg").and_then(Value::as_str) != Some("RS256") {
        return Err(AccessError::UnsupportedAlg);
    }
    let kid = header_json
        .get("kid")
        .and_then(Value::as_str)
        .ok_or(AccessError::Malformed)?
        .to_owned();
    let signature_bytes = decode(signature).ok_or(AccessError::Malformed)?;
    Ok(Parsed {
        kid,
        // `header.payload`, the part Access signed.
        signing_input: &token[..header.len() + 1 + payload.len()],
        payload,
        signature: signature_bytes,
    })
}

fn decode_json(part: &str) -> Result<Value, AccessError> {
    let bytes = decode(part).ok_or(AccessError::Malformed)?;
    serde_json::from_slice(&bytes).map_err(|_| AccessError::Malformed)
}

impl Parsed<'_> {
    /// Checks the signature with `key`, then the claims. The payload is read only after the
    /// signature is good.
    pub fn check(
        &self,
        key: &RsaKey,
        team: &str,
        audience: &str,
        now: u64,
    ) -> Result<Claims, AccessError> {
        if !key.verify(self.signing_input.as_bytes(), &self.signature) {
            return Err(AccessError::BadSignature);
        }
        let payload = decode_json(self.payload)?;
        check_claims(&payload, team, audience, now)
    }
}

/// The issuer, audience and times of a verified token.
pub fn check_claims(
    payload: &Value,
    team: &str,
    audience: &str,
    now: u64,
) -> Result<Claims, AccessError> {
    let issuer = format!("https://{team}");
    if payload.get("iss").and_then(Value::as_str) != Some(issuer.as_str()) {
        return Err(AccessError::Issuer);
    }
    let for_us = match payload.get("aud") {
        Some(Value::String(aud)) => aud == audience,
        Some(Value::Array(list)) => list.iter().any(|aud| aud.as_str() == Some(audience)),
        _ => false,
    };
    if !for_us {
        return Err(AccessError::Audience);
    }
    let exp = time_claim(payload, "exp").ok_or(AccessError::Malformed)?;
    if now >= exp.saturating_add(SKEW_SECS) {
        return Err(AccessError::Expired);
    }
    for name in ["nbf", "iat"] {
        if time_claim(payload, name).is_some_and(|t| t > now.saturating_add(SKEW_SECS)) {
            return Err(AccessError::NotYetValid);
        }
    }
    Ok(Claims {
        email: payload
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn time_claim(payload: &Value, name: &str) -> Option<u64> {
    payload
        .get(name)?
        .as_f64()
        .filter(|t| *t >= 0.0)
        .map(|t| t as u64)
}

/// The Access token of a request: the `Cf-Access-Jwt-Assertion` header, else the
/// `CF_Authorization` cookie.
pub fn token_from_headers(headers: &HeaderMap) -> Option<&str> {
    let header = headers
        .get(ASSERTION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    header.or_else(|| {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|cookies| cookies.split(';'))
            .map(str::trim)
            .find_map(|cookie| cookie.strip_prefix(COOKIE_PREFIX))
            .filter(|token| !token.is_empty())
    })
}

/// True for a host name a team domain can have, e.g. `myteam.cloudflareaccess.com`.
pub fn valid_team_domain(team: &str) -> bool {
    team.len() <= 253
        && team.contains('.')
        && team.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The Access settings the server checks requests with, and the team's keys once fetched.
pub struct Gate {
    state: Mutex<State>,
    http: reqwest::Client,
    /// Tests serve the keys from a local address; the app always uses the team's own address.
    test_keys_url: Option<String>,
}

#[derive(Default)]
struct State {
    policy: Policy,
    team: String,
    audience: String,
    keys: HashMap<String, RsaKey>,
    /// When the keys were last fetched, successfully or not.
    fetched: Option<Instant>,
    /// Why the last fetch failed, until one succeeds.
    fetch_failed: Option<String>,
    /// The last error a token was refused for, until one is accepted.
    error: Option<String>,
}

impl Gate {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            state: Mutex::new(State::default()),
            http,
            test_keys_url: None,
        }
    }

    /// A gate that reads the keys from `url` instead of the team's address.
    #[cfg(test)]
    pub fn for_tests(url: &str) -> Self {
        Self {
            state: Mutex::new(State::default()),
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(FETCH_TIMEOUT)
                .build()
                .unwrap(),
            test_keys_url: Some(url.to_owned()),
        }
    }

    /// Takes the Access part of the tunnel settings. Access applies to named tunnels only.
    /// Keys are kept while the team stays the same.
    pub fn configure(&self, tunnel: &TunnelSettings) {
        let access = &tunnel.access;
        let (policy, team, audience) = if access.enabled && tunnel.mode == TunnelMode::Named {
            let policy = if access.skip_password {
                Policy::SignIn
            } else {
                Policy::Require
            };
            (
                policy,
                team_host(&access.team_domain),
                access.audience.trim().to_owned(),
            )
        } else {
            (Policy::Off, String::new(), String::new())
        };
        let mut s = lock(&self.state);
        if s.team != team {
            s.keys.clear();
            s.fetched = None;
            s.fetch_failed = None;
        }
        if policy == Policy::Off {
            s.error = None;
        }
        s.policy = policy;
        s.team = team;
        s.audience = audience;
    }

    pub fn policy(&self) -> Policy {
        lock(&self.state).policy
    }

    /// The last error an Access token was refused for, or a failed key fetch.
    pub fn last_error(&self) -> Option<String> {
        lock(&self.state).error.clone()
    }

    /// Checks the request's Access token. A request without one is `Missing`.
    pub async fn verify(&self, token: Option<&str>) -> Result<Claims, AccessError> {
        let result = self.check(token).await;
        let mut s = lock(&self.state);
        match &result {
            Ok(_) => s.error = None,
            Err(AccessError::Missing) => {}
            Err(e) => s.error = Some(e.to_string()),
        }
        result
    }

    async fn check(&self, token: Option<&str>) -> Result<Claims, AccessError> {
        let token = token.ok_or(AccessError::Missing)?;
        let (team, audience) = {
            let s = lock(&self.state);
            if s.policy == Policy::Off || !valid_team_domain(&s.team) || s.audience.is_empty() {
                return Err(AccessError::NotConfigured);
            }
            (s.team.clone(), s.audience.clone())
        };
        let parsed = parse(token)?;
        let key = self.key(&team, &parsed.kid).await?;
        parsed.check(&key, &team, &audience, now_secs())
    }

    /// The key with this id. Unknown ids fetch the team's keys again, at most once a minute.
    async fn key(&self, team: &str, kid: &str) -> Result<RsaKey, AccessError> {
        {
            let mut s = lock(&self.state);
            if let Some(key) = s.keys.get(kid) {
                return Ok(key.clone());
            }
            let due = s.fetched.is_none_or(|at| at.elapsed() >= REFETCH_EVERY);
            if !due {
                return Err(match &s.fetch_failed {
                    Some(e) => AccessError::Keys(e.clone()),
                    None => AccessError::UnknownKey,
                });
            }
            // Claimed before the fetch, so requests that arrive meanwhile don't fetch too.
            s.fetched = Some(Instant::now());
        }
        let fetched = self.fetch(team).await;
        let mut s = lock(&self.state);
        if s.team != team {
            // The settings changed while the keys were fetched; they are for the old team.
            return Err(AccessError::NotConfigured);
        }
        match fetched {
            Ok(keys) => {
                s.keys = keys;
                s.fetch_failed = None;
                s.keys.get(kid).cloned().ok_or(AccessError::UnknownKey)
            }
            Err(e) => {
                s.fetch_failed = Some(e.clone());
                Err(AccessError::Keys(e))
            }
        }
    }

    async fn fetch(&self, team: &str) -> Result<HashMap<String, RsaKey>, String> {
        let url = match &self.test_keys_url {
            Some(url) => url.clone(),
            None => format!("https://{team}/cdn-cgi/access/certs"),
        };
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("{team} didn't answer: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{team} answered {}", resp.status()));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| format!("{team} sent no keys: {e}"))?;
        parse_keys(&text)
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tokens signed the way Cloudflare Access signs them, with an RSA key made once per test run
/// (no private key is kept in the repository). Shared with the server tests.
#[cfg(test)]
pub(crate) mod testkeys {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
    use serde_json::json;
    use std::sync::OnceLock;

    pub const TEAM: &str = "myteam.cloudflareaccess.com";
    pub const AUD: &str = "aud-tag-123";
    pub const KID: &str = "test-key-2026";
    pub const NOW: u64 = 1_800_000_000;

    /// The test key: its PKCS#8 DER, and the JWK set publishing its public half under `KID`,
    /// shaped like the set Cloudflare serves at `https://<team>/cdn-cgi/access/certs`.
    fn generated() -> &'static (Vec<u8>, String) {
        static KEY: OnceLock<(Vec<u8>, String)> = OnceLock::new();
        KEY.get_or_init(|| {
            use rsa::pkcs8::EncodePrivateKey;
            use rsa::traits::PublicKeyParts;
            let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
            let der = key.to_pkcs8_der().unwrap().as_bytes().to_vec();
            let jwks = json!({"keys": [{
                "kty": "RSA", "alg": "RS256", "use": "sig", "kid": KID,
                "n": b64(&key.n().to_bytes_be()),
                "e": b64(&key.e().to_bytes_be()),
            }]});
            (der, jwks.to_string())
        })
    }

    /// The JWK set with the test key's public half.
    pub fn jwks() -> &'static str {
        &generated().1
    }

    /// The test key as a key pair.
    pub fn key_pair() -> RsaKeyPair {
        RsaKeyPair::from_pkcs8(&generated().0).unwrap()
    }

    pub fn b64(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// A compact JWT signed with the test key, with `header` as given.
    pub fn sign_with(header: &Value, payload: &Value) -> String {
        let input = format!(
            "{}.{}",
            b64(header.to_string().as_bytes()),
            b64(payload.to_string().as_bytes())
        );
        let pair = key_pair();
        let mut sig = vec![0u8; pair.public().modulus_len()];
        pair.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            input.as_bytes(),
            &mut sig,
        )
        .unwrap();
        format!("{input}.{}", b64(&sig))
    }

    pub fn header() -> Value {
        json!({ "alg": "RS256", "kid": KID, "typ": "JWT" })
    }

    /// Claims that are valid at `now`, for the test team and audience.
    pub fn claims_at(now: u64) -> Value {
        json!({
            "iss": format!("https://{TEAM}"),
            "aud": [AUD],
            "exp": now + 3600,
            "iat": now - 10,
            "nbf": now - 10,
            "email": "aaron@example.com",
            "sub": "user-1",
        })
    }

    /// Claims valid at the fixed time `NOW`.
    pub fn claims() -> Value {
        claims_at(NOW)
    }

    /// A token for `payload`, signed with the test key under the test key id.
    pub fn token(payload: &Value) -> String {
        sign_with(&header(), payload)
    }

    /// A token that is valid right now, as the server tests need.
    pub fn token_now() -> String {
        token(&claims_at(now_secs()))
    }
}

#[cfg(test)]
mod tests {
    use super::testkeys::*;
    use super::*;
    use crate::testutil;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn keys() -> HashMap<String, RsaKey> {
        parse_keys(jwks()).unwrap()
    }

    /// Parses, looks the key up and checks, as `Gate::check` does, at a fixed time.
    fn check_at(token: &str, now: u64) -> Result<Claims, AccessError> {
        let parsed = parse(token)?;
        let key = keys()
            .get(&parsed.kid)
            .cloned()
            .ok_or(AccessError::UnknownKey)?;
        parsed.check(&key, TEAM, AUD, now)
    }

    #[test]
    fn a_valid_token_passes_and_gives_the_email() {
        let claims = check_at(&token(&claims()), NOW).unwrap();
        assert_eq!(claims.email.as_deref(), Some("aaron@example.com"));
    }

    #[test]
    fn audience_may_be_a_string_or_a_list() {
        let mut p = claims();
        p["aud"] = json!(AUD);
        assert!(check_at(&token(&p), NOW).is_ok());
        p["aud"] = json!(["other", AUD]);
        assert!(check_at(&token(&p), NOW).is_ok());
    }

    #[test]
    fn wrong_audience_is_refused() {
        let mut p = claims();
        p["aud"] = json!(["someone-else"]);
        assert_eq!(check_at(&token(&p), NOW), Err(AccessError::Audience));
        p.as_object_mut().unwrap().remove("aud");
        assert_eq!(check_at(&token(&p), NOW), Err(AccessError::Audience));
    }

    #[test]
    fn wrong_issuer_is_refused() {
        let mut p = claims();
        p["iss"] = json!("https://evil.cloudflareaccess.com");
        assert_eq!(check_at(&token(&p), NOW), Err(AccessError::Issuer));
        // Plain http is not the team's issuer either.
        p["iss"] = json!(format!("http://{TEAM}"));
        assert_eq!(check_at(&token(&p), NOW), Err(AccessError::Issuer));
    }

    #[test]
    fn expired_tokens_are_refused_with_sixty_seconds_of_slack() {
        let mut p = claims();
        p["exp"] = json!(NOW - 3600);
        assert_eq!(check_at(&token(&p), NOW), Err(AccessError::Expired));
        p["exp"] = json!(NOW - 30);
        assert!(check_at(&token(&p), NOW).is_ok());
        p["exp"] = json!(NOW - 60);
        assert_eq!(check_at(&token(&p), NOW), Err(AccessError::Expired));
    }

    #[test]
    fn tokens_from_the_future_are_refused_with_sixty_seconds_of_slack() {
        let mut p = claims();
        p["nbf"] = json!(NOW + 3600);
        assert_eq!(check_at(&token(&p), NOW), Err(AccessError::NotYetValid));
        p["nbf"] = json!(NOW + 30);
        p["iat"] = json!(NOW + 3600);
        assert_eq!(check_at(&token(&p), NOW), Err(AccessError::NotYetValid));
        p["iat"] = json!(NOW + 30);
        assert!(check_at(&token(&p), NOW).is_ok());
    }

    #[test]
    fn a_tampered_payload_fails_the_signature() {
        let good = token(&claims());
        let mut parts: Vec<&str> = good.split('.').collect();
        let mut forged = claims();
        forged["email"] = json!("mallory@example.com");
        let payload = b64(forged.to_string().as_bytes());
        parts[1] = &payload;
        let forged_token = parts.join(".");
        assert_eq!(check_at(&forged_token, NOW), Err(AccessError::BadSignature));
    }

    #[test]
    fn a_corrupted_signature_fails() {
        let signed = token(&claims());
        let mut parts: Vec<String> = signed.split('.').map(str::to_owned).collect();
        // Flip one signature byte: the signature no longer matches the signed input.
        let mut sig = URL_SAFE_NO_PAD.decode(&parts[2]).unwrap();
        sig[10] ^= 0x01;
        parts[2] = b64(&sig);
        assert_eq!(
            check_at(&parts.join("."), NOW),
            Err(AccessError::BadSignature)
        );
    }

    #[test]
    fn unknown_key_ids_are_not_found() {
        let other = sign_with(&json!({ "alg": "RS256", "kid": "rotated" }), &claims());
        assert_eq!(check_at(&other, NOW), Err(AccessError::UnknownKey));
    }

    #[test]
    fn only_rs256_is_accepted() {
        let unsigned = format!(
            "{}.{}.{}",
            b64(json!({ "alg": "none", "kid": KID }).to_string().as_bytes()),
            b64(claims().to_string().as_bytes()),
            b64(b"x")
        );
        assert_eq!(parse(&unsigned).err(), Some(AccessError::UnsupportedAlg));
        // An HS256 token signed with the public key's bytes, as an attacker might try.
        let hs = format!(
            "{}.{}.{}",
            b64(json!({ "alg": "HS256", "kid": KID }).to_string().as_bytes()),
            b64(claims().to_string().as_bytes()),
            b64(b"not-a-real-mac")
        );
        assert_eq!(parse(&hs).err(), Some(AccessError::UnsupportedAlg));
        let no_alg = format!(
            "{}.{}.{}",
            b64(json!({ "kid": KID }).to_string().as_bytes()),
            b64(claims().to_string().as_bytes()),
            b64(b"x")
        );
        assert_eq!(parse(&no_alg).err(), Some(AccessError::UnsupportedAlg));
    }

    #[test]
    fn malformed_tokens_are_refused() {
        for bad in [
            "",
            "one-part",
            "two.parts",
            "a.b.c.d",
            "..",
            "a..c",
            "!!!.!!!.!!!",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let no_kid = format!(
            "{}.{}.{}",
            b64(json!({ "alg": "RS256" }).to_string().as_bytes()),
            b64(claims().to_string().as_bytes()),
            b64(b"x")
        );
        assert_eq!(parse(&no_kid).err(), Some(AccessError::Malformed));
    }

    #[test]
    fn keys_come_from_the_jwk_set_and_skip_other_keys() {
        let set = keys();
        assert_eq!(set.len(), 1);
        assert!(set.contains_key(KID));
        let mixed = r#"{"keys":[
            {"kty":"EC","kid":"ec","crv":"P-256","x":"a","y":"b"},
            {"kty":"RSA","kid":"enc","use":"enc","n":"AQAB","e":"AQAB"},
            {"kty":"RSA","kid":"ok","n":"AQAB","e":"AQAB"}]}"#;
        let found = parse_keys(mixed).unwrap();
        assert_eq!(found.keys().collect::<Vec<_>>(), vec!["ok"]);
        assert!(parse_keys("not json").is_err());
        assert!(parse_keys(r#"{"nope":1}"#).is_err());
    }

    #[test]
    fn claims_check_rejects_a_non_string_issuer_and_missing_expiry() {
        let mut p = claims();
        p["iss"] = json!(7);
        assert_eq!(check_claims(&p, TEAM, AUD, NOW), Err(AccessError::Issuer));
        let mut p = claims();
        p.as_object_mut().unwrap().remove("exp");
        assert_eq!(
            check_claims(&p, TEAM, AUD, NOW),
            Err(AccessError::Malformed)
        );
    }

    #[test]
    fn team_domains_are_hostnames() {
        assert!(valid_team_domain("myteam.cloudflareaccess.com"));
        assert!(valid_team_domain("a-b.example.org"));
        assert!(!valid_team_domain(""));
        assert!(!valid_team_domain("nodots"));
        assert!(!valid_team_domain("https://x.example.com"));
        assert!(!valid_team_domain("x.example.com/path"));
        assert!(!valid_team_domain("-x.example.com"));
        assert!(!valid_team_domain("x..example.com"));
        assert!(!valid_team_domain("X.example.com"));
    }

    #[test]
    fn the_token_comes_from_the_header_or_the_cookie() {
        let mut headers = HeaderMap::new();
        assert_eq!(token_from_headers(&headers), None);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; CF_Authorization=cookie-tok"),
        );
        assert_eq!(token_from_headers(&headers), Some("cookie-tok"));
        headers.insert(ASSERTION, HeaderValue::from_static("header-tok"));
        assert_eq!(token_from_headers(&headers), Some("header-tok"));
        let mut empty = HeaderMap::new();
        empty.insert(ASSERTION, HeaderValue::from_static("  "));
        assert_eq!(token_from_headers(&empty), None);
    }

    fn tunnel(enabled: bool, mode: TunnelMode, skip: bool) -> TunnelSettings {
        TunnelSettings {
            enabled: true,
            mode,
            access: crate::config::AccessSettings {
                enabled,
                team_domain: TEAM.into(),
                audience: AUD.into(),
                skip_password: skip,
            },
            ..TunnelSettings::default()
        }
    }

    #[test]
    fn access_applies_to_named_tunnels_only() {
        let gate = Gate::new();
        gate.configure(&tunnel(true, TunnelMode::Named, false));
        assert_eq!(gate.policy(), Policy::Require);
        gate.configure(&tunnel(true, TunnelMode::Named, true));
        assert_eq!(gate.policy(), Policy::SignIn);
        gate.configure(&tunnel(true, TunnelMode::Quick, true));
        assert_eq!(gate.policy(), Policy::Off);
        gate.configure(&tunnel(false, TunnelMode::Named, false));
        assert_eq!(gate.policy(), Policy::Off);
    }

    #[tokio::test]
    async fn an_unconfigured_gate_refuses_everything() {
        let gate = Gate::new();
        gate.configure(&TunnelSettings {
            mode: TunnelMode::Named,
            access: crate::config::AccessSettings {
                enabled: true,
                team_domain: "no-dots".into(),
                audience: AUD.into(),
                skip_password: false,
            },
            ..TunnelSettings::default()
        });
        assert_eq!(
            gate.verify(Some(token(&claims()).as_str())).await.err(),
            Some(AccessError::NotConfigured)
        );
    }

    /// Serves the test JWK set on a local port; `keys` counts the fetches.
    async fn serve_keys(body: &'static str) -> (String, std::sync::Arc<Mutex<usize>>) {
        let fetches = std::sync::Arc::new(Mutex::new(0));
        let seen = fetches.clone();
        let (base, _) = testutil::serve(move |req| {
            if req.path == "/certs" {
                *seen.lock().unwrap() += 1;
                testutil::Response::with_type(200, "application/json", body)
            } else {
                testutil::Response::status(404)
            }
        })
        .await;
        (format!("{base}/certs"), fetches)
    }

    async fn gate_with_keys(
        body: &'static str,
        skip: bool,
    ) -> (Gate, std::sync::Arc<Mutex<usize>>) {
        let (url, fetches) = serve_keys(body).await;
        let gate = Gate::for_tests(&url);
        gate.configure(&tunnel(true, TunnelMode::Named, skip));
        (gate, fetches)
    }

    #[tokio::test]
    async fn gate_accepts_a_valid_token_and_clears_the_error() {
        let (gate, fetches) = gate_with_keys(jwks(), false).await;
        // Times are checked against the real clock, so the tokens are made from it.
        let now = now_secs();
        let mut stale = claims_at(now);
        stale["iat"] = json!(now - 7200);
        stale["nbf"] = json!(now - 7200);
        stale["exp"] = json!(now - 3600);
        assert!(gate.verify(Some(token(&stale).as_str())).await.is_err());
        assert!(gate.last_error().unwrap().contains("expired"));
        let fresh = claims_at(now);
        let claims = gate.verify(Some(token(&fresh).as_str())).await.unwrap();
        assert_eq!(claims.email.as_deref(), Some("aaron@example.com"));
        assert_eq!(gate.last_error(), None);
        // One fetch served both tokens.
        assert_eq!(*fetches.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn gate_refuses_a_missing_token_without_recording_an_error() {
        let (gate, _) = gate_with_keys(jwks(), false).await;
        assert_eq!(gate.verify(None).await.err(), Some(AccessError::Missing));
        assert_eq!(gate.last_error(), None);
    }

    #[tokio::test]
    async fn unknown_kid_refetches_the_keys_at_most_once_a_minute() {
        let (gate, fetches) = gate_with_keys(jwks(), false).await;
        let other = sign_with(&json!({ "alg": "RS256", "kid": "rotated" }), &claims());
        let other = other.as_str();
        assert_eq!(
            gate.verify(Some(other)).await.err(),
            Some(AccessError::UnknownKey)
        );
        assert_eq!(*fetches.lock().unwrap(), 1);
        // Asking again within the minute does not fetch.
        assert_eq!(
            gate.verify(Some(other)).await.err(),
            Some(AccessError::UnknownKey)
        );
        assert_eq!(*fetches.lock().unwrap(), 1);
        // A known key still works without a fetch.
        assert!(gate.verify(Some(token_now().as_str())).await.is_ok());
        assert_eq!(*fetches.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_failed_key_fetch_is_the_recorded_error() {
        let (gate, _) = gate_with_keys("<html>not keys</html>", false).await;
        let signed = token(&claims());
        let err = gate.verify(Some(signed.as_str())).await.err().unwrap();
        assert!(matches!(err, AccessError::Keys(_)), "{err:?}");
        assert!(gate.last_error().unwrap().contains("couldn't get"));
        // Within the minute the same failure is reported, without another fetch.
        let again = gate.verify(Some(signed.as_str())).await.err().unwrap();
        assert_eq!(again, err);
    }

    #[tokio::test]
    async fn an_unreachable_team_is_a_key_error() {
        let gate = Gate::for_tests("http://127.0.0.1:1/certs");
        gate.configure(&tunnel(true, TunnelMode::Named, false));
        let signed = token(&claims());
        let err = gate.verify(Some(signed.as_str())).await.err().unwrap();
        assert!(matches!(err, AccessError::Keys(_)), "{err:?}");
    }
}
