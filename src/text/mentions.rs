//! Prompt references and cursor spans. No filesystem access.
use std::ops::Range;
use std::sync::LazyLock;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct Text {
    pub title: String,
    pub loading: String,
    pub incomplete: String,
    pub empty: String,
    pub file: String,
    pub folder: String,
    pub agent: String,
    pub multiple_agents: String,
    pub too_many: String,
    pub task_description: String,
    pub directory_cut: String,
}

pub fn text() -> &'static Text {
    static TEXT: LazyLock<Text> = LazyLock::new(|| {
        toml::from_str(include_str!("../../data/mentions.toml")).expect("mention text")
    });
    &TEXT
}

#[derive(Debug, PartialEq, Eq)]
pub struct Mention {
    pub span: Range<usize>,
    pub path: String,
    pub closed: bool,
}

pub fn parse(text: &str) -> Vec<Mention> {
    scan(text).0
}

pub fn unescape(text: &str) -> String {
    let mut output = text.to_owned();
    for range in scan(text).1.into_iter().rev() {
        output.replace_range(range, "@");
    }
    output
}

fn scan(text: &str) -> (Vec<Mention>, Vec<Range<usize>>) {
    let mut literals = Vec::new();
    let mut found = Vec::new();
    let mut i = 0;
    let mut code = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        if c == '`' {
            let count = text[i..].bytes().take_while(|b| *b == b'`').count();
            if code == 0 {
                code = count;
            } else if code == count {
                code = 0;
            }
            i += count;
            continue;
        }
        let boundary = i == 0 || text[..i].ends_with(char::is_whitespace);
        if code == 0 && boundary && text[i..].starts_with("@@") {
            literals.push(i..i + 2);
            i += 2;
            continue;
        }
        if code == 0 && c == '@' && boundary {
            let start = i;
            i += 1;
            let quoted = text[i..].starts_with('"');
            i += usize::from(quoted);
            let body = i;
            while i < text.len() {
                let next = text[i..].chars().next().unwrap();
                if (quoted && next == '"')
                    || (!quoted && (next.is_whitespace() || ",;!?()[]{}\"`".contains(next)))
                {
                    break;
                }
                i += next.len_utf8();
            }
            let closed = !quoted || text[i..].starts_with('"');
            let path = if quoted {
                &text[body..i]
            } else {
                text[body..i].trim_end_matches('.')
            };
            let end = if quoted {
                i + usize::from(closed)
            } else {
                body + path.len()
            };
            found.push(Mention {
                span: start..end,
                path: path.replace('\\', "/"),
                closed,
            });
            i = i.max(end);
        } else {
            i += c.len_utf8();
        }
    }
    (found, literals)
}

pub fn active(text: &str, cursor: usize) -> Option<Mention> {
    parse(text)
        .into_iter()
        .find(|m| m.span.start < cursor && cursor <= m.span.end)
}

pub fn quote(path: &str) -> String {
    if path
        .chars()
        .any(|c| c.is_whitespace() || ",;!?()[]{}\"`".contains(c))
        || path.ends_with('.')
    {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

pub fn score(path: &str, query: &str) -> Option<u8> {
    let path = path.to_lowercase();
    let query = query.to_lowercase();
    if path == query {
        return Some(0);
    }
    if path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .starts_with(&query)
    {
        return Some(1);
    }
    if path.starts_with(&query) {
        return Some(2);
    }
    let mut chars = query.chars();
    let mut next = chars.next();
    for c in path.chars() {
        if next == Some(c) {
            next = chars.next();
        }
    }
    next.is_none().then_some(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn references_skip_literals_and_preserve_unicode_spans() {
        let input = "你好 @src/main.rs. @\"a b.md\" x@y.com https://x/@a @@literal `@code` ```\n@code\n``` @src\\a.rs";
        let paths: Vec<_> = parse(input).into_iter().map(|m| m.path).collect();
        assert_eq!(paths, ["src/main.rs", "a b.md", "src/a.rs"]);
        let m = active("你好 @sr rest", "你好 @sr".len()).unwrap();
        assert_eq!(&"你好 @sr rest"[m.span], "@sr");
        assert_eq!(active("@", 1).unwrap().path, "");
        assert!(!parse("@\"unfinished")[0].closed);
    }
    #[test]
    fn ranking_and_quoting() {
        assert_eq!(score("src/main.rs", "main"), Some(1));
        assert_eq!(score("src/main.rs", "smrs"), Some(3));
        assert_eq!(score("src/main.rs", "xyz"), None);
        assert_eq!(quote("a b.md"), "@\"a b.md\"");
        assert_eq!(
            unescape("@@file `@@code` x@@mail"),
            "@file `@@code` x@@mail"
        );
    }
}
