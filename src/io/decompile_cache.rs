//! Successful decompiler answers, invalidated when the program, binary or adjacent PDB changes.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use crate::io::fs::{self, Access};
use crate::paths::Paths;

// CEILING: external PDB locations and cache pruning are not tracked. Upgrade by resolving
// CodeView records and expiring old entries if cache growth becomes material.
pub fn file(paths: &Paths, program: &Path, binary: &Path, argv: &[String]) -> Option<PathBuf> {
    let mut fingerprint = String::new();
    for path in [
        program.to_owned(),
        binary.to_owned(),
        binary.with_extension("pdb"),
    ] {
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => Some((metadata.len(), metadata.modified().ok()?)),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && path == binary.with_extension("pdb") =>
            {
                None
            }
            Err(_) => return None,
        };
        fingerprint.push_str(&format!(
            "{}\0{metadata:?}\n",
            fs::canonical(&path).display()
        ));
    }
    fingerprint.push_str(&format!("{}\0{}", paths.project.display(), argv.join("\0")));
    Some(paths.decompile_cache().join(format!(
        "{}.txt",
        crate::text::digest::sha256_hex(fingerprint.as_bytes())
    )))
}

pub fn get(file: &Path) -> Option<String> {
    fs::read_string(file)
        .ok()
        .flatten()
        .filter(|text| !text.trim().is_empty())
}

pub fn put(file: &Path, answer: &str) {
    if !answer.trim().is_empty() {
        let _ = fs::write_atomic(file, answer.as_bytes(), Access::Private);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_private_and_invalidated_by_inputs() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let program = root.path().join("kuna");
        let binary = root.path().join("program.exe");
        std::fs::write(&program, "kuna").unwrap();
        std::fs::write(&binary, "binary").unwrap();
        let ask = |question: &str| file(&paths, &program, &binary, &[question.into()]).unwrap();
        let first = ask("functions");
        assert_eq!(get(&first), None);
        put(&first, "main");
        assert_eq!(get(&ask("functions")).as_deref(), Some("main"));
        assert_ne!(first, ask("main"));
        let empty = ask("empty");
        put(&empty, " \n");
        assert!(!empty.exists());
        std::fs::write(binary.with_extension("pdb"), "symbols").unwrap();
        let symbols = ask("functions");
        assert_ne!(first, symbols);
        std::fs::write(&binary, "rebuilt binary").unwrap();
        let rebuilt = ask("functions");
        assert_ne!(symbols, rebuilt);
        std::fs::write(&program, "new version of kuna").unwrap();
        assert_ne!(rebuilt, ask("functions"));
        std::fs::remove_file(&binary).unwrap();
        assert!(file(&paths, &program, &binary, &[]).is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
