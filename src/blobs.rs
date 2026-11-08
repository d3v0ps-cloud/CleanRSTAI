//! Chat storage helpers: image attachments kept beside the chats by content, and a
//! background thread that writes chat files.
//!
//! A chat file holds an image as `"blob": "<sha-256>"` instead of its base64 text; the bytes
//! live once in `attachments/<sha-256>.bin` in the data dir, shared by every chat (and fork)
//! that has them. Loading a chat leaves them on disk; [`crate::attach::Attachment`] reads one
//! when it is shown or sent. Older files with the base64 inline still load, and are moved
//! over at their next save.
//!
//! Saving a chat encodes and writes it on the saver thread, so the window doesn't wait on the
//! disk. Saves of the same chat queued together are written once (the newest), and a delete
//! queued after a save wins over it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::types::Conversation;

/// Where the image files live.
pub fn dir() -> PathBuf {
    #[cfg(test)]
    {
        test_dir().to_path_buf()
    }
    #[cfg(not(test))]
    {
        crate::config::data_dir().join("attachments")
    }
}

/// One folder for every test, so tests running at once agree on it.
#[cfg(test)]
pub fn test_dir() -> &'static Path {
    static DIR: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        std::env::temp_dir().join(format!("cleanrstai-blobs-{}", Uuid::new_v4()))
    });
    &DIR
}

thread_local! {
    /// Set while a chat is being encoded for its file: images go to this folder.
    static WRITING: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Run `f` (which serialises chats) with images written to `dir` and referenced by key.
pub fn externalizing<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            WRITING.with(|w| *w.borrow_mut() = None);
        }
    }
    WRITING.with(|w| *w.borrow_mut() = Some(dir.to_path_buf()));
    let _reset = Reset;
    f()
}

/// The folder images go to, while [`externalizing`].
pub(crate) fn writing_dir() -> Option<PathBuf> {
    WRITING.with(|w| w.borrow().clone())
}

/// The key of an image: the SHA-256 of its base64 text, in hex.
pub fn key(base64: &str) -> String {
    let digest = Sha256::digest(base64.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A key as [`key`] makes it, so a file name built from one stays inside the folder.
pub fn valid_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

fn path_in(dir: &Path, key: &str) -> Option<PathBuf> {
    valid_key(key).then(|| dir.join(format!("{key}.bin")))
}

/// Keep the image `base64` under `key` in `dir`; returns its size in bytes. An image already
/// there is not written again, only marked as recently used (see [`collect_garbage`]).
pub fn store(dir: &Path, key: &str, base64: &str) -> Result<u64> {
    let path = path_in(dir, key).context("bad attachment key")?;
    if let Ok(meta) = std::fs::metadata(&path) {
        if let Ok(file) = std::fs::File::options().append(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        return Ok(meta.len());
    }
    let bytes = B64.decode(base64).context("attachment isn't base64")?;
    crate::config::write_private(&path, &bytes)?;
    Ok(bytes.len() as u64)
}

/// The bytes of a kept image.
pub fn load(key: &str) -> Option<Vec<u8>> {
    std::fs::read(path_in(&dir(), key)?)
        .inspect_err(|e| log::warn!("attachment {key} couldn't be read: {e}"))
        .ok()
}

/// The `"blob": "<key>"` references in a chat file's text.
fn references(text: &str, into: &mut HashSet<String>) {
    let mut rest = text;
    while let Some(at) = rest.find("\"blob\"") {
        rest = &rest[at + 6..];
        let value = rest
            .trim_start()
            .strip_prefix(':')
            .map(str::trim_start)
            .and_then(|v| v.strip_prefix('"'));
        if let Some(v) = value
            && let Some(k) = v.get(..64)
            && valid_key(k)
        {
            into.insert(k.to_owned());
        }
    }
}

/// Remove images in `blobs` that no chat file in `chats` refers to and that weren't written
/// or used in the last `min_age` (so one being saved right now stays). Returns how many went.
pub fn collect_garbage(blobs: &Path, chats: &Path, min_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(blobs) else {
        return 0;
    };
    let mut used = HashSet::new();
    for chat in std::fs::read_dir(chats).into_iter().flatten().flatten() {
        if chat.path().extension().is_some_and(|x| x == "json")
            && let Ok(text) = std::fs::read_to_string(chat.path())
        {
            references(&text, &mut used);
        }
    }
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(key) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".bin"))
        else {
            continue;
        };
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= min_age);
        if valid_key(key) && old && !used.contains(key) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Encode a chat for its file, with its images kept in `blobs`.
pub fn encode(conv: &Conversation, blobs: &Path) -> Result<Vec<u8>> {
    externalizing(blobs, || serde_json::to_vec_pretty(conv)).map_err(anyhow::Error::from)
}

/// Write a chat's file now, on this thread.
pub fn write(conv: &Conversation, path: &Path, blobs: &Path) -> Result<()> {
    crate::config::write_private(path, &encode(conv, blobs)?)
}

enum Job {
    Save(Box<Conversation>),
    Delete(Uuid),
    Flush(mpsc::Sender<()>),
}

fn saver() -> &'static mpsc::Sender<Job> {
    static SAVER: std::sync::LazyLock<mpsc::Sender<Job>> = std::sync::LazyLock::new(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("chat-saver".into())
            .spawn(move || run_saver(rx))
            .expect("couldn't start the chat saver");
        tx
    });
    &SAVER
}

