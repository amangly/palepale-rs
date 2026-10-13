//! MCP OAuth and its loopback callback stay within the native I/O layer.
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, bail};
use async_trait::async_trait;
use base64::Engine;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession,
    CredentialRefreshGuard, CredentialStore, StoredCredentials,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::extensions::text;
use crate::mcp::Config;
use crate::{io, limits};

#[derive(Clone)]
pub struct Store {
    path: PathBuf,
    shared: Arc<StoreState>,
    generation: u64,
}

#[derive(Default)]
struct StoreState {
    generation: AtomicU64,
    writes: std::sync::Mutex<()>,
    refresh: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Serialize, Deserialize)]
struct Sealed {
    scheme: String,
    bytes: String,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        static STORES: std::sync::LazyLock<
            std::sync::Mutex<std::collections::BTreeMap<PathBuf, std::sync::Weak<StoreState>>>,
        > = std::sync::LazyLock::new(Default::default);
        let mut stores = STORES.lock().unwrap();
        stores.retain(|_, state| state.strong_count() > 0);
        let shared = stores
            .get(&path)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| {
                let state = Arc::new(StoreState::default());
                stores.insert(path.clone(), Arc::downgrade(&state));
                state
            });
        let generation = shared.generation.load(Ordering::Acquire);
        Self {
            path,
            shared,
            generation,
        }
    }

    pub fn read(&self) -> anyhow::Result<Option<StoredCredentials>> {
        let _write = self.shared.writes.lock().unwrap();
        if self.generation != self.shared.generation.load(Ordering::Acquire) {
            return Ok(None);
        }
        let Some(source) =
            io::fs::read_limited(&self.path, limits::get().extensions.skill_file_bytes.value)?
        else {
            return Ok(None);
        };
        let sealed: Sealed =
            serde_json::from_str(&source).context("Invalid MCP credential store")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(sealed.bytes)?;
        let plain = io::secret::open(&sealed.scheme, &bytes)?;
        Ok(Some(serde_json::from_slice(&plain)?))
    }

    pub fn remove(&self) -> anyhow::Result<()> {
        let _write = self.shared.writes.lock().unwrap();
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        io::fs::remove(&self.path)?;
        Ok(())
    }

    pub async fn logout(&self) -> anyhow::Result<()> {
        let _refresh = self.acquire_refresh_guard().await?;
        self.remove()
    }
}

fn storage_error(e: impl std::fmt::Display) -> AuthError {
    AuthError::CredentialStoreError(e.to_string())
}

#[async_trait]
impl CredentialStore for Store {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        self.read().map_err(storage_error)
    }
    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let plain = serde_json::to_vec(&credentials).map_err(storage_error)?;
        let (scheme, bytes) = io::secret::seal(&plain).map_err(storage_error)?;
        let sealed = Sealed {
            scheme: scheme.to_owned(),
            bytes: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        let bytes = serde_json::to_vec(&sealed).map_err(storage_error)?;
        let _write = self.shared.writes.lock().unwrap();
        if self.generation != self.shared.generation.load(Ordering::Acquire) {
            return Err(AuthError::CredentialStoreError(
                "MCP credentials changed during authentication".into(),
            ));
        }
        io::fs::write_atomic(&self.path, &bytes, io::fs::Access::Private).map_err(storage_error)
    }
    async fn clear(&self) -> Result<(), AuthError> {
        self.remove().map_err(storage_error)
    }
    async fn on_refresh_token_rejected(&self, _: &StoredCredentials) -> Result<(), AuthError> {
        self.clear().await
    }
    async fn acquire_refresh_guard(&self) -> Result<Option<CredentialRefreshGuard>, AuthError> {
        let guard = self.shared.refresh.clone().lock_owned().await;
        let lock_path = self.path.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            io::fs::make_dir(parent).map_err(storage_error)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(lock_path).map_err(storage_error)?;
        let acquire = async {
            loop {
                match file.try_lock() {
                    Ok(()) => return Ok::<_, AuthError>(()),
                    Err(std::fs::TryLockError::WouldBlock) => {
                        tokio::time::sleep(crate::limits::get().extensions.auth_lock_poll_ms.ms())
                            .await
                    }
                    Err(error) => return Err(storage_error(error)),
                }
            }
        };
        tokio::time::timeout(
            crate::limits::get()
                .extensions
                .auth_lock_timeout_secs
                .secs(),
            acquire,
        )
        .await
        .map_err(|_| storage_error("MCP credential refresh is busy"))??;
        Ok(Some(CredentialRefreshGuard::new((guard, file))))
    }
}

pub async fn manager(config: &Config, path: PathBuf) -> anyhow::Result<AuthorizationManager> {
    let mut manager = AuthorizationManager::new(
        config
            .url
            .as_deref()
            .context("OAuth requires a remote URL")?,
    )
    .await?;
    manager.set_credential_store(Store::new(path));
    Ok(manager)
}

pub struct Login {
    session: AuthorizationSession,
    listener: TcpListener,
    authority: String,
}

impl Login {
    pub fn url(&self) -> &str {
        self.session.get_authorization_url()
    }

