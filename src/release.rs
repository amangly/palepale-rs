//! Which release this binary is, and which file of a release each platform runs
//! (data/release.toml).

use std::sync::LazyLock;

use serde::Deserialize;

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    toml::from_str(include_str!("../data/release.toml"))
        .expect("data/release.toml is checked by tests")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    repository: String,
    sums: String,
    asset: Vec<Asset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    os: String,
    arch: String,
    pub file: String,
}

/// CI stamps published tags; optimized local builds start at build zero for their version.
pub fn tag() -> Option<&'static str> {
    option_env!("PALEPALE_RELEASE")
        .filter(|t| !t.is_empty())
        .or_else(|| {
            (!cfg!(debug_assertions)).then_some(concat!("v", env!("CARGO_PKG_VERSION"), "-0"))
        })
}

pub fn repository() -> &'static str {
    &CATALOG.repository
}

/// Published tags use vMAJOR.MINOR.PATCH-BUILD, with a missing build counted as zero.
pub fn newer(latest: &str, current: &str) -> Option<bool> {
    fn number(tag: &str) -> Option<[u64; 4]> {
        let tag = tag.strip_prefix('v')?;
        let (version, build) = tag.split_once('-').unwrap_or((tag, "0"));
        let mut fields = version.split('.');
        let mut parsed = [0; 4];
        for slot in &mut parsed[..3] {
            let field = fields.next()?;
            if !field.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            *slot = field.parse().ok()?;
        }
        if fields.next().is_some() || !build.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        parsed[3] = build.parse().ok()?;
        Some(parsed)
    }
    Some(number(latest)? > number(current)?)
}

/// What `--version` prints: the package version, and the release when there is one.
pub fn version() -> &'static str {
    static VERSION: LazyLock<String> = LazyLock::new(|| match tag() {
        Some(tag) => format!("{} ({tag})", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").to_owned(),
    });
    &VERSION
}

/// The file this platform runs, if releases build one for it.
pub fn asset() -> Option<&'static Asset> {
    asset_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn asset_for(os: &str, arch: &str) -> Option<&'static Asset> {
    CATALOG.asset.iter().find(|a| a.os == os && a.arch == arch)
}

/// The checksum file every release carries.
pub fn sums() -> &'static str {
    &CATALOG.sums
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_versions_are_numeric_and_never_downgrade() {
        for (latest, current, expected) in [
            ("v0.1.1", "v0.1.0", Some(true)),
            ("v0.1.0", "v0.1.0", Some(false)),
            ("v0.1.0", "v0.1.0-0", Some(false)),
            ("v0.2.0-10", "v0.2.0-9", Some(true)),
            ("v0.3.0-1", "v0.2.0-999", Some(true)),
            ("v1.0.0", "v0.9.9-999", Some(true)),
            ("v0.2.0-1", "v0.2.0-0", Some(true)),
            ("v0.2.0-1", "v0.2.0-1", Some(false)),
            ("v0.2.0-9", "v0.2.0-10", Some(false)),
            ("v0.1.0-999", "v0.2.0-0", Some(false)),
            ("v0.2.0-beta", "v0.2.0-0", None),
            ("v0.2.0-1", "unknown", None),
            ("v0.2.0-1-extra", "v0.2.0-0", None),
            ("v+0.2.0-1", "v0.2.0-0", None),
        ] {
            assert_eq!(newer(latest, current), expected, "{latest} vs {current}");
        }
    }

    #[test]
    fn public_download_repository_matches_the_default_update_feed() {
        let endpoints: toml::Value =
            toml::from_str(include_str!("../data/endpoints.toml")).unwrap();
        assert_eq!(
            endpoints["origins"]["releases"]["url"].as_str().unwrap(),
            "https://api.github.com/repos"
        );
        assert_eq!(
            endpoints["endpoints"]["latest_release"]["path"]
                .as_str()
                .unwrap()
                .replace("{repository}", repository()),
            "/amangly/palepale-rs/releases/latest"
        );
    }

    #[test]
    fn each_platform_finds_its_file() {
        assert_eq!(
            asset_for("windows", "x86_64").unwrap().file,
            "palepale-x86_64-pc-windows-msvc.exe"
        );
        assert_eq!(
            asset_for("macos", "aarch64").unwrap().file,
            "palepale-aarch64-apple-darwin"
        );
        assert!(asset_for("macos", "x86_64").is_none());
    }

    #[test]
    fn every_asset_is_a_target_the_release_builds() {
        let ci = include_str!("../.github/workflows/ci.yml");
        for asset in &CATALOG.asset {
            let target = asset
                .file
                .trim_start_matches("palepale-")
                .trim_end_matches(".exe");
            assert!(
                ci.contains(target),
                "ci.yml builds no {target} for {}",
                asset.file
            );
        }
    }

    #[test]
    fn a_development_build_reports_the_plain_version() {
        if tag().is_none() {
            assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        }
    }
}
