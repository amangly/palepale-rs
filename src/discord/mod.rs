//! Discord conversation policy. I/O and credentials live in io::discord.
pub mod bot;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Deserialize)]
pub struct Data {
    pub api: String,
    pub gateway: String,
    pub request_secs: u64,
    pub stream_update_ms: u64,
    pub reconnect_secs: u64,
    pub max_reconnect_secs: u64,
    pub max_event_bytes: usize,
    pub max_reply_bytes: usize,
    pub max_display_units: usize,
    pub message_units: usize,
    pub event_queue: usize,
    pub dedup_events: usize,
    pub system: String,
    pub defaults: Config,
    pub text: Text,
}

#[derive(Deserialize)]
pub struct Text {
    pub help: String,
    pub denied: String,
    pub busy: String,
    pub capacity: String,
    pub thinking: String,
    pub empty: String,
    pub reset: String,
    pub stopping: String,
    pub idle: String,
    pub stopped: String,
    pub limit: String,
    pub failed: String,
    pub unsupported: String,
    pub no_reply: String,
    pub no_usage: String,
    pub delivery_failed: String,
}

pub fn data() -> &'static Data {
    static DATA: LazyLock<Data> = LazyLock::new(|| {
        toml::from_str(include_str!("../../data/discord.toml")).expect("Discord data")
    });
    &DATA
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub owner: String,
    pub model: String,
    pub allow_users: Vec<String>,
    pub allow_roles: Vec<String>,
    pub free_response_channels: Vec<String>,
    pub auto_thread: bool,
    pub max_concurrent: usize,
    pub max_sessions: usize,
    pub history_turns: usize,
    pub history_bytes: usize,
    pub max_output_tokens: u64,
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            snowflake(&self.owner),
            "Set a Discord owner user ID with setup --owner ID"
        );
        for id in self
            .allow_users
            .iter()
            .chain(&self.allow_roles)
            .chain(&self.free_response_channels)
        {
            anyhow::ensure!(snowflake(id), "Discord IDs must contain 1 to 20 digits");
        }
        anyhow::ensure!(
            (1..=8).contains(&self.max_concurrent),
            "max_concurrent must be 1 to 8"
        );
        anyhow::ensure!(
            (1..=256).contains(&self.max_sessions),
            "max_sessions must be 1 to 256"
        );
        anyhow::ensure!(
            (1..=50).contains(&self.history_turns),
            "history_turns must be 1 to 50"
        );
        anyhow::ensure!(
            (4096..=1048576).contains(&self.history_bytes),
            "history_bytes must be 4096 to 1048576"
        );
        Ok(())
    }

    pub fn authorized(&self, event: &Incoming) -> bool {
        event.user == self.owner
            || self.allow_users.contains(&event.user)
            || event.guild.is_some()
                && event
                    .roles
                    .iter()
                    .any(|role| self.allow_roles.contains(role))
    }
}

// No Debug implementation: interaction tokens are credentials.
pub struct Interaction {
    pub id: String,
    pub token: String,
    pub application: String,
}

pub struct Incoming {
    pub id: String,
    pub channel: String,
    pub guild: Option<String>,
    pub user: String,
    pub roles: Vec<String>,
    pub content: String,
    pub mentioned: bool,
    pub interaction: Option<Interaction>,
    pub command: Option<(String, String)>,
    pub attachments: bool,
}

impl Incoming {
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.guild.as_deref().unwrap_or("dm"),
            self.channel,
            self.user
        )
    }
}

