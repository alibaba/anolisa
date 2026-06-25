#[path = "adapter/public.rs"]
pub mod adapter;
#[path = "agent/public.rs"]
pub mod agent;
mod command;
#[allow(dead_code, unused_imports)]
mod config;
#[allow(dead_code, unused_imports)]
mod diagnostics;
#[allow(dead_code, unused_imports)]
#[path = "evidence/public.rs"]
mod evidence;
#[allow(dead_code, unused_imports)]
#[path = "hooks/public.rs"]
mod hooks;

mod i18n;
mod input;
#[path = "journal/public.rs"]
pub mod journal;
#[path = "ledger/public.rs"]
pub mod ledger;
#[path = "parser/public.rs"]
pub mod parser;
#[allow(dead_code)]
#[path = "question/public.rs"]
mod question;
#[path = "raw_input/public.rs"]
pub mod raw_input;
#[path = "shell_host/public.rs"]
pub mod shell_host;
#[allow(dead_code)]
#[path = "slash/public.rs"]
mod slash;
#[allow(dead_code, unused_imports)]
mod tools;
#[path = "types/public.rs"]
pub mod types;
#[allow(dead_code, unused_imports)]
#[path = "ui/public.rs"]
mod ui;

pub use adapter::{AuthFieldInfo, AuthProviderInfo, AuthResponse};
pub use config::{
    language_config_status, load_config, parse_language_setting, resolve_language_setting,
    write_user_language_config, CoshConfig, Language, LanguageConfigStatus,
};
pub use i18n::{I18n, MessageId};
