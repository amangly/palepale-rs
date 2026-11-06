use crate::fake_palepale::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};

#[tokio::test(flavor = "multi_thread")]
async fn decision_follows_permission_mode_and_keeps_chat_model_and_budget() {
    for mode in ["auto", "manual"] {
        let sandbox = Sandbox::default();
        let fake = Fake::start().await;
        fake.script([Reply::sse("decision.sse"), Reply::sse("hello.sse")]);
        sandbox
            .run(
                &fake.url,
                &[
                    "--mode",
                    mode,
                    "--budget",
                    "2",
                    "--print",
                    "Review this diff",
                ],
                WITH_KEY,
            )
            .await
            .success();
        let bodies = fake.chat_bodies();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["model"], bodies[1]["model"]);
        assert!(
            bodies[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["function"]["name"] == "decision")
        );
        let messages = bodies[1]["messages"].to_string();
        if mode == "auto" {
            assert_eq!(fake.hits("/v1/decisions"), 1);
            let requests = fake.decision_bodies();
            assert_eq!(requests[0]["model"], "jev-latest");
            assert_eq!(
                requests[0]["budget_id"],
                "44444444-4444-4444-8444-444444444444"
            );
            assert!(messages.contains("review: review"));
            assert!(messages.contains("346 input, 31 output tokens"));
        } else {
            assert_eq!(fake.hits("/v1/decisions"), 0);
            assert!(messages.contains("needs the user's permission"));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_decision_is_not_retried() {
    let sandbox = Sandbox::default();
    let fake = Fake::start().await;
    fake.fail_decision();
    fake.script([Reply::sse("decision.sse"), Reply::sse("hello.sse")]);
    sandbox
        .run(
            &fake.url,
            &["--mode", "auto", "--print", "Review this diff"],
            WITH_KEY,
        )
        .await
        .success();
    assert_eq!(fake.hits("/v1/decisions"), 1);
    assert_eq!(fake.chat_bodies().len(), 2);
    assert!(
        fake.chat_bodies()[1]["messages"]
            .to_string()
            .contains("pending_usage")
    );
}
