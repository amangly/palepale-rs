//! Per-installation trial signing. The private seed never leaves protected local storage.
use super::{clock, fs, secret};
use crate::text::digest::sha256_hex;
use anyhow::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as BASE64};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::LazyLock};

#[derive(Deserialize)]
struct Config {
    directory: String,
    header: String,
    protocol: String,
    unavailable: String,
    max_store_bytes: usize,
}
static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/trial-device.toml")).expect("trial device settings")
});

#[derive(Serialize, Deserialize)]
struct Stored {
    scheme: String,
    secret: String,
}

fn load(home: &Path, origin: &str) -> Result<SigningKey> {
    let path = home
        .join(&CONFIG.directory)
        .join(format!("{}.json", sha256_hex(origin.as_bytes())));
    let _lock = fs::lock(&path.with_extension("lock"))?;
    let stored = match fs::read_bytes_limited(&path, CONFIG.max_store_bytes)? {
        Some(bytes) => serde_json::from_slice::<Stored>(&bytes)?,
        None => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|_| anyhow::anyhow!(CONFIG.unavailable.clone()))?;
            let (scheme, sealed) = secret::seal(&seed)?;
            seed.fill(0);
            let stored = Stored {
                scheme: scheme.to_owned(),
                secret: BASE64.encode(sealed),
            };
            fs::write_atomic(&path, &serde_json::to_vec(&stored)?, fs::Access::Private)?;
            stored
        }
    };
    let sealed = BASE64.decode(stored.secret)?;
    let mut opened = secret::open(&stored.scheme, &sealed)?;
    let mut seed: [u8; 32] = opened.as_slice().try_into()?;
    let key = SigningKey::from_bytes(&seed);
    seed.fill(0);
    opened.fill(0);
    Ok(key)
}

pub fn header() -> &'static str {
    &CONFIG.header
}

pub fn proof(
    home: &Path,
    url: &url::Url,
    token: &str,
    method: &str,
    idempotency: &str,
    body: &[u8],
) -> Result<String> {
    let key = load(home, &url.origin().ascii_serialization())
        .map_err(|_| anyhow::anyhow!(CONFIG.unavailable.clone()))?;
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!(CONFIG.unavailable.clone()))?;
    Ok(sign(
        &key,
        token,
        method,
        url.path(),
        idempotency,
        body,
        clock::now().as_second(),
        &nonce,
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "fixed fields of the public cryptographic transcript"
)]
fn sign(
    key: &SigningKey,
    token: &str,
    method: &str,
    path: &str,
    idempotency: &str,
    body: &[u8],
    timestamp: i64,
    nonce: &[u8; 32],
) -> String {
    let timestamp = timestamp.to_string();
    let nonce = BASE64.encode(nonce);
    let public = BASE64.encode(key.verifying_key().to_bytes());
    let message = [
        CONFIG.protocol.as_str(),
        method,
        path,
        &timestamp,
        &nonce,
        &public,
        &sha256_hex(token.as_bytes()),
        idempotency,
        &sha256_hex(body),
    ]
    .join("\n");
    let signature = BASE64.encode(key.sign(message.as_bytes()).to_bytes());
    format!("v1.{timestamp}.{nonce}.{public}.{signature}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_store_is_stable_origin_scoped_and_never_replaced_on_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let a = load(dir.path(), "https://fixture.invalid")
            .unwrap()
            .verifying_key();
        assert_eq!(
            a,
            load(dir.path(), "https://fixture.invalid")
                .unwrap()
                .verifying_key()
        );
        assert_ne!(
            a,
            load(dir.path(), "https://other.invalid")
                .unwrap()
                .verifying_key()
        );
        let path = dir
            .path()
            .join(&CONFIG.directory)
            .join(format!("{}.json", sha256_hex(b"https://fixture.invalid")));
        fs::write_atomic(&path, b"broken", fs::Access::Private).unwrap();
        assert!(load(dir.path(), "https://fixture.invalid").is_err());
        assert_eq!(fs::read_bytes(&path).unwrap(), b"broken");
    }
    #[test]
    fn signature_matches_shared_protocol_vector() {
        let signature = sign(
            &SigningKey::from_bytes(&[7; 32]),
            "fixture-key",
            "POST",
            "/v1/trial/flows",
            "",
            br#"{"action":"trial_request"}"#,
            1793426400,
            &[9; 32],
        );
        assert_eq!(
            signature,
            include_str!("../../tests/fixtures/trial-proof.txt").trim()
        );
    }
}
