use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{CLEARED, Sandbox, binary, launch_env};

/// Run with PALEPALE_TEST_BINARY pointing at the optimized build to check real self-replacement.
#[tokio::test(flavor = "multi_thread")]
async fn github_update_keeps_credentials_and_installs_only_verified_newer_releases() {
    let source = binary();
    let version = tokio::process::Command::new(&source)
        .arg("--version")
        .output()
        .await
        .unwrap();
    let version = String::from_utf8(version.stdout).unwrap();
    let Some((_, tag)) = version.split_once('(') else {
        return;
    };
    let current = tag.trim().trim_end_matches(')');
    let (base, build) = current.rsplit_once('-').unwrap();
    let next = format!("{base}-{}", build.parse::<u64>().unwrap() + 1);
    let payload = b"verified executable fixture";
    let checksum: String = Sha256::digest(payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    #[cfg(windows)]
    let asset = "palepale-x86_64-pc-windows-msvc.exe";
    #[cfg(target_os = "macos")]
    let asset = "palepale-aarch64-apple-darwin";
    #[cfg(target_os = "linux")]
    let asset = "palepale-x86_64-unknown-linux-musl";

    for scenario in [
        "new",
        "same",
        "older",
        "invalid",
        "checksum",
        "missing",
        "unpublished",
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let target = match scenario {
            "same" => current,
            "older" => "v0.0.0-0",
            "invalid" => "v0.2.0-preview",
            _ => &next,
        };
        let assets = if scenario == "missing" {
            vec![]
        } else {
            vec![
                json!({ "name": asset, "browser_download_url": format!("{origin}/binary") }),
                json!({ "name": "SHA256SUMS", "browser_download_url": format!("{origin}/checksums") }),
            ]
        };
        let latest = json!({ "tag_name": target, "assets": assets });
        let sums = format!(
            "{}  {asset}\n",
            if scenario == "checksum" {
                "bad"
            } else {
                &checksum
            }
        );
        let downloads = Arc::new(AtomicUsize::new(0));
        let leaked = Arc::new(AtomicBool::new(false));
        let (seen, fetched) = (leaked.clone(), downloads.clone());
        let router = Router::new()
            .route(
                "/amangly/palepale-releases/releases/latest",
                get(move |headers: HeaderMap| async move {
                    seen.store(headers.contains_key("authorization"), Ordering::Relaxed);
                    (
                        if scenario == "unpublished" {
                            StatusCode::NOT_FOUND
                        } else {
                            StatusCode::OK
                        },
                        axum::Json(latest),
                    )
                }),
            )
            .route(
                "/binary",
                get(move || async move {
                    fetched.fetch_add(1, Ordering::Relaxed);
                    payload.to_vec()
                }),
            )
            .route("/checksums", get(move || async move { sums }));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let sandbox = Sandbox::default();
        let install = tempfile::tempdir().unwrap();
        let exe = install.path().join(source.file_name().unwrap());
        std::fs::copy(&source, &exe).unwrap();
        let original = std::fs::read(&exe).unwrap();
        std::fs::write(
            sandbox.home().join("auth.json"),
            "synthetic stored credential marker",
        )
        .unwrap();
        std::fs::write(sandbox.home().join("config.toml"), "auto_update = false\n").unwrap();
        let mut child = tokio::process::Command::new(&exe);
        for name in CLEARED {
            child.env_remove(name);
        }
        child
            .arg("update")
            .current_dir(sandbox.project.path())
            .envs(launch_env(
                sandbox.home(),
                &origin,
                Some("synthetic-env-key"),
            ))
            .env("PALEPALE_RELEASES_URL", &origin)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(20), child.output())
            .await
            .unwrap()
            .unwrap();
        let err = String::from_utf8_lossy(&output.stderr);
        if scenario == "new" {
            assert!(output.status.success(), "{err}");
            assert_eq!(std::fs::read(&exe).unwrap(), payload);
            assert!(String::from_utf8_lossy(&output.stdout).contains("Restart palepale"));
        } else {
            assert_eq!(std::fs::read(&exe).unwrap(), original, "{scenario}");
            assert_eq!(
                output.status.success(),
                matches!(scenario, "same" | "older"),
                "{scenario}: {err}"
            );
            let expected = match scenario {
                "invalid" => Some("expected vMAJOR"),
                "checksum" => Some("does not match"),
                "missing" => Some("has no"),
                "unpublished" => Some("No public Palepale release"),
                _ => None,
            };
            if let Some(expected) = expected {
                assert!(err.contains(expected), "{scenario}: {err}");
            }
        }
        assert_eq!(
            downloads.load(Ordering::Relaxed),
            usize::from(matches!(scenario, "new" | "checksum")),
            "{scenario}"
        );
        assert!(!leaked.load(Ordering::Relaxed));
        assert_eq!(
            std::fs::read_to_string(sandbox.home().join("auth.json")).unwrap(),
            "synthetic stored credential marker"
        );
        assert_eq!(
            std::fs::read_to_string(sandbox.home().join("config.toml")).unwrap(),
            "auto_update = false\n"
        );
        assert!(sandbox.sessions().is_empty());
        server.abort();
    }
}
