use std::sync::LazyLock;

use grep_matcher::Matcher;
use grep_regex::RegexMatcher;

#[derive(serde::Deserialize)]
struct Config {
    pattern: String,
    partial_pattern: String,
    max_pending_bytes: usize,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/reply-prefixes.toml"))
        .expect("reply prefix configuration is valid")
});

static PATTERN: LazyLock<RegexMatcher> =
    LazyLock::new(|| RegexMatcher::new(&CONFIG.pattern).expect("reply prefix regex is valid"));
static PARTIAL: LazyLock<RegexMatcher> = LazyLock::new(|| {
    RegexMatcher::new(&CONFIG.partial_pattern).expect("partial reply prefix regex is valid")
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
        if let Some(tag) = PATTERN.find(self.pending.as_bytes()).expect("regex search") {
            let rest = &self.pending[tag.end()..];
            // Wait one more chunk to distinguish a label from a Markdown link.
            if rest.is_empty() {
                return String::new();
            }
            if !rest.starts_with(['(', '[', ':']) {
                let text = rest.trim_start().to_owned();
                self.pending.clear();
                self.resolved = true;
                self.trim = text.is_empty();
                return text;
            }
        }
        if self.pending.len() < CONFIG.max_pending_bytes
            && PARTIAL
                .is_match(self.pending.as_bytes())
                .expect("regex search")
        {
            return String::new();
        }
        self.resolved = true;
        std::mem::take(&mut self.pending)
    }

    pub fn finish(&mut self) -> String {
        if !self.resolved
            && PATTERN
                .find(self.pending.as_bytes())
                .expect("regex search")
                .is_some_and(|tag| tag.end() == self.pending.len())
        {
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
        for prefix in [
            "[J]",
            "[abcd]",
            "[long profile name]",
            "[神模式]",
            "[x-2_⚡]",
            "[1]",
        ] {
            let input = format!("{prefix} Hello [D] later");
            for split in 0..=input.len() {
                if !input.is_char_boundary(split) {
                    continue;
                }
                let mut filter = ReplyPrefix::default();
                let actual =
                    filter.push(&input[..split]) + &filter.push(&input[split..]) + &filter.finish();
                assert_eq!(actual, "Hello [D] later");
            }
        }
        for input in [
            "[D](https://example.com)",
            "[abcd][reference]",
            "[abcd]: https://example.com",
            "```text\n[D] code",
            "[unfinished label",
            "[nested[label]] text",
            "[line\nbreak] text",
            "[",
            "Hello [J]",
        ] {
            for split in 0..=input.len() {
                let mut filter = ReplyPrefix::default();
                assert_eq!(
                    filter.push(&input[..split]) + &filter.push(&input[split..]) + &filter.finish(),
                    input
                );
            }
        }
    }

    #[test]
    fn handles_whitespace_adjacent_text_and_tag_only_replies() {
        for (input, expected) in [
            (" \n[arbitrary profile] \nHello", "Hello"),
            ("[xxxxxx]Hello", "Hello"),
            ("[xxxxxx]", ""),
            ("[xxxxxx]  ", ""),
            ("[] text", "[] text"),
        ] {
            let mut filter = ReplyPrefix::default();
            let mut actual = String::new();
            for c in input.chars() {
                actual.push_str(&filter.push(&c.to_string()));
            }
            actual.push_str(&filter.finish());
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn flushes_unclosed_labels_at_the_buffer_limit() {
        let input = format!("[{}", "a".repeat(CONFIG.max_pending_bytes));
        let mut filter = ReplyPrefix::default();
        assert_eq!(filter.push(&input), input);
        assert_eq!(filter.push("] later"), "] later");
    }
}
