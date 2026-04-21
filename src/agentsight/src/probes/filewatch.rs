// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// File watch probe - monitors openat syscalls for .jsonl files from traced processes

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
    include!(concat!(env!("OUT_DIR"), "/filewatch.skel.rs"));
    include!(concat!(env!("OUT_DIR"), "/filewatch.rs"));
}
use bpf::*;

// Re-export raw type for size calculation in probes.rs
pub type RawFileWatchEvent = bpf::filewatch_event;

/// User-space file watch event
#[derive(Debug, Clone)]
pub struct FileWatchEvent {
    pub pid: u32,
    pub tid: u32,
    pub uid: u32,
    pub timestamp_ns: u64,
    pub flags: i32,
    pub comm: String,
    pub filename: String,
}

impl FileWatchEvent {
    /// Parse event from raw ring buffer data
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let event_size = std::mem::size_of::<RawFileWatchEvent>();
        if data.len() < event_size {
            return None;
        }

        // SAFETY: BPF guarantees proper alignment and layout
        let raw = unsafe { &*(data.as_ptr() as *const RawFileWatchEvent) };

        // Parse comm (null-terminated)
        let comm = raw.comm
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect::<Vec<u8>>();
        let comm = String::from_utf8_lossy(&comm).into_owned();

        // Parse filename (null-terminated)
        let filename = raw.filename
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect::<Vec<u8>>();
        let filename = String::from_utf8_lossy(&filename).into_owned();

        Some(FileWatchEvent {
            pid: raw.pid,
            tid: raw.tid,
            uid: raw.uid,
            timestamp_ns: config::ktime_to_unix_ns(raw.timestamp_ns),
            flags: raw.flags,
            comm,
            filename,
        })
    }
}

// ─── Main struct ──────────────────────────────────────────────────────────────
pub struct FileWatch {
    _open_object: Box<MaybeUninit<libbpf_rs::OpenObject>>,
    skel: Box<FilewatchSkel<'static>>,
    _links: Vec<Link>,
}

impl FileWatch {
    /// Create a new FileWatch that reuses existing traced_processes and ring buffer maps
    ///
    /// # Arguments
    /// * `traced_processes` - External MapHandle for process filtering
    /// * `rb` - External ring buffer MapHandle
    pub fn new_with_maps(traced_processes: &MapHandle, rb: &MapHandle) -> Result<Self> {
        let mut builder = FilewatchSkelBuilder::default();
        builder.obj_builder.debug(config::verbose());

        let open_object = Box::new(MaybeUninit::<libbpf_rs::OpenObject>::uninit());
        let mut open_skel = builder.open().context("failed to open filewatch BPF object")?;

        // Reuse external traced_processes map
        open_skel
            .maps_mut()
            .traced_processes()
            .reuse_fd(traced_processes.as_fd())
            .context("failed to reuse external traced_processes map for filewatch")?;

        // Reuse external ring buffer
        open_skel
            .maps_mut()
            .rb()
            .reuse_fd(rb.as_fd())
            .context("failed to reuse external rb map for filewatch")?;

        let skel = open_skel.load().context("failed to load filewatch BPF object")?;

        // SAFETY: skel borrows open_object which lives in a Box<MaybeUninit>
        let skel =
            unsafe { Box::from_raw(Box::into_raw(Box::new(skel)) as *mut FilewatchSkel<'static>) };

        Ok(Self {
            _open_object: open_object,
            skel,
            _links: Vec::new(),
        })
    }

    /// Attach tracepoint for file monitoring
    pub fn attach(&mut self) -> Result<()> {
        let mut links = Vec::new();

        let link = self
            .skel
            .progs_mut()
            .trace_openat_enter()
            .attach()
            .context("failed to attach openat tracepoint")?;
        links.push(link);

        self._links = links;
        Ok(())
    }
}
