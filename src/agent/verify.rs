//! Configured checks use normal bash calls so permissions and session replay still apply.
use crate::{
    agent::{
        message::{Part, ToolCall, ToolResult},
        model_text,
    },
    limits,
    settings::Settings,
    text::template::fill,
};

pub fn call(
    settings: &Settings,
    changed: bool,
    top_level: bool,
    attempts: u32,
) -> Option<ToolCall> {
    let command = settings
        .verify
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())?;
    if !changed || !top_level || attempts >= limits::get().agent.verify_attempts.value {
        return None;
    }
    Some(ToolCall {
        id: format!("verify-{}", crate::session::entry::new_id()),
        name: "bash".into(),
        arguments: serde_json::json!({"command": command, "timeout": limits::get().agent.verify_timeout_secs.value}).to_string(),
    })
}

pub fn failure(command: &str, result: &ToolResult) -> Option<Vec<Part>> {
    // Refusals, cancellation, signals and timeouts are not failed test results.
    if result.refused.is_some()
        || !result.is_error
        || !result
            .details
            .as_ref()?
            .get("exit_code")?
            .as_i64()
            .is_some_and(|code| code != 0)
    {
        return None;
    }
    Some(vec![Part::Reminder {
        text: fill(
            &model_text::get().verify_failed,
            &[("command", command), ("output", &result.content)],
        ),
    }])
}

// CEILING: without a shadow tree, shell calls are treated as potential edits. A portable
// filesystem change journal would distinguish read-only commands outside Git work trees.
pub fn changed(result: &ToolResult) -> bool {
    let writes = [
        crate::permission::catalog::EDIT.name,
        crate::permission::catalog::BASH_WRITE.name,
        crate::permission::catalog::BASH.name,
        crate::permission::catalog::BASH_COMPLEX.name,
    ];
    let delegated_changes = result.name == "task"
        && result
            .details
            .as_ref()
            .and_then(|details| details["changed_files"].as_bool())
            .unwrap_or(false);
    delegated_changes
        || (!result.is_error
            && result.refused.is_none()
            && result
                .asks
                .iter()
                .any(|ask| writes.contains(&ask.permission.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        paths::Paths,
        settings::{self, Overrides},
    };

    #[test]
    fn child_edits_trigger_verification_even_when_the_child_ends_with_an_error() {
        let result = ToolResult {
            call_id: "task".into(),
            name: "task".into(),
            content: String::new(),
            attachments: Vec::new(),
            is_error: true,
            details: Some(serde_json::json!({"changed_files":true})),
            asks: Vec::new(),
            refused: None,
        };
        assert!(changed(&result));
        let read_only = ToolResult {
            details: Some(serde_json::json!({"changed_files":false})),
            ..result
        };
        assert!(!changed(&read_only));
    }
    #[test]
    fn configured_checks_are_bounded_and_top_level_only() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("project"));
        let mut settings = settings::load(&paths, &Overrides::default()).unwrap();
        assert!(call(&settings, true, true, 0).is_none());
        settings.verify = Some("exit 1".into());
        assert!(call(&settings, false, true, 0).is_none());
        assert!(call(&settings, true, false, 0).is_none());
        assert!(
            call(
                &settings,
                true,
                true,
                limits::get().agent.verify_attempts.value
            )
            .is_none()
        );
        assert_eq!(call(&settings, true, true, 0).unwrap().name, "bash");
    }
}
