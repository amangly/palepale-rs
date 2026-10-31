//! The interactive session through a real pseudo-terminal, read back through a terminal emulator.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::fake_palepale::{ENV_KEY, FORUM_PASSWORD, FORUM_PATH, Fake, Reply};
use crate::{CLEARED, Sandbox, launch_env};

const ROWS: u16 = 30;
const COLS: u16 = 100;
/// Longest wait for something to appear on screen or go away.
const WAIT: Duration = Duration::from_secs(20);
/// Longest wait for palepale to exit once asked to.
const EXIT_WAIT: Duration = Duration::from_secs(10);
/// How often the screen is checked while waiting.
const POLL: Duration = Duration::from_millis(50);
/// Between typed keys: far above `tui.paste_gap_ms` (data/limits.toml), so typing never looks
/// like a paste. CEILING: keys are timestamped when palepale reads them, so a stall longer than
/// two gaps can still bunch three into a paste (tui/paste.rs).
const KEY_GAP: Duration = Duration::from_millis(30);

#[tokio::test(flavor = "multi_thread")]
async fn one_trial_command_starts_a_terminal_session_with_trial_funding() {
    let fake = Fake::start().await;
    let mut catalog: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    catalog["data"][0]["id"] = serde_json::json!("deepseek-v4.1-flash-uncensored-zero-refusal");
    fake.model_lists([catalog.clone(), catalog]);
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut session = Session::start_as(&sandbox, &url, Some(ENV_KEY), &["trial"]);
        session.wait_for("palepale v");
        session.type_text("trial-session-marker");
        session.send("\r");
        session.wait_for("Hello there!");
        session.send("\x04");
        assert!(session.wait_exit());
    })
    .await
    .unwrap();
    let bodies = fake.chat_bodies();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["funding"], "trial");
    assert_eq!(
        bodies[0]["model"],
        "deepseek-v4.1-flash-uncensored-zero-refusal"
    );
}

fn contains_number(text: &str, number: &str) -> bool {
    text.split(|c: char| !c.is_ascii_digit())
        .any(|part| part == number)
}

#[tokio::test(flavor = "multi_thread")]
async fn trial_model_switch_keeps_deepseek_and_never_saves_or_dispatches_the_paid_choice() {
    let fake = Fake::start().await;
    let mut catalog: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    catalog["data"][0]["id"] = serde_json::json!("deepseek-v4.1-flash-uncensored-zero-refusal");
    fake.model_lists([catalog.clone(), catalog]);
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut session = Session::start_as(&sandbox, &url, Some(ENV_KEY), &["--trial"]);
        session.wait_for("AUTO");
        session.type_text("/models");
        session.send("\r");
        session.wait_for("┌ Models");
        session.send("\x1b[B");
        session.send("\r");
        session.wait_for("only for DeepSeek");
        assert!(
            !session.contents().contains("Saved as your default"),
            "{}",
            session.contents()
        );
        assert!(
            session.bottom_row().contains("deepseek-v4.1"),
            "{}",
            session.contents()
        );
        session.type_text("trial-model-marker");
        session.send("\r");
        session.wait_for("Hello there!");
        session.send("\x04");
        assert!(session.wait_exit());
        let prefs = sandbox.home().join("prefs.json");
        if prefs.exists() {
            assert!(
                !std::fs::read_to_string(prefs)
                    .unwrap()
                    .contains("fake-coder")
            );
        }
    })
    .await
    .unwrap();
    let bodies = fake.chat_bodies();
    assert_eq!(bodies.len(), 1);
    assert_eq!(
        bodies[0]["model"],
        "deepseek-v4.1-flash-uncensored-zero-refusal"
    );
    assert_eq!(bodies[0]["funding"], "trial");
}

