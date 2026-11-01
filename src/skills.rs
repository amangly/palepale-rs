//! Agent Skills discovery and invocation. Files are read only through the I/O boundary.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::agent::event::AgentEvent;
use crate::agent::message::{Message, Part};
use crate::agent::prompt::{self, Prompt, Reply};
use crate::agent::state::Shared;
use crate::app::App;
use crate::extensions::{Scope, text};
use crate::permission::gate::{Decision, Policy};
use crate::text::template::fill;
use crate::{io, limits, settings, trust};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    #[serde(default)]
    pub disabled: Vec<PathBuf>,
    #[serde(default)]
    pub urls: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub scope: Scope,
    pub state: State,
    pub error: Option<String>,
    pub explicit_only: bool,
    pub invocable: bool,
    pub slash: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ready,
    Disabled,
    Untrusted,
    Shadowed,
    Invalid,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Disabled => "off",
            Self::Untrusted => "untrusted",
            Self::Shadowed => "shadowed",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Deserialize)]
struct Metadata {
    name: String,
    description: String,
    #[serde(default, rename = "disable-model-invocation")]
    explicit_only: bool,
    #[serde(default = "invocable", rename = "user-invocable")]
    invocable: bool,
    #[serde(default)]
    metadata: std::collections::BTreeMap<String, serde_yaml_ng::Value>,
    compatibility: Option<String>,
    slash: Option<bool>,
}

fn invocable() -> bool {
    true
}

fn parse(source: &str, folder: &str) -> anyhow::Result<(Metadata, String)> {
    let source = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let rest = source
        .strip_prefix("---\n")
        .context("SKILL.md needs YAML frontmatter")?;
    let (header, body) = rest
        .split_once("\n---\n")
        .or_else(|| rest.strip_suffix("\n---").map(|header| (header, "")))
        .context("Missing closing frontmatter marker")?;
    let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(header)?;
    for field in ["context", "agent", "hooks"] {
        if document.get(field).is_some() {
            bail!("Unsupported skill execution field: {field}");
        }
    }
    let mut metadata: Metadata = serde_yaml_ng::from_value(document)?;
    if metadata
        .compatibility
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.chars().count() > 500)
    {
        bail!("Skill compatibility must contain 1–500 characters");
    }
    for key in ["opencode/autoinvoke", "palepale/autoinvoke"] {
        if metadata
            .metadata
            .get(key)
            .is_some_and(|v| v.as_bool() == Some(false) || v.as_str() == Some("false"))
        {
            metadata.explicit_only = true;
        }
    }
    if !valid_name(&metadata.name) || metadata.name != folder {
        bail!(
            "Skill name must match its folder and contain 1–64 lowercase letters, digits or single hyphens"
        );
    }
    if metadata.description.trim().is_empty() || metadata.description.chars().count() > 1024 {
        bail!("Skill description must contain 1–1024 characters");
    }
    Ok((metadata, body.trim().to_owned()))
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

pub fn discover(app: &App) -> Vec<Skill> {
    let config = settings::load(&app.paths, &settings::Overrides::default())
        .map(|s| s.skills)
        .unwrap_or_else(|_| app.settings.skills.clone());
    discover_at(&app.paths, config)
}

pub async fn load(app: &App) -> Vec<Skill> {
    let paths = app.paths.clone();
    let config = settings::load(&paths, &settings::Overrides::default())
        .map(|s| s.skills)
        .unwrap_or_else(|_| app.settings.skills.clone());
    let mut errors = Vec::new();
    for url in &config.urls {
        if let Err(error) = io::skill_cache::pull(url, &paths.home, false).await {
            errors.push(Skill {
                name: "remote-skills".into(),
                description: url.clone(),
                path: io::skill_cache::source_dir(url, &paths.home),
                scope: Scope::User,
                state: State::Invalid,
                error: Some(error.to_string()),
                explicit_only: true,
                invocable: false,
                slash: false,
            });
        }
    }
    let mut found = tokio::task::spawn_blocking(move || discover_at(&paths, config))
        .await
        .unwrap_or_default();
    found.extend(errors);
    found
}

