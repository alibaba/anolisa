// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// UDP DNS probe - captures domain names from DNS query packets
// by hooking udp_sendmsg and filtering for destination port 53.

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

// --- Generated skeleton ---
mod bpf {
    include!(concat!(env!("OUT_DIR"), "/udpdns.skel.rs"));
    include!(concat!(env!("OUT_DIR"), "/udpdns.rs"));
}
use bpf::*;

// Re-export raw type for size calculation in probes.rs
pub type RawUdpDnsEvent = bpf::udpdns_event;

/// User-space UDP DNS event
#[derive(Debug, Clone)]
pub struct UdpDnsEvent {
    pub pid: u32,
    pub tid: u32,
    pub uid: u32,
    pub timestamp_ns: u64,
    pub comm: String,
    pub domain: String,
}

impl UdpDnsEvent {
    /// Parse event from raw ring buffer data
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let event_size = std::mem::size_of::<RawUdpDnsEvent>();
        if data.len() < event_size {
            return None;
        }

        // SAFETY: BPF guarantees proper alignment and layout
        let raw = unsafe { &*(data.as_ptr() as *const RawUdpDnsEvent) };

        // Parse comm (null-terminated)
        let comm = raw.comm
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect::<Vec<u8>>();
        let comm = String::from_utf8_lossy(&comm).into_owned();

        // Parse domain using domain_len field
        let domain_len = raw.domain_len as usize;
        let domain = if domain_len > 0 && domain_len < raw.domain.len() {
            let domain_bytes: Vec<u8> = raw.domain[..domain_len]
                .iter()
                .map(|&c| c as u8)
                .collect();
            String::from_utf8_lossy(&domain_bytes).into_owned()
        } else {
            // Fallback: read until null terminator
            let domain_bytes: Vec<u8> = raw.domain
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| c as u8)
                .collect();
            String::from_utf8_lossy(&domain_bytes).into_owned()
        };

        Some(UdpDnsEvent {
            pid: raw.pid,
            tid: raw.tid,
            uid: raw.uid,
            timestamp_ns: config::ktime_to_unix_ns(raw.timestamp_ns),
            comm,
            domain,
        })
    }
}

// --- Main struct ---
pub struct UdpDns {
    _open_object: Box<MaybeUninit<libbpf_rs::OpenObject>>,
    skel: Box<UdpdnsSkel<'static>>,
    _links: Vec<Link>,
}

impl UdpDns {
    /// Create a new UdpDns that reuses existing traced_processes and ring buffer maps
    ///
    /// # Arguments
    /// * `traced_processes` - External MapHandle for process filtering (skip already-traced)
    /// * `rb` - External ring buffer map handle to reuse
    pub fn new_with_maps(traced_processes: &MapHandle, rb: &MapHandle) -> Result<Self> {
        let mut builder = UdpdnsSkelBuilder::default();
        builder.obj_builder.debug(config::verbose());

        let open_object = Box::new(MaybeUninit::<libbpf_rs::OpenObject>::uninit());
        let mut open_skel = builder.open().context("failed to open udpdns BPF object")?;

        // Reuse external traced_processes map
        open_skel
            .maps_mut()
            .traced_processes()
            .reuse_fd(traced_processes.as_fd())
            .context("failed to reuse external traced_processes map for udpdns")?;

        // Reuse external ring buffer
        open_skel
            .maps_mut()
            .rb()
            .reuse_fd(rb.as_fd())
            .context("failed to reuse external rb map for udpdns")?;

        let skel = open_skel.load().context("failed to load udpdns BPF object")?;

        // SAFETY: skel borrows open_object which lives in a Box<MaybeUninit>
        let skel =
            unsafe { Box::from_raw(Box::into_raw(Box::new(skel)) as *mut UdpdnsSkel<'static>) };

        Ok(Self {
            _open_object: open_object,
            skel,
            _links: Vec::new(),
        })
    }

    /// Attach fentry hook for udp_sendmsg
    pub fn attach(&mut self) -> Result<()> {
        let mut links = Vec::new();

        let link = self
            .skel
            .progs_mut()
            .trace_udp_sendmsg()
            .attach()
            .context("failed to attach udp_sendmsg fentry")?;
        links.push(link);

        self._links = links;
        Ok(())
    }
}
