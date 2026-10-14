//! Two compact extension managers sharing search, details, and form controls.

use crate::app::App;
use crate::extensions::{Kind, Scope, text};
use crate::io::term::{KeyCode, KeyEvent, KeyModifiers};
use crate::text::styled::{Line, Style};
use crate::tui::boxed::boxed;
use crate::tui::editor::Editor;
use crate::tui::keymap::Action;
use crate::{io, limits, mcp, skills};
use futures_util::future::BoxFuture;

pub struct Manager {
    pub id: u64,
    kind: Kind,
    skills: Vec<skills::Skill>,
    servers: Vec<mcp::View>,
    selected: usize,
    filter: Editor,
    searching: bool,
    stage: Stage,
    note: Option<String>,
    trust_warning: String,
    cancel: tokio_util::sync::CancellationToken,
}

enum Stage {
    List,
    Detail {
        offset: usize,
        preview: String,
    },
    Form {
        inputs: Vec<Editor>,
        at: usize,
        scope: Scope,
    },
    Working,
    Signing {
        name: String,
        url: String,
    },
    Trust {
        offset: usize,
    },
}

pub enum Next {
    Stay,
    Close,
    Insert(String),
    Copy(String),
    Run(Box<Job>),
}

pub enum Job {
    Load,
    AddSkill {
        path: String,
        scope: Scope,
    },
    ToggleSkill(skills::Skill),
    PreviewSkill(skills::Skill),
    AddServer {
        name: String,
        config: mcp::Config,
        scope: Scope,
    },
    Connect(String),
    Refresh(String),
    ToggleServer(mcp::View),
    RemoveServer(mcp::View),
    Login(String),
    WaitLogin {
        name: String,
        login: Box<io::mcp_auth::Login>,
    },
    Logout(mcp::View),
    Trust,
}

pub enum Done {
    Loaded {
        id: u64,
        skills: Vec<skills::Skill>,
        servers: Vec<mcp::View>,
        warning: String,
        error: Option<String>,
    },
    Login {
        id: u64,
        name: String,
        login: Box<io::mcp_auth::Login>,
    },
    Preview {
        id: u64,
        body: String,
    },
}

enum Completion {
    Login(String, Box<io::mcp_auth::Login>),
    Preview(String),
}

