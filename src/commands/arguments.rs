//! Quoted slash arguments, without shell expansion. Backslashes in Windows paths stay literal.

pub fn parse(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') if chars.peek() == Some(&'"') => {
                word.push(chars.next()?);
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        words.push(word);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_json_empty_arguments_and_substitutions_remain_literal() {
        let words = parse(r#"mcp add docs --env 'TOKEN=two words' -- "C:\Program Files\mcp.exe" '' '{"key":"value"}' $(nothing)"#).unwrap();
        assert_eq!(
            words,
            [
                "mcp",
                "add",
                "docs",
                "--env",
                "TOKEN=two words",
                "--",
                r"C:\Program Files\mcp.exe",
                "",
                r#"{"key":"value"}"#,
                "$(nothing)"
            ]
        );
        assert!(parse("mcp add 'unfinished").is_none());
        assert_eq!(
            parse(r#"mcp -- "say \"hello\"""#).unwrap()[2],
            "say \"hello\""
        );
        assert_eq!(
            parse(r#"mcp -- '\\server\folder name'"#).unwrap()[2],
            r"\\server\folder name"
        );
    }
}
