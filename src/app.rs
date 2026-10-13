use crate::api::Api;
use crate::io::fetch::Fetch;
use crate::io::http::Http;
use crate::paths::Paths;
use crate::settings::{self, Overrides, Settings};
use crate::{auth, limits};

/// What every command and the agent run against, built once at startup.
pub struct App {
    pub paths: Paths,
    pub settings: Settings,
    /// palepale's endpoints.
    pub api: Api,
    /// Everything else on the web (the webfetch tool).
    pub fetch: Fetch,
    pub mcp: std::sync::Arc<crate::mcp::Runtime>,
}

impl App {
    pub fn load(flags: &Overrides) -> anyhow::Result<Self> {
        let paths = Paths::resolve()?;
        let settings = settings::load(&paths, flags)?;
        let http = Http::new(limits::get().http.connect_timeout_ms.ms())?;
        // An unreadable auth.json must not stop every command, `logout` (which removes it) and
        // `login --status` (which explains it) included; it just leaves this run without a key.
        let key = if crate::api::endpoints::origin("api").is_empty() {
            None
        } else {
            auth::load(&paths).ok().flatten().map(|k| k.secret)
        };
        let app = Self {
            api: Api::new(http, key),
            fetch: Fetch::new(limits::get().tools.webfetch_timeout_secs.secs())?,
            paths,
            settings,
            mcp: std::sync::Arc::default(),
        };
        let _ = app.mcp.reload(&app);
        Ok(app)
    }

    /// Defaults only, rooted in `project`, with no key: for tests that never reach the network.
    #[cfg(test)]
    pub fn for_tests(project: &std::path::Path) -> Self {
        let paths = Paths::at(project.join(".palepale-home"), project.to_path_buf());
        let settings = settings::load(&paths, &Overrides::default()).expect("defaults load");
        let http = Http::new(limits::get().http.connect_timeout_ms.ms()).expect("http client");
        Self {
            api: Api::new(http, None),
            fetch: Fetch::new(limits::get().tools.webfetch_timeout_secs.secs())
                .expect("fetch client"),
            paths,
            settings,
            mcp: std::sync::Arc::default(),
        }
    }
}