#[test]
fn credential_checks_match_whole_numbers_without_matching_timestamps() {
    assert!(!contains_number("17:24:33.7901278Z", "9012"));
    assert!(contains_number("key=9012", "9012"));
    assert!(contains_number("[1234567890123456]", "1234567890123456"));
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_session_picker_restores_history_and_its_writer_lock_at_64_columns() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    sandbox
        .run(
            &fake.url,
            &["--mode", "plan", "-p", "resume-marker"],
            crate::WITH_KEY,
        )
        .await
        .success();
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut session = Session::start_sized(&sandbox, &url, Some(ENV_KEY), &[], 30, 64, false);
        session.wait_for("palepale v");
        session.type_text("/sessions");
        session.send("\r");
        session.wait_for("Saved sessions");
        session.wait_for("resume-marker");
        session.send("\r");
        session.wait_gone("Saved sessions");
        session.wait_for("PLAN");
        session.wait_for("Hello there!");
        let mut check = std::process::Command::new(crate::binary());
        check
            .args(["--continue", "-p", "must not run"])
            .current_dir(sandbox.project.path())
            .stdin(std::process::Stdio::null());
        for name in CLEARED {
            check.env_remove(name);
        }
        check.envs(launch_env(sandbox.home(), &url, Some(ENV_KEY)));
        let refused = check.output().unwrap();
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains("another process"));
        session.type_text("follow-up-marker");
        session.send("\r");
        session.wait_for("follow-up-marker");
        session.poll(WAIT, "the follow-up reply did not finish", |s| {
            (s.contents().matches("Hello there!").count() >= 2).then_some(())
        });
        session.send("\x04");
        assert!(session.wait_exit());
    })
    .await
    .unwrap();
    assert_eq!(fake.chat_bodies().len(), 2);
    assert!(
        fake.chat_bodies()[1]["messages"]
            .to_string()
            .contains("resume-marker")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paid_refusal_returns_control_without_automatic_redispatch() {
    let fake = Fake::start().await;
    fake.script([
        Reply::Error {
            status: 503,
            kind: "model_busy",
            retry_after: Some(1),
        },
        Reply::sse("hello.sse"),
    ]);
    let sandbox = Sandbox::default();
    let url = fake.url.clone();
    let recorded = fake.clone();
    tokio::task::spawn_blocking(move || {
        let mut session = Session::start_sized(&sandbox, &url, Some(ENV_KEY), &[], 30, 100, false);
        session.wait_for("palepale v");
        session.type_text("hello");
        session.send("\r");
        session.wait_for("model_busy");
        assert_eq!(recorded.chat_bodies().len(), 1);
        session.send("\x04");
        assert!(session.wait_exit());
    })
    .await
    .unwrap();
    assert_eq!(fake.chat_bodies().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn discovered_mcp_prompt_slash_commands_resolve_arguments_before_sending() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let config = serde_json::json!({"mcp_servers":{"fixture":{"command":super::extensions::fixture(),"args":["--modern",sandbox.home().join("rpc.log"),"--catalogs"]}}});
    std::fs::write(
        sandbox.home().join("config.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut session = Session::start_sized(&sandbox, &url, Some(ENV_KEY), &[], 24, 64, false);
        session.wait_for("palepale v");
        session.type_text("/fixture:review native");
        session.send("\r");
        session.wait_for("Hello there!");
        session.send("\x04");
        assert!(session.wait_exit());
    })
    .await
    .unwrap();
    let requests = fake.chat_bodies();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]["messages"]
            .to_string()
            .contains("Review native fixture instructions")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn discovered_skill_slash_commands_load_the_workflow_and_prompt_in_the_same_turn() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let skill = sandbox
        .project
        .path()
        .join(".agents/skills/native-review/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(skill,"---\nname: native-review\ndescription: Native slash workflow\n---\nSlash workflow body marker").unwrap();
    sandbox.trust_project();
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut session = Session::start_sized(&sandbox, &url, Some(ENV_KEY), &[], 24, 64, false);
        session.wait_for("palepale v");
        session.type_text("/native-review inspect this diff");
        session.send("\r");
        session.wait_for("Hello there!");
        session.send("\x04");
        assert!(session.wait_exit());
    })
    .await
    .unwrap();
    let requests = fake.chat_bodies();
    assert_eq!(requests.len(), 1);
    let messages = requests[0]["messages"].to_string();
    assert!(
        messages.contains("Slash workflow body marker") && messages.contains("inspect this diff")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn compact_extension_managers_add_a_server_and_select_a_skill_without_sending() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let skill = sandbox
        .project
        .path()
        .join(".agents/skills/native-check/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(skill, "---\nname: native-check\ndescription: Check the UI fixture.\n---\nInspect the UI fixture.\n").unwrap();
    sandbox.trust_project();
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut session = Session::start_sized(&sandbox, &url, Some(ENV_KEY), &[], 24, 64, false);
        session.wait_for("palepale v");
        session.type_text("/mcp");
        session.send("\r");
        session.wait_for("MCP servers");
        session.wait_for("No servers added");
        session.type_text("a");
        session.wait_for("Server name");
        session.type_text("native");
        session.send("\r");
        session.wait_for("HTTPS URL");
        session.type_text("http://127.0.0.1:9/mcp");
        session.send("\r");
        session.wait_for("Bearer token");
        session.send("\r");
        session.wait_for("OAuth client ID");
        session.send("\r");
        session.wait_for("native  error");
        assert!(
            !session.contents().contains("rmcp::"),
            "Internal transport types should stay out of the card"
        );
        session.capture("mcp-compact");
        session.send("\x1b");
        session.wait_gone("MCP servers");
        session.type_text("/skills");
        session.send("\r");
        session.wait_for("Skills");
        session.wait_for("native-check");
        session.send("\r");
        session.wait_for("Inspect the UI fixture.");
        session.capture("skills-compact");
        session.send("\r");
        session.wait_for("$native-check");
        session.send("\x15");
        session.wait_gone("$native-check");
        session.send("\x04");
        assert!(session.wait_exit());
        let config = std::fs::read_to_string(sandbox.home().join("config.toml")).unwrap();
        assert!(config.contains("127.0.0.1:9/mcp"));
    })
    .await
    .unwrap();
    assert!(
        fake.chat_bodies().is_empty(),
        "selecting a skill must preserve the unsent draft"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn startup_update_shows_progress_reopens_and_preserves_state_or_keeps_the_old_copy() {
    use std::sync::atomic::Ordering;
    let source = crate::binary();
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
    let (base, build) = current.rsplit_once('-').unwrap_or((current, "0"));
    let next = format!("{base}-{}", build.parse::<u64>().unwrap() + 1);
    let payload = std::fs::read(&source).unwrap();
    for scenario in [
        "new",
        "checksum",
        "disabled",
        "no_update",
        "offline",
        "cancelled",
    ] {
        let fake = Fake::start().await;
        let (feed, hits, leaked) = crate::update::startup_feed(
            payload.clone(),
            &next,
            scenario == "checksum",
            scenario == "cancelled",
        )
        .await;
        let sandbox = Sandbox::default();
        let install = tempfile::tempdir().unwrap();
        let exe = install.path().join(source.file_name().unwrap());
        std::fs::copy(&source, &exe).unwrap();
        // An executable can carry an unused overlay; replacement must remove this marker.
        let mut original = payload.clone();
        original.extend_from_slice(b"old test copy marker");
        std::fs::write(&exe, &original).unwrap();
        let config = format!("auto_update = {}\n", scenario != "disabled");
        std::fs::write(sandbox.home().join("config.toml"), &config).unwrap();
        std::fs::write(
            sandbox.home().join("auth.json"),
            "synthetic credential marker",
        )
        .unwrap();
        #[cfg(windows)]
        std::fs::write(sandbox.home().join("install.json"), r#"{"declined":true}"#).unwrap();
        let api = fake.url.clone();
        let next_tag = next.clone();
        let installed_payload = payload.clone();
        tokio::task::spawn_blocking(move || {
            let mut cmd = CommandBuilder::new(&exe);
            cmd.args(["--model", "fake-coder", "startup prompt"]);
            cmd.cwd(sandbox.project.path());
            for name in CLEARED {
                cmd.env_remove(name);
            }
            for (name, value) in launch_env(sandbox.home(), &api, Some(ENV_KEY)) {
                cmd.env(name, value);
            }
            cmd.env("PALEPALE_RELEASES_URL", &feed);
            cmd.env("TERM", "xterm-256color");
            if scenario == "no_update" {
                cmd.env("PALEPALE_NO_UPDATE", "1");
            }
            if scenario == "offline" {
                cmd.env("PALEPALE_OFFLINE", "1");
            }
            let mut session = Session::spawn(cmd, ROWS, COLS);
            let checks = !matches!(scenario, "disabled" | "no_update" | "offline");
            if checks {
                session.wait_for("Downloading update");
                assert!(session.contents().contains('%'), "{}", session.contents());
                if scenario == "cancelled" {
                    session.send("\x03");
                }
            }
            session.wait_for("palepale v");
            session.wait_for("startup prompt");
            if scenario != "offline" {
                assert!(
                    session.bottom_row().contains("fake-coder"),
                    "{}",
                    session.contents()
                );
            }
            if scenario == "new" {
                assert!(
                    std::fs::read(&exe).unwrap() == installed_payload,
                    "{scenario}: installed bytes differ; updater state: {:?}; screen:\n{}",
                    std::fs::read_to_string(sandbox.home().join("update.json")),
                    session.contents()
                );
                session.wait_for(&format!("Updated to {next_tag}"));
            } else {
                assert!(
                    std::fs::read(&exe).unwrap() == original,
                    "{scenario}: the original copy changed; updater state: {:?}; screen:\n{}",
                    std::fs::read_to_string(sandbox.home().join("update.json")),
                    session.contents()
                );
            }
            assert_eq!(
                std::fs::read_to_string(sandbox.home().join("auth.json")).unwrap(),
                "synthetic credential marker"
            );
            assert_eq!(
                std::fs::read_to_string(sandbox.home().join("config.toml")).unwrap(),
                config
            );
            assert!(sandbox.sessions().is_empty());
            assert_eq!(hits.load(Ordering::Relaxed), usize::from(checks));
            assert!(!leaked.load(Ordering::Relaxed));
            if scenario == "cancelled" {
                assert!(!sandbox.home().join("update.json").exists());
            }
            session.send("\x15");
            session.send("\x04");
            assert!(session.wait_exit());
        })
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_login_pairs_on_first_open_and_reuses_the_key_on_next_open() {
    let fake = Fake::start().await;
    fake.pair_after(1);
    fake.script([Reply::sse("hello.sse")]);
    let url = fake.url.clone();
    let sandbox = tokio::task::spawn_blocking(move || {
        let sandbox = Sandbox::default();
        let mut first = Session::start_sized(&sandbox, &url, None, &[], ROWS, COLS, true);
        first.wait_for("012345");
        first.wait_for("Logged in with device key …9999");
        first.type_text("hi");
        first.send("\r");
        first.wait_for("Hello there!");
        first.send("\x04");
        assert!(first.wait_exit());
        let mut next = Session::start_sized(&sandbox, &url, None, &[], ROWS, COLS, true);
        next.wait_for("Account …3456. Available balance $12.30.");
        next.type_text("/status");
        next.send("\r");
        next.wait_for("Context:");
        next.send("\x04");
        assert!(next.wait_exit());
        sandbox
    })
    .await
    .unwrap();
    assert!(sandbox.home().join("auth.json").exists());
    assert_eq!(fake.hits("/v1/pair/start"), 1);
    for path in sandbox.sessions() {
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains(crate::fake_palepale::DEVICE_KEY)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_login_reconnects_a_revoked_key_but_keeps_it_on_a_server_outage() {
    for status in [401, 503] {
        let fake = Fake::start().await;
        let sandbox = Sandbox::default();
        sandbox
            .run(
                &fake.url,
                &["login", "--key", crate::fake_palepale::ACCOUNT_NUMBER],
                None,
            )
            .await
            .success();
        let original = std::fs::read(sandbox.home().join("auth.json")).unwrap();
        fake.me_errors([status]);
        let url = fake.url.clone();
        let sandbox = tokio::task::spawn_blocking(move || {
            let mut session = Session::start_sized(&sandbox, &url, None, &[], ROWS, COLS, true);
            if status == 401 {
                session.wait_for("Logged in with device key …9999");
            } else {
                session.wait_for("storage_unavailable");
            }
            session.send("\x04");
            assert!(session.wait_exit());
            sandbox
        })
        .await
        .unwrap();
        assert_eq!(fake.hits("/v1/pair/start"), usize::from(status == 401));
        if status == 503 {
            assert_eq!(
                original,
                std::fs::read(sandbox.home().join("auth.json")).unwrap()
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_login_can_be_cancelled_without_saving_a_key() {
    let fake = Fake::start().await;
    fake.pair_after(100);
    let url = fake.url.clone();
    let sandbox = tokio::task::spawn_blocking(move || {
        let sandbox = Sandbox::default();
        let mut session = Session::start_sized(&sandbox, &url, None, &[], ROWS, COLS, true);
        session.wait_for("012345");
        session.send("\x1b");
        session.wait_gone("Waiting for approval");
        session.send("\x04");
        assert!(session.wait_exit());
        sandbox
    })
    .await
    .unwrap();
    assert!(!sandbox.home().join("auth.json").exists());
    assert!(sandbox.sessions().is_empty());
}

struct Session {
    screen: vt100::Parser,
    /// Every byte palepale wrote, for what the emulator does not show (OSC 52).
    written: Vec<u8>,
    output: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Held so the child keeps its terminal; dropping it would hang it up.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

/// A failed test must not leave palepale running.
impl Drop for Session {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
    }
}

impl Session {
    fn start(sandbox: &Sandbox, api_url: &str) -> Self {
        Self::start_as(sandbox, api_url, Some(ENV_KEY), &[])
    }

    /// `key: None` starts logged out; `args` go on the command line.
    fn start_as(sandbox: &Sandbox, api_url: &str, key: Option<&str>, args: &[&str]) -> Self {
        Self::start_sized(sandbox, api_url, key, args, ROWS, COLS, false)
    }

    fn start_sized(
        sandbox: &Sandbox,
        api_url: &str,
        key: Option<&str>,
        args: &[&str],
        rows: u16,
        cols: u16,
        auto_login: bool,
    ) -> Self {
        Self::spawn(
            Self::command(sandbox, api_url, key, args, auto_login),
            rows,
            cols,
        )
    }

    fn command(
        sandbox: &Sandbox,
        api_url: &str,
        key: Option<&str>,
        args: &[&str],
        auto_login: bool,
    ) -> CommandBuilder {
        // Packaged Windows tests stay portable instead of installing into the runner profile.
        #[cfg(windows)]
        std::fs::write(sandbox.home().join("install.json"), r#"{"declined":true}"#).unwrap();
        let mut cmd = CommandBuilder::new(crate::binary());
        cmd.args(args);
        cmd.cwd(sandbox.project.path());
        for name in CLEARED {
            cmd.env_remove(name);
        }
        for (name, value) in launch_env(sandbox.home.path(), api_url, key) {
            if auto_login && name == "PALEPALE_NO_AUTO_LOGIN" {
                continue;
            }
            cmd.env(name, value);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("PALEPALE_NO_UPDATE", "1");
        cmd
    }

    fn spawn(cmd: CommandBuilder, rows: u16, cols: u16) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pseudo-terminal");
        let child = pty.slave.spawn_command(cmd).expect("palepale starts");
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let writer = pty.master.take_writer().unwrap();
        Self {
            screen: vt100::Parser::new(rows, cols, 500),
            written: Vec::new(),
            output,
            writer,
            child,
            _master: pty.master,
        }
    }

    fn send(&mut self, bytes: &str) {
        self.writer.write_all(bytes.as_bytes()).unwrap();
        self.writer.flush().unwrap();
    }

    /// Types like a person: one key at a time, far slower than a paste.
    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.send(&c.to_string());
            std::thread::sleep(KEY_GAP);
        }
    }

    fn contents(&self) -> String {
        self.screen.screen().contents()
    }

    fn capture(&self, name: &str) {
        if let Ok(folder) = std::env::var("PALEPALE_CAPTURE_DIR") {
            let path = std::path::Path::new(&folder).join(name);
            std::fs::write(path.with_extension("txt"), self.contents()).unwrap();
            std::fs::write(
                path.with_extension("ansi"),
                self.screen.screen().contents_formatted(),
            )
            .unwrap();
        }
    }

    fn bottom_row(&self) -> String {
        self.row(usize::from(self.screen.screen().size().0) - 1)
    }

    /// Feeds output to the emulator, answering cursor-position queries as a real terminal
    /// would: Windows' ConPTY asks one before it lets any output through. True if any came.
    fn pump(&mut self) -> bool {
        let mut any = false;
        while let Ok(bytes) = self.output.try_recv() {
            any = true;
            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                let (row, col) = self.screen.screen().cursor_position();
                self.send(&format!("\x1b[{};{}R", row + 1, col + 1));
            }
            self.screen.process(&bytes);
            self.written.extend(bytes);
        }
        any
    }

    /// Pumps until `done` answers, or fails after `limit` naming `what` and the screen.
    #[track_caller]
    fn poll<T>(
        &mut self,
        limit: Duration,
        what: &str,
        mut done: impl FnMut(&mut Self) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            self.pump();
            if let Some(found) = done(self) {
                return found;
            }
            std::thread::sleep(POLL);
        }
        panic!("{what}; screen:\n{}", self.contents());
    }

    #[track_caller]
    fn wait_for(&mut self, text: &str) {
        self.poll(WAIT, &format!("never saw {text:?}"), |s| {
            s.contents().contains(text).then_some(())
        });
    }

    fn wait_for_logo(&mut self) {
        for row in include_str!("../../data/palepale-logo-cells.txt").lines() {
            if !row.trim().is_empty() {
                self.wait_for(row.trim_end());
            }
        }
    }

    #[track_caller]
    fn wait_gone(&mut self, text: &str) {
        self.poll(WAIT, &format!("{text:?} never went away"), |s| {
            (!s.contents().contains(text)).then_some(())
        });
    }

    /// Until one poll interval passes with no new output: the screen has settled.
    #[track_caller]
    fn wait_quiet(&mut self) {
        self.poll(WAIT, "output never settled", |s| {
            std::thread::sleep(POLL);
            (!s.pump()).then_some(())
        });
    }

    /// Row `i` of the screen, zero-based.
    fn row(&self, i: usize) -> String {
        self.screen
            .screen()
            .rows(0, self.screen.screen().size().1)
            .nth(i)
            .unwrap_or_default()
    }

    /// The screen row showing `text`, zero-based.
    #[track_caller]
    fn row_of(&self, text: &str) -> usize {
        self.screen
            .screen()
            .rows(0, self.screen.screen().size().1)
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("no row shows {text:?}; screen:\n{}", self.contents()))
    }

    /// A left click as a terminal reports it (SGR mouse encoding, one-based).
    fn click(&mut self, row: usize) {
        self.press(row, 0);
    }

    // Only the Linux copy tests right-click (see them for why).
    #[cfg(target_os = "linux")]
    fn right_click(&mut self, row: usize) {
        self.press(row, 2);
    }

    fn press(&mut self, row: usize, button: u8) {
        self.send(&format!(
            "\x1b[<{button};3;{0}M\x1b[<{button};3;{0}m",
            row + 1
        ));
    }

    #[track_caller]
    fn wait_exit(&mut self) -> bool {
        self.poll(EXIT_WAIT, "palepale did not exit", |s| {
            s.child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| status.success())
        })
    }
}

/// What a session left behind: `body`'s answer, the screen before and after quitting (the
/// session's own screen is gone once it exits), and the sandbox, for the files it wrote.
struct Ended<T> {
    out: T,
    screen: String,
    exited: String,
    sandbox: Sandbox,
}

/// Starts a session (logged out when `key` is `None`), waits for its input bar, runs `body`, then
/// quits with Ctrl+D and asserts a clean exit.
async fn in_session<T: Send + 'static>(
    fake: &Fake,
    sandbox: Sandbox,
    key: Option<&'static str>,
    body: impl FnOnce(&mut Session) -> T + Send + 'static,
) -> Ended<T> {
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut s = Session::start_as(&sandbox, &url, key, &[]);
        s.poll(WAIT, "input bar never appeared", |s| {
            s.bottom_row().contains('─').then_some(())
        });
        let out = body(&mut s);
        let screen = s.contents();
        s.send("\x04");
        assert!(
            s.wait_exit(),
            "palepale exited with an error; screen:\n{}",
            s.contents()
        );
        Ended {
            out,
            screen,
            exited: s.contents(),
            sandbox,
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_welcomes_chats_switches_mode_and_quits() {
    let version = tokio::process::Command::new(crate::binary())
        .arg("--version")
        .output()
        .await
        .unwrap();
    assert!(version.status.success());
    let version = String::from_utf8(version.stdout).unwrap();
    let version = version.trim();
    let expected = match version.split_once('(') {
        Some((_, tag)) => tag.trim_end_matches(')').to_owned(),
        None => format!("v{}", version.split_whitespace().nth(1).unwrap()),
    };
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), move |s| {
        s.wait_for(&format!("palepale {expected}"));
        s.wait_for("AUTO");
        // The message bar and the rule under it, with the model, sit at the bottom from the start.
        assert!(s.bottom_row().contains("demo-2"), "{}", s.contents());
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        s.send("\x1b[Z");
        s.poll(WAIT, "Shift+Tab never reached MANUAL", |s| {
            s.bottom_row().contains("MANUAL").then_some(())
        });
        s.send("\x1b[Z");
        s.poll(WAIT, "Shift+Tab never reached PLAN", |s| {
            s.bottom_row().contains("PLAN").then_some(())
        });
    })
    .await;
    assert!(ended.exited.contains("Session saved"), "{}", ended.exited);
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_matrix_intro_animates_through_resize_with_and_without_color() {
    tokio::task::spawn_blocking(|| {
        for no_color in [false, true] {
            let sandbox = Sandbox::default();
            let mut cmd = Session::command(&sandbox, "", None, &[], false);
            if no_color {
                cmd.env("NO_COLOR", "1");
            }
            let mut s = Session::spawn(cmd, ROWS, COLS);
            s.wait_for("palepale v");
            s._master
                .resize(PtySize {
                    rows: ROWS + 1,
                    cols: COLS,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .unwrap();
            s.screen.screen_mut().set_size(ROWS + 1, COLS);
            let mut rain = std::collections::HashSet::new();
            s.poll(
                WAIT,
                &format!("Matrix rain did not animate after resize (NO_COLOR={no_color})"),
                |s| {
                    let columns = include_str!("../../data/palepale-logo-cells.txt")
                        .lines()
                        .map(|row| row.chars().count())
                        .max()
                        .unwrap();
                    let logo: String = (0..usize::from(ROWS + 1))
                        .map(|row| s.row(row).chars().skip(2).take(columns).collect::<String>())
                        .collect();
                    if logo.chars().any(|c| c.is_ascii_digit() || c == 'Z') {
                        rain.insert(logo);
                    }
                    (rain.len() >= 3).then_some(())
                },
            );
            if !no_color {
                s.capture("matrix-rain");
            }
            s.wait_for_logo();
            s.wait_quiet();
            if !no_color {
                s.capture("matrix-logo");
            }
            s.send("\x04");
            assert!(s.wait_exit());
            assert!(sandbox.sessions().is_empty());
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_during_the_matrix_intro_keeps_the_draft_and_reveals_the_logo() {
    let fake = Fake::start().await;
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.type_text("keep this draft");
        s.wait_for("keep this draft");
        s.wait_for_logo();
        let screen = s.contents();
        assert!(screen.contains("/help commands"), "{screen}");
        assert!(
            !screen.contains("Your terminal. Your workspace."),
            "{screen}"
        );
        s.send("\x15");
    })
    .await;
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn keytest_reads_the_terminal_and_leaves_it_as_it_found_it() {
    let fake = Fake::start().await;
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let sandbox = Sandbox::default();
        let mut s = Session::start_as(&sandbox, &url, None, &["keytest", "shift+tab"]);
        s.wait_for("Press Shift+Tab");
        assert!(s.screen.screen().bracketed_paste() && s.screen.screen().hide_cursor());
        s.send("\x1b[Z");
        s.wait_for("Shift+Tab: BackTab");
        assert!(s.wait_exit(), "{}", s.contents());
        let screen = s.screen.screen();
        assert!(!screen.bracketed_paste() && !screen.hide_cursor());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_folder_is_asked_about_once_before_its_config_loosens_anything() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    std::fs::write(sandbox.home().join("config.toml"), "mode = 'manual'\n").unwrap();
    sandbox.copy_project("untrusted_auto");
    let url = fake.url.clone();
    let (screens, trusted) = tokio::task::spawn_blocking(move || {
        let mut screens = Vec::new();
        for answer in ["n\r", "y\r", ""] {
            let mut s = Session::start(&sandbox, &url);
            if answer.is_empty() {
                s.wait_for("AUTO");
            } else {
                s.wait_for("Trust this folder");
                s.send(answer);
                s.wait_for(if answer == "y\r" { "AUTO" } else { "MANUAL" });
            }
            s.send("\x04");
            assert!(s.wait_exit(), "{}", s.contents());
            screens.push(s.contents());
        }
        let trusted = std::fs::read_to_string(sandbox.home.path().join("trusted.json")).unwrap();
        (screens, trusted)
    })
    .await
    .unwrap();
    assert!(screens[0].contains("mode"), "{}", screens[0]);
    assert!(screens[0].contains("Not trusted"), "{}", screens[0]);
    assert!(!screens[2].contains("Trust this folder"), "{}", screens[2]);
    let trusted: serde_json::Value = serde_json::from_str(&trusted).unwrap();
    assert_eq!(
        trusted["projects"].as_array().unwrap().len(),
        1,
        "{trusted}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_typed_during_a_run_never_reaches_the_model() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("pty/sleep.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    std::fs::create_dir_all(sandbox.home.path()).unwrap();
    std::fs::write(sandbox.home.path().join("config.toml"), "mode = \"auto\"\n").unwrap();
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("go");
        s.send("\r");
        s.wait_for("sleep 3");
        s.type_text("/login --key 1234 5678 9012 3456");
        s.send("\r");
        s.wait_for("Logged in with device key …3456");
        s.wait_for("Hello there!");
        s.contents()
    })
    .await;
    // The command ran mid-turn, echoed with its number hidden.
    assert!(ended.out.contains("/login --key …3456"), "{}", ended.out);
    let sessions: String = ended
        .sandbox
        .sessions()
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        // Timestamps and random record IDs can contain the same digits as the fake key.
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| {
                    let record: serde_json::Value = serde_json::from_str(line).unwrap();
                    record.get("message").map(ToString::to_string)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let bodies = serde_json::to_string(&fake.chat_bodies()).unwrap();
    assert_eq!(fake.hits("/v1/chat/completions"), 2);
    for secret in ["9012", "3456", crate::fake_palepale::ACCOUNT_NUMBER] {
        assert!(
            !contains_number(&bodies, secret),
            "{secret} reached the model: {bodies}"
        );
        assert!(
            !contains_number(&sessions, secret),
            "{secret} was saved: {sessions}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_doubled_slash_sends_a_message_that_starts_with_a_slash() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("//etc/hosts is odd");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    let bodies = fake.chat_bodies();
    assert!(
        bodies[0]["messages"][1]["content"]
            .as_str()
            .unwrap()
            .ends_with("\n\n/etc/hosts is odd")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_mode_shows_the_edit_and_applies_it_once_approved() {
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("scenarios/auto_fix/1.sse"),
        Reply::sse("scenarios/auto_fix/2.sse"),
        Reply::sse("scenarios/auto_fix/4.sse"),
    ]);
    let sandbox = Sandbox::default();
    std::fs::write(sandbox.home().join("config.toml"), "mode = 'manual'\n").unwrap();
    sandbox.copy_project("calc");
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        s.type_text("fix the sum");
        s.send("\r");
        s.wait_for("edit src/calc.txt");
        s.wait_for("+2 + 2 = 4");
        s.wait_for("1 Allow this action");
        s.send("1");
        s.wait_for("Fixed: 2 + 2 = 4.");
    })
    .await;
    let fixed = std::fs::read_to_string(ended.sandbox.project.path().join("src/calc.txt")).unwrap();
    assert_eq!(fixed, "2 + 2 = 4\n", "{}", ended.screen);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slash_opens_the_command_popup_which_completes_and_runs() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("/mo");
        s.wait_for("┌ commands");
        s.wait_for("/models");
        // Tab completes the first match and leaves room for arguments; the popup closes.
        s.send("\t");
        s.wait_gone("┌ commands");
        // The bar never leaves the bottom row, whatever opened and closed above it.
        assert!(s.bottom_row().contains("demo-2"), "{}", s.contents());
        s.send("\r");
        s.poll(WAIT, "/mode did not switch to MANUAL", |s| {
            s.bottom_row().contains("MANUAL").then_some(())
        });
        // Down picks the second match and Enter runs it.
        s.type_text("/mo");
        s.wait_for("┌ commands");
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("┌ Models");
        s.wait_for("fake-coder");
        s.wait_gone("┌ commands");
        let picked = s.contents();
        // ← closes the picker, like Esc.
        s.send("\x1b[D");
        s.wait_gone("┌ Models");
        picked
    })
    .await;
    assert!(ended.out.contains("/models"), "{}", ended.out);
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_pairs_this_device_and_the_session_chats_at_once() {
    let fake = Fake::start().await;
    fake.pair_after(1);
    fake.script([Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("/help commands");
        s.type_text("/login");
        s.send("\r");
        s.wait_for("Log in to palepale");
        s.send("1");
        s.wait_for("012345");
        s.wait_for("Logged in with device key …9999");
        // The key is live in this session: no restart needed.
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    assert!(ended.screen.contains("$12.30"), "{}", ended.screen);
    assert!(ended.sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_takes_a_masked_account_number_and_retries_a_wrong_one() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("/help commands");
        s.type_text("/login");
        s.send("\r");
        s.wait_for("Log in to palepale");
        s.send("2");
        s.type_text("0000 0000 0000 0000");
        s.send("\r");
        s.wait_for("invalid_api_key");
        s.send("2");
        s.type_text("1234 5678 9012 3456");
        s.wait_for("•••• •••• •••• 3456");
        assert!(!s.contents().contains("1234 5678"), "{}", s.contents());
        s.send("\r");
        s.wait_for("Logged in with device key …3456");
    })
    .await;
    assert!(!ended.screen.contains("1234 5678"), "{}", ended.screen);
    assert!(ended.sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_takes_the_same_arguments_as_the_command_line() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("/help commands");
        s.type_text("/login --status");
        s.send("\r");
        s.wait_for("Not logged in. Run /login");
        s.type_text("/login --key 1234 5678 9012 3456");
        s.send("\r");
        s.wait_for("Logged in with device key …3456");
    })
    .await;
    assert!(ended.sandbox.home().join("auth.json").exists());
}

/// A forum store holding one guest forum on the fake server, sealed the way a non-Windows
/// machine seals it.
fn seed_forum(sandbox: &Sandbox, fake_url: &str) {
    use base64::Engine;
    let forums = serde_json::json!([{
        "id": "1",
        "name": "Fake Forum",
        "hint": "A stand-in forum for tests.",
        "base_url": format!("{fake_url}{FORUM_PATH}"),
        "kind": "mobiquo",
    }]);
    let blob = base64::engine::general_purpose::STANDARD.encode(forums.to_string());
    let sealed = serde_json::json!({"v": 1, "scheme": "plain", "blob": blob});
    std::fs::write(sandbox.home().join("forums.json"), sealed.to_string()).unwrap();
}

/// Every file under `dir`, read as text where it is text.
fn all_text(dir: &std::path::Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&all_text(&path));
        } else {
            out.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap()));
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_forums_logs_in_with_a_hidden_password_and_retries_a_wrong_one() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    seed_forum(&sandbox, &fake.url);
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("/forums");
        s.send("\r");
        s.wait_for("┌ Forums");
        s.wait_for("Fake Forum");
        s.send("\r");
        s.wait_for("1 Log in");
        s.send("1");
        s.wait_for("Log in to Fake Forum");
        s.type_text("demo");
        s.send("\r");
        s.type_text("wrong");
        s.send("\r");
        s.wait_for("Wrong password");
        // Back at the name, still typed: Enter, then the right password.
        s.send("\r");
        s.type_text(FORUM_PASSWORD);
        s.wait_for("•••••••");
        assert!(!s.contents().contains(FORUM_PASSWORD), "{}", s.contents());
        s.send("\r");
        s.wait_for("Logged in to Fake Forum as demo.");
        s.wait_for("logged in as demo");
        s.send("\x1b");
        s.wait_gone("┌ Forums");
    })
    .await;
    assert!(!ended.screen.contains(FORUM_PASSWORD), "{}", ended.screen);
    let home = all_text(ended.sandbox.home());
    assert!(
        !home.contains(FORUM_PASSWORD),
        "the password reached the disk"
    );
    let forums = std::fs::read_to_string(ended.sandbox.home().join("forums.json")).unwrap();
    let sealed: serde_json::Value = serde_json::from_str(&forums).unwrap();
    // A DPAPI blob is opaque to the test; `io::secret`'s unit test covers its round trip.
    if cfg!(windows) {
        assert_eq!(sealed["scheme"], "dpapi");
        return;
    }
    let opened = {
        use base64::Engine;
        let blob = sealed["blob"].as_str().unwrap();
        base64::engine::general_purpose::STANDARD
            .decode(blob)
            .unwrap()
    };
    let opened = String::from_utf8(opened).unwrap();
    assert!(opened.contains("bbsessionhash=fake"), "{opened}");
    assert!(opened.contains("\"username\":\"demo\""), "{opened}");
    assert!(!opened.contains(FORUM_PASSWORD), "{opened}");
}

// Elsewhere a system clipboard is always there, and a test must not overwrite the developer's;
// here DISPLAY is cleared, so the copy goes out as OSC 52, which the test can read.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn right_click_copies_a_reply_and_says_so_at_the_top_right() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        let row = s.row_of("Hello there!");
        s.right_click(row);
        s.poll(WAIT, "no copied note", |s| {
            s.row(0).contains("✓ Copied").then_some(())
        });
        // "Hello there!" in base64, as OSC 52 carries it.
        let osc = String::from_utf8_lossy(&s.written).contains("\x1b]52;c;SGVsbG8gdGhlcmUh\x07");
        assert!(osc, "no OSC 52 with the reply");
        s.poll(WAIT, "the copied note stayed", |s| {
            (!s.row(0).contains("✓ Copied")).then_some(())
        });
    })
    .await;
}

// Linux only, as above.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn slash_copy_copies_the_last_reply() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("/copy");
        s.send("\r");
        s.wait_for("Nothing to copy yet");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        s.type_text("/copy");
        s.send("\r");
        s.poll(WAIT, "no copied note", |s| {
            s.row(0).contains("✓ Copied").then_some(())
        });
        // "Hello there!" in base64.
        let osc = String::from_utf8_lossy(&s.written).contains("\x1b]52;c;SGVsbG8gdGhlcmUh\x07");
        assert!(osc, "no OSC 52 with the reply");
    })
    .await;
}

