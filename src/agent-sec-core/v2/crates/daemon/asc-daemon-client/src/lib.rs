//! Bounded synchronous client for the local `AgentSecCore` daemon.

#![forbid(unsafe_code)]

mod client;
mod credential;
mod error;
mod trace;

pub use client::DaemonClient;
pub use error::ClientError;
