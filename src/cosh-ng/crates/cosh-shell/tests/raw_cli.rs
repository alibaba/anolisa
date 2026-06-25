use std::fs;
use std::io::Write;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use wait_timeout::ChildExt;

#[path = "raw_cli/activity.rs"]
mod activity;
#[path = "raw_cli/agent_input.rs"]
mod agent_input;
#[path = "raw_cli/animation.rs"]
mod animation;
#[path = "raw_cli/approval.rs"]
mod approval;
#[path = "raw_cli/cancellation.rs"]
mod cancellation;
#[path = "raw_cli/config.rs"]
mod config;
#[path = "raw_cli/cosh_core/mod.rs"]
mod cosh_core;
#[path = "raw_cli/evidence_request.rs"]
mod evidence_request;
#[path = "raw_cli/failed_command.rs"]
mod failed_command;
#[path = "raw_cli/heavy.rs"]
mod heavy;
#[path = "raw_cli/host_executed.rs"]
mod host_executed;
#[path = "raw_cli/i18n.rs"]
mod i18n;
#[path = "raw_cli/memory_hook.rs"]
mod memory_hook;
#[path = "raw_cli/mode.rs"]
mod mode;
#[path = "raw_cli/native.rs"]
mod native;
#[path = "raw_cli/passthrough.rs"]
mod passthrough;
#[path = "raw_cli/provider_handoff/mod.rs"]
mod provider_handoff;
#[path = "raw_cli/provider_tools.rs"]
mod provider_tools;
#[path = "raw_cli/question.rs"]
mod question;
#[path = "raw_cli/recommendation.rs"]
mod recommendation;
#[path = "raw_cli/registry.rs"]
mod registry;
#[path = "raw_cli/renderer.rs"]
mod renderer;
#[path = "raw_cli/slash.rs"]
mod slash;
#[path = "raw_cli/startup.rs"]
mod startup;
#[path = "support/mod.rs"]
mod support;

pub(crate) use i18n::*;
use support::raw_cli::*;
