//! Password check, login sessions and the login throttle for the remote-access server.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use uuid::Uuid;

const COOKIE_NAME: &str = "cleanrstai_session";
const SESSION_ABSOLUTE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const SESSION_IDLE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// This many failed logins within `FAILURE_WINDOW` block the address for `BLOCK_FOR`.
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
const BLOCK_FOR: Duration = Duration::from_secs(60);

struct Session {
    created: Instant,
    last_used: Instant,
}

#[derive(Default)]
struct Failures {
    /// Failed logins within the window, oldest first.
    times: Vec<Instant>,
    blocked_until: Option<Instant>,
}

/// Login state of one remote-access server: the password, sessions and throttle.
pub struct Auth {
    /// SHA-256 of the configured password, so comparisons are of fixed length.
    password_digest: [u8; 32],
    /// Add `Secure` to the cookie (TLS on).
    secure_cookie: bool,
    sessions: Mutex<HashMap<String, Session>>,
    failures: Mutex<HashMap<IpAddr, Failures>>,
}

impl Auth {
    pub fn new(password: &str, secure_cookie: bool) -> Self {
        Self {
            password_digest: digest(password),
            secure_cookie,
            sessions: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// True when `given` is the configured password. Compares digests in constant time.
    pub fn check_password(&self, given: &str) -> bool {
        let given = digest(given);
        let mut diff = 0u8;
        for (a, b) in given.iter().zip(self.password_digest.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }

    /// True when an `Authorization` header carries `Bearer <password>`.
    pub fn check_bearer(&self, header: &str) -> bool {
        let header = header.trim();
        let Some(scheme) = header.get(..7) else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("Bearer ") {
            return false;
        }
        self.check_password(header[7..].trim())
    }

    pub fn throttled(&self, ip: IpAddr) -> bool {
        self.throttled_at(ip, Instant::now())
    }

    pub fn record_failure(&self, ip: IpAddr) {
        self.record_failure_at(ip, Instant::now());
    }

    pub fn clear_failures(&self, ip: IpAddr) {
        lock(&self.failures).remove(&ip);
    }

    pub fn create_session(&self) -> String {
        self.create_session_at(Instant::now())
    }

    /// True for a live session, and refreshes its idle timer.
    pub fn valid_session(&self, token: &str) -> bool {
        self.valid_session_at(token, Instant::now())
    }

    /// True for a live session, without refreshing its idle timer. Event streams use this, so
    /// that an open stream alone does not keep a session alive.
    pub fn session_alive(&self, token: &str) -> bool {
        self.session_alive_at(token, Instant::now())
    }

    pub fn remove_session(&self, token: &str) {
        lock(&self.sessions).remove(token);
    }

    /// The `Set-Cookie` value that starts a session.
    pub fn session_cookie(&self, token: &str) -> String {
        format!(
            "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
            SESSION_ABSOLUTE.as_secs(),
            self.secure_attr()
        )
    }

    /// The `Set-Cookie` value that removes the session cookie.
    pub fn clear_cookie(&self) -> String {
        format!(
            "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
            self.secure_attr()
        )
    }

    /// The session token from a `Cookie` header with one or more cookies.
    pub fn token_from_cookie_header(header: &str) -> Option<&str> {
        header
            .split(';')
            .map(str::trim)
            .filter_map(|cookie| cookie.strip_prefix(COOKIE_NAME)?.strip_prefix('='))
            .find(|token| !token.is_empty())
    }

    fn secure_attr(&self) -> &'static str {
        if self.secure_cookie { "; Secure" } else { "" }
    }

    fn throttled_at(&self, ip: IpAddr, now: Instant) -> bool {
        lock(&self.failures)
            .get(&ip)
            .and_then(|f| f.blocked_until)
            .is_some_and(|until| now < until)
    }

    fn record_failure_at(&self, ip: IpAddr, now: Instant) {
        let mut failures = lock(&self.failures);
        let entry = failures.entry(ip).or_default();
        entry
            .times
            .retain(|t| now.saturating_duration_since(*t) < FAILURE_WINDOW);
        entry.times.push(now);
        if entry.times.len() >= MAX_FAILURES {
            entry.times.clear();
            entry.blocked_until = Some(now + BLOCK_FOR);
        }
        // Forget addresses whose window and block have both passed, so the map stays small.
        failures.retain(|_, f| {
            f.blocked_until.is_some_and(|until| now < until)
                || f.times
                    .iter()
                    .any(|t| now.saturating_duration_since(*t) < FAILURE_WINDOW)
        });
    }

    fn create_session_at(&self, now: Instant) -> String {
        // Two random v4 UUIDs: 64 lowercase hex characters.
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let mut sessions = lock(&self.sessions);
        prune(&mut sessions, now);
        sessions.insert(
            token.clone(),
            Session {
                created: now,
                last_used: now,
            },
        );
        token
    }

    fn valid_session_at(&self, token: &str, now: Instant) -> bool {
        let mut sessions = lock(&self.sessions);
        prune(&mut sessions, now);
        match sessions.get_mut(token) {
            Some(session) => {
                session.last_used = now;
                true
            }
            None => false,
        }
    }

    fn session_alive_at(&self, token: &str, now: Instant) -> bool {
        let mut sessions = lock(&self.sessions);
        prune(&mut sessions, now);
        sessions.contains_key(token)
    }
}

fn digest(password: &str) -> [u8; 32] {
    Sha256::digest(password.as_bytes()).into()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A panic while holding the lock leaves plain data behind; keep serving.
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Drops sessions past their absolute or idle lifetime.
fn prune(sessions: &mut HashMap<String, Session>, now: Instant) {
    sessions.retain(|_, s| {
        now.saturating_duration_since(s.created) < SESSION_ABSOLUTE
            && now.saturating_duration_since(s.last_used) < SESSION_IDLE
    });
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))
    }

