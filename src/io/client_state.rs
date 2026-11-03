use super::fs::{self, Access};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const FILE: &str = "palepale-connection.json";

#[derive(Deserialize, Serialize)]
struct State {
    before: Value,
    installed: Value,
}

pub fn remember(
    root: &Path,
    current: &Value,
    installed: &Value,
) -> anyhow::Result<(PathBuf, Vec<u8>)> {
    let before = restore(root, current)?.unwrap_or_else(|| current.clone());
    Ok((
        root.join(FILE),
        serde_json::to_vec_pretty(&State {
            before,
            installed: installed.clone(),
        })?,
    ))
}

pub fn restore(root: &Path, current: &Value) -> anyhow::Result<Option<Value>> {
    let Some(saved) = fs::read_string(&root.join(FILE))? else {
        return Ok(None);
    };
    let state: State = serde_json::from_str(&saved)
        .context("Invalid Palepale connection backup; no settings were changed")?;
    let mut restored = undo(Some(current), Some(&state.before), Some(&state.installed));
    let owns_model = (current["model_provider"] == "palepale"
        && state.installed["model_provider"] == "palepale")
        || (current.get("apiKeyHelper") == state.installed.get("apiKeyHelper")
            && state.installed["modelPicker"]["options"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|r| r.get("model") == current.get("model"))));
    if owns_model && let Some(object) = restored.as_mut().and_then(Value::as_object_mut) {
        match state.before.get("model") {
            Some(value) => {
                object.insert("model".into(), value.clone());
            }
            None => {
                object.remove("model");
            }
        }
    }
    Ok(restored)
}

// Undo only our edits. A setting edited after setup belongs to the user.
fn undo(
    current: Option<&Value>,
    before: Option<&Value>,
    installed: Option<&Value>,
) -> Option<Value> {
    if before == installed {
        return current.cloned();
    }
    if current == installed {
        return before.cloned();
    }
    if let (Some(Value::Object(current)), Some(Value::Object(installed))) = (current, installed)
        && before.is_none_or(Value::is_object)
    {
        let mut restored = current.clone();
        let keys: BTreeSet<_> = installed
            .keys()
            .chain(
                before
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|m| m.keys()),
            )
            .collect();
        for key in keys {
            match undo(
                current.get(key),
                before.and_then(|b| b.get(key)),
                installed.get(key),
            ) {
                Some(value) => {
                    restored.insert(key.clone(), value);
                }
                None => {
                    restored.remove(key);
                }
            }
        }
        if restored.is_empty() && before.is_none() {
            return None;
        }
        return Some(Value::Object(restored));
    }
    current.cloned()
}

pub fn forget(root: &Path, artifacts: &[&str]) -> anyhow::Result<()> {
    for name in artifacts.iter().copied().chain([FILE]) {
        fs::remove(&root.join(name))?;
    }
    // Older installers backed up this app's credential when reconnecting.
    if root.is_dir() {
        for entry in fs::list_dir(root)? {
            let name = &entry.name;
            if let Some(suffix) = name
                .strip_prefix("palepale.palepale-")
                .and_then(|s| s.strip_suffix(".bak"))
                && suffix.len() == 16
                && suffix.bytes().all(|b| b.is_ascii_hexdigit())
                && !entry.is_dir
            {
                fs::remove(&root.join(&entry.name))?;
            }
        }
    }
    Ok(())
}

pub fn save(path: &Path, value: &[u8]) -> anyhow::Result<()> {
    fs::write_atomic(path, value, Access::Private)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn restore_preserves_later_edits_and_removes_only_inserted_fields() {
        let before = json!({"model":"old","env":{"KEEP":"old"}});
        let installed =
            json!({"model":"new","env":{"KEEP":"old","URL":"ours"},"provider":{"ours":{}}});
        let current = json!({"model":"user","env":{"KEEP":"changed","URL":"ours"},"provider":{"ours":{},"other":{}}});
        assert_eq!(
            undo(Some(&current), Some(&before), Some(&installed)),
            Some(json!({"model":"user","env":{"KEEP":"changed"},"provider":{"other":{}}}))
        );
    }
}
