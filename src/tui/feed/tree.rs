//! Child tool events and saved child sessions become the same display tree.
use super::{Block, Entry, Feed};
use crate::agent::event::AgentEvent;
use crate::agent::message::{Message, Part, ToolResult};
use crate::text::styled::Line;
use crate::tui::cards;

pub(super) struct SubCall {
    call_id: String,
    name: String,
    arguments: String,
    result: Option<ToolResult>,
    children: Vec<SubCall>,
}

pub(super) fn route(nodes: &mut Vec<SubCall>, event: &AgentEvent) {
    match event {
        AgentEvent::Child { call_id, event } => {
            if let Some(node) = nodes.iter_mut().find(|node| node.call_id == *call_id) {
                route(&mut node.children, event);
            }
        }
        AgentEvent::ToolStart {
            call_id,
            name,
            arguments,
        } => nodes.push(SubCall {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            result: None,
            children: Vec::new(),
        }),
        AgentEvent::ToolEnd { result } => {
            if let Some(node) = nodes.iter_mut().find(|node| node.call_id == result.call_id) {
                node.result = Some(result.clone());
            }
        }
        _ => {}
    }
}

pub(super) fn child_lines(calls: &[SubCall], depth: usize, width: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    for call in calls {
        lines.push(cards::step(&call.name, &call.arguments, depth).truncate(width));
        lines.extend(child_lines(&call.children, depth + 1, width));
    }
    lines
}

impl Feed {
    pub(super) fn flatten(&mut self, calls: Vec<SubCall>, depth: usize) {
        for call in calls {
            let result = call.result.unwrap_or_else(|| ToolResult {
                call_id: call.call_id,
                name: call.name,
                content: String::new(),
                attachments: Vec::new(),
                is_error: false,
                details: None,
                asks: Vec::new(),
                refused: None,
            });
            self.entries.push(Entry {
                block: Block::Tool {
                    arguments: call.arguments,
                    result,
                    first: false,
                    depth,
                },
                open: false,
                drawn: None,
            });
            self.flatten(call.children, depth + 1);
        }
    }

    /// A missing child session leaves its task card usable as a flat result.
    pub fn restore_with(
        &mut self,
        messages: &[Message],
        load_child: &dyn Fn(&str) -> Vec<Message>,
    ) {
        let mut arguments = std::collections::HashMap::new();
        let mut batch_open = false;
        for message in messages {
            match message {
                Message::User { parts } => {
                    batch_open = false;
                    let text = parts
                        .iter()
                        .filter_map(|part| match part {
                            Part::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        self.user(&text);
                    }
                }
                Message::Assistant(reply) => {
                    batch_open = false;
                    for part in &reply.parts {
                        match part {
                            Part::Text { text } => self.push(Block::Reply {
                                md: text.clone(),
                                first: true,
                            }),
                            Part::Reasoning { text } => self.push(Block::Thinking {
                                text: text.clone(),
                                secs: 0,
                            }),
                            Part::ToolCall(call) => {
                                arguments.insert(call.id.clone(), call.arguments.clone());
                            }
                            _ => {}
                        }
                    }
                    self.outcome_of(reply);
                }
                Message::Tool(result) => {
                    self.push(Block::Tool {
                        arguments: arguments.remove(&result.call_id).unwrap_or_default(),
                        result: result.clone(),
                        first: !batch_open,
                        depth: 0,
                    });
                    batch_open = true;
                    self.restore_children(result, 1, load_child);
                }
            }
        }
    }

    fn restore_children(
        &mut self,
        task: &ToolResult,
        depth: usize,
        load_child: &dyn Fn(&str) -> Vec<Message>,
    ) {
        if task.name != "task" || depth > crate::limits::get().subagent.max_depth.value {
            return;
        }
        let Some(path) = task
            .details
            .as_ref()
            .and_then(|details| details["session"].as_str())
        else {
            return;
        };
        let mut arguments = std::collections::HashMap::new();
        for message in load_child(path) {
            match message {
                Message::Assistant(reply) => {
                    for part in reply.parts {
                        if let Part::ToolCall(call) = part {
                            arguments.insert(call.id, call.arguments);
                        }
                    }
                }
                Message::Tool(result) => {
                    self.entries.push(Entry {
                        block: Block::Tool {
                            arguments: arguments.remove(&result.call_id).unwrap_or_default(),
                            result: result.clone(),
                            first: false,
                            depth,
                        },
                        open: false,
                        drawn: None,
                    });
                    self.restore_children(&result, depth + 1, load_child);
                }
                _ => {}
            }
        }
    }
}