fn discover_at(paths: &crate::paths::Paths, config: Config) -> Vec<Skill> {
    let trusted = trust::is_trusted(paths).unwrap_or(false);
    let mut roots = Vec::new();
    let mut repo = None;
    for dir in paths.project.ancestors() {
        if io::fs::kind(&dir.join(".git")).is_ok_and(|k| k != io::fs::Kind::Missing) {
            repo = Some(dir.to_path_buf());
            break;
        }
    }
    for dir in paths.project.ancestors() {
        for root in &text().project_skill_roots {
            roots.push((dir.join(root), Scope::Project));
        }
        if repo.as_deref().is_none_or(|root| dir == root) {
            break;
        }
    }
    if settings::env("PALEPALE_NO_USER_SKILLS").is_none()
        && let Some(home) = io::fs::home_dir()
    {
        for root in &text().user_skill_roots {
            roots.push((home.join(root), Scope::User));
        }
    }
    roots.push((paths.home.join("skills"), Scope::User));
    for path in &config.paths {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            paths.project.join(path)
        };
        let scope = if io::fs::canonical(&path).starts_with(io::fs::canonical(&paths.project)) {
            Scope::Project
        } else {
            Scope::User
        };
        roots.push((path, scope));
    }
    for url in &config.urls {
        roots.extend(
            io::skill_cache::cached(url, &paths.home)
                .into_iter()
                .map(|path| (path, Scope::User)),
        );
    }
    roots.sort_by_key(|(_, scope)| if *scope == Scope::Project { 0 } else { 1 });
    let disabled: HashSet<PathBuf> = config
        .disabled
        .iter()
        .map(|p| {
            io::fs::canonical(&if p.is_absolute() {
                p.clone()
            } else {
                paths.project.join(p)
            })
        })
        .collect();
    let mut seen = HashSet::new();
    let mut names = HashSet::new();
    let mut skills = Vec::new();
    for (root, scope) in roots {
        let direct = if root.file_name().is_some_and(|n| n == "SKILL.md") {
            root.clone()
        } else {
            root.join("SKILL.md")
        };
        let candidates = if io::fs::kind(&direct).is_ok_and(|k| k == io::fs::Kind::File) {
            vec![direct]
        } else {
            io::fs::skill_files(
                &root,
                limits::get().extensions.skill_depth.value,
                limits::get().extensions.skill_count.value,
            )
        };
        for candidate in candidates {
            if skills.len() >= limits::get().extensions.skill_count.value {
                break;
            }
            let path = io::fs::canonical(&candidate);
            if !seen.insert(path.clone()) {
                continue;
            }
            let folder = path
                .parent()
                .and_then(Path::file_name)
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let parsed =
                io::fs::read_limited(&path, limits::get().extensions.skill_file_bytes.value)
                    .map_err(anyhow::Error::from)
                    .and_then(|s| s.context("SKILL.md not found"))
                    .and_then(|s| parse(&s, &folder));
            let (name, description, error, explicit_only, invocable, slash) = match parsed {
                Ok((meta, _)) => (
                    meta.name,
                    meta.description,
                    None,
                    meta.explicit_only,
                    meta.invocable,
                    meta.slash.unwrap_or_else(|| {
                        !["opencode/slash", "palepale/slash"].iter().any(|key| {
                            meta.metadata.get(*key).is_some_and(|v| {
                                v.as_bool() == Some(false) || v.as_str() == Some("false")
                            })
                        })
                    }),
                ),
                Err(e) => (
                    folder,
                    String::new(),
                    Some(e.to_string()),
                    false,
                    false,
                    false,
                ),
            };
            let first = if error.is_none() && (scope != Scope::Project || trusted) {
                names.insert(name.clone())
            } else {
                true
            };
            let state = if error.is_some() {
                State::Invalid
            } else if disabled.contains(&path) {
                State::Disabled
            } else if scope == Scope::Project && !trusted {
                State::Untrusted
            } else if !first {
                State::Shadowed
            } else {
                State::Ready
            };
            skills.push(Skill {
                name,
                description,
                path,
                scope,
                state,
                error,
                explicit_only,
                invocable,
                slash,
            });
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

pub fn body(skill: &Skill) -> anyhow::Result<String> {
    let source =
        io::fs::read_limited(&skill.path, limits::get().extensions.skill_file_bytes.value)?
            .context("SKILL.md no longer exists")?;
    let folder = skill
        .path
        .parent()
        .and_then(Path::file_name)
        .unwrap_or_default()
        .to_string_lossy();
    let (metadata, body) = parse(&source, &folder)?;
    if metadata.name != skill.name {
        bail!("Skill changed; refresh /skills");
    }
    Ok(body)
}

pub fn catalog(skills: &[Skill]) -> String {
    let mut lines = String::new();
    let max = limits::get().extensions.skill_catalog_chars.value;
    for skill in skills
        .iter()
        .filter(|s| s.state == State::Ready && !s.explicit_only)
    {
        let description = skill
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let line = format!(
            "- {}: {} (file: {})\n",
            skill.name,
            description,
            skill.path.display()
        );
        if lines.chars().count() + line.chars().count() > max {
            lines.push_str(&text().catalog_cut);
            break;
        }
        lines.push_str(&line);
    }
    if lines.is_empty() {
        String::new()
    } else {
        fill(&text().skills_frame, &[("skills", &lines)])
    }
}

fn invocations(parts: &[Part]) -> Vec<String> {
    let mut names = Vec::new();
    for part in parts {
        if let Part::Text { text } = part {
            for word in text.split_whitespace() {
                if let Some(name) = word.strip_prefix('$')
                    && valid_name(name)
                    && !names.iter().any(|n| n == name)
                {
                    names.push(name.to_owned());
                }
            }
        }
    }
    names
}

pub async fn expand(
    app: &App,
    shared: &Shared,
    emit: &(dyn Fn(AgentEvent) + Send + Sync),
    plan_file: &str,
    cancel: &tokio_util::sync::CancellationToken,
    history: &[Message],
    parts: &mut Vec<Part>,
) {
    let skills = load(app).await;
    let names = invocations(parts);
    let mut catalog = visible_catalog(&skills, app, shared, plan_file);
    let previous = last_catalog(history, "<available_skills>");
    if catalog.is_empty() && previous.is_some() {
        catalog = fill(&text().skills_frame, &[("skills", "")]);
    }
    if !catalog.is_empty() && previous != Some(catalog.as_str()) {
        parts.push(Part::Reminder { text: catalog });
    }
    for name in names {
        let result = async {
            let skill = skills
                .iter()
                .find(|s| s.name == name && s.state == State::Ready && s.invocable)
                .with_context(|| {
                    format!("Skill is missing, disabled or untrusted; check /skills: {name}")
                })?;
            let target = crate::tools::target::resolve(&app.paths, &skill.path.to_string_lossy())
                .map_err(anyhow::Error::msg)?;
            let mut asks = crate::tools::target::asks("read", &target);
            asks.push(crate::permission::gate::Ask::new(
                crate::tools::permissions::SKILL.name,
                &name,
            ));
            let policy = Policy {
                rules: &app.settings.permission,
                mode: shared.mode(),
                plan_file,
            };
            match policy.decide(&asks) {
                Decision::Allow => {}
                Decision::Deny(why) => bail!("Read denied: {why}"),
                Decision::Ask => {
                    let prompt = Prompt::Permission {
                        tool: format!("skill:{name}"),
                        asks,
                        preview: Some(target.shown),
                        confirm: false,
                    };
                    if prompt::ask(shared, emit, &format!("skill:{name}"), prompt, cancel).await
                        != Some(Reply::Yes)
                    {
                        bail!("Skill read was not approved");
                    }
                }
            }
            context(skill)
        }
        .await;
        let context = match result {
            Ok(context) => context,
            Err(e) => fill(
                &text().skill_error,
                &[("name", &name), ("error", &format!("{e:#}"))],
            ),
        };
        parts.push(Part::Reminder { text: context });
    }
}

pub fn visible_catalog(skills: &[Skill], app: &App, shared: &Shared, plan_file: &str) -> String {
    let policy = Policy {
        rules: &app.settings.permission,
        mode: shared.mode(),
        plan_file,
    };
    let visible: Vec<_> = skills
        .iter()
        .filter(|s| {
            !matches!(
                policy.decide(&[crate::permission::gate::Ask::new("skill", &s.name)]),
                Decision::Deny(_)
            )
        })
        .cloned()
        .collect();
    catalog(&visible)
}

pub fn context(skill: &Skill) -> anyhow::Result<String> {
    let instructions = body(skill)?;
    let directory = skill.path.parent().context("Skill has no directory")?;
    let files = io::fs::skill_support_files(
        directory,
        limits::get().extensions.skill_depth.value,
        limits::get().extensions.skill_support_files.value,
    )
    .iter()
    .map(|p| p.display().to_string())
    .collect::<Vec<_>>()
    .join("\n");
    Ok(fill(
        &text().skill_frame,
        &[
            ("name", &skill.name),
            ("path", &skill.path.to_string_lossy()),
            ("directory", &directory.to_string_lossy()),
            ("body", &instructions),
            ("files", &files),
        ],
    ))
}

pub fn last_catalog<'a>(history: &'a [Message], prefix: &str) -> Option<&'a str> {
    history.iter().rev().find_map(|message| match message {
        Message::User { parts } => parts.iter().rev().find_map(|part| match part {
            Part::Reminder { text } if text.starts_with(prefix) => Some(text.as_str()),
            _ => None,
        }),
        _ => None,
    })
}

