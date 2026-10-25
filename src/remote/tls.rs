//! TLS for the remote-access server: a cached self-signed certificate, or PEM files chosen
//! by the user, turned into a `rustls::ServerConfig` with the ring provider.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{CertificateParams, DnType, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::config::{RemoteSettings, RemoteTls, config_dir, write_private};

/// Where the self-signed certificate and key are cached: `<config>/remote/`.
pub fn self_signed_dir() -> PathBuf {
    config_dir().join("remote")
}

/// The TLS config for these settings: `None` for plain HTTP, otherwise a config for
/// `tls == Files` or `tls == SelfSigned`. Errors name the file that failed.
pub fn server_config(cfg: &RemoteSettings) -> Result<Option<Arc<rustls::ServerConfig>>, String> {
    let hosts = self_signed_hosts();
    server_config_in(cfg, &self_signed_dir(), &hosts)
}

/// `server_config` with the self-signed cache folder and host names given.
fn server_config_in(
    cfg: &RemoteSettings,
    dir: &Path,
    hosts: &[String],
) -> Result<Option<Arc<rustls::ServerConfig>>, String> {
    let (chain, key) = match cfg.tls {
        RemoteTls::Off => return Ok(None),
        RemoteTls::Files => load_files(Path::new(&cfg.cert_path), Path::new(&cfg.key_path))?,
        RemoteTls::SelfSigned => self_signed_in(dir, hosts)?,
    };
    build(chain, key).map(|config| Some(Arc::new(config)))
}

/// Names the self-signed certificate is issued for.
fn self_signed_hosts() -> Vec<String> {
    let mut hosts = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(lan) = super::lan_ipv4() {
        hosts.push(lan.to_string());
    }
    hosts
}

/// Loads the certificate chain and key from `dir`, creating them first when missing.
/// The certificate is cached: a later call returns the same certificate, as long as it was
/// issued for every host now asked for. `hosts.txt` next to it lists the names it was issued for.
fn self_signed_in(
    dir: &Path,
    hosts: &[String],
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), String> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let hosts_path = dir.join("hosts.txt");
    if cert_path.is_file()
        && key_path.is_file()
        && cached_hosts_cover(&hosts_path, hosts)
        && let Ok(loaded) = load_files(&cert_path, &key_path)
    {
        return Ok(loaded);
    }

    let key_pair = KeyPair::generate().map_err(|e| format!("Can't create a key: {e}"))?;
    let mut params = CertificateParams::new(hosts.to_vec())
        .map_err(|e| format!("Can't create the certificate: {e}"))?;
    params
        .distinguished_name
        .push(DnType::CommonName, "CleanRSTAI");
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| format!("Can't create the certificate: {e}"))?;

    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Can't create the folder {}: {e}", dir.display()))?;
    std::fs::write(&cert_path, cert.pem()).map_err(|e| {
        format!(
            "Can't write the certificate file {}: {e}",
            cert_path.display()
        )
    })?;
    write_private(&key_path, key_pair.serialize_pem().as_bytes())
        .map_err(|e| format!("Can't write the key file {}: {e}", key_path.display()))?;
    std::fs::write(&hosts_path, hosts.join("\n"))
        .map_err(|e| format!("Can't write the file {}: {e}", hosts_path.display()))?;

    load_files(&cert_path, &key_path)
}

/// True when `hosts_path` lists every one of `hosts`. A missing or unreadable list means no.
fn cached_hosts_cover(hosts_path: &Path, hosts: &[String]) -> bool {
    let Ok(text) = std::fs::read_to_string(hosts_path) else {
        return false;
    };
    let cached: Vec<&str> = text.lines().map(str::trim).collect();
    hosts.iter().all(|host| cached.contains(&host.as_str()))
}

/// Reads a PEM certificate chain and its PEM private key.
fn load_files(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), String> {
    let cert_pem = std::fs::read(cert_path).map_err(|e| {
        format!(
            "Can't read the certificate file {}: {e}",
            cert_path.display()
        )
    })?;
    let chain = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            format!(
                "Can't read the certificate file {}: {e}",
                cert_path.display()
            )
        })?;
    if chain.is_empty() {
        return Err(format!("No certificate found in {}", cert_path.display()));
    }

    let key_pem = std::fs::read(key_path)
        .map_err(|e| format!("Can't read the key file {}: {e}", key_path.display()))?;
    let key = match PrivateKeyDer::from_pem_slice(&key_pem) {
        Ok(key) => key,
        Err(rustls::pki_types::pem::Error::NoItemsFound) => {
            return Err(format!("No private key found in {}", key_path.display()));
        }
        Err(e) => {
            return Err(format!(
                "Can't read the key file {}: {e}",
                key_path.display()
            ));
        }
    };
    Ok((chain, key))
}

