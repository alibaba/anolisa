#[allow(dead_code, unused_imports)]
#[path = "agent_render/mod.rs"]
pub(crate) mod agent_render;
mod question;
#[path = "renderer.rs"]
mod renderer;

pub(crate) use agent_render::*;
pub(crate) use renderer::render_transcript;
