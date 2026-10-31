//! Spots interleaved repeated tool calls whose output is not changing.

use crate::agent::message::{Message, ToolCall};

/// Count identical calls in the current user turn, including interleaved calls. A changed
/// result resets the streak; later unchanged results can still trigger the guard.
pub fn repeats(history: &[Message], reply: &[ToolCall], i: usize) -> usize {
    let current = &reply[i];
    let turn = history
        .iter()
        .rposition(|m| matches!(m, Message::User { .. }))
        .map_or(0, |u| u + 1);
    let before: Vec<&ToolCall> = history[turn..]
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a.tool_calls()),
            _ => None,
        })
        .flatten()
        .chain(reply[..i].iter())
        .filter(|c| same(c, current))
        .collect();
    if before.is_empty() {
        return 1;
    }
    // The result of each earlier identical call (those already run, i.e. in `history`), by call id.
    let results: std::collections::HashMap<&str, &str> = history[turn..]
        .iter()
        .filter_map(|m| match m {
            Message::Tool(r) => Some((r.call_id.as_str(), r.content.as_str())),
            _ => None,
        })
        .collect();
    let mut count = 1;
    let mut previous = None;
    for call in before {
        if let Some(output) = results.get(call.id.as_str()).copied() {
            if previous.is_some_and(|last| last != output) {
                count = 1;
            }
            previous = Some(output);
        }
        count += 1;
    }
    count
}

/// Same tool and same arguments, however the JSON was spaced.
fn same(a: &ToolCall, b: &ToolCall) -> bool {
    if a.name != b.name {
        return false;
    }
    match (
        serde_json::from_str::<serde_json::Value>(&a.arguments),
        serde_json::from_str::<serde_json::Value>(&b.arguments),
    ) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.arguments == b.arguments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::{Assistant, Finish, Part, ToolResult};

    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    fn made(calls: Vec<ToolCall>) -> Message {
        let parts = calls.into_iter().map(Part::ToolCall).collect();
        Message::Assistant(Assistant {
            parts,
            model: "m".into(),
            effort: None,
            usage: None,
            finish: Finish::ToolCalls,
            error: None,
            request_id: None,
        })
    }

    fn result(id: &str, content: &str) -> Message {
        Message::Tool(ToolResult {
            call_id: id.into(),
            name: "x".into(),
            content: content.into(),
            is_error: false,
            details: None,
            asks: Vec::new(),
            refused: None,
            attachments: vec![],
        })
    }

    #[test]
    fn counts_interleaved_identical_calls_with_unchanging_output() {
        // read(a), bash(ls), read(a) — the two read(a) are interleaved (not consecutive) and both
        // returned the same output, so a third is a futile loop.
        let history = [
            Message::user_text("go"),
            made(vec![call("c1", "read", r#"{"path":"a"}"#)]),
            result("c1", "FILE"),
            made(vec![call("c2", "bash", r#"{"command":"ls"}"#)]),
            result("c2", "LS"),
            made(vec![call("c3", "read", r#"{"path":"a"}"#)]),
            result("c3", "FILE"),
        ];
        let reply = [call("c4", "read", r#"{ "path": "a" }"#)];
        assert_eq!(repeats(&history, &reply, 0), 3);
    }

    #[test]
    fn a_changing_result_is_progress_not_a_loop() {
        // The same read returns different content (the file was edited between) — the tool is making
        // progress, so it is not flagged however many times it is read.
        let history = [
            Message::user_text("fix it"),
            made(vec![call("c1", "read", r#"{"path":"a"}"#)]),
            result("c1", "BEFORE"),
            made(vec![call("c2", "read", r#"{"path":"a"}"#)]),
            result("c2", "AFTER"),
        ];
        let reply = [call("c3", "read", r#"{"path":"a"}"#)];
        assert_eq!(repeats(&history, &reply, 0), 2);
    }

    #[test]
    fn an_earlier_change_does_not_hide_a_later_stall() {
        let history = [
            Message::user_text("fix it"),
            made(vec![call("c1", "read", r#"{"path":"a"}"#)]),
            result("c1", "old"),
            made(vec![call("c2", "read", r#"{"path":"a"}"#)]),
            result("c2", "new"),
            made(vec![call("c3", "read", r#"{"path":"a"}"#)]),
            result("c3", "new"),
        ];
        let reply = [call("c4", "read", r#"{"path":"a"}"#)];
        assert_eq!(repeats(&history, &reply, 0), 3);
    }

    #[test]
    fn a_new_user_message_starts_the_count_over() {
        let history = [
            Message::user_text("list it"),
            made(vec![call("c1", "bash", r#"{"command":"ls"}"#)]),
            result("c1", "LS"),
            made(vec![call("c2", "bash", r#"{"command":"ls"}"#)]),
            result("c2", "LS"),
            Message::user_text("list it again"),
        ];
        let reply = [call("c3", "bash", r#"{"command":"ls"}"#)];
        assert_eq!(repeats(&history, &reply, 0), 1);
    }
}