fn build(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<rustls::ServerConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("Can't set up TLS: {e}"))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| format!("The certificate and the key don't match: {e}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("cleanrstai-test-{}", Uuid::new_v4()))
    }

    fn hosts() -> Vec<String> {
        vec!["localhost".into(), "127.0.0.1".into()]
    }

    #[test]
    fn self_signed_creates_then_reuses_files() {
        let dir = temp_dir();
        let (chain, _key) = self_signed_in(&dir, &hosts()).unwrap();
        assert_eq!(chain.len(), 1);
        let cert_bytes = std::fs::read(dir.join("cert.pem")).unwrap();
        let key_bytes = std::fs::read(dir.join("key.pem")).unwrap();
        assert!(String::from_utf8_lossy(&key_bytes).contains("PRIVATE KEY"));

        let (again, _) = self_signed_in(&dir, &hosts()).unwrap();
        assert_eq!(again[0].as_ref(), chain[0].as_ref());
        assert_eq!(std::fs::read(dir.join("cert.pem")).unwrap(), cert_bytes);
        assert_eq!(std::fs::read(dir.join("key.pem")).unwrap(), key_bytes);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn self_signed_key_is_private_on_unix() {
        let dir = temp_dir();
        self_signed_in(&dir, &hosts()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("key.pem"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn self_signed_is_reissued_when_a_host_is_missing() {
        let dir = temp_dir();
        let two = hosts();
        let three: Vec<String> = ["localhost", "127.0.0.1", "192.168.1.50"]
            .map(String::from)
            .to_vec();
        let (first, _) = self_signed_in(&dir, &two).unwrap();
        let first_bytes = std::fs::read(dir.join("cert.pem")).unwrap();

        // The LAN address is new: the certificate is issued again.
        let (second, _) = self_signed_in(&dir, &three).unwrap();
        assert_ne!(second[0].as_ref(), first[0].as_ref());
        let second_bytes = std::fs::read(dir.join("cert.pem")).unwrap();
        assert_ne!(second_bytes, first_bytes);

        // Every host is listed now: the cached certificate is kept.
        let (third, _) = self_signed_in(&dir, &three).unwrap();
        assert_eq!(third[0].as_ref(), second[0].as_ref());
        assert_eq!(std::fs::read(dir.join("cert.pem")).unwrap(), second_bytes);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn self_signed_without_the_host_list_is_reissued() {
        let dir = temp_dir();
        let (first, _) = self_signed_in(&dir, &hosts()).unwrap();
        std::fs::remove_file(dir.join("hosts.txt")).unwrap();
        let (second, _) = self_signed_in(&dir, &hosts()).unwrap();
        assert_ne!(second[0].as_ref(), first[0].as_ref());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn off_has_no_tls_config() {
        let cfg = RemoteSettings {
            tls: RemoteTls::Off,
            ..RemoteSettings::default()
        };
        assert!(server_config(&cfg).unwrap().is_none());
    }

    #[test]
    fn files_with_missing_paths_name_the_file() {
        let cfg = RemoteSettings {
            tls: RemoteTls::Files,
            cert_path: "/nonexistent-cleanrstai/cert.pem".into(),
            key_path: "/nonexistent-cleanrstai/key.pem".into(),
            ..RemoteSettings::default()
        };
        let err = server_config_in(&cfg, &temp_dir(), &hosts()).err().unwrap();
        assert!(err.contains("/nonexistent-cleanrstai/cert.pem"), "{err}");
    }

    #[test]
    fn files_without_a_key_say_so() {
        let dir = temp_dir();
        self_signed_in(&dir, &hosts()).unwrap();
        // A key file with no key in it.
        let key_path = dir.join("bad-key.pem");
        std::fs::write(&key_path, "not a key\n").unwrap();
        let cfg = RemoteSettings {
            tls: RemoteTls::Files,
            cert_path: dir.join("cert.pem").display().to_string(),
            key_path: key_path.display().to_string(),
            ..RemoteSettings::default()
        };
        let err = server_config_in(&cfg, &dir, &hosts()).err().unwrap();
        assert!(err.contains("No private key found in"), "{err}");
        assert!(err.contains("bad-key.pem"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_from_disk_build_a_config() {
        let dir = temp_dir();
        self_signed_in(&dir, &hosts()).unwrap();
        let cfg = RemoteSettings {
            tls: RemoteTls::Files,
            cert_path: dir.join("cert.pem").display().to_string(),
            key_path: dir.join("key.pem").display().to_string(),
            ..RemoteSettings::default()
        };
        assert!(
            server_config_in(&cfg, &temp_dir(), &hosts())
                .unwrap()
                .is_some()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn self_signed_config_builds() {
        let dir = temp_dir();
        let cfg = RemoteSettings {
            tls: RemoteTls::SelfSigned,
            ..RemoteSettings::default()
        };
        let config = server_config_in(&cfg, &dir, &hosts()).unwrap().unwrap();
        assert_eq!(config.alpn_protocols, vec![b"http/1.1".to_vec()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
