use crate::fake_palepale::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};
#[tokio::test(flavor = "multi_thread")]
async fn decision_keeps_chat_model_and_requires_confirmation_before_dispatch() {
    let sandbox = Sandbox::default();
    let fake = Fake::start().await;
    fake.script([Reply::sse("decision.sse"), Reply::sse("hello.sse")]);
    sandbox
        .run(
            &fake.url,
            &["--print", "Review this diff using the decision tool"],
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
    assert_eq!(fake.hits("/v1/decisions"), 0);
    assert!(
        bodies[1]["messages"]
            .to_string()
            .contains("needs direct confirmation")
    );
}
