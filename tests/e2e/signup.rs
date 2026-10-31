use crate::fake_palepale::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};

#[tokio::test(flavor = "multi_thread")]
async fn signup_saves_owner_and_client_credentials_separately() {
    let fake = Fake::start().await;

    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["signup"], None).await;
    let out = run.success().stdout();
    assert!(out.contains("Account created"), "{out}");
    assert!(!out.contains(crate::fake_palepale::OWNER_KEY));
    assert!(!out.contains(crate::fake_palepale::CLIENT_KEY));
    assert!(sandbox.home().join("owner.json").exists());

    // Signup and key issuance are separate, single requests.
    assert_eq!(fake.hits("/v1/accounts"), 1);
    assert_eq!(fake.hits("/v1/account/keys"), 1);

    fake.script([Reply::sse("hello.sse")]);
    let chat = sandbox.run(&fake.url, &["-p", "hi"], None).await;
    assert_eq!(chat.success().stdout(), "Hello there!\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_key_neither_blocks_signup_nor_hides_what_it_shadows() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let note = "PALEPALE_API_KEY is set in this environment";

    let signup = sandbox.run(&fake.url, &["signup"], WITH_KEY).await;
    assert!(
        signup.success().stdout().contains(note),
        "{}",
        signup.stdout()
    );
    assert!(sandbox.home().join("auth.json").exists());

    let args = ["login", "--key", "1234 5678 9012 3456"];
    let login = sandbox.run(&fake.url, &args, WITH_KEY).await;
    assert!(
        login.success().stdout().contains(note),
        "{}",
        login.stdout()
    );

    let logout = sandbox.run(&fake.url, &["logout"], WITH_KEY).await;
    assert!(
        logout.success().stdout().contains(note),
        "{}",
        logout.stdout()
    );
    let quiet = sandbox.run(&fake.url, &["logout"], None).await;
    assert!(!quiet.success().stdout().contains(note));
}

#[tokio::test(flavor = "multi_thread")]
async fn signup_refuses_to_replace_an_existing_key() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox
        .run(&fake.url, &["login", "--key", "1234567890123456"], None)
        .await
        .success();

    let run = sandbox.run(&fake.url, &["signup"], None).await;
    assert_eq!(run.code(), Some(1));
    assert!(
        run.stderr().contains("already logged in"),
        "{}",
        run.stderr()
    );
    assert_eq!(fake.hits("/v1/accounts"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_key_issuance_retains_owner_and_recovers_without_another_signup() {
    let fake = Fake::start().await;
    fake.key_failures(1);
    let sandbox = Sandbox::default();
    let run = sandbox.run(&fake.url, &["signup"], None).await;
    assert_ne!(run.code(), Some(0));
    assert!(run.stderr().contains("owner credential was saved"));
    assert!(sandbox.home().join("owner.json").exists());
    assert!(!sandbox.home().join("auth.json").exists());
    assert_eq!(fake.hits("/v1/account/keys"), 1);
    let refused = sandbox.run(&fake.url, &["signup", "--force"], None).await;
    assert_ne!(refused.code(), Some(0));
    assert_eq!(fake.hits("/v1/accounts"), 1);
    sandbox
        .run(&fake.url, &["keys", "create", "recovered", "--use"], None)
        .await
        .success();
    fake.script([Reply::sse("hello.sse")]);
    assert_eq!(
        sandbox
            .run(&fake.url, &["-p", "hi"], None)
            .await
            .success()
            .stdout(),
        "Hello there!\n"
    );
    sandbox.run(&fake.url, &["logout"], None).await.success();
    assert!(sandbox.home().join("owner.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_cannot_be_adopted_as_an_inference_key() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(
            &fake.url,
            &["login", "--key", crate::fake_palepale::OWNER_KEY],
            None,
        )
        .await;
    assert_ne!(run.code(), Some(0));
    assert!(run.stderr().contains("inference key"));
    assert!(!run.stderr().contains(crate::fake_palepale::OWNER_KEY));
    assert_eq!(fake.hits("/v1/account/summary"), 0);
    assert!(!sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_environment_key_is_never_sent_to_inference() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(
            &fake.url,
            &["-p", "hi"],
            Some(crate::fake_palepale::OWNER_KEY),
        )
        .await;
    assert_ne!(run.code(), Some(0));
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
    assert!(!run.stderr().contains(crate::fake_palepale::OWNER_KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_export_is_explicit_private_and_never_overwrites_a_file() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.run(&fake.url, &["signup"], None).await.success();
    let path = sandbox.home().join("owner-export.txt");
    let destination = path.to_str().unwrap();
    let refused = sandbox
        .run(&fake.url, &["keys", "export-owner", destination], None)
        .await;
    assert_ne!(refused.code(), Some(0));
    assert!(!path.exists());
    let exported = sandbox
        .run(
            &fake.url,
            &["keys", "export-owner", destination, "--yes"],
            None,
        )
        .await;
    assert!(
        !exported
            .success()
            .stdout()
            .contains(crate::fake_palepale::OWNER_KEY)
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        crate::fake_palepale::OWNER_KEY
    );
    std::fs::write(&path, "keep existing data").unwrap();
    let refused = sandbox
        .run(
            &fake.url,
            &["keys", "export-owner", destination, "--yes"],
            None,
        )
        .await;
    assert_ne!(refused.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "keep existing data"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn trial_commands_start_a_cli_bound_browser_flow_without_printing_secrets() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.run(&fake.url, &["signup"], None).await.success();
    for action in ["request", "redeem"] {
        let code = format!("pt_{}\n", "x".repeat(43));
        let output = sandbox
            .run_input(
                &fake.url,
                &["trial", action],
                None,
                if action == "redeem" {
                    Some(&code)
                } else {
                    None
                },
            )
            .await;
        let text = output.success().stdout();
        assert!(text.contains("verification in the browser"), "{text}");
        assert!(!text.contains("pt_"));
        if action == "redeem" {
            assert!(text.contains("$2 trial credit redeemed"), "{text}");
        }
        assert!(!text.contains(crate::fake_palepale::OWNER_KEY));
        assert!(!text.contains("bh_"));
        assert!(!text.contains(&"n".repeat(43)));
    }
    assert_eq!(fake.hits("/v1/trial/flows"), 2);
    assert_eq!(fake.hits("/v1/trial/redeem"), 2);
    assert_eq!(fake.hits("/v1/account/browser-handoffs"), 2);
    let status = sandbox.run(&fake.url, &["trial"], None).await;
    assert!(status.success().stdout().contains("1999000"));
}

#[tokio::test(flavor = "multi_thread")]
async fn trial_flow_opens_verification_without_local_owner_access() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let output = sandbox
        .run(&fake.url, &["trial", "request"], WITH_KEY)
        .await;
    assert!(
        output
            .success()
            .stdout()
            .contains("verification in the browser")
    );
    assert_eq!(fake.hits("/v1/account/browser-handoffs"), 0);
    sandbox.run(&fake.url, &["signup"], None).await.success();
    fake.fail_handoff();
    let output = sandbox.run(&fake.url, &["trial", "request"], None).await;
    assert!(
        output
            .success()
            .stdout()
            .contains("verification in the browser")
    );
    assert_eq!(fake.hits("/v1/account/browser-handoffs"), 1);
    assert!(!output.stdout().contains("bh_"));
}