    fn auth(secure: bool) -> Auth {
        Auth::new("correct horse", secure)
    }

    #[test]
    fn password_right_and_wrong() {
        let a = auth(false);
        assert!(a.check_password("correct horse"));
        assert!(!a.check_password("correct horsE"));
        assert!(!a.check_password(""));
        assert!(!a.check_password("correct horse "));
    }

    #[test]
    fn bearer_header() {
        let a = auth(false);
        assert!(a.check_bearer("Bearer correct horse"));
        assert!(a.check_bearer("bearer correct horse"));
        assert!(!a.check_bearer("Bearer wrong"));
        assert!(!a.check_bearer("correct horse"));
        assert!(!a.check_bearer("Basic correct horse"));
        assert!(!a.check_bearer("Bear"));
        assert!(!a.check_bearer(""));
    }

    #[test]
    fn cookie_parsing_with_several_cookies() {
        assert_eq!(
            Auth::token_from_cookie_header("a=1; cleanrstai_session=abc123; b=2"),
            Some("abc123")
        );
        assert_eq!(
            Auth::token_from_cookie_header("cleanrstai_session=xyz"),
            Some("xyz")
        );
        assert_eq!(Auth::token_from_cookie_header("a=1; b=2"), None);
        assert_eq!(Auth::token_from_cookie_header("cleanrstai_session="), None);
        assert_eq!(
            Auth::token_from_cookie_header("x=cleanrstai_session=no"),
            None
        );
        assert_eq!(
            Auth::token_from_cookie_header("cleanrstai_session_old=no; cleanrstai_session=yes"),
            Some("yes")
        );
    }

    #[test]
    fn session_create_valid_remove() {
        let a = auth(false);
        let t0 = Instant::now();
        let token = a.create_session_at(t0);
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(a.valid_session_at(&token, t0));
        assert!(!a.valid_session_at("nope", t0));
        a.remove_session(&token);
        assert!(!a.valid_session_at(&token, t0));
    }

    #[test]
    fn sessions_expire_after_seven_idle_days() {
        let a = auth(false);
        let t0 = Instant::now();
        let token = a.create_session_at(t0);
        assert!(a.valid_session_at(&token, t0 + Duration::from_secs(6 * 24 * 3600)));
        // Used 6 days ago: still fine.
        assert!(a.valid_session_at(&token, t0 + Duration::from_secs(12 * 24 * 3600)));
        // Left alone for 7 days: gone.
        let idle = a.create_session_at(t0);
        assert!(!a.valid_session_at(&idle, t0 + SESSION_IDLE));
    }

