//! The commands the battery runs itself: git, and the repos' setup, prepare and check lines.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};

pub struct Ran {
    pub ok: bool,
    /// stdout, then stderr.
    pub output: String,
}

// CEILING: a line runs under `sh` and only `sh` is ended on a timeout; what it started may
// outlive it. Windows uses Git Bash from the shell catalog. Upgrade: run lines through palepale's own
// process-group shell (src/io/shell.rs) once the battery can link against it.
pub async fn sh(line: &str, cwd: &Path, env: &[(String, String)], limit: Duration) -> Ran {
    let mut command = tokio::process::Command::new(posix_shell());
    command
        .args(["-c", line])
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(limit, command.output()).await {
        Ok(Ok(out)) => Ran {
            ok: out.status.success(),
            output: format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        },
        Ok(Err(e)) => Ran {
            ok: false,
            output: format!("could not start sh: {e}"),
        },
        Err(_) => Ran {
            ok: false,
            output: format!("still running after {limit:?}"),
        },
    }
}

fn posix_shell() -> String {
    #[cfg(windows)]
    {
        let catalog: toml::Value = toml::from_str(include_str!("../../data/shells.toml")).unwrap();
        for shell in catalog["shell"].as_array().unwrap() {
            if shell["os"].as_str() != Some("windows") || shell["args"][0].as_str() != Some("-c") {
                continue;
            }
            for candidate in shell["candidates"].as_array().unwrap() {
                let mut program = candidate.as_str().unwrap().to_owned();
                if let Some((_, tail)) = program.split_once('{')
                    && let Some((name, _)) = tail.split_once('}')
                {
                    let Ok(value) = std::env::var(name) else {
                        continue;
                    };
                    program = program.replace(&format!("{{{name}}}"), &value);
                }
                if Path::new(&program).is_file() {
                    return program;
                }
            }
        }
    }
    "sh".into()
}

/// Runs git with an identity and no signing, so commits work on any machine; fails on a
/// non-zero exit.
pub async fn git(args: &[&str], cwd: &Path) -> anyhow::Result<String> {
    let out = tokio::process::Command::new("git")
        .args([
            "-c",
            "user.name=battery",
            "-c",
            "user.email=battery@localhost",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .await
        .context("git")?;
    if !out.status.success() {
        bail!(
            "git {} in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
