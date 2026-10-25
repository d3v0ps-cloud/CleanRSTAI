//! Keep API keys and other secrets in the OS credential store (macOS Keychain, Windows
//! Credential Manager, Secret Service on Linux) instead of plain text in `settings.json`.
//!
//! In memory, [`Settings`] always holds secrets in plain text. Only the file on disk changes:
//! on save each secret is written to the store and blanked in the file, and on load the
//! blanks are filled back in from the store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};

use crate::config::{KeyValue, McpTransportConfig, Settings};

const SERVICE: &str = "CleanRSTAI";

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// The platform credential store.
pub struct OsKeychain;

impl OsKeychain {
    /// `Err` with a reason when no credential store can be used on this machine.
    pub fn status() -> Result<(), String> {
        keyring::Entry::store_status()
            .as_ref()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())
    }
}

impl SecretStore for OsKeychain {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        match Self::entry(account)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        Self::entry(account)?
            .set_password(secret)
            .map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Fallback when no OS keychain is available: a JSON file readable only by the user.
pub struct FileStore {
    path: std::path::PathBuf,
    lock: std::sync::Mutex<()>,
}

impl FileStore {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            lock: std::sync::Mutex::new(()),
        }
    }

    fn read(&self) -> std::collections::BTreeMap<String, String> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write(&self, map: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
        crate::config::write_private(&self.path, &bytes).map_err(|e| format!("{e:#}"))
    }
}

impl SecretStore for FileStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let _g = self.lock.lock().unwrap();
        Ok(self.read().get(account).cloned())
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let _g = self.lock.lock().unwrap();
        let mut map = self.read();
        map.insert(account.into(), secret.into());
        self.write(&map)
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        let _g = self.lock.lock().unwrap();
        let mut map = self.read();
        if map.remove(account).is_some() {
            self.write(&map)?;
        }
        Ok(())
    }
}

/// Account of the one item that [`Bundled`] keeps every secret in.
const BUNDLE: &str = "secrets";

/// Every secret in one store item (a JSON object), read once per run. macOS asks for the login
/// password for each Keychain item an app may not read yet (a new install or a new build), so
/// one item means one prompt instead of one per API key, header and sign-in. Secrets that
/// older versions saved as separate items move into the bundle the first time they are read.
pub struct Bundled<S> {
    inner: S,
    /// The bundle, or why it couldn't be read (kept, so a refusal isn't asked again).
    cache: Mutex<Option<Result<BTreeMap<String, String>, String>>>,
}

impl<S: SecretStore> Bundled<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            cache: Mutex::new(None),
        }
    }

    fn with_map<R>(
        &self,
        f: impl FnOnce(&S, &mut BTreeMap<String, String>) -> Result<R, String>,
    ) -> Result<R, String> {
        let mut cache = self.cache.lock().unwrap();
        let loaded = cache.get_or_insert_with(|| match self.inner.get(BUNDLE)? {
            Some(json) => serde_json::from_str(&json).map_err(|e| format!("reading secrets: {e}")),
            None => Ok(BTreeMap::new()),
        });
        match loaded {
            Ok(map) => f(&self.inner, map),
            Err(e) => Err(e.clone()),
        }
    }

    fn save(inner: &S, map: &BTreeMap<String, String>) -> Result<(), String> {
        inner.set(
            BUNDLE,
            &serde_json::to_string(map).map_err(|e| e.to_string())?,
        )
    }
}

impl<S: SecretStore> SecretStore for Bundled<S> {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        self.with_map(|inner, map| {
            if let Some(v) = map.get(account) {
                return Ok(Some(v.clone()));
            }
            let Some(v) = inner.get(account)? else {
                return Ok(None);
            };
            map.insert(account.into(), v.clone());
            match Self::save(inner, map) {
                Ok(()) => {
                    if let Err(e) = inner.delete(account) {
                        log::warn!("removing {account} after moving it: {e}");
                    }
                }
                Err(e) => log::warn!("moving {account} into one keychain item: {e}"),
            }
            Ok(Some(v))
        })
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.with_map(|inner, map| {
            if map.get(account).is_some_and(|v| v == secret) {
                return Ok(());
            }
            let old = map.insert(account.into(), secret.into());
            Self::save(inner, map).inspect_err(|_| match old {
                Some(old) => {
                    map.insert(account.into(), old);
                }
                None => {
                    map.remove(account);
                }
            })
        })
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.with_map(|inner, map| {
            let Some(old) = map.remove(account) else {
                return Ok(());
            };
            Self::save(inner, map).inspect_err(|_| {
                map.insert(account.into(), old);
            })
        })
    }
}

