// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// TLS SNI probe - captures Server Name Indication from TLS ClientHello
// by hooking tcp_sendmsg at the kernel level (library-agnostic)

use crate::config;
use anyhow::{Context, Result};
use libbpf_rs::{
    Link, MapHandle,
    skel::{OpenSkel, SkelBuilder},
};
use std::{
    mem::MaybeUninit,
    os::fd::AsFd,
};

// ─── Generated skeleton ───────────────────────────────────────────────────────
mod bpf {
    include!(concat!(env!("OUT_DIR"), "/tlssni.skel.rs"));
    include!(concat!(env!("OUT_DIR"), "/tlssni.rs"));
}
use bpf::*;

// Re-export raw type for size calculation in probes.rs
pub type RawTlsSniEvent = bpf::tlssni_event;

/// User-space TLS SNI event
#[derive(Debug, Clone)]
pub struct TlsSniEvent {
    pub pid: u32,
    pub tid: u32,
    pub uid: u32,
    pub timestamp_ns: u64,
    pub comm: String,
    pub sni_name: String,
}

impl TlsSniEvent {
    /// Parse event from raw ring buffer data
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let event_size = std::mem::size_of::<RawTlsSniEvent>();
        if data.len() < event_size {
            return None;
        }

        // SAFETY: BPF guarantees proper alignment and layout
        let raw = unsafe { &*(data.as_ptr() as *const RawTlsSniEvent) };

        // Parse comm (null-terminated)
        let comm = raw.comm
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect::<Vec<u8>>();
        let comm = String::from_utf8_lossy(&comm).into_owned();

        // Parse sni_name using sni_len field
        let sni_len = raw.sni_len as usize;
        let sni_name = if sni_len > 0 && sni_len < raw.sni_name.len() {
            let sni_bytes: Vec<u8> = raw.sni_name[..sni_len]
                .iter()
                .map(|&c| c as u8)
                .collect();
            String::from_utf8_lossy(&sni_bytes).into_owned()
        } else {
            // Fallback: read until null terminator
            let sni_bytes: Vec<u8> = raw.sni_name
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| c as u8)
                .collect();
            String::from_utf8_lossy(&sni_bytes).into_owned()
        };

        Some(TlsSniEvent {
            pid: raw.pid,
            tid: raw.tid,
            uid: raw.uid,
            timestamp_ns: config::ktime_to_unix_ns(raw.timestamp_ns),
            comm,
            sni_name,
        })
    }
}

// ─── Main struct ──────────────────────────────────────────────────────────────
pub struct TlsSni {
    _open_object: Box<MaybeUninit<libbpf_rs::OpenObject>>,
    skel: Box<TlssniSkel<'static>>,
    _links: Vec<Link>,
}

impl TlsSni {
    /// Create a new TlsSni that reuses an existing ring buffer
    ///
    /// # Arguments
    /// * `rb` - External ring buffer map handle to reuse
    pub fn new_with_rb(rb: &MapHandle) -> Result<Self> {
        let mut builder = TlssniSkelBuilder::default();
        builder.obj_builder.debug(config::verbose());

        let open_object = Box::new(MaybeUninit::<libbpf_rs::OpenObject>::uninit());
        let mut open_skel = builder.open().context("failed to open tlssni BPF object")?;

        // Reuse external ring buffer
        open_skel
            .maps_mut()
            .rb()
            .reuse_fd(rb.as_fd())
            .context("failed to reuse external rb map for tlssni")?;

        let skel = open_skel.load().context("failed to load tlssni BPF object")?;

        // SAFETY: skel borrows open_object which lives in a Box<MaybeUninit>
        let skel =
            unsafe { Box::from_raw(Box::into_raw(Box::new(skel)) as *mut TlssniSkel<'static>) };

        Ok(Self {
            _open_object: open_object,
            skel,
            _links: Vec::new(),
        })
    }

    /// Attach fentry hook for tcp_sendmsg
    pub fn attach(&mut self) -> Result<()> {
        let mut links = Vec::new();

        let link = self
            .skel
            .progs_mut()
            .trace_tcp_sendmsg()
            .attach()
            .context("failed to attach tcp_sendmsg fentry")?;
        links.push(link);

        self._links = links;
        Ok(())
    }
}