fn run_saver(rx: mpsc::Receiver<Job>) {
    while let Ok(first) = rx.recv() {
        // Everything queued meanwhile, so a chat saved many times is written once.
        let mut pending: HashMap<Uuid, Option<Box<Conversation>>> = HashMap::new();
        let mut flushes = Vec::new();
        for job in std::iter::once(first).chain(rx.try_iter()) {
            match job {
                Job::Save(conv) => {
                    pending.insert(conv.id, Some(conv));
                }
                Job::Delete(id) => {
                    pending.insert(id, None);
                }
                Job::Flush(done) => flushes.push(done),
            }
        }
        let blobs = dir();
        for (id, job) in pending {
            let path = Conversation::path(id);
            match job {
                Some(conv) => {
                    if let Err(e) = write(&conv, &path, &blobs) {
                        log::error!("failed to save conversation {id}: {e:#}");
                    }
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        for done in flushes {
            let _ = done.send(());
        }
    }
}

/// Queue a chat to be written.
pub fn save_later(conv: Conversation) {
    let _ = saver().send(Job::Save(Box::new(conv)));
}

/// Queue a chat's file to be removed (after any save queued before).
pub fn delete_later(id: Uuid) {
    let _ = saver().send(Job::Delete(id));
}

/// Wait until everything queued so far is on disk.
pub fn flush() {
    let (tx, rx) = mpsc::channel();
    if saver().send(Job::Flush(tx)).is_ok() {
        let _ = rx.recv_timeout(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cleanrstai-{name}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keys_are_checked_before_they_become_paths() {
        let k = key("aGVsbG8=");
        assert!(valid_key(&k));
        assert!(!valid_key("../../etc/passwd"));
        assert!(path_in(Path::new("/x"), "../a").is_none());
    }

    #[test]
    fn stores_once_and_finds_references() {
        let blobs = temp("blobs");
        let k = key("aGVsbG8=");
        assert_eq!(store(&blobs, &k, "aGVsbG8=").unwrap(), 5);
        assert_eq!(store(&blobs, &k, "aGVsbG8=").unwrap(), 5);
        assert_eq!(
            std::fs::read(blobs.join(format!("{k}.bin"))).unwrap(),
            b"hello"
        );

        let mut found = HashSet::new();
        references(
            &format!("{{\"a\": {{\"blob\": \"{k}\"}}, \"blob\":\"short\"}}"),
            &mut found,
        );
        assert_eq!(found, HashSet::from([k]));
    }

    #[test]
    fn garbage_collection_keeps_used_and_recent_images() {
        let (blobs, chats) = (temp("blobs"), temp("chats"));
        let (used, unused) = (key("dXNlZA=="), key("dW51c2Vk"));
        store(&blobs, &used, "dXNlZA==").unwrap();
        store(&blobs, &unused, "dW51c2Vk").unwrap();
        std::fs::write(chats.join("c.json"), format!("{{\"blob\": \"{used}\"}}")).unwrap();
        // Both are new, so nothing goes yet.
        assert_eq!(
            collect_garbage(&blobs, &chats, Duration::from_secs(3600)),
            0
        );
        assert_eq!(collect_garbage(&blobs, &chats, Duration::ZERO), 1);
        assert!(blobs.join(format!("{used}.bin")).exists());
        assert!(!blobs.join(format!("{unused}.bin")).exists());
    }
}