    #[test]
    fn session_alive_does_not_refresh_the_idle_timer() {
        let a = auth(false);
        let t0 = Instant::now();
        let token = a.create_session_at(t0);
        let day = |d: u64| t0 + Duration::from_secs(d * 24 * 3600);
        assert!(a.session_alive_at(&token, day(6)));
        // The check on day 6 did not count as use, so the session is idle for 8 days by day 8.
        assert!(!a.session_alive_at(&token, day(8)));
        assert!(!a.session_alive_at("nope", day(0)));
    }

    #[test]
    fn sessions_expire_after_thirty_days_even_when_used() {
        let a = auth(false);
        let t0 = Instant::now();
        let token = a.create_session_at(t0);
        let day = |d: u64| t0 + Duration::from_secs(d * 24 * 3600);
        for d in [6, 12, 18, 24] {
            assert!(a.valid_session_at(&token, day(d)), "day {d}");
        }
        assert!(a.valid_session_at(&token, day(29)));
        assert!(!a.valid_session_at(&token, day(30)));
    }

    #[test]
    fn throttle_after_five_failures_then_release_after_a_minute() {
        let a = auth(false);
        let t0 = Instant::now();
        for i in 0..4 {
            a.record_failure_at(ip(), t0 + Duration::from_secs(i));
        }
        assert!(!a.throttled_at(ip(), t0 + Duration::from_secs(4)));
        a.record_failure_at(ip(), t0 + Duration::from_secs(4));
        assert!(a.throttled_at(ip(), t0 + Duration::from_secs(5)));
        assert!(a.throttled_at(ip(), t0 + Duration::from_secs(63)));
        assert!(!a.throttled_at(ip(), t0 + Duration::from_secs(64)));
        // Other addresses are not affected.
        assert!(!a.throttled_at(IpAddr::V4(Ipv4Addr::LOCALHOST), t0));
    }

    #[test]
    fn failures_outside_the_window_do_not_count() {
        let a = auth(false);
        let t0 = Instant::now();
        for i in 0..4 {
            a.record_failure_at(ip(), t0 + Duration::from_secs(i * 200));
        }
        // The first failure is more than 10 minutes before the fifth.
        a.record_failure_at(ip(), t0 + Duration::from_secs(4 * 200 + 1));
        assert!(!a.throttled_at(ip(), t0 + Duration::from_secs(900)));
        a.record_failure_at(ip(), t0 + Duration::from_secs(4 * 200 + 2));
        assert!(!a.throttled_at(ip(), t0 + Duration::from_secs(900)));
    }

    #[test]
    fn failure_table_forgets_quiet_addresses() {
        let a = auth(false);
        let t0 = Instant::now();
        for i in 0..200u8 {
            a.record_failure_at(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)), t0);
        }
        for _ in 0..MAX_FAILURES {
            a.record_failure_at(ip(), t0);
        }
        assert_eq!(lock(&a.failures).len(), 201);
        // A blocked address is kept while its block lasts, with no failures left in its window.
        let other = IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1));
        a.record_failure_at(other, t0 + Duration::from_secs(30));
        assert!(a.throttled_at(ip(), t0 + Duration::from_secs(30)));
        // Ten minutes on, the quiet addresses and the expired block are gone.
        a.record_failure_at(other, t0 + FAILURE_WINDOW);
        assert_eq!(lock(&a.failures).len(), 1);
    }

    #[test]
    fn clear_failures_lifts_the_block() {
        let a = auth(false);
        let t0 = Instant::now();
        for _ in 0..MAX_FAILURES {
            a.record_failure_at(ip(), t0);
        }
        assert!(a.throttled_at(ip(), t0));
        a.clear_failures(ip());
        assert!(!a.throttled_at(ip(), t0));
    }

    #[test]
    fn cookies_have_the_spec_attributes() {
        let plain = auth(false).session_cookie("tok");
        assert_eq!(
            plain,
            "cleanrstai_session=tok; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000"
        );
        assert!(!plain.contains("Secure"));
        let secure = auth(true).session_cookie("tok");
        assert!(secure.ends_with("Max-Age=2592000; Secure"));

        let clear = auth(false).clear_cookie();
        assert_eq!(
            clear,
            "cleanrstai_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
        );
        assert!(auth(true).clear_cookie().ends_with("Max-Age=0; Secure"));
    }
}