pub fn run(
    app: &App,
    id: u64,
    job: Job,
    cancel: tokio_util::sync::CancellationToken,
) -> BoxFuture<'_, Done> {
    Box::pin(async move {
        let result = async {
            match job {
                Job::Load => {
                    app.mcp.reload(app)?;
                    skills::refresh(app).await?;
                }
                Job::AddSkill { path, scope } => skills::add_source(app, &path, scope).await?,
                Job::ToggleSkill(skill) => skills::toggle(app, &skill)?,
                Job::PreviewSkill(skill) => {
                    let body = tokio::task::spawn_blocking(move || {
                        skills::body(&skill).unwrap_or_else(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_default();
                    return Ok(Some(Completion::Preview(body)));
                }
                Job::AddServer {
                    name,
                    config,
                    scope,
                } => {
                    mcp::save(app, &name, &config, scope)?;
                    if !app
                        .mcp
                        .list()
                        .iter()
                        .any(|s| s.name == name && s.state == mcp::State::Untrusted)
                    {
                        app.mcp.connect(app, &name, true).await?;
                    }
                }
                Job::Connect(name) => {
                    app.mcp.connect(app, &name, true).await?;
                }
                Job::Refresh(name) => {
                    app.mcp.disconnect(&name);
                    app.mcp.connect(app, &name, true).await?;
                }
                Job::ToggleServer(mut server) => {
                    server.config.enabled = !server.config.enabled;
                    mcp::save(app, &server.name, &server.config, server.scope)?;
                    if server.config.enabled {
                        app.mcp.connect(app, &server.name, true).await?;
                    }
                }
                Job::RemoveServer(server) => {
                    mcp::remove(app, &server.name, server.scope)?;
                }
                Job::Login(name) => {
                    let server = app
                        .mcp
                        .list()
                        .into_iter()
                        .find(|s| s.name == name)
                        .ok_or_else(|| anyhow::anyhow!("MCP server was removed"))?;
                    if matches!(
                        server.state,
                        mcp::State::Untrusted | mcp::State::Review | mcp::State::Off
                    ) {
                        anyhow::bail!("Connect and review this server before signing in");
                    }
                    let challenge = app.mcp.challenge(&name);
                    let begin = io::mcp_auth::Login::begin(
                        &server.config,
                        mcp::credential_path(&app.paths, &server.config),
                        challenge.as_deref(),
                    );
                    let login = tokio::time::timeout(
                        limits::get().extensions.connect_timeout_secs.secs(),
                        begin,
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!("OAuth discovery timed out"))??;
                    if crate::settings::env("PALEPALE_NO_BROWSER").is_none() {
                        let _ = io::proc::open_url(login.url());
                    }
                    return Ok::<_, anyhow::Error>(Some(Completion::Login(name, Box::new(login))));
                }
                Job::WaitLogin { name, login } => {
                    login.finish(&cancel).await?;
                    app.mcp.disconnect(&name);
                    app.mcp.connect(app, &name, true).await?;
                }
                Job::Logout(server) => {
                    app.mcp.disconnect(&server.name);
                    io::mcp_auth::Store::new(mcp::credential_path(&app.paths, &server.config))
                        .logout()
                        .await?;
                }
                Job::Trust => {
                    crate::trust::trust(&app.paths)?;
                    app.mcp.reload(app)?;
                }
            }
            Ok(None)
        }
        .await;
        match result {
            Ok(Some(Completion::Login(name, login))) => return Done::Login { id, name, login },
            Ok(Some(Completion::Preview(body))) => return Done::Preview { id, body },
            _ => {}
        }
        let error = result.err().map(|e| format!("{e:#}"));
        let warning = crate::settings::load(&app.paths, &crate::settings::Overrides::default())
            .ok()
            .and_then(|s| s.untrusted)
            .map(|u| crate::trust::warning(&u.file, &u.keys))
            .unwrap_or_else(|| text().untrusted.clone());
        Done::Loaded {
            id,
            skills: skills::load(app).await,
            servers: app.mcp.list(),
            warning,
            error,
        }
    })
}

impl Manager {
    pub fn new(id: u64, kind: Kind) -> Self {
        Self {
            id,
            kind,
            skills: vec![],
            servers: vec![],
            selected: 0,
            filter: Editor::new(0),
            searching: false,
            stage: Stage::Working,
            note: None,
            trust_warning: String::new(),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    pub fn cancel(&self) -> tokio_util::sync::CancellationToken {
        self.cancel.clone()
    }

    pub fn sync_servers(&mut self, servers: Vec<mcp::View>) {
        if self.kind != Kind::Mcp {
            return;
        }
        self.servers = servers;
        self.selected = self.selected.min(self.indices().len().saturating_sub(1));
        if matches!(self.stage, Stage::Detail { .. }) {
            self.stage = match self.index() {
                Some(index) => Stage::Detail {
                    offset: 0,
                    preview: self.details(index, ""),
                },
                None => Stage::List,
            };
        }
    }

    fn indices(&self) -> Vec<usize> {
        let query = self.filter.text().to_lowercase();
        match self.kind {
            Kind::Skills => self
                .skills
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    format!("{} {}", s.name, s.description)
                        .to_lowercase()
                        .contains(&query)
                })
                .map(|(i, _)| i)
                .collect(),
            Kind::Mcp => self
                .servers
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    format!("{} {}", s.name, s.config.address())
                        .to_lowercase()
                        .contains(&query)
                })
                .map(|(i, _)| i)
                .collect(),
        }
    }

    fn index(&self) -> Option<usize> {
        self.indices().get(self.selected).copied()
    }

    fn work(&mut self, job: Job) -> Next {
        self.stage = Stage::Working;
        self.note = None;
        Next::Run(Box::new(job))
    }

    pub fn insert(&mut self, value: &str) {
        if let Stage::Form { inputs, at, .. } = &mut self.stage {
            inputs[*at].insert(value);
        } else if matches!(self.stage, Stage::List) {
            self.filter.insert(value);
            self.selected = 0;
        }
    }

    pub fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        if action == Some(Action::CtrlC) {
            return Next::Close;
        }
        if let Stage::Form { inputs, at, scope } = &mut self.stage {
            match action {
                Some(Action::Escape) => self.stage = Stage::List,
                Some(Action::Tab) => {
                    *scope = if *scope == Scope::User {
                        Scope::Project
                    } else {
                        Scope::User
                    }
                }
                Some(Action::Submit) => {
                    if self.kind == Kind::Mcp && *at == 1 && inputs.len() == 2 {
                        let remote = inputs[1].text().trim().starts_with("http");
                        let mut extra = Editor::new(0);
                        if !remote {
                            extra.set_text("[]");
                        }
                        inputs.push(extra);
                        if remote {
                            inputs.push(Editor::new(0));
                        }
                    }
                    if *at + 1 < inputs.len() {
                        *at += 1;
                        return Next::Stay;
                    }
                    let result = match self.kind {
                        Kind::Skills => Ok(Job::AddSkill {
                            path: inputs[0].text(),
                            scope: *scope,
                        }),
                        Kind::Mcp => {
                            let name = inputs[0].text().trim().to_owned();
                            let endpoint = inputs[1].text().trim().to_owned();
                            let mut config = mcp::Config::default();
                            if endpoint.starts_with("http") {
                                config.url = Some(endpoint);
                                config.bearer_token_env_var = nonempty(inputs[2].text());
                                config.client_id = nonempty(inputs[3].text());
                                config.validate().map(|()| Job::AddServer {
                                    name,
                                    config,
                                    scope: *scope,
                                })
                            } else {
                                config.command = Some(endpoint);
                                serde_json::from_str(&inputs[2].text())
                                    .map_err(anyhow::Error::from)
                                    .and_then(|args| {
                                        config.args = args;
                                        config.validate()?;
                                        Ok(Job::AddServer {
                                            name,
                                            config,
                                            scope: *scope,
                                        })
                                    })
                            }
                        }
                    };
                    match result {
                        Ok(job) => return self.work(job),
                        Err(e) => self.note = Some(e.to_string()),
                    }
                }
                Some(other) => {
                    inputs[*at].apply(other);
                }
                None => inputs[*at].type_key(key),
            }
            return Next::Stay;
        }
        if let Stage::Signing { url, .. } = &self.stage
            && key.code == KeyCode::Char('c')
            && key.modifiers.is_empty()
        {
            return Next::Copy(url.clone());
        }
        if matches!(self.stage, Stage::Working | Stage::Signing { .. }) {
            return if action == Some(Action::Escape) {
                Next::Close
            } else {
                Next::Stay
            };
        }
        if let Stage::Trust { offset } = &mut self.stage {
            return match action {
                Some(Action::Submit) => self.work(Job::Trust),
                Some(Action::Up) => {
                    *offset = offset.saturating_sub(1);
                    Next::Stay
                }
                Some(Action::Down) => {
                    *offset =
                        (*offset + 1).min(self.trust_warning.lines().count().saturating_sub(1));
                    Next::Stay
                }
                Some(Action::Escape) => {
                    self.stage = Stage::List;
                    Next::Stay
                }
                _ => Next::Stay,
            };
        }
        if let Stage::Detail { offset, preview } = &mut self.stage {
            match action {
                Some(Action::Escape | Action::Left) => self.stage = Stage::List,
                Some(Action::Up) => *offset = offset.saturating_sub(1),
                Some(Action::Down) => {
                    *offset = (*offset + 1).min(preview.lines().count().saturating_sub(1))
                }
                _ => {}
            }
            let Some(index) = self.index() else {
                self.stage = Stage::List;
                return Next::Stay;
            };
            if action == Some(Action::Submit) {
                match self.kind {
                    Kind::Skills
                        if self.skills[index].state == skills::State::Ready
                            && self.skills[index].invocable =>
                    {
                        return Next::Insert(format!("${} ", self.skills[index].name));
                    }
                    Kind::Mcp => return self.work(Job::Connect(self.servers[index].name.clone())),
                    _ => {}
                }
            }
            if let KeyCode::Char(letter) = key.code
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                match (self.kind, letter) {
                    (_, 't') => self.stage = Stage::Trust { offset: 0 },
                    (Kind::Skills, 'e') => {
                        return self.work(Job::ToggleSkill(self.skills[index].clone()));
                    }
                    (Kind::Mcp, 'e') => {
                        return self.work(Job::ToggleServer(self.servers[index].clone()));
                    }
                    (Kind::Mcp, 'r') => {
                        return self.work(Job::Refresh(self.servers[index].name.clone()));
                    }
                    (Kind::Mcp, 'd') => {
                        return self.work(Job::RemoveServer(self.servers[index].clone()));
                    }
                    (Kind::Mcp, 'l') => {
                        return self.work(Job::Login(self.servers[index].name.clone()));
                    }
                    (Kind::Mcp, 'o') => return self.work(Job::Logout(self.servers[index].clone())),
                    _ => {}
                }
            }
            return Next::Stay;
        }
        if self.searching {
            match action {
                Some(Action::Escape) => {
                    self.searching = false;
                    self.filter.clear();
                    self.selected = 0;
                }
                Some(Action::Submit | Action::Down) => self.searching = false,
                Some(other) => {
                    self.filter.apply(other);
                    self.selected = 0;
                }
                None => {
                    self.filter.type_key(key);
                    self.selected = 0;
                }
            }
            return Next::Stay;
        }
        match action {
            Some(Action::Escape | Action::Left) => return Next::Close,
            Some(Action::Up) => self.selected = self.selected.saturating_sub(1),
            Some(Action::Down) => {
                self.selected = (self.selected + 1).min(self.indices().len().saturating_sub(1))
            }
            Some(Action::Submit | Action::Right) => {
                if let Some(index) = self.index() {
                    if self.kind == Kind::Skills {
                        return self.work(Job::PreviewSkill(self.skills[index].clone()));
                    }
                    self.stage = Stage::Detail {
                        offset: 0,
                        preview: self.details(index, ""),
                    };
                }
            }
            _ => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    match key.code {
                        KeyCode::Char('a') => {
                            self.stage = Stage::Form {
                                inputs: (0..if self.kind == Kind::Skills { 1 } else { 2 })
                                    .map(|_| Editor::new(0))
                                    .collect(),
                                at: 0,
                                scope: Scope::User,
                            };
                            self.note = None;
                        }
                        KeyCode::Char('r') => return self.work(Job::Load),
                        KeyCode::Char('/') => self.searching = true,
                        KeyCode::Char(_) => {
                            self.searching = true;
                            self.filter.type_key(key);
                            self.selected = 0;
                        }
                        _ => {}
                    }
                }
            }
        }
        Next::Stay
    }

    pub fn finished(&mut self, done: Done) -> Next {
        match done {
            Done::Loaded {
                id,
                skills,
                servers,
                warning,
                error,
            } if id == self.id => {
                self.skills = skills;
                self.servers = servers;
                self.trust_warning = warning;
                self.note = error;
                self.selected = self.selected.min(self.indices().len().saturating_sub(1));
                self.stage = Stage::List;
            }
            Done::Login { id, name, login } if id == self.id => {
                self.stage = Stage::Signing {
                    name: name.clone(),
                    url: login.url().to_owned(),
                };
                return Next::Run(Box::new(Job::WaitLogin { name, login }));
            }
            Done::Preview { id, body } if id == self.id => {
                if let Some(index) = self.index() {
                    self.stage = Stage::Detail {
                        offset: 0,
                        preview: self.details(index, &body),
                    };
                } else {
                    self.stage = Stage::List;
                }
            }
            _ => {}
        }
        Next::Stay
    }

    fn details(&self, index: usize, body: &str) -> String {
        match self.kind {
            Kind::Skills => {
                let skill = &self.skills[index];
                [
                    format!(
                        "{} · {} · {}",
                        skill.name,
                        skill.state.label(),
                        skill.scope.label()
                    ),
                    skill.path.display().to_string(),
                    skill.description.clone(),
                    if skill.invocable {
                        String::new()
                    } else {
                        text().skill_implicit_only.clone()
                    },
                    skill.error.clone().unwrap_or_else(|| body.into()),
                ]
                .join("\n")
            }
            Kind::Mcp => {
                let server = &self.servers[index];
                let mut lines = vec![
                    format!(
                        "{} · {} · {}",
                        server.name,
                        server.state.label(),
                        server.scope.label()
                    ),
                    server.config.address(),
                    server.config.args.join(" "),
                    server.capabilities.join(" · "),
                ];
                if let Some(error) = &server.message {
                    lines.push(error.clone());
                }
                for (name, description) in &server.tools {
                    lines.push(format!("{name}: {description}"));
                }
                lines.join("\n")
            }
        }
    }

    pub fn render(&self, width: usize, height: usize) -> Vec<Line> {
        let width = width.min(limits::get().extensions.card_width.value).max(8);
        let inner = width.saturating_sub(4);
        let rows = height.saturating_sub(7).clamp(3, 14);
        let muted = Style::fg("muted");
        let title = match self.kind {
            Kind::Mcp => &text().mcp_title,
            Kind::Skills => &text().skills_title,
        };
        let mut body = Vec::new();
        let mut hint = &text().list_hint;
        match &self.stage {
            Stage::Working => body.push(Line::styled(&text().working, muted.clone())),
            Stage::List => {
                if self.searching {
                    body.extend(self.filter.render(
                        inner,
                        1,
                        &Line::plain("/ "),
                        &text().search_hint,
                    ));
                } else if !self.filter.is_empty() {
                    body.push(Line::styled(
                        format!("/ {}", self.filter.text()),
                        muted.clone(),
                    ));
                }
                let indices = self.indices();
                if indices.is_empty() {
                    body.push(Line::styled(
                        match self.kind {
                            Kind::Mcp => &text().empty_mcp,
                            Kind::Skills => &text().empty_skills,
                        },
                        muted.clone(),
                    ));
                }
                let visible = (rows / 2).max(1);
                let start = (self.selected + 1).saturating_sub(visible);
                for (position, index) in indices.into_iter().enumerate().skip(start).take(visible) {
                    let (name, state, scope, detail) = match self.kind {
                        Kind::Skills => {
                            let s = &self.skills[index];
                            (&s.name, s.state.label(), s.scope, s.description.clone())
                        }
                        Kind::Mcp => {
                            let s = &self.servers[index];
                            (
                                &s.name,
                                s.state.label(),
                                s.scope,
                                format!("{} tools", s.tools.len()),
                            )
                        }
                    };
                    let selected = position == self.selected;
                    let style = if selected {
                        Style::fg("accent").bold()
                    } else {
                        Style::default()
                    };
                    body.push(Line::styled(
                        format!(
                            "{}{}  {state}  {}",
                            if selected { "› " } else { "  " },
                            name,
                            scope.label()
                        ),
                        style,
                    ));
                    body.push(Line::styled(
                        format!(
                            "  {}",
                            detail.split_whitespace().collect::<Vec<_>>().join(" ")
                        ),
                        muted.clone(),
                    ));
                }
            }
            Stage::Detail { offset, preview } => {
                hint = if self.kind == Kind::Skills {
                    &text().skill_hint
                } else {
                    match self.index().map(|i| &self.servers[i]) {
                        Some(server) if server.state == mcp::State::Untrusted => {
                            &text().mcp_trust_hint
                        }
                        Some(server) if server.state == mcp::State::Review => {
                            &text().mcp_review_hint
                        }
                        Some(server) if server.config.command.is_some() => &text().mcp_local_hint,
                        _ => &text().detail_hint,
                    }
                };
                body.extend(preview.lines().skip(*offset).take(rows).map(Line::plain));
            }
            Stage::Form { inputs, at, scope } => {
                hint = &text().form_hint;
                let label = match (self.kind, at) {
                    (Kind::Skills, _) => &text().add_skill,
                    (Kind::Mcp, 0) => &text().add_name,
                    (Kind::Mcp, 1) => &text().add_endpoint,
                    (Kind::Mcp, 2) if inputs[1].text().trim().starts_with("http") => {
                        &text().add_bearer
                    }
                    (Kind::Mcp, 2) => &text().add_args,
                    _ => &text().add_client_id,
                };
                body.push(Line::styled(label, muted.clone()));
                body.extend(inputs[*at].render(
                    inner,
                    rows.min(3),
                    &Line::styled("› ", Style::fg("accent")),
                    "",
                ));
                body.push(Line::styled(
                    format!("{} · {}/{}", scope.label(), at + 1, inputs.len()),
                    muted.clone(),
                ));
            }
            Stage::Signing { name, url } => {
                hint = &text().login_hint;
                body.push(Line::plain(name));
                body.push(Line::styled(&text().login_wait, muted.clone()));
                body.push(Line::plain(&text().login_ready));
                for chunk in url.as_bytes().chunks(inner.max(1)) {
                    let mut style = Style::fg("accent");
                    style.link = Some(url.clone());
                    body.push(Line::styled(String::from_utf8_lossy(chunk), style));
                }
            }
            Stage::Trust { offset } => {
                hint = &text().trust_hint;
                body.extend(
                    self.trust_warning
                        .lines()
                        .skip(*offset)
                        .take(rows)
                        .map(Line::plain),
                );
            }
        }
        if let Some(note) = &self.note {
            body.extend(
                note.lines()
                    .take(2)
                    .map(|s| Line::styled(s, Style::fg("error"))),
            );
        }
        body.truncate(height.saturating_sub(3).max(1));
        let mut lines = boxed(Line::styled(title, Style::default().bold()), &body, width);
        lines.push(Line::styled(hint, muted).truncate(width));
        lines
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
fn nonempty(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_connections_update_the_card_without_discarding_an_open_form() {
        let mut card = Manager::new(1, Kind::Mcp);
        card.finished(Done::Loaded {
            id: 1,
            skills: vec![],
            servers: vec![],
            warning: String::new(),
            error: None,
        });
        let add = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        card.key(None, &add);
        card.insert("work-in-progress");
        card.sync_servers(vec![mcp::View {
            name: "docs".into(),
            config: mcp::Config::default(),
            scope: Scope::User,
            state: mcp::State::Connected,
            message: None,
            tools: vec![],
            capabilities: vec![],
        }]);
        let screen = card
            .render(64, 24)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("work-in-progress"));
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        card.key(Some(Action::Escape), &escape);
        let screen = card
            .render(64, 24)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("docs  connected"));
    }
    #[test]
    fn forms_and_empty_lists_fit_small_terminals_and_escape_closes() {
        let mut card = Manager::new(1, Kind::Skills);
        card.finished(Done::Loaded {
            id: 1,
            skills: vec![],
            servers: vec![],
            warning: String::new(),
            error: None,
        });
        for width in [32, 48, 64, 100] {
            let lines = card.render(width, 20);
            assert!(lines.iter().all(|l| l.width() <= width));
            assert!(lines.len() <= 20);
        }
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(card.key(Some(Action::Escape), &key), Next::Close));
    }
}