    pub async fn begin(
        config: &Config,
        path: PathBuf,
        challenge: Option<&str>,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let authority = listener.local_addr()?.to_string();
        let redirect = format!("http://{authority}/callback");
        let mut manager = manager(config, path).await?;
        let resolution = if let Some(challenge) = challenge {
            manager
                .resolve_metadata_from_challenge(Some(challenge))
                .await?
        } else {
            manager.resolve_metadata().await?
        };
        manager.set_metadata(resolution.metadata);
        let mut request = AuthorizationRequest::new(redirect)
            .with_client_name("Palepale")
            .with_scopes(config.scopes.clone());
        if let Some(client_id) = &config.client_id {
            request = request.with_preregistered_client(client_id);
        }
        if let Some(challenge) = challenge {
            request = request.with_challenge(challenge);
        }
        let session = AuthorizationSession::new(manager, request)
            .await
            .map_err(|(_, error)| anyhow::Error::from(error))?;
        Ok(Self {
            session,
            listener,
            authority,
        })
    }

    pub async fn finish(self, cancel: &CancellationToken) -> anyhow::Result<()> {
        let wait = async {
            loop {
                let (mut stream, _) = self.listener.accept().await?;
                let mut bytes = Vec::new();
                let read = async {
                    let mut chunk = [0u8; 1024];
                    while bytes.len() <= 8192 {
                        let n = stream.read(&mut chunk).await?;
                        if n == 0 {
                            break;
                        }
                        bytes.extend_from_slice(&chunk[..n]);
                        if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Ok::<_, std::io::Error>(())
                };
                if tokio::time::timeout(std::time::Duration::from_secs(5), read)
                    .await
                    .is_err()
                {
                    continue;
                }
                let callback = callback_url(&bytes, &self.authority);
                let accepted = match callback {
                    Ok(url) => self.session.handle_callback_url(&url).await.is_ok(),
                    Err(_) => false,
                };
                let (status, body) = if accepted {
                    ("200 OK", text().callback_ok.as_str())
                } else {
                    ("400 Bad Request", text().callback_bad.as_str())
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    stream.write_all(response.as_bytes()),
                )
                .await;
                if accepted {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        };
        tokio::select! {
            result = tokio::time::timeout(limits::get().extensions.oauth_timeout_secs.secs(), wait) => result.context("MCP sign-in timed out")?,
            () = cancel.cancelled() => bail!("MCP sign-in cancelled"),
        }
    }
}

fn callback_url(bytes: &[u8], authority: &str) -> anyhow::Result<String> {
    if bytes.len() > 8192 {
        bail!("Callback too large");
    }
    let request = std::str::from_utf8(bytes)?;
    let mut lines = request.split("\r\n");
    let first: Vec<_> = lines.next().unwrap_or_default().split(' ').collect();
    if first.len() != 3 || first[0] != "GET" || !first[2].starts_with("HTTP/1.") {
        bail!("Invalid callback request");
    }
    let mut host = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (key, value) = line.split_once(':').context("Invalid callback header")?;
        if key.eq_ignore_ascii_case("host") && host.replace(value.trim()).is_some() {
            bail!("Duplicate Host");
        }
        if key.eq_ignore_ascii_case("origin") && value.trim() != format!("http://{authority}") {
            bail!("Foreign Origin");
        }
    }
    if host != Some(authority) || !first[1].starts_with("/callback?") {
        bail!("Wrong callback destination");
    }
    let url = format!("http://{authority}{}", first[1]);
    let parsed = url::Url::parse(&url)?;
    let mut keys = std::collections::HashSet::new();
    for (key, _) in parsed.query_pairs() {
        if !keys.insert(key.into_owned()) {
            bail!("Duplicate callback parameter");
        }
    }
    if !keys.contains("state") || !keys.contains("code") {
        bail!("Incomplete callback");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_rejects_foreign_hosts_duplicate_parameters_and_wrong_routes() {
        let good = b"GET /callback?code=c&state=s HTTP/1.1\r\nHost: 127.0.0.1:4321\r\n\r\n";
        assert!(callback_url(good, "127.0.0.1:4321").is_ok());
        for request in [
            "GET /callback?code=c&state=s HTTP/1.1\r\nHost: evil.test\r\n\r\n",
            "GET /callback?code=c&state=s&state=t HTTP/1.1\r\nHost: 127.0.0.1:4321\r\n\r\n",
            "GET /other?code=c&state=s HTTP/1.1\r\nHost: 127.0.0.1:4321\r\n\r\n",
            "GET /callback?code=c&state=s HTTP/1.1\r\nHost: 127.0.0.1:4321\r\nOrigin: https://evil.test\r\n\r\n",
        ] {
            assert!(callback_url(request.as_bytes(), "127.0.0.1:4321").is_err());
        }
    }

    #[tokio::test]
    async fn credentials_roundtrip_and_logout_remove_private_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("auth.json"));
        store
            .save(StoredCredentials::new("client".into(), None, vec![], None))
            .await
            .unwrap();
        assert_eq!(store.load().await.unwrap().unwrap().client_id, "client");
        store.clear().await.unwrap();
        assert!(store.load().await.unwrap().is_none());
        assert!(
            store
                .save(StoredCredentials::new(
                    "stale-client".into(),
                    None,
                    vec![],
                    None
                ))
                .await
                .is_err()
        );
        assert!(
            !store.path.exists(),
            "a stale refresh must not recreate logged-out credentials"
        );
    }
}
