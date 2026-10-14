//! Classify shell commands without resolving files or depending on the tool implementation.

use std::path::Path;

use super::gate::Ask;
use super::{catalog, filter, shell_split, workdir};

/// The caller resolves each file's read permissions, including external paths and secrets.
pub fn asks(command: &str, project: &Path, read: &dyn Fn(&str) -> Option<Vec<Ask>>) -> Vec<Ask> {
    let split = shell_split::split(command);
    let mut reads = Vec::new();
    let mut commands = Vec::new();
    let mut moved = false;
    for command in &split.commands {
        let Some(command) = workdir::strip(command, project) else {
            continue;
        };
        moved |= workdir::moves(&command);
        let read = filter::reads_files(&command)
            .filter(|_| !moved)
            .and_then(|files| {
                files
                    .iter()
                    .map(|file| read(file))
                    .collect::<Option<Vec<_>>>()
            })
            .map(|asks| asks.concat());
        match read {
            Some(asks) => reads.extend(asks),
            None => commands.push(command),
        }
    }
    // Pipe filters need no separate grant while another command or file supplies their input.
    let fed: Vec<String> = commands
        .iter()
        .filter(|command| !filter::reads_only_input(command))
        .cloned()
        .collect();
    let patterns = if fed.is_empty() && reads.is_empty() {
        commands
    } else {
        fed
    };
    let mut asks = reads;
    if !patterns.is_empty() {
        asks.push(Ask {
            permission: catalog::BASH.name.into(),
            patterns,
        });
    }
    if split.complex {
        asks.push(Ask::new(catalog::BASH_COMPLEX.name, command.trim()));
    }
    if split.writes {
        asks.push(Ask::new(catalog::BASH_WRITE.name, command.trim()));
    }
    asks
}
