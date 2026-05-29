// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// Scheduler monitor probe — detects idle/active state transitions for traced
// Agent processes via the BTF-typed sched_switch / sched_wakeup tracepoints.

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

mod bpf {
    include!(concat!(env!("OUT_DIR"), "/schedmon.skel.rs"));
    include!(concat!(env!("OUT_DIR"), "/schedmon.rs"));
}
use bpf::*;

pub type RawSchedEvent = bpf::sched_event;

pub const SCHED_EVENT_SLEEP: u8 = 1;
pub const SCHED_EVENT_WAKEUP: u8 = 2;

#[derive(Debug, Clone)]
pub struct SchedEvent {
    /// Thread-group id — identifies the Agent family.
    pub tgid: u32,
    /// Thread id — tracked individually (a family is ACTIVE while any thread runs).
    pub tid: u32,
    pub timestamp_ns: u64,
    pub event_type: u8,
}

impl SchedEvent {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < std::mem::size_of::<RawSchedEvent>() {
            return None;
        }

        let raw = unsafe { &*(data.as_ptr() as *const RawSchedEvent) };

        Some(SchedEvent {
            tgid: raw.tgid,
            tid: raw.tid,
            timestamp_ns: config::ktime_to_unix_ns(raw.timestamp_ns),
            event_type: raw.event_type,
        })
    }

    pub fn is_sleep(&self) -> bool {
        self.event_type == SCHED_EVENT_SLEEP
    }

    pub fn is_wakeup(&self) -> bool {
        self.event_type == SCHED_EVENT_WAKEUP
    }
}

pub struct SchedMon {
    // Vestigial under libbpf-rs 0.23.x: `builder.open()` takes no OpenObject and
    // owns its bpf_object internally, so this field is unused (kept for parity
    // with the sibling probes). TODO: on upgrade to libbpf-rs 0.24+, `open()`
    // requires `&mut MaybeUninit<OpenObject>` and the 'static lifetime laundering
    // below must be revisited across all probes.
    _open_object: Box<MaybeUninit<libbpf_rs::OpenObject>>,
    skel: Box<SchedmonSkel<'static>>,
    _links: Vec<Link>,
}

impl SchedMon {
    pub fn new_with_maps(traced_processes: &MapHandle, rb: &MapHandle) -> Result<Self> {
        let mut builder = SchedmonSkelBuilder::default();
        builder.obj_builder.debug(config::verbose());

        let open_object = Box::new(MaybeUninit::<libbpf_rs::OpenObject>::uninit());
        let mut open_skel = builder.open().context("failed to open schedmon BPF object")?;

        open_skel
            .maps_mut()
            .traced_processes()
            .reuse_fd(traced_processes.as_fd())
            .context("failed to reuse traced_processes map")?;

        open_skel
            .maps_mut()
            .rb()
            .reuse_fd(rb.as_fd())
            .context("failed to reuse rb map")?;

        let skel = open_skel.load().context("failed to load schedmon BPF object")?;

        let skel =
            unsafe { Box::from_raw(Box::into_raw(Box::new(skel)) as *mut SchedmonSkel<'static>) };

        Ok(Self {
            _open_object: open_object,
            skel,
            _links: Vec::new(),
        })
    }

    pub fn attach(&mut self) -> Result<()> {
        let mut links = Vec::new();

        let link = self
            .skel
            .progs_mut()
            .handle_sched_switch()
            .attach()
            .context("failed to attach sched_switch tracepoint")?;
        links.push(link);

        let link = self
            .skel
            .progs_mut()
            .handle_sched_wakeup()
            .attach()
            .context("failed to attach sched_wakeup tracepoint")?;
        links.push(link);

        self._links = links;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sched_event_from_bytes_too_short() {
        let data = [0u8; 4];
        assert!(SchedEvent::from_bytes(&data).is_none());
    }

    #[test]
    fn test_sched_event_is_sleep_wakeup() {
        let ev = SchedEvent {
            tgid: 100,
            tid: 100,
            timestamp_ns: 12345,
            event_type: SCHED_EVENT_SLEEP,
        };
        assert!(ev.is_sleep());
        assert!(!ev.is_wakeup());

        let ev2 = SchedEvent {
            tgid: 100,
            tid: 101,
            timestamp_ns: 12345,
            event_type: SCHED_EVENT_WAKEUP,
        };
        assert!(ev2.is_wakeup());
        assert!(!ev2.is_sleep());
    }
}