pub fn add(app: &App, path: &str, scope: Scope) -> anyhow::Result<()> {
    let path = Path::new(path.trim());
    let path = io::fs::canonical(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        app.paths.project.join(path)
    });
    if io::fs::kind(&path)? == io::fs::Kind::Missing {
        bail!("Folder does not exist");
    }
    settings::edit(&app.paths, scope == Scope::Project, |doc| {
        let mut paths = doc
            .get("skills")
            .and_then(|i| i.get("paths"))
            .and_then(toml_edit::Item::as_array)
            .cloned()
            .unwrap_or_default();
        let value = path.to_string_lossy();
        if !paths.iter().any(|v| v.as_str() == Some(value.as_ref())) {
            paths.push(value.as_ref());
        }
        doc["skills"]["paths"] = toml_edit::value(paths);
        Ok(())
    })
}

pub async fn add_source(app: &App, source: &str, scope: Scope) -> anyhow::Result<()> {
    if source.starts_with("https://") || source.starts_with("http://") {
        io::skill_cache::validate(source)?;
        settings::edit(&app.paths, scope == Scope::Project, |doc| {
            let mut urls = doc
                .get("skills")
                .and_then(|s| s.get("urls"))
                .and_then(toml_edit::Item::as_array)
                .cloned()
                .unwrap_or_default();
            if !urls.iter().any(|s| s.as_str() == Some(source)) {
                urls.push(source);
            }
            doc["skills"]["urls"] = toml_edit::value(urls);
            Ok(())
        })?;
        if scope == Scope::User || trust::is_trusted(&app.paths)? {
            io::skill_cache::pull(source, &app.paths.home, true).await?;
        }
        Ok(())
    } else {
        add(app, source, scope)
    }
}