// Linux only, as above.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn right_click_copies_the_pairing_code_from_its_card() {
    let fake = Fake::start().await;
    fake.pair_after(100);
    in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("/help commands");
        s.type_text("/login");
        s.send("\r");
        s.wait_for("Log in to palepale");
        s.send("1");
        s.wait_for("right-click copies it");
        s.right_click(2);
        s.poll(WAIT, "no copied note", |s| {
            s.row(0).contains("✓ Copied").then_some(())
        });
        // "012345" in base64.
        let osc = String::from_utf8_lossy(&s.written).contains("\x1b]52;c;MDEyMzQ1\x07");
        assert!(osc, "no OSC 52 with the code");
        s.send("\x1b");
        s.wait_gone("Log in to palepale");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn thinking_collapses_opens_on_click_and_ctrl_t_toggles_it_all() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        s.wait_for("▸ Thought for");
        assert!(!s.contents().contains("The user greets me."));
        s.send("\x14");
        s.wait_for("The user greets me.");
        s.send("\x14");
        s.wait_gone("The user greets me.");
        let row = s.row_of("▸ Thought for");
        s.click(row);
        s.wait_for("The user greets me.");
        s.click(row);
        s.wait_gone("The user greets me.");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_context_share_uses_the_selected_models_latest_request() {
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("pty/long_context.sse"),
        Reply::sse("pty/long_context.sse"),
    ]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Read it all.");
        // 100k tokens: a tenth of demo-2's million, three quarters of fake-coder's 131k.
        s.poll(WAIT, "ctx 10% on demo-2", |s| {
            s.bottom_row().contains("ctx 10%").then_some(())
        });
        // Wait for buffered input to be consumed before Enter, which otherwise
        // joins the paste burst on a busy runner and inserts a newline.
        s.send("/models");
        s.wait_for("› /models");
        s.wait_quiet();
        s.send("\r");
        s.wait_for("┌ Models");
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("Model: fake-coder. Saved as your default.");
        s.poll(WAIT, "old-model context cleared", |s| {
            (!s.bottom_row().contains("ctx ")).then_some(())
        });
        s.type_text("hi again");
        s.send("\r");
        s.poll(WAIT, "ctx 76% on fake-coder", |s| {
            s.bottom_row().contains("ctx 76%").then_some(())
        });
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_clear_starts_a_fresh_conversation_in_a_new_file() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("first question");
        s.send("\r");
        s.wait_for("Hello there!");
        s.type_text("/clear");
        s.send("\r");
        s.wait_for("The last one is saved in");
        assert!(!s.contents().contains("first question"), "{}", s.contents());
        s.type_text("second question");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    let second = fake.chat_bodies()[1]["messages"].to_string();
    assert!(second.contains("second question"), "{second}");
    assert!(!second.contains("first question"), "{second}");
    assert_eq!(ended.sandbox.sessions().len(), 2);
    assert_eq!(
        ended.exited.matches("Session saved").count(),
        2,
        "{}",
        ended.exited
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_status_refreshes_account_balance_with_local_model_and_mode() {
    let fake = Fake::start().await;
    fake.overview_delays([Duration::ZERO, Duration::from_millis(500)]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("/status");
        s.send("\r");
        s.wait_for("┌ Status");
        s.wait_for("Available: $12.30");
        let screen = s.contents();
        assert!(screen.contains("Model: demo-2"), "{screen}");
        assert!(screen.contains("Mode: AUTO"), "{screen}");
        assert!(screen.contains("not used yet"), "{screen}");
        assert!(
            !screen.contains("Session file:")
                && !screen.contains("Project:")
                && !screen.contains("Version:"),
            "{screen}"
        );
        s.capture("status-session");
        s.send("\t");
        s.wait_for("Account: …3456");
        s.wait_for("all time");
        s.wait_for("Charged: $3.00");
        s.wait_for("Requests: 42");
        s.wait_for("50%");
        let screen = s.contents();
        let input = screen.lines().find(|row| row.contains("│ Input ")).unwrap();
        let output = screen
            .lines()
            .find(|row| row.contains("│ Output "))
            .unwrap();
        assert!(input.contains("12K") && output.contains("3K"), "{screen}");
        assert_eq!(input.matches('█').count(), 24, "{screen}");
        assert_eq!(output.matches('█').count(), 6, "{screen}");
        assert!(screen.contains("Cached: 6K (50%)"), "{screen}");
        assert!(screen.contains("Reasoning: 1K"), "{screen}");
        s.capture("status-usage");
        s.send("r");
        s.wait_for("Refreshing…");
        s.wait_gone("Refreshing…");
        s.send("\x1b");
        s.wait_gone("┌ Status");
    })
    .await;
    assert_eq!(fake.hits("/v1/account/summary"), 2);
    assert_eq!(fake.hits("/v1/account/usage/summary"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn status_loads_without_blocking_input_and_does_not_reopen_after_dismissal() {
    let fake = Fake::start().await;
    fake.overview_delays([Duration::from_millis(600)]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.type_text("/status");
        s.send("\r");
        s.wait_for("┌ Status");
        s.wait_for("Refreshing…");
        s.send("\x1b[Z");
        s.wait_for("Mode: MANUAL");
        s.send("\x1b");
        s.wait_gone("┌ Status");
        s.type_text("still typing");
        s.wait_for("still typing");
        std::thread::sleep(Duration::from_millis(700));
        s.pump();
        assert!(!s.contents().contains("┌ Status"), "{}", s.contents());
        s.send("\x15");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_account_usage_keeps_the_balance_without_inventing_zero_usage() {
    let fake = Fake::start().await;
    fake.overview_errors([503]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.type_text("/status");
        s.send("\r");
        s.wait_for("Available: $12.30");
        s.send("\t");
        s.wait_for("Usage unavailable:");
        assert!(!s.contents().contains("Charged: $0.00"));
        s.send("\x1b");
        s.wait_gone("┌ Status");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_godmode_selects_server_profile_and_clears_it() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("persona-hello.sse"), Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.type_text("/godmode");
        s.send("\r");
        s.wait_for("┌ Coding profile");
        s.wait_for("jb-1  (GLM 5.3)");
        s.wait_for("CINDER");
        s.wait_for("XYRO");
        s.send("\x1b[B");
        s.wait_for("│ › jb-1  (GLM 5.3)");
        s.send("\x1b[B");
        s.wait_for("│ › CINDER");
        s.send("\r");
        s.wait_for("Coding profile: jb-2");
        s.type_text("first");
        s.send("\r");
        s.wait_for("Hello there!");
        assert!(!s.contents().contains("[Six]"));
        s.type_text("/godmode off");
        s.send("\r");
        s.wait_for("Coding profile: off");
        s.type_text("second");
        s.send("\r");
        s.poll(WAIT, "second response", |s| {
            (s.contents().matches("Hello there!").count() == 2).then_some(())
        });
    })
    .await;
    assert_eq!(fake.hits("/v1/agent-profiles"), 1);
    let requests = fake.chat_bodies();
    assert_eq!(requests[0]["server_profile"], "jb-2");
    assert_eq!(
        fake.profile_queries(),
        vec![format!("model={}", requests[0]["model"].as_str().unwrap())]
    );
    assert!(requests[0].get("agent_profile").is_none());
    assert!(requests[1].get("server_profile").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_godmode_replaces_server_profile_and_off_restores_default() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    std::fs::create_dir_all(sandbox.home().join("prompts")).unwrap();
    std::fs::write(
        sandbox.home().join("prompts/godmode-custom.md"),
        "CUSTOM_SYSTEM_MARKER",
    )
    .unwrap();
    in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.type_text("/godmode jb-2");
        s.send("\r");
        s.wait_for("Coding profile: jb-2");
        s.type_text("/godmode custom apply");
        s.send("\r");
        s.wait_for("Custom system prompt applied");
        s.type_text("/godmode status");
        s.send("\r");
        s.wait_for("Coding profile: Custom");
        s.type_text("first");
        s.send("\r");
        s.wait_for("Hello there!");
        s.type_text("/godmode off");
        s.send("\r");
        s.wait_for("Coding profile: off");
        s.type_text("second");
        s.send("\r");
        s.poll(WAIT, "second response", |s| {
            (s.contents().matches("Hello there!").count() == 2).then_some(())
        });
    })
    .await;
    let requests = fake.chat_bodies();
    assert!(
        requests[0]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("CUSTOM_SYSTEM_MARKER")
    );
    assert!(
        !requests[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("CUSTOM_SYSTEM_MARKER")
    );
    for request in requests {
        assert!(request.get("server_profile").is_none());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_godmode_refetches_edited_admin_labels() {
    let fake = Fake::start().await;
    fake.agent_profile_lists([
        serde_json::json!({"data":[{"id":"jb-1","name":"jb-1","description":"GLM 5.3"}]}),
        serde_json::json!({"data":[{"id":"jb-1","name":"GLM","description":"Updated in admin"}]}),
    ]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.type_text("/godmode");
        s.send("\r");
        s.wait_for("jb-1  (GLM 5.3)");
        s.send("\x1b");
        s.wait_gone("┌ Coding profile");
        s.type_text("/godmode");
        s.send("\r");
        s.wait_for("GLM  (Updated in admin)");
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("Coding profile: jb-1");
    })
    .await;
    assert_eq!(fake.hits("/v1/agent-profiles"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_godmode_syncs_created_deleted_and_empty_profiles() {
    let fake = Fake::start().await;
    fake.agent_profile_lists([
        serde_json::json!({"data":[{"id":"jb-4","name":"Custom","description":"From admin"}]}),
        serde_json::json!({"data":[]}),
    ]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.type_text("/godmode");
        s.send("\r");
        s.wait_for("Custom  (From admin)");
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("Coding profile: jb-4");
        s.type_text("/godmode");
        s.send("\r");
        s.wait_for("┌ Coding profile");
        assert!(!s.contents().contains("Custom  (From admin)"));
        s.send("\r");
        s.wait_for("Coding profile: off");
    })
    .await;
    assert_eq!(fake.hits("/v1/agent-profiles"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn choosing_a_model_keeps_refresh_responsive_and_uses_server_reasoning() {
    let fake = Fake::start().await;
    fake.model_delays([Duration::ZERO, Duration::from_secs(2)]);
    fake.script([Reply::sse("hello.sse")]);
    let requests = fake.clone();
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), move |s| {
        s.wait_for("AUTO");
        s.type_text("/model");
        s.send("\r");
        s.poll(WAIT, "catalog refresh started", |_| {
            (requests.hits("/v1/models") == 2).then_some(())
        });
        s.send("\x1b[Z");
        s.wait_for("MANUAL");
        assert!(!s.contents().contains("┌ Models"), "{}", s.contents());
        s.wait_for("┌ Models");
        s.send("\r");
        s.wait_gone("┌ Models");
        s.wait_for("┌ Reasoning effort");
        s.send("\x1b");
        s.wait_gone("┌ Reasoning effort");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    assert_eq!(fake.chat_bodies()[0]["model"], "demo-2");
    assert!(fake.chat_bodies()[0].get("reasoning_effort").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn model_effort_choice_reaches_the_gateway_and_survives_resume() {
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("hello.sse"),
        Reply::sse("hello.sse"),
        Reply::sse("pty/long_context.sse"),
    ]);
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.type_text("/models");
        s.send("\r");
        s.wait_for("┌ Models");
        s.send("\r");
        s.wait_for("┌ Reasoning effort");
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("Reasoning effort: high.");
        s.poll(WAIT, "selected effort in footer", |s| {
            s.bottom_row()
                .contains("demo-2 · effort high")
                .then_some(())
        });
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    assert_eq!(fake.chat_bodies()[0]["reasoning_effort"], "high");
    ended
        .sandbox
        .run(&fake.url, &["--continue", "-p", "again"], crate::WITH_KEY)
        .await
        .success();
    assert_eq!(fake.chat_bodies()[1]["reasoning_effort"], "high");
    let bodies = fake.chat_bodies();
    let assistant = bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("resumed conversation includes the assistant reply");
    assert!(assistant.get("reasoning_content").is_none(), "{assistant}");
    in_session(&fake, ended.sandbox, Some(ENV_KEY), |s| {
        s.type_text("/sessions");
        s.send("\r");
        s.wait_for("Saved sessions");
        s.send("\r");
        s.wait_gone("Saved sessions");
        s.type_text("/models");
        s.send("\r");
        s.wait_for("┌ Models");
        s.send("\r");
        s.wait_for("┌ Reasoning effort");
        s.wait_for("› high");
        assert!(s.bottom_row().contains("effort high"));
        s.send("\x1b");
        s.wait_gone("┌ Reasoning effort");
        s.type_text("/models");
        s.send("\r");
        s.wait_for("┌ Models");
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("Model: fake-coder.");
        assert!(!s.contents().contains("┌ Reasoning effort"));
        assert!(!s.bottom_row().contains("effort high"));
        s.type_text("use this model");
        s.send("\r");
        s.wait_for("use this model");
        s.wait_for("Read it all.");
    })
    .await;
    assert_eq!(fake.chat_bodies()[2]["model"], "fake-coder");
    assert!(fake.chat_bodies()[2].get("reasoning_effort").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_completed_turn_syncs_context_window_and_keeps_cached_list() {
    let fake = Fake::start().await;
    let first: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    let mut changed = first.clone();
    changed["data"][0]["context_window"] = serde_json::json!(200_000);
    // A malformed response falls back; a valid empty catalog now clears the list.
    fake.model_lists([first, changed, serde_json::json!({"error":"offline"})]);
    fake.model_delays([Duration::ZERO, Duration::from_millis(400)]);
    fake.script([Reply::sse("pty/long_context.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Read it all.");
        s.poll(WAIT, "ctx 10% before refresh", |s| {
            s.bottom_row().contains("ctx 10%").then_some(())
        });
        s.poll(WAIT, "ctx 50% after background sync", |s| {
            s.bottom_row().contains("ctx 50%").then_some(())
        });
        s.type_text("/models");
        s.send("\r");
        s.wait_for("┌ Models");
        s.wait_for("Cached list");
        s.wait_for("Could not reach palepale");
        s.wait_for("demo-2");
        s.send("\x1b");
        s.wait_gone("┌ Models");
        s.poll(WAIT, "cached window stays current", |s| {
            s.bottom_row().contains("ctx 50%").then_some(())
        });
    })
    .await;
    assert_eq!(fake.hits("/v1/models"), 3);
    assert!(fake.chat_bodies()[0].get("reasoning_effort").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_init_asks_for_an_agents_md_that_the_next_conversation_reads() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let agents = sandbox.project.path().join("AGENTS.md");
    in_session(&fake, sandbox, Some(ENV_KEY), move |s| {
        s.wait_for("AUTO");
        s.type_text("/init");
        s.send("\r");
        s.wait_for("Hello there!");
        // What the model would have written, then a conversation that starts after it.
        std::fs::write(&agents, "Run make test before a push.").unwrap();
        s.type_text("/new");
        s.send("\r");
        s.wait_for("A new conversation");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    let bodies = fake.chat_bodies();
    let asked = bodies[0]["messages"].to_string();
    assert!(asked.contains("Write an AGENTS.md"), "{asked}");
    let current = bodies[1]["messages"].to_string();
    assert!(
        current.contains("Run make test before a push."),
        "{current}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_models_questions_are_answered_on_one_card() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("pty/questions.sse"), Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("set it up");
        s.send("\r");
        s.wait_for("Which database should the app use?");
        s.wait_for("what production runs");
        s.send("2");
        s.wait_for("Which checks run before a push?");
        s.send("1");
        s.wait_for("[x] lint");
        s.send("3");
        s.wait_for("[x] e2e");
        s.send("\r");
        s.wait_for("Your answers");
        s.wait_for("lint, e2e");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    let answered = fake.chat_bodies()[1]["messages"].to_string();
    assert!(answered.contains("Postgres"), "{answered}");
    assert!(answered.contains("lint, e2e"), "{answered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_models_filters_uncensored_then_selects_the_cheapest_model() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let mut listing: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    listing["data"][0]["presentation"] =
        serde_json::json!({"name":"Uncensored fixture","badges":["Uncensored","Zero Refusal"]});
    fake.model_lists([listing.clone(), listing]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("/models");
        s.send("\r");
        s.wait_for("[All]");
        s.send("\t");
        s.wait_for("[Uncensored]");
        assert!(!s.contents().contains("fake-coder"), "{}", s.contents());
        s.send("\t");
        s.wait_for("[Cheapest]");
        s.send("\r");
        s.wait_for("Model: fake-coder. Saved as your default.");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    assert_eq!(fake.chat_bodies()[0]["model"], "fake-coder");
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_models_picks_the_model_for_the_rest_of_the_session() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let prefs_path = sandbox.home().join("prefs.json");
    in_session(&fake, sandbox, Some(ENV_KEY), move |s| {
        s.wait_for("AUTO");
        s.type_text("/models");
        s.send("\r");
        s.wait_for("┌ Models");
        // The list opens on the model in use; the next one down is picked with Enter.
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("Model: fake-coder. Saved as your default.");
        let prefs: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&prefs_path).unwrap()).unwrap();
        assert_eq!(prefs["model"], "fake-coder");
        assert!(s.bottom_row().contains("fake-coder"), "{}", s.contents());
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    assert_eq!(fake.chat_bodies()[0]["model"], "fake-coder");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_command_popup_floats_over_the_conversation_without_moving_it() {
    let fake = Fake::start().await;
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        // More output than the screen holds, so the conversation fills every row above the bar.
        for _ in 0..3 {
            s.type_text("/help");
            s.send("\r");
            s.wait_gone("┌ commands");
        }
        s.wait_quiet();
        let top: Vec<String> = (0..8).map(|i| s.row(i)).collect();
        s.send("/");
        s.wait_for("┌ commands");
        let with_popup: Vec<String> = (0..8).map(|i| s.row(i)).collect();
        assert_eq!(
            with_popup,
            top,
            "the conversation moved; screen:\n{}",
            s.contents()
        );
        s.send("\x7f");
        s.wait_gone("┌ commands");
        let after: Vec<String> = (0..8).map(|i| s.row(i)).collect();
        assert_eq!(
            after,
            top,
            "the conversation moved; screen:\n{}",
            s.contents()
        );
    })
    .await;
}
// Runs the shipped interaction loop without any backend endpoint.
#[tokio::test(flavor = "multi_thread")]
async fn disconnected_session_keeps_local_commands_and_restores_the_terminal() {
    tokio::task::spawn_blocking(move || {
        let sandbox = Sandbox::default();
        let mut s = Session::start_sized(&sandbox, "", None, &[], 40, 100, false);
        s.wait_for("/help commands");
        s.wait_for("Backend not configured");
        s.wait_quiet();
        assert!(s.row(0).contains("palepale"));
        s.wait_for_logo();
        s.capture("welcome");
        if let Ok(path) = std::env::var("PALEPALE_CAPTURE_PATH") {
            std::fs::write(&path, s.contents()).unwrap();
            std::fs::write(
                std::path::Path::new(&path).with_extension("ansi"),
                s.screen.screen().contents_formatted(),
            )
            .unwrap();
        }
        s.type_text("/status");
        s.send("\r");
        s.wait_for("Offline");
        assert!(!s.contents().contains("Balance"));
        s.send("\x1b");
        s.wait_gone("┌ Status");
        s.type_text("hello");
        s.send("\r");
        s.wait_for("PALEPALE_API_URL to connect");
        assert!(sandbox.sessions().is_empty());
        s.type_text("/new");
        s.send("\r");
        s.wait_for("/help commands");
        s.send("\x04");
        assert!(s.wait_exit());
        let screen = s.screen.screen();
        assert!(!screen.alternate_screen() && !screen.bracketed_paste() && !screen.hide_cursor());
        assert!(sandbox.sessions().is_empty());
        assert!(!sandbox.home().join("auth.json").exists());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn narrow_disconnected_session_keeps_the_editor_and_quits() {
    tokio::task::spawn_blocking(move || {
        let sandbox = Sandbox::default();
        let mut s = Session::start_sized(&sandbox, "", None, &[], 20, 40, false);
        s.wait_for("palepale");
        s.wait_for("Write a message");
        assert!(s.row(0).contains("palepale"));
        s.type_text("/status");
        s.send("\r");
        s.wait_for("Offline");
        s.send("\x04");
        assert!(s.wait_exit());
    })
    .await
    .unwrap();
}