pub fn incoming(kind: &str, value: &Value, bot: &str) -> Option<Incoming> {
    let interaction = kind == "INTERACTION_CREATE";
    if !interaction && kind != "MESSAGE_CREATE" {
        return None;
    }
    if interaction && (value["type"] != 2 || value["data"]["name"] != "palepale") {
        return None;
    }
    let author = if interaction {
        if value["guild_id"].is_string() {
            &value["member"]["user"]
        } else {
            &value["user"]
        }
    } else {
        &value["author"]
    };
    if author["bot"] == true || value["webhook_id"].is_string() {
        return None;
    }
    let event_id = id(value, "id")?;
    let user = id(author, "id")?;
    let channel = id(value, "channel_id").or_else(|| id(&value["channel"], "id"))?;
    let guild = value["guild_id"].as_str().map(str::to_owned);
    if guild.as_deref().is_some_and(|v| !snowflake(v)) {
        return None;
    }
    let roles = value["member"]["roles"]
        .as_array()
        .map(|roles| {
            roles
                .iter()
                .filter_map(Value::as_str)
                .filter(|v| snowflake(v))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let command = interaction.then(|| {
        let sub = &value["data"]["options"][0];
        (
            sub["name"].as_str().unwrap_or("help").to_owned(),
            sub["options"][0]["value"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    });
    let content = if command.as_ref().is_some_and(|(cmd, _)| cmd == "ask") {
        command.as_ref()?.1.clone()
    } else {
        value["content"].as_str().unwrap_or_default().to_owned()
    };
    let token = if interaction {
        let token = value["token"].as_str()?;
        if token.is_empty()
            || token.len() > 512
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
        {
            return None;
        }
        Some(Interaction {
            id: event_id.clone(),
            token: token.to_owned(),
            application: id(value, "application_id")?,
        })
    } else {
        None
    };
    Some(Incoming {
        id: event_id,
        channel,
        guild,
        user,
        roles,
        content,
        mentioned: value["mentions"]
            .as_array()
            .is_some_and(|m| m.iter().any(|v| v["id"] == bot)),
        interaction: token,
        command,
        attachments: value["attachments"]
            .as_array()
            .is_some_and(|v| !v.is_empty()),
    })
}

fn id(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .filter(|v| snowflake(v))
        .map(str::to_owned)
}
pub fn snowflake(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

pub fn strip_mention(text: &str, bot: &str) -> String {
    text.replace(&format!("<@{bot}>"), "")
        .replace(&format!("<@!{bot}>"), "")
        .trim()
        .to_owned()
}

/// Discord counts UTF-16 units. Keep code, newlines and whitespace byte-for-byte.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut units = 0;
    for ch in text.chars() {
        if units + ch.len_utf16() > max && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
            units = 0;
        }
        current.push(ch);
        units += ch.len_utf16();
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn mentions_preserve_code_and_unicode_fits_discord() {
        assert_eq!(strip_mention("<@1>\n```\n  x\n```", "1"), "```\n  x\n```");
        let text = "👋\n  ``` x ``` ".repeat(500);
        let parts = chunks(&text, 1900);
        assert_eq!(parts.concat(), text);
        assert!(parts.iter().all(|p| p.encode_utf16().count() <= 1900));
    }
    #[test]
    fn owner_is_required_and_empty_allowlists_are_closed() {
        let mut config = data().defaults.clone();
        assert!(config.validate().is_err());
        config.owner = "1".into();
        config.validate().unwrap();
        let mut e = incoming(
            "MESSAGE_CREATE",
            &json!({"id":"4","channel_id":"5","author":{"id":"2"},"content":"hello"}),
            "9",
        )
        .unwrap();
        assert!(!config.authorized(&e));
        e.user = "1".into();
        assert!(config.authorized(&e));
        e.user = "2".into();
        config.allow_roles.push("3".into());
        e.roles.push("3".into());
        assert!(!config.authorized(&e));
        e.guild = Some("8".into());
        assert!(config.authorized(&e));
    }
    #[test]
    fn slash_commands_are_parsed_and_bot_messages_ignored() {
        let value = json!({"type":2,"id":"4","application_id":"9","token":"fixture","channel_id":"5","guild_id":"8","member":{"user":{"id":"2"},"roles":["3"]},"data":{"name":"palepale","options":[{"name":"ask","options":[{"name":"message","value":"Hi"}]}]}});
        let e = incoming("INTERACTION_CREATE", &value, "9").unwrap();
        assert_eq!(e.content, "Hi");
        assert_eq!(e.key(), "8:5:2");
        assert!(
            incoming(
                "MESSAGE_CREATE",
                &json!({"id":"4","channel_id":"5","author":{"id":"2","bot":true}}),
                "9"
            )
            .is_none()
        );
    }
}
