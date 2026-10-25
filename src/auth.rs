//! The palepale key on this machine: `PALEPALE_API_KEY`, else `~/.palepale/auth.json`.

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::LazyLock};

use crate::api::account::{self, Me};
use crate::app::App;
use crate::io::{self, fs::Access};
use crate::paths::Paths;
use crate::settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    /// An owner credential, or the legacy account-number tag in older files.
    Account,
    /// A per-machine key from pairing, revocable without touching the account.
    Device,
}

/// No `Debug`: it holds the secret.
pub struct Key {
    pub secret: String,
    pub origin: Origin,
}

#[derive(Debug)]
pub enum Origin {
    Env,
    File { kind: KeyKind, hint: String },
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    api_origin: Option<String>,
    #[serde(default = "version")]
    v: u32,
    kind: KeyKind,
    hint: String,
    created: Timestamp,
    scheme: String,
    secret: String,
}

fn version() -> u32 {
    1
}

/// The key this process uses: the environment's, else the stored one.
pub fn load(paths: &Paths) -> anyhow::Result<Option<Key>> {
    let key = match from_env() {
        Some(key) => Some(key),
        None => stored(paths)?,
    };
    if let Some(key) = &key {
        require_inference(&key.secret)?;
    }
    Ok(key)
}

/// The variable a key in the environment comes from.
pub const ENV_VAR: &str = "PALEPALE_API_KEY";

/// `PALEPALE_API_KEY`, which wins over a stored key while it is set.
pub fn from_env() -> Option<Key> {
    settings::env(ENV_VAR).map(|secret| Key {
        secret: normalize(&secret),
        origin: Origin::Env,
    })
}

/// The key saved on this machine, whatever the environment says.
pub fn stored(paths: &Paths) -> anyhow::Result<Option<Key>> {
    stored_at(&paths.auth())
}

fn stored_at(path: &Path) -> anyhow::Result<Option<Key>> {
    let Some(text) = io::fs::read_string(path)? else {
        return Ok(None);
    };
    let unreadable = || {
        if path
            .file_name()
            .is_some_and(|name| name == OWNER.file.as_str())
        {
            format!(
                "{} is unreadable; restore its protected backup or supply PALEPALE_OWNER_CREDENTIAL",
                path.display()
            )
        } else {
            format!(
                "{} is unreadable; `palepale logout` removes it",
                path.display()
            )
        }
    };
    let stored: Stored = serde_json::from_str(&text).with_context(unreadable)?;
    let origin = crate::api::endpoints::origin("api");
    anyhow::ensure!(
        stored.api_origin.as_deref() == Some(origin.as_str()),
        "The stored key belongs to another or older backend configuration; run `palepale login` to reconnect."
    );
    let sealed = BASE64.decode(&stored.secret).with_context(unreadable)?;
    let opened = io::secret::open(&stored.scheme, &sealed).with_context(unreadable)?;
    let secret = String::from_utf8(opened).with_context(unreadable)?;
    Ok(Some(Key {
        secret,
        origin: Origin::File {
            kind: stored.kind,
            hint: stored.hint,
        },
    }))
}

pub fn save(paths: &Paths, secret: &str, kind: KeyKind) -> anyhow::Result<()> {
    save_at(&paths.auth(), secret, kind)
}

fn save_at(path: &Path, secret: &str, kind: KeyKind) -> anyhow::Result<()> {
    let secret = normalize(secret);
    let (scheme, sealed) = io::secret::seal(secret.as_bytes())?;
    let stored = Stored {
        api_origin: Some(crate::api::endpoints::origin("api")),
        v: version(),
        kind,
        hint: hint(&secret),
        created: io::clock::now(),
        scheme: scheme.to_owned(),
        secret: BASE64.encode(sealed),
    };
    io::fs::write_atomic(path, &serde_json::to_vec_pretty(&stored)?, Access::Private)?;
    Ok(())
}

/// Checks `secret` with palepale, stores it, and makes it the key this process uses from now on.
pub async fn adopt(app: &App, secret: &str, kind: KeyKind) -> anyhow::Result<Me> {
    let secret = normalize(secret);
    require_inference(&secret)?;
    let me = account::me(&app.api.with_key(secret.clone()))
        .await
        .context("checking that key with palepale")?;
    save(&app.paths, &secret, kind)?;
    app.api.set_key(Some(secret));
    Ok(me)
}

#[derive(Deserialize)]
struct OwnerConfig {
    env: String,
    file: String,
    required: String,
    invalid: String,
    inference_only: String,
    lock: String,
    busy: String,
}
static OWNER: LazyLock<OwnerConfig> =
    LazyLock::new(|| toml::from_str(include_str!("../data/owner.toml")).expect("owner settings"));