pub async fn refresh(app: &App) -> anyhow::Result<()> {
    let settings = settings::load(&app.paths, &settings::Overrides::default())?;
    for url in settings.skills.urls {
        io::skill_cache::pull(&url, &app.paths.home, true).await?;
    }
    Ok(())
}

pub fn remove_source(app: &App, source: &str, scope: Scope) -> anyhow::Result<()> {
    let remote = source.starts_with("http://") || source.starts_with("https://");
    let path = if Path::new(source).is_absolute() {
        PathBuf::from(source)
    } else {
        app.paths.project.join(source)
    };
    let target = io::fs::canonical(&path);
    settings::edit(&app.paths, scope == Scope::Project, |doc| {
        let field = if remote { "urls" } else { "paths" };
        if let Some(items) = doc
            .get_mut("skills")
            .and_then(|s| s.get_mut(field))
            .and_then(toml_edit::Item::as_array_mut)
        {
            items.retain(|item| {
                item.as_str().is_none_or(|v| {
                    if remote {
                        v != source
                    } else {
                        let path = if Path::new(v).is_absolute() {
                            PathBuf::from(v)
                        } else {
                            app.paths.project.join(v)
                        };
                        io::fs::canonical(&path) != target
                    }
                })
            });
        }
        Ok(())
    })
}