/// The OS credential store, shared by settings and MCP sign-ins. On macOS every secret goes in
/// one Keychain item ([`Bundled`]); elsewhere each has its own item, since Windows Credential
/// Manager holds at most 2.5 KB per item and doesn't prompt anyway.
pub fn keychain() -> Arc<dyn SecretStore> {
    static STORE: LazyLock<Arc<dyn SecretStore>> = LazyLock::new(|| {
        if cfg!(target_os = "macos") {
            Arc::new(Bundled::new(OsKeychain))
        } else {
            Arc::new(OsKeychain)
        }
    });
    STORE.clone()
}

/// The keychain when available, otherwise a private file in the config directory.
/// Used for data that must persist even without a keychain (MCP sign-in tokens).
pub fn durable_store() -> Arc<dyn SecretStore + Send + Sync> {
    if OsKeychain::status().is_ok() {
        keychain()
    } else {
        Arc::new(FileStore::new(
            crate::config::config_dir().join("tokens.json"),
        ))
    }
}

/// Every secret value in `settings`, with a stable account name for the store.
fn slots(settings: &mut Settings) -> Vec<(String, &mut String)> {
    fn kv<'a>(prefix: &str, list: &'a mut [KeyValue], out: &mut Vec<(String, &'a mut String)>) {
        for item in list.iter_mut().filter(|kv| !kv.key.trim().is_empty()) {
            out.push((format!("{prefix}:{}", item.key.trim()), &mut item.value));
        }
    }
    let mut out = Vec::new();
    for p in &mut settings.providers {
        out.push((format!("provider:{}:api_key", p.id), &mut p.api_key));
        kv(
            &format!("provider:{}:header", p.id),
            &mut p.extra_headers,
            &mut out,
        );
    }
    for s in &mut settings.mcp_servers {
        match &mut s.transport {
            McpTransportConfig::Stdio { env, .. } => {
                kv(&format!("mcp:{}:env", s.id), env, &mut out)
            }
            McpTransportConfig::Http { headers, .. } => {
                kv(&format!("mcp:{}:header", s.id), headers, &mut out)
            }
        }
    }
    out
}

pub fn has_any_secret(settings: &Settings) -> bool {
    let mut copy = settings.clone();
    slots(&mut copy).iter().any(|(_, v)| !v.is_empty())
}

/// Move secrets from `settings` into `store`, blanking them. Returns the accounts written.
/// On error nothing is blanked, so the caller can fall back to saving plain text.
pub fn externalize(
    settings: &mut Settings,
    store: &dyn SecretStore,
) -> Result<Vec<String>, String> {
    let mut accounts = Vec::new();
    for (account, value) in slots(&mut settings.clone()) {
        if !value.is_empty() {
            store.set(&account, value)?;
            accounts.push(account);
        }
    }
    for (_, value) in slots(settings) {
        value.clear();
    }
    Ok(accounts)
}

/// Fill blank secrets in `settings` from `store`. Returns accounts that couldn't be read.
pub fn internalize(
    settings: &mut Settings,
    store: &dyn SecretStore,
    accounts: &[String],
) -> Vec<String> {
    let wanted: BTreeSet<&str> = accounts.iter().map(String::as_str).collect();
    let mut failed = Vec::new();
    for (account, value) in slots(settings) {
        if !value.is_empty() || !wanted.contains(account.as_str()) {
            continue;
        }
        match store.get(&account) {
            Ok(Some(v)) => *value = v,
            Ok(None) => failed.push(account),
            Err(e) => {
                log::warn!("reading {account} from keychain: {e}");
                failed.push(account);
            }
        }
    }
    failed
}

