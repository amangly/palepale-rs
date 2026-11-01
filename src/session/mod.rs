pub mod entry;
pub mod reader;
pub mod store;
pub mod text;
pub mod transcript;
pub mod workflow;

#[derive(Default)]
pub struct Start {
    pub selection: store::Selection,
    pub model_override: bool,
    pub mode_override: bool,
}

impl Start {
    pub fn model<'a>(&self, app: &'a crate::app::App, saved: &'a store::SavedState) -> &'a str {
        if self.model_override || app.api.uses_trial() || saved.model.is_empty() {
            &app.settings.model
        } else {
            &saved.model
        }
    }
    pub fn mode(
        &self,
        app: &crate::app::App,
        saved: &store::SavedState,
    ) -> anyhow::Result<&'static crate::mode::Mode> {
        let id = if self.mode_override || saved.mode.is_empty() {
            &app.settings.mode
        } else {
            &saved.mode
        };
        crate::mode::get(id).ok_or_else(|| anyhow::anyhow!("unknown saved mode `{id}`"))
    }
    pub fn prompt<'a>(&self, app: &'a crate::app::App, saved: &'a store::SavedState) -> &'a str {
        if saved.system_prompt.is_empty() {
            &app.settings.system_prompt
        } else {
            &saved.system_prompt
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trial_resume_uses_the_trial_model_instead_of_the_saved_paid_model() {
        let home = tempfile::tempdir().unwrap();
        let mut app = crate::app::App::for_tests(home.path());
        app.settings.model = crate::commands::trial::model().to_owned();
        let saved = store::SavedState {
            model: "paid-model".into(),
            ..Default::default()
        };
        let start = Start::default();
        assert_eq!(start.model(&app, &saved), "paid-model");
        app.api.set_trial(true);
        assert_eq!(start.model(&app, &saved), crate::commands::trial::model());
    }
}
