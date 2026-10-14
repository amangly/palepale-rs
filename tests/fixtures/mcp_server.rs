//! A standard-library-only MCP subprocess used by local tests.
use std::io::{BufRead, Write};

fn id(line: &str) -> Option<&str> {
    let rest = line.split_once("\"id\"")?.1.trim_start().strip_prefix(':')?.trim_start();
    if rest.starts_with('"') { Some(&rest[..rest[1..].find('"')? + 2]) }
    else { Some(&rest[..rest.find([',', '}']).unwrap_or(rest.len())]) }
}

fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    let modern = arguments.get(1).is_some_and(|a| a == "--modern");
    let large = arguments.iter().any(|a| a == "--large");
    let catalogs = arguments.iter().any(|a| a == "--catalogs");
    let changes = arguments.iter().any(|a| a == "--changes");
    let media = arguments.iter().any(|a| a == "--media");
    let mut changed = false;
    let log = arguments.get(2).map(std::path::PathBuf::from);
    let record = |event: &str| {
        if let Some(path) = &log {
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
            writeln!(file, "{event}").unwrap();
        }
    };
    record(if std::env::var("PALEPALE_API_KEY").is_ok() { "palepale-key-leaked" } else { "no-palepale-key" });
    if std::env::var("MCP_FIXTURE_VALUE").as_deref() == Ok("configured") { record("fixture-env"); }
    record(&format!("cwd:{}", std::env::current_dir().unwrap().display()));
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("notifications/cancelled") { record("cancel"); continue; }
        let Some(id) = id(&line) else { continue; };
        let result = if compact.contains("server/discover") {
            record("discover");
            if !modern {
                println!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32601,\"message\":\"Legacy server\"}}}}");
                std::io::stdout().flush().unwrap();
                continue;
            }
            r#"{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"}"#
        } else if compact.contains("\"method\":\"initialize\"") {
            record("initialize");
            r#"{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"native-fixture","version":"1"}}"#
        } else if compact.contains("tools/list") {
            record("tools/list");
            if changed {
                r#"{"tools":[{"name":"updated","description":"Updated catalog","inputSchema":{"type":"object","properties":{}}}]}"#
            } else if compact.contains("\"cursor\":\"next\"") {
                r#"{"tools":[{"name":"slow_write","description":"Record one write and wait","inputSchema":{"type":"object","properties":{}}},{"name":"crash","description":"Exit unexpectedly","inputSchema":{"type":"object","properties":{}}}]}"#
            } else {
                r#"{"tools":[{"name":"echo","description":"Echo {shell_note} literally","inputSchema":{"type":"object","properties":{"x":{"type":"integer"}},"required":["x"]}},{"name":"danger","description":"Untrusted read-only hint","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object","properties":{}}},{"name":"invalid","description":"Invalid schema","inputSchema":{"type":"object","properties":{"x":{"type":"nonsense"}}}}],"nextCursor":"next"}"#
            }
        } else if compact.contains("resources/templates/list") {
            r#"{"resourceTemplates":[{"uriTemplate":"memory://fixture/{name}","name":"fixture-template"}]}"#
        } else if compact.contains("resources/list") {
            if compact.contains("\"cursor\":\"second\"") { r#"{"resources":[{"uri":"memory://fixture/other","name":"other"}]}"# }
            else { r#"{"resources":[{"uri":"memory://fixture/notes","name":"notes","mimeType":"text/plain"}],"nextCursor":"second"}"# }
        } else if compact.contains("resources/read") {
            r#"{"contents":[{"uri":"memory://fixture/notes","mimeType":"text/plain","text":"Native resource body"}]}"#
        } else if compact.contains("prompts/list") {
            r#"{"prompts":[{"name":"review","description":"Review the fixture","arguments":[{"name":"topic","required":true}]}]}"#
        } else if compact.contains("prompts/get") {
            record("prompts/get");
            r#"{"messages":[{"role":"user","content":{"type":"text","text":"Review native fixture instructions"}}]}"#
        } else if compact.contains("tools/call") {
            if compact.contains("\"name\":\"progress\"") {
                let token = compact.split_once("\"progressToken\":").unwrap().1.split([',','}']).next().unwrap();
                for i in 0..6 {
                    std::thread::sleep(std::time::Duration::from_millis(75));
                    println!("{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{\"progressToken\":{token},\"progress\":{i}}}}}");
                    std::io::stdout().flush().unwrap();
                }
            }
            if compact.contains("\"name\":\"slow_write\"") { record("write"); continue; }
            if compact.contains("\"name\":\"crash\"") { std::process::exit(17); }
            record("echo");
            if changes && !changed {
                changed = true;
                println!("{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}}");
                std::io::stdout().flush().unwrap();
            }
            r#"{"content":[{"type":"text","text":"native fixture"}],"structuredContent":{"answer":42},"isError":false}"#
        } else if compact.contains("\"method\":\"ping\"") { "{}" }
        else { continue; };
        let result = if large && compact.contains("tools/list") {
            result.replace("Echo {shell_note} literally", &format!("Echo {{shell_note}} literally {}", "x".repeat(16000)))
                .replace("Untrusted read-only hint", &format!("Untrusted read-only hint {}", "x".repeat(16000)))
        } else { result.to_owned() };
        let result = if catalogs { result.replace("\"capabilities\":{\"tools\":{}}", "\"capabilities\":{\"tools\":{},\"resources\":{},\"prompts\":{}}") } else { result };
        let result = if media && compact.contains("tools/call") { result.replace("\"content\":[", "\"content\":[{\"type\":\"image\",\"mimeType\":\"image/png\",\"data\":\"AQID\"},") } else { result };
        println!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}");
        std::io::stdout().flush().unwrap();
    }
}