pub fn toggle(app: &App, skill: &Skill) -> anyhow::Result<()> {
    if skill.state == State::Disabled {
        for project in [false, true] {
            let file = if project {
                app.paths.project_config()
            } else {
                app.paths.user_config()
            };
            let Some(source) = io::fs::read_string(&file)? else {
                continue;
            };
            let document: toml::Table = toml::from_str(&source)?;
            if document
                .get("skills")
                .and_then(|s| s.get("disabled"))
                .is_none()
            {
                continue;
            }
            settings::edit(&app.paths, project, |doc| {
                let old = doc["skills"]["disabled"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let mut updated = toml_edit::Array::new();
                for value in old.iter() {
                    let matches = value.as_str().is_some_and(|s| {
                        let path = Path::new(s);
                        let path = if path.is_absolute() {
                            path.to_path_buf()
                        } else {
                            app.paths.project.join(path)
                        };
                        io::fs::canonical(&path) == skill.path
                    });
                    if !matches {
                        updated.push(value.clone());
                    }
                }
                doc["skills"]["disabled"] = toml_edit::value(updated);
                Ok(())
            })?;
        }
        return Ok(());
    }
    settings::edit(&app.paths, false, |doc| {
        let mut disabled = doc
            .get("skills")
            .and_then(|i| i.get("disabled"))
            .and_then(toml_edit::Item::as_array)
            .cloned()
            .unwrap_or_default();
        let path = skill.path.to_string_lossy();
        let position = disabled
            .iter()
            .position(|v| v.as_str() == Some(path.as_ref()));
        if let Some(at) = position {
            disabled.remove(at);
        } else {
            disabled.push(path.as_ref());
        }
        doc["skills"]["disabled"] = toml_edit::value(disabled);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_skill_tool_loads_grouped_compatibility_folders_and_supporting_files() {
        use crate::tools::{Registry, testing::Project};
        let project = Project::new();
        let root = project
            .app
            .paths
            .project
            .join(".palepale/skills/team/review");
        io::fs::write_atomic(&root.join("SKILL.md"), b"---\nname: review\ndescription: Review native changes\nlicense: MIT\ncompatibility: Palepale\nmetadata:\n  author: team\n---\nRead references/checks.md before editing.", io::fs::Access::Private).unwrap();
        io::fs::write_atomic(
            &root.join("references/checks.md"),
            b"Supporting reference marker",
            io::fs::Access::Private,
        )
        .unwrap();
        io::fs::write_atomic(
            &root.join("scripts/run.sh"),
            b"exit 99",
            io::fs::Access::Private,
        )
        .unwrap();
        trust::trust(&project.app.paths).unwrap();
        let skills = discover(&project.app);
        assert!(
            skills
                .iter()
                .any(|s| s.name == "review" && s.state == State::Ready)
        );
        let registry = Registry::builtin(&project.app);
        let skill = registry.get("skill").unwrap();
        let cx = project.cx();
        let check = skill
            .check(&serde_json::json!({"name":"review"}), &cx)
            .unwrap();
        assert!(
            check
                .asks
                .iter()
                .any(|a| a.permission == "skill" && a.patterns == ["review"])
        );
        let output = skill.run(serde_json::json!({"name":"review"}), &cx).await;
        assert!(!output.is_error, "{}", output.content);
        assert!(
            output.content.contains("Base directory:")
                && output.content.contains("checks.md")
                && output.content.contains("run.sh")
        );
        assert!(
            !output.content.contains("Supporting reference marker")
                && !output.content.contains("exit 99")
        );
    }

    #[tokio::test]
    async fn compatible_metadata_and_disabled_skills_are_excluded_from_automatic_loading() {
        use crate::tools::{Registry, testing::Project};
        let project = Project::new();
        let file = project
            .app
            .paths
            .project
            .join(".claude/skills/group/review/SKILL.md");
        io::fs::write_atomic(&file, b"---\nname: review\ndescription: Review changes\nmetadata:\n  opencode/autoinvoke: false\n---\nManual workflow body", io::fs::Access::Private).unwrap();
        trust::trust(&project.app.paths).unwrap();
        let skills = discover(&project.app);
        let skill = skills.iter().find(|s| s.name == "review").unwrap();
        assert!(skill.explicit_only);
        assert!(!catalog(&skills).contains("- review:"));
        let registry = Registry::builtin(&project.app);
        assert!(
            registry
                .get("skill")
                .unwrap()
                .check(&serde_json::json!({"name":"review"}), &project.cx())
                .is_err()
        );
        let mut parts = vec![Part::Text {
            text: "$review verify".into(),
        }];
        let cx = project.cx();
        expand(
            &project.app,
            cx.shared,
            cx.emit,
            cx.plan_file,
            &cx.cancel,
            &[],
            &mut parts,
        )
        .await;
        assert!(
            parts.iter().any(
                |p| matches!(p,Part::Reminder { text } if text.contains("Manual workflow body"))
            )
        );
    }
    #[test]
    fn frontmatter_supports_multiline_unicode_and_plain_arguments() {
        let (meta, body) = parse("---\nname: review\ndescription: >\n  Check changes\n  carefully.\n---\nUse $ARGUMENTS as text.", "review").unwrap();
        assert!(meta.description.contains("Check changes carefully."));
        assert!(body.contains("$ARGUMENTS"));
        for name in ["", "Bad", "-bad", "bad-", "bad--name", "../bad"] {
            assert!(!valid_name(name));
        }
        assert!(
            parse(
                "---\nname: review\ndescription: Check\ncontext: fork\n---\nRun",
                "review"
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn skill_name_denials_hide_the_catalog_and_reach_the_normal_gate() {
        use crate::permission::{
            gate::Decision,
            rule::{Action, Rule},
        };
        use crate::tools::{Registry, testing::Project};
        let mut project = Project::new();
        let file = project
            .app
            .paths
            .project
            .join(".agents/skills/private-review/SKILL.md");
        io::fs::write_atomic(&file, b"---\nname: private-review\ndescription: Private review workflow\n---\nPrivate instructions", io::fs::Access::Private).unwrap();
        trust::trust(&project.app.paths).unwrap();
        project.app.settings.permission.push(Rule {
            permission: "skill".into(),
            pattern: "private-*".into(),
            action: Action::Deny,
        });
        project.shared.set_mode(crate::mode::get("manual").unwrap());
        let cx = project.cx();
        assert!(
            !visible_catalog(
                &discover(&project.app),
                &project.app,
                cx.shared,
                cx.plan_file
            )
            .contains("- private-review:")
        );
        let registry = Registry::builtin(&project.app);
        let check = registry
            .get("skill")
            .unwrap()
            .check(&serde_json::json!({"name":"private-review"}), &cx)
            .unwrap();
        let mode = project.shared.mode();
        let policy = Policy {
            mode,
            rules: &project.app.settings.permission,
            plan_file: cx.plan_file,
        };
        assert!(
            check
                .asks
                .iter()
                .any(|a| matches!(policy.decide(std::slice::from_ref(a)), Decision::Deny(_)))
        );
    }

    #[tokio::test]
    async fn explicit_skills_obey_reads_and_keep_their_arguments_literal() {
        use crate::tools::testing::Project;
        let mut project = Project::new();
        let file = project
            .app
            .paths
            .project
            .join(".agents/skills/native-check/SKILL.md");
        io::fs::write_atomic(&file, b"---\nname: native-check\ndescription: Check edits\n---\nPrivate workflow body marker.", io::fs::Access::Private).unwrap();
        trust::trust(&project.app.paths).unwrap();
        let mut parts = vec![Part::Text {
            text: "$native-check inspect $(literal)".into(),
        }];
        let cx = project.cx();
        expand(
            &project.app,
            cx.shared,
            cx.emit,
            cx.plan_file,
            &cx.cancel,
            &[],
            &mut parts,
        )
        .await;
        assert!(
            matches!(&parts[0], Part::Text { text } if text == "$native-check inspect $(literal)")
        );
        assert!(parts.iter().any(|p| matches!(p, Part::Reminder { text } if text.contains("Private workflow body marker."))));
        project
            .app
            .settings
            .permission
            .push(crate::permission::rule::Rule {
                permission: "read".into(),
                pattern: "*".into(),
                action: crate::permission::rule::Action::Deny,
            });
        project.shared.set_mode(crate::mode::get("plan").unwrap());
        let mut parts = vec![Part::Text {
            text: "$native-check inspect".into(),
        }];
        let cx = project.cx();
        expand(
            &project.app,
            cx.shared,
            cx.emit,
            cx.plan_file,
            &cx.cancel,
            &[],
            &mut parts,
        )
        .await;
        assert!(
            parts
                .iter()
                .any(|p| matches!(p, Part::Reminder { text } if text.contains("Read denied")))
        );
        assert!(!parts.iter().any(|p| matches!(p, Part::Reminder { text } if text.contains("Private workflow body marker."))));
    }

    #[test]
    fn explicit_only_skills_stay_out_of_the_automatic_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(dir.path());
        let path = dir.path().join(".agents/skills/explicit-workflow/SKILL.md");
        io::fs::write_atomic(&path, b"---\nname: explicit-workflow\ndescription: Run only on request\ndisable-model-invocation: true\n---\nDo work", io::fs::Access::Private).unwrap();
        trust::trust(&app.paths).unwrap();
        let skills = discover(&app);
        let skill = skills
            .iter()
            .find(|s| s.name == "explicit-workflow")
            .unwrap();
        assert!(skill.explicit_only && skill.invocable);
        assert!(!catalog(&skills).contains("explicit-workflow"));
    }

    #[test]
    fn discovery_handles_trust_invalid_files_and_shadowing() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(dir.path());
        let path = dir.path().join(".agents/skills/review/SKILL.md");
        io::fs::write_atomic(
            &path,
            b"---\nname: review\ndescription: Check edits\n---\nInspect diff",
            io::fs::Access::Private,
        )
        .unwrap();
        assert_eq!(
            discover(&app)
                .iter()
                .find(|s| s.name == "review")
                .unwrap()
                .state,
            State::Untrusted
        );
        trust::trust(&app.paths).unwrap();
        let skill = discover(&app)
            .into_iter()
            .find(|s| s.name == "review")
            .unwrap();
        assert_eq!(skill.state, State::Ready);
        assert_eq!(body(&skill).unwrap(), "Inspect diff");
        toggle(&app, &skill).unwrap();
        assert_eq!(
            discover(&app)
                .iter()
                .find(|s| s.name == "review")
                .unwrap()
                .state,
            State::Disabled
        );
        toggle(&app, &skill).unwrap();
        assert_eq!(
            discover(&app)
                .iter()
                .find(|s| s.name == "review")
                .unwrap()
                .state,
            State::Ready
        );
        io::fs::write_atomic(&path, b"missing metadata", io::fs::Access::Private).unwrap();
        assert_eq!(
            discover(&app)
                .iter()
                .find(|s| s.name == "review")
                .unwrap()
                .state,
            State::Invalid
        );
    }
}
