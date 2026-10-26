use std::sync::LazyLock;

#[derive(serde::Deserialize)]
struct Config {
    prefixes: Vec<String>,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/reply-prefixes.toml"))
        .expect("reply prefix configuration is valid")
});

#[derive(Default)]
pub struct ReplyPrefix {
    pending: String,
    resolved: bool,
    trim: bool,
}

impl ReplyPrefix {
    pub fn push(&mut self, text: &str) -> String {
        if self.resolved {
            return if self.trim {
                let text = text.trim_start();
                self.trim = text.is_empty();
                text.to_owned()
            } else {
                text.to_owned()
            };
        }
        self.pending.push_str(text);
        for prefix in &CONFIG.prefixes {
            if let Some(rest) = self.pending.strip_prefix(prefix)
                && rest.starts_with(char::is_whitespace)
            {
                let text = rest.trim_start().to_owned();
                self.pending.clear();
                self.resolved = true;
                self.trim = text.is_empty();
                return text;
            }
        }
        if CONFIG.prefixes.iter().any(|p| p.starts_with(&self.pending)) {
            return String::new();
        }
        self.resolved = true;
        std::mem::take(&mut self.pending)
    }

    pub fn finish(&mut self) -> String {
        if CONFIG.prefixes.contains(&self.pending) {
            self.pending.clear();
        }
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_split_tags_but_preserves_links_code_and_later_tags() {
        for prefix in &CONFIG.prefixes {
            let input = format!("{prefix} Hello [D] later");
            for split in 0..=input.len() {
                let mut filter = ReplyPrefix::default();
                let actual =
                    filter.push(&input[..split]) + &filter.push(&input[split..]) + &filter.finish();
                assert_eq!(actual, "Hello [D] later");
            }
        }
        for input in [
            "[D](https://example.com)",
            "[1] item",
            "```text\n[D] code",
            "[Data] value",
            "[",
            "Hello [J]",
        ] {
            let mut filter = ReplyPrefix::default();
            assert_eq!(filter.push(input) + &filter.finish(), input);
        }
    }
}
