pub mod adapter;
pub mod agent_render;
pub mod builtin_hooks;
pub mod config;
pub mod context_window;
pub mod exit_classify;
pub mod governance;
pub mod hook_engine;

pub mod hook_types;
mod i18n;
mod input;
pub mod interactive;
pub mod journal;
pub mod ledger;
mod linux_memory_hooks;
pub mod parser;
mod question_choices;
pub mod raw_input;
pub mod renderer;
pub mod shell_host;
pub mod slash_registry;
pub mod tools;
pub mod types;

pub use adapter::{adapter_for_kind, AdapterInstance, AdapterKind, AgentAdapter};
pub use config::{
    language_config_status, load_config, parse_language_setting, resolve_language_setting,
    write_user_language_config, CoshConfig, Language, LanguageConfigStatus,
};
pub use i18n::{I18n, MessageId};