/// Delete accounts that were stored before but aren't any more (removed providers, keys…).
pub fn remove_stale(store: &dyn SecretStore, before: &[String], now: &[String]) {
    let keep: BTreeSet<&String> = now.iter().collect();
    for account in before.iter().filter(|a| !keep.contains(a)) {
        if let Err(e) = store.delete(account) {
            log::warn!("removing {account} from keychain: {e}");
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::config::{McpServerConfig, ProviderConfig};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    pub struct MemoryStore {
        pub items: Mutex<HashMap<String, String>>,
        pub fail: bool,
        pub reads: AtomicUsize,
        pub writes: AtomicUsize,
    }

    impl SecretStore for MemoryStore {
        fn get(&self, a: &str) -> Result<Option<String>, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.items.lock().unwrap().get(a).cloned())
        }
        fn set(&self, a: &str, s: &str) -> Result<(), String> {
            if self.fail {
                return Err("locked".into());
            }
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.items.lock().unwrap().insert(a.into(), s.into());
            Ok(())
        }
        fn delete(&self, a: &str) -> Result<(), String> {
            self.items.lock().unwrap().remove(a);
            Ok(())
        }
    }

    pub fn sample() -> Settings {
        Settings {
            providers: vec![ProviderConfig {
                id: "p1".into(),
                api_key: "sk-1".into(),
                extra_headers: vec![KeyValue {
                    key: "X-Org".into(),
                    value: "org".into(),
                }],
                ..Default::default()
            }],
            mcp_servers: vec![McpServerConfig {
                id: "m1".into(),
                transport: McpTransportConfig::Stdio {
                    command: "x".into(),
                    args: vec![],
                    env: vec![KeyValue {
                        key: "TOKEN".into(),
                        value: "t".into(),
                    }],
                    cwd: String::new(),
                },
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_through_store() {
        let store = MemoryStore::default();
        let original = sample();
        let mut on_disk = original.clone();
        let accounts = externalize(&mut on_disk, &store).unwrap();
        assert_eq!(accounts.len(), 3);
        assert!(!has_any_secret(&on_disk), "file holds no secrets");
        assert!(!serde_json::to_string(&on_disk).unwrap().contains("sk-1"));
        let failed = internalize(&mut on_disk, &store, &accounts);
        assert!(failed.is_empty());
        assert_eq!(on_disk, original);
    }

    #[test]
    fn failure_leaves_plain_text_and_reports_missing() {
        let store = MemoryStore {
            fail: true,
            ..Default::default()
        };
        let mut s = sample();
        assert!(externalize(&mut s, &store).is_err());
        assert_eq!(s, sample(), "nothing blanked when the store fails");

        let empty = MemoryStore::default();
        let mut blank = sample();
        let accounts = externalize(&mut blank, &MemoryStore::default()).unwrap();
        let failed = internalize(&mut blank, &empty, &accounts);
        assert_eq!(failed.len(), 3);
    }

    #[test]
    fn removes_stale_entries() {
        let store = MemoryStore::default();
        let mut s = sample();
        let before = externalize(&mut s.clone(), &store).unwrap();
        s.providers[0].extra_headers.clear();
        let now = externalize(&mut s, &store).unwrap();
        remove_stale(&store, &before, &now);
        let items = store.items.lock().unwrap();
        assert!(!items.contains_key("provider:p1:header:X-Org"));
        assert!(items.contains_key("provider:p1:api_key"));
    }

    #[test]
    fn bundles_every_secret_in_one_item() {
        let store = Bundled::new(MemoryStore::default());
        let original = sample();
        let mut on_disk = original.clone();
        let accounts = externalize(&mut on_disk, &store).unwrap();
        assert_eq!(accounts.len(), 3);
        let items: Vec<String> = store.inner.items.lock().unwrap().keys().cloned().collect();
        assert_eq!(items, [BUNDLE]);

        // Saving again unchanged writes nothing; a fresh run reads the item once.
        let writes = store.inner.writes.load(Ordering::SeqCst);
        externalize(&mut original.clone(), &store).unwrap();
        assert_eq!(store.inner.writes.load(Ordering::SeqCst), writes);
        let fresh = Bundled::new(MemoryStore::default());
        *fresh.inner.items.lock().unwrap() = store.inner.items.lock().unwrap().clone();
        assert!(internalize(&mut on_disk, &fresh, &accounts).is_empty());
        assert_eq!(on_disk, original);
        assert_eq!(fresh.inner.reads.load(Ordering::SeqCst), 1);

        let mut s = original.clone();
        s.providers[0].extra_headers.clear();
        let now = externalize(&mut s, &store).unwrap();
        remove_stale(&store, &accounts, &now);
        assert_eq!(store.get("provider:p1:header:X-Org").unwrap(), None);
        assert_eq!(
            store.get("provider:p1:api_key").unwrap().as_deref(),
            Some("sk-1")
        );
    }

    #[test]
    fn moves_separate_items_into_the_bundle() {
        let old = MemoryStore::default();
        let mut on_disk = sample();
        let accounts = externalize(&mut on_disk, &old).unwrap();
        let store = Bundled::new(old);
        assert!(internalize(&mut on_disk, &store, &accounts).is_empty());
        assert_eq!(on_disk, sample());
        let items: Vec<String> = store.inner.items.lock().unwrap().keys().cloned().collect();
        assert_eq!(items, [BUNDLE], "old items are removed once moved");
    }

    #[test]
    fn bundle_failures_change_nothing() {
        let store = Bundled::new(MemoryStore {
            fail: true,
            ..Default::default()
        });
        assert!(store.set("a", "1").is_err());
        assert_eq!(store.get("a").unwrap(), None, "a failed write isn't kept");

        let unreadable = Bundled::new(MemoryStore::default());
        unreadable
            .inner
            .items
            .lock()
            .unwrap()
            .insert(BUNDLE.into(), "not json".into());
        assert!(unreadable.get("a").is_err());
        assert!(
            unreadable.set("a", "1").is_err(),
            "never overwrites what it can't read"
        );
        assert_eq!(unreadable.inner.reads.load(Ordering::SeqCst), 1);
    }
}