pub fn is_owner(secret: &str) -> bool {
    token_shape(secret, "po_")
}
pub fn is_inference(secret: &str) -> bool {
    token_shape(secret, "pk_")
}
fn token_shape(secret: &str, prefix: &str) -> bool {
    secret.len() == 46
        && secret.starts_with(prefix)
        && secret[3..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub fn owner_path(paths: &Paths) -> std::path::PathBuf {
    paths.home.join(&OWNER.file)
}
pub fn owner_stored(paths: &Paths) -> anyhow::Result<bool> {
    Ok(io::fs::read_string(&owner_path(paths))?.is_some())
}
pub fn owner_lock(paths: &Paths) -> anyhow::Result<io::fs::FileLock> {
    io::fs::lock(&paths.home.join(&OWNER.lock)).context(OWNER.busy.clone())
}
fn require_inference(secret: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !secret.starts_with("po_") && !secret.starts_with("ps_"),
        OWNER.inference_only.clone()
    );
    Ok(())
}
pub fn save_owner(paths: &Paths, secret: &str) -> anyhow::Result<()> {
    anyhow::ensure!(is_owner(secret), OWNER.invalid.clone());
    save_at(&owner_path(paths), secret, KeyKind::Account)
}
pub fn owner_api(app: &App) -> anyhow::Result<crate::api::Api> {
    Ok(app.api.with_key(owner_secret(app)?))
}
pub fn export_owner(app: &App, destination: &Path) -> anyhow::Result<()> {
    let secret = owner_secret(app)?;
    Ok(io::fs::write_new(
        destination,
        secret.as_bytes(),
        Access::Private,
    )?)
}
fn owner_secret(app: &App) -> anyhow::Result<String> {
    let secret = match settings::env(&OWNER.env) {
        Some(secret) => normalize(&secret),
        None => {
            stored_at(&owner_path(&app.paths))?
                .context(OWNER.required.clone())?
                .secret
        }
    };
    anyhow::ensure!(is_owner(&secret), OWNER.invalid.clone());
    Ok(secret)
}

/// `Ok(false)` when there was no stored key.
pub fn remove(paths: &Paths) -> anyhow::Result<bool> {
    Ok(io::fs::remove(&paths.auth())?)
}

/// How many trailing characters of a key are ever shown.
pub const HINT_CHARS: usize = 4;

/// The last few characters, enough to tell keys apart without revealing them.
pub fn hint(secret: &str) -> String {
    let tail: String = secret
        .chars()
        .rev()
        .take(HINT_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

/// Account numbers are shown grouped with spaces; people paste them that way.
pub fn normalize(secret: &str) -> String {
    secret.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_storage_is_separate_and_signup_lock_is_exclusive() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        let owner = format!("po_{}", "A".repeat(43));
        let client = format!("pk_{}", "B".repeat(43));
        assert!(save_owner(&paths, &client).is_err());
        let lock = owner_lock(&paths).unwrap();
        assert!(owner_lock(&paths).is_err());
        save_owner(&paths, &owner).unwrap();
        save(&paths, &client, KeyKind::Device).unwrap();
        assert_eq!(stored(&paths).unwrap().unwrap().secret, client);
        assert_eq!(
            stored_at(&owner_path(&paths)).unwrap().unwrap().secret,
            owner
        );
        assert!(require_inference(&owner).is_err());
        assert!(require_inference(&format!("ps_{}", "C".repeat(43))).is_err());
        remove(&paths).unwrap();
        assert!(owner_stored(&paths).unwrap());
        drop(lock);
        assert!(owner_lock(&paths).is_ok());
        let mut record: serde_json::Value =
            serde_json::from_str(&io::fs::read_string(&owner_path(&paths)).unwrap().unwrap())
                .unwrap();
        record["api_origin"] = "https://other.invalid".into();
        io::fs::write_atomic(
            &owner_path(&paths),
            record.to_string().as_bytes(),
            Access::Private,
        )
        .unwrap();
        assert!(stored_at(&owner_path(&paths)).is_err());
    }

    #[test]
    fn save_load_remove_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        save(&paths, "4192 0837 5561 2094", KeyKind::Account).unwrap();
        let text = io::fs::read_string(&paths.auth()).unwrap().unwrap();
        assert!(!text.contains("4192083755612094") || cfg!(not(windows)));
        // `stored`, not `load`: a developer's exported PALEPALE_API_KEY must not win here.
        let key = stored(&paths).unwrap().unwrap();
        assert_eq!(key.secret, "4192083755612094");
        assert!(
            matches!(key.origin, Origin::File { kind: KeyKind::Account, ref hint } if hint == "…2094")
        );
        assert!(remove(&paths).unwrap());
        assert!(stored(&paths).unwrap().is_none());
    }

    #[test]
    fn a_key_from_another_or_unbound_backend_is_never_loaded() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        save(&paths, "synthetic-key", KeyKind::Device).unwrap();
        let good: serde_json::Value =
            serde_json::from_str(&io::fs::read_string(&paths.auth()).unwrap().unwrap()).unwrap();
        for origin in [
            serde_json::json!("https://foreign.invalid"),
            serde_json::Value::Null,
        ] {
            let mut stored = good.clone();
            stored["api_origin"] = origin;
            io::fs::write_atomic(
                &paths.auth(),
                stored.to_string().as_bytes(),
                Access::Private,
            )
            .unwrap();
            let error = stored_error(&paths);
            assert!(error.contains("backend configuration") && !error.contains("synthetic-key"));
        }
    }

    fn stored_error(paths: &Paths) -> String {
        format!("{:#}", stored(paths).err().expect("key must be refused"))
    }

    #[test]
    fn every_way_the_file_can_be_unreadable_names_it_and_the_fix() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        save(&paths, "4192 0837 5561 2094", KeyKind::Account).unwrap();
        let good: serde_json::Value =
            serde_json::from_str(&io::fs::read_string(&paths.auth()).unwrap().unwrap()).unwrap();
        let mut bad_secret = good.clone();
        bad_secret["secret"] = "not base64!".into();
        let mut bad_scheme = good;
        bad_scheme["scheme"] = "rot13".into();
        for broken in [
            "{".to_owned(),
            bad_secret.to_string(),
            bad_scheme.to_string(),
        ] {
            io::fs::write_atomic(&paths.auth(), broken.as_bytes(), Access::Private).unwrap();
            let err = format!("{:#}", stored(&paths).err().expect("unreadable"));
            assert!(err.contains(&paths.auth().display().to_string()), "{err}");
            assert!(err.contains("palepale logout"), "{err}");
        }
    }
}
