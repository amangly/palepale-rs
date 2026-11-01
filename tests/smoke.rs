//! Opt-in checks against an explicitly configured backend. Never part of CI.
//! Requires PALEPALE_SMOKE_URL, PALEPALE_SMOKE_KEY and PALEPALE_SMOKE_MODEL.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

use std::process::{Command, Output, Stdio};

fn model() -> String {
    std::env::var("PALEPALE_SMOKE_MODEL").expect("set PALEPALE_SMOKE_MODEL")
}
/// Upper bound on what one smoke request may cost, in micro-dollars.
const MAX_COST_MICROS: u64 = 1000;

fn palepale(args: &[&str]) -> Output {
    let key = std::env::var("PALEPALE_SMOKE_KEY").expect("set PALEPALE_SMOKE_KEY");
    let url = std::env::var("PALEPALE_SMOKE_URL").expect("set PALEPALE_SMOKE_URL");
    assert!(
        !url.trim().is_empty(),
        "PALEPALE_SMOKE_URL must be explicit"
    );
    let home = tempfile::tempdir().unwrap();
    // An empty project: the live model sees neither this checkout nor its .palepale config.
    let project = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_palepale"))
        .args(args)
        .current_dir(project.path())
        .env("PALEPALE_HOME", home.path())
        .env("PALEPALE_API_KEY", key)
        .env("PALEPALE_API_URL", url)
        .env_remove("PALEPALE_CHAT_URL")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
#[ignore = "spends real money; run with `make smoke`"]
fn live_models_include_the_smoke_model() {
    let out = palepale(&["models", "--refresh"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("Live list"), "{text}");
    assert!(text.contains(&model()), "{text}");
}

#[test]
#[ignore = "spends real money; run with `make smoke`"]
fn a_real_round_trip_is_cheap_and_well_formed() {
    let out = palepale(&[
        "--model",
        &model(),
        "--json",
        "Reply with exactly the word PONG and nothing else.",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let end: serde_json::Value = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|e| e["type"] == "message_end")
        .expect("a message_end event");
    let message = &end["message"];
    let text: String = message["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["type"] == "text")
        .filter_map(|p| p["text"].as_str())
        .collect();
    assert!(text.contains("PONG"), "{text}");
    assert_eq!(message["finish"], "done");
    assert!(message["request_id"].is_string(), "{message}");
    let cost = message["usage"]["cost_micros"].as_u64().unwrap();
    assert!(cost > 0 && cost < MAX_COST_MICROS, "cost {cost}");
}
