mod detector;
mod feedback;
mod loading;
mod policy;
mod presentation;
mod prompt;
mod queue;
pub(crate) mod runtime;
pub(crate) mod slash;

pub(crate) use feedback::load_hook_feedback_preferences_into_state;
pub(crate) use loading::{
    dirs_for_hook_loading, is_trusted_project_root, project_hook_root_from_cwd,
};
pub(crate) use presentation::render_consultation_details;
