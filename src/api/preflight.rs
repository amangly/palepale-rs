//! One request boundary for wire metadata and bounded context estimates.
use std::sync::LazyLock;

use serde::Deserialize;

use crate::api::chat::Turn;
use crate::api::error::ErrorInfo;
use crate::text::template::fill;

#[derive(Deserialize)]
pub struct Config {
    pub tools: String,
    pub streaming: String,
    pub unknown: String,
    pub unsupported: String,
    pub limits: String,
    pub context: String,
    pub effort: String,
    pub inputs_unknown: String,
    pub bytes_per_token: u64,
    pub message_overhead_tokens: u64,
}

pub fn config() -> &'static Config {
    static CONFIG: LazyLock<Config> = LazyLock::new(|| {
        toml::from_str(include_str!("../../data/preflight.toml")).expect("preflight configuration")
    });
    &CONFIG
}

pub fn output_limit(model: &crate::api::models::Model) -> u64 {
    model.max_output_tokens
}

pub fn check(turn: &Turn<'_>) -> Result<(), ErrorInfo> {
    let model = turn.model;
    let data = config();
    let error =
        |text: &str| ErrorInfo::local("incompatible_request", fill(text, &[("model", &model.id)]));
    if !model.capabilities.streaming {
        return Err(error(&data.streaming));
    }
    if model.context_window == 0
        || model.max_output_tokens == 0
        || model.max_output_tokens > model.context_window
    {
        return Err(error(&data.limits));
    }
    model
        .preflight(
            turn.messages,
            if model.capabilities.tool_calling {
                turn.tools.len()
            } else {
                0
            },
        )
        .map_err(|error| ErrorInfo::local("incompatible_request", error.to_string()))?;
    if let (Some(effort), Some(efforts)) =
        (&model.default_reasoning_effort, &model.reasoning_efforts)
        && (!model.capabilities.reasoning || !efforts.contains(effort))
    {
        return Err(error(&data.effort));
    }
    // Estimate only the wire projection; local diff details and permission records are not sent.
    let wire_messages = crate::api::wire::messages(
        turn.messages,
        &crate::api::wire::Replay {
            model: &model.id,
            reasoning: model.capabilities.reasoning,
            tools: model.capabilities.tool_calling,
        },
    );
    let bytes = turn.system.len() as u64
        + serde_json::to_vec(&wire_messages).map_or(0, |v| v.len() as u64)
        + if model.capabilities.tool_calling {
            serde_json::to_vec(turn.tools).map_or(0, |v| v.len() as u64)
        } else {
            0
        };
    let input = bytes
        .div_ceil(data.bytes_per_token.max(1))
        .saturating_add((turn.messages.len() as u64 + 1) * data.message_overhead_tokens);
    let output = output_limit(model);
    if input.saturating_add(output) > model.context_window {
        return Err(ErrorInfo::local(
            "context_budget",
            fill(
                &data.context,
                &[
                    ("model", &model.id),
                    ("tokens", &input.to_string()),
                    ("output", &output.to_string()),
                    ("context", &model.context_window.to_string()),
                ],
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::{Attachment, Message, Part};

    #[test]
    fn missing_unsupported_and_supported_modalities_preserve_the_original_messages() {
        let mut model = crate::api::models::fixture().remove(0);
        let messages = vec![Message::User {
            parts: vec![Part::Attachment(Attachment {
                mime_type: "image/png".into(),
                data: "fixture".into(),
                filename: Some("image.png".into()),
            })],
        }];
        let original = messages.clone();
        model.input_modalities = None;
        assert!(
            model
                .preflight(&messages, 0)
                .unwrap_err()
                .to_string()
                .contains("unknown")
        );
        model.input_modalities = Some(vec!["text".into()]);
        assert!(
            model
                .preflight(&messages, 0)
                .unwrap_err()
                .to_string()
                .contains("does not support image")
        );
        model.input_modalities = Some(vec!["image".into()]);
        model.preflight(&messages, 0).unwrap();
        assert_eq!(messages, original);
    }

    #[test]
    fn streaming_limits_and_reasoning_metadata_are_checked_before_dispatch() {
        let mut model = crate::api::models::fixture().remove(0);
        let check_model = |model: &crate::api::models::Model| {
            check(&Turn {
                model,
                agent_profile: None,
                system: "system",
                messages: &[],
                tools: &[],
            })
        };
        assert!(check_model(&model).is_ok());
        assert_eq!(output_limit(&model), model.max_output_tokens);
        model.capabilities.streaming = false;
        assert!(
            check_model(&model)
                .unwrap_err()
                .message
                .contains("streaming")
        );
        model.capabilities.streaming = true;
        model.context_window = 1;
        model.max_output_tokens = 1;
        assert_eq!(check_model(&model).unwrap_err().kind, "context_budget");
        model.context_window = 100000;
        model.default_reasoning_effort = Some("not-a-level".into());
        assert!(
            check_model(&model)
                .unwrap_err()
                .message
                .contains("reasoning")
        );
    }

    #[test]
    fn attachment_types_and_tool_requests_require_declared_support() {
        let mut model = crate::api::models::fixture().remove(0);
        for (mime, kind) in [
            ("audio/wav", "audio"),
            ("application/pdf", "pdf"),
            ("application/octet-stream", "file"),
        ] {
            let messages = vec![Message::User {
                parts: vec![Part::Attachment(Attachment {
                    mime_type: mime.into(),
                    data: String::new(),
                    filename: None,
                })],
            }];
            model.input_modalities = Some(vec!["text".into()]);
            assert!(
                model
                    .preflight(&messages, 0)
                    .unwrap_err()
                    .to_string()
                    .contains(kind)
            );
            model.input_modalities = Some(vec![kind.into()]);
            model.preflight(&messages, 0).unwrap();
        }
        model.capabilities.tool_calling = false;
        assert!(model.preflight(&[], 1).is_err());
    }
}
