// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// Unified probes manager - manages sslsniff and proctrace probes
// with shared traced_processes map and shared ring buffer for coordinated process tracing

use anyhow::{Context, Result};
use libbpf_rs::{MapHandle, RingBufferBuilder};
use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::event::Event;

use super::filewatch::{FileWatch, RawFileWatchEvent};
use super::filewrite::{FileWrite as FileWriteProbe, RawFileWriteEvent};
use super::procmon::{ProcMon, ProcMonEvent};
use super::proctrace::{ProcEventHeader, ProcTrace, VariableEvent};
use super::shared_maps::SharedMaps;
use super::sslsniff::SslSniff;
use super::sslsniff::bpf::probe_SSL_data_t as RawSslEvent;
use super::tcpsniff::TcpSniff;
use super::udpdns::{RawUdpDnsEvent, UdpDns};
use crate::config::TcpTarget;

const POLL_TIMEOUT_MS: u64 = 100;

// Event source constants matching common.h event_source_t
const EVENT_SOURCE_PROC: u32 = 1;
const EVENT_SOURCE_SSL: u32 = 2;
const EVENT_SOURCE_PROCMON: u32 = 3;
const EVENT_SOURCE_FILEWATCH: u32 = 4;
const EVENT_SOURCE_FILEWRITE: u32 = 5;
const EVENT_SOURCE_UDPDNS: u32 = 6;

/// Unified probe manager that coordinates sslsniff and proctrace
///
/// This manager ensures both probes share the same traced_processes map
/// and the same ring buffer, allowing coordinated process tracing where:
/// - proctrace captures process creation events
/// - sslsniff captures SSL traffic from those processes
/// Both write to a single shared ring buffer to save memory.
pub struct Probes {
    /// Process trace probe (owns the traced_processes map and ring buffer)
    proctrace: ProcTrace,
    /// SSL sniff probe (reuses proctrace's traced_processes map and ring buffer)
    sslsniff: SslSniff,
    /// Process monitor probe (reuses ring buffer)
    procmon: ProcMon,
    /// File watch probe (reuses traced_processes map and ring buffer, optional)
    filewatch: Option<FileWatch>,
    /// File write probe (reuses traced_processes map and ring buffer, always enabled)
    filewrite: FileWriteProbe,
    /// UDP DNS probe (reuses ring buffer, captures domains from DNS queries, optional)
    udpdns: Option<UdpDns>,
    /// TCP sniff probe (captures plain HTTP traffic on configured ports, optional)
    tcpsniff: Option<TcpSniff>,
    /// Shared ring buffer handle (cloned from proctrace) for polling
    rb_handle: MapHandle,
    /// Unified event channel - events are converted to Event type inside the poller
    event_tx: crossbeam_channel::Sender<Event>,
    event_rx: crossbeam_channel::Receiver<Event>,
}

impl Probes {
    /// Create a new unified probe manager
    ///
    /// # Arguments
    /// * `target_pids` - Initial PIDs to trace (empty means trace all matching UID)
    /// * `target_uid` - Optional UID filter
    /// * `enable_filewatch` - Enable filewatch probe
    /// * `enable_udpdns` - Enable udpdns probe
    /// * `tcp_targets` - TCP targets for plain HTTP capture
    pub fn new(
        target_pids: &[u32],
        target_uid: Option<u32>,
        enable_filewatch: bool,
        enable_udpdns: bool,
        tcp_targets: &[TcpTarget],
    ) -> Result<Self> {
        Self::new_with_cgroup_filter(
            target_pids,
            target_uid,
            enable_filewatch,
            enable_udpdns,
            tcp_targets,
            false,
        )
    }

    /// Create a new unified probe manager with explicit cgroup-level filtering toggle.
    ///
    /// When `cgroup_filter_enabled` is true, every probe (except procmon, which
    /// keeps full audit coverage) gates its events behind the shared
    /// `cgroup_filter` map. Cgroup ids can then be registered at runtime via
    /// `add_traced_cgroup`. When false (default), every probe behaves exactly
    /// like before this feature existed.
    pub fn new_with_cgroup_filter(
        target_pids: &[u32],
        target_uid: Option<u32>,
        enable_filewatch: bool,
        enable_udpdns: bool,
        tcp_targets: &[TcpTarget],
        cgroup_filter_enabled: bool,
    ) -> Result<Self> {
        // Create proctrace first - it owns the traced_processes map, the ring
        // buffer, and (when enabled) the cgroup_filter map.
        let proctrace = ProcTrace::new_with_target_and_maps(
            target_pids,
            target_uid,
            None,
            None,
            cgroup_filter_enabled,
        )
        .context("failed to create proctrace")?;

        // Ring buffer handle kept for the polling thread (see `run`).
        let rb_handle = proctrace.rb_handle().context("failed to get rb handle")?;

        // Bundle the maps proctrace owns so every follower probe can reuse them
        // through a single `&SharedMaps` argument instead of a growing list of
        // individual `&MapHandle` parameters.
        //
        // The cgroup_filter handle is only fetched when the feature is on; when
        // off, each probe loads its own private (unused) cgroup_filter map so we
        // never burn an extra fd in the steady state.
        let mut shared = SharedMaps::new(proctrace.rb_handle().context("failed to get rb handle")?)
            .with_traced_processes(
                proctrace
                    .traced_processes_handle()
                    .context("failed to get traced_processes handle")?,
            )
            .with_cgroup_filter_enabled(cgroup_filter_enabled);
        if cgroup_filter_enabled {
            shared = shared.with_cgroup_filter(
                proctrace
                    .cgroup_filter_handle()
                    .context("failed to get cgroup_filter handle")?,
            );
        }

        // Create sslsniff - reuses the shared ring buffer and process filter.
        let sslsniff = SslSniff::new_with_shared(&shared).context("failed to create sslsniff")?;

        // Create procmon - reuses the ring buffer only (no cgroup filter: full audit)
        let procmon = ProcMon::new_with_shared(&shared).context("failed to create procmon")?;

        // Optionally create filewatch - reuses ring buffer + process/cgroup filters
        let filewatch = if enable_filewatch {
            let fw = FileWatch::new_with_shared(&shared).context("failed to create filewatch")?;
            Some(fw)
        } else {
            log::info!("FileWatch probe disabled");
            None
        };

        // Create filewrite - reuses ring buffer + process/cgroup filters (always enabled)
        let filewrite =
            FileWriteProbe::new_with_shared(&shared).context("failed to create filewrite")?;

        // Optionally create udpdns - reuses ring buffer + process filter
        // Skips already-traced processes to avoid redundant discovery events
        let udpdns = if enable_udpdns {
            let dns = UdpDns::new_with_shared(&shared).context("failed to create udpdns")?;
            Some(dns)
        } else {
            log::info!("UDP DNS probe disabled (no https/http domain rules configured)");
            None
        };

        // Optionally create tcpsniff - captures plain HTTP traffic to configured IP/port targets
        let tcpsniff = if !tcp_targets.is_empty() {
            let mut tcp =
                TcpSniff::new_with_shared(&shared).context("failed to create tcpsniff")?;
            tcp.set_targets(tcp_targets)
                .context("failed to set tcp targets")?;
            Some(tcp)
        } else {
            log::info!("TcpSniff probe disabled (no tcp_targets configured)");
            None
        };

        let (event_tx, event_rx) = crossbeam_channel::unbounded();

        Ok(Self {
            proctrace,
            sslsniff,
            procmon,
            filewatch,
            filewrite,
            udpdns,
            tcpsniff,
            rb_handle,
            event_tx,
            event_rx,
        })
    }

    /// Attach all probes
    pub fn attach(&mut self) -> Result<()> {
        // Attach procmon for process monitoring
        self.procmon.attach().context("failed to attach procmon")?;
        self.proctrace
            .attach()
            .context("failed to attach proctrace")?;
        // Attach filewatch for .jsonl file monitoring (if enabled)
        if let Some(ref mut fw) = self.filewatch {
            fw.attach().context("failed to attach filewatch")?;
        }
        // Attach filewrite for JSON write monitoring (always enabled)
        self.filewrite
            .attach()
            .context("failed to attach filewrite")?;
        // Attach udpdns for DNS query capture (if enabled)
        if let Some(ref mut dns) = self.udpdns {
            dns.attach().context("failed to attach udpdns")?;
        }
        // Attach tcpsniff for plain HTTP traffic capture (if enabled)
        if let Some(ref mut tcp) = self.tcpsniff {
            tcp.attach().context("failed to attach tcpsniff")?;
        }
        // sslsniff uses uprobes attached per-process via attach_process()
        Ok(())
    }

    pub fn attach_process(&mut self, pid: i32) -> Result<()> {
        self.attach_ssl_to_process(pid)?;
        self.add_traced_pid(pid as u32)
    }

    /// Attach SSL probes to a specific process
    pub fn attach_ssl_to_process(&mut self, pid: i32) -> Result<()> {
        self.sslsniff
            .attach_process(pid)
            .context("failed to attach sslsniff to process")?;
        Ok(())
    }

    /// Start polling for events from the shared ring buffer
    ///
    /// A single background thread polls the shared ring buffer and dispatches
    /// events as unified Event type to the channel.
    pub fn run(&self) -> Result<ProbesPoller> {
        let proc_min_sz = mem::size_of::<ProcEventHeader>();
        let ssl_event_size = mem::size_of::<RawSslEvent>();
        let procmon_event_size = mem::size_of::<ProcMonEvent>();
        let filewatch_event_size = mem::size_of::<RawFileWatchEvent>();
        let filewrite_event_size = mem::size_of::<RawFileWriteEvent>();
        let udpdns_event_size = mem::size_of::<RawUdpDnsEvent>();

        let event_tx = self.event_tx.clone();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_inner = Arc::clone(&stop_flag);

        // Build ring buffer from the shared rb handle
        let mut rb_builder = RingBufferBuilder::new();
        rb_builder
            .add(&self.rb_handle, move |data: &[u8]| {
                // Read the first u32 to determine event source (common_event_hdr.source)
                if data.len() < 4 {
                    return 0;
                }
                let source = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
                let event = match source {
                    EVENT_SOURCE_PROC => {
                        // Process event - variable size, starts with proc_event_header
                        if data.len() >= proc_min_sz {
                            VariableEvent::from_bytes(data).map(Event::Proc)
                        } else {
                            None
                        }
                    }
                    EVENT_SOURCE_SSL => {
                        // SSL event - convert raw BPF data to user-space SslEvent
                        if data.len() >= ssl_event_size {
                            // SAFETY: BPF guarantees layout and alignment
                            let raw = unsafe { &*(data.as_ptr() as *const RawSslEvent) };
                            let ssl_event = crate::probes::sslsniff::SslEvent::from_bpf(raw);
                            Some(Event::Ssl(ssl_event))
                        } else {
                            None
                        }
                    }
                    EVENT_SOURCE_PROCMON => {
                        // Process monitor event
                        if data.len() >= procmon_event_size {
                            super::procmon::Event::from_bytes(data).map(Event::ProcMon)
                        } else {
                            None
                        }
                    }
                    EVENT_SOURCE_FILEWATCH => {
                        // File watch event
                        if data.len() >= filewatch_event_size {
                            super::filewatch::FileWatchEvent::from_bytes(data).map(Event::FileWatch)
                        } else {
                            None
                        }
                    }
                    EVENT_SOURCE_FILEWRITE => {
                        // File write event (JSON content)
                        if data.len() >= filewrite_event_size {
                            super::filewrite::FileWriteEvent::from_bytes(data).map(Event::FileWrite)
                        } else {
                            None
                        }
                    }
                    EVENT_SOURCE_UDPDNS => {
                        // UDP DNS event (domain name from DNS query)
                        if data.len() >= udpdns_event_size {
                            super::udpdns::UdpDnsEvent::from_bytes(data).map(Event::UdpDns)
                        } else {
                            None
                        }
                    }
                    _ => {
                        // Unknown source - ignore
                        log::warn!("probes: unknown event source {source}");
                        None
                    }
                };

                if let Some(e) = event {
                    let _ = event_tx.send(e);
                }
                0
            })
            .context("failed to add shared ring buffer")?;
        let rb = rb_builder.build().context("failed to build ring buffer")?;

        let handle = thread::Builder::new()
            .name("probes-poll".into())
            .spawn(move || {
                let timeout = Duration::from_millis(POLL_TIMEOUT_MS);
                loop {
                    if stop_flag_inner.load(Ordering::Relaxed) {
                        break;
                    }
                    match rb.poll(timeout) {
                        Ok(_) => {}
                        Err(e) if e.kind() == libbpf_rs::ErrorKind::Interrupted => break,
                        Err(e) => {
                            eprintln!("probes poll error: {e:#}");
                            break;
                        }
                    }
                }
            })
            .context("failed to spawn poll thread")?;

        Ok(ProbesPoller {
            handle: Some(handle),
            stop_flag,
        })
    }

    /// Receive the next event from any probe (blocking)
    pub fn recv(&self) -> Option<Event> {
        self.event_rx.recv().ok()
    }

    /// Try to receive an event from any probe (non-blocking)
    pub fn try_recv(&self) -> Option<Event> {
        self.event_rx.try_recv().ok()
    }

    /// Add a PID to the traced_processes map at runtime
    pub fn add_traced_pid(&mut self, pid: u32) -> Result<()> {
        self.proctrace
            .add_traced_pid(pid)
            .context("failed to add traced pid")
    }

    /// Remove a PID from the traced_processes map at runtime
    pub fn remove_traced_pid(&mut self, pid: u32) -> Result<()> {
        self.proctrace
            .remove_traced_pid(pid)
            .context("failed to remove traced pid")
    }

    /// Detach SSL probes for a process and clean up traced inodes.
    pub fn detach_ssl_probes(&mut self, pid: u32) {
        self.sslsniff.detach_process(pid);
    }

    pub fn add_tcp_target(&mut self, target: &TcpTarget) -> Result<()> {
        if let Some(ref mut tcp) = self.tcpsniff {
            tcp.add_target(target)
        } else {
            log::warn!("TcpSniff not enabled, cannot add runtime target {target:?}");
            Ok(())
        }
    }

    /// Get a handle to the traced_processes map
    pub fn traced_processes_handle(&self) -> Result<MapHandle> {
        self.proctrace.traced_processes_handle()
    }

    /// Add a cgroup inode id to the shared cgroup_filter map at runtime.
    ///
    /// Has no observable effect unless probes were created with
    /// `cgroup_filter_enabled = true`; in that case, only events from
    /// processes whose cgroup id is registered here will be emitted by
    /// proctrace / filewatch / filewrite. sslsniff, udpdns, and procmon are
    /// unaffected.
    pub fn add_traced_cgroup(&mut self, cgroup_id: u64) -> Result<()> {
        self.proctrace
            .add_traced_cgroup(cgroup_id)
            .context("failed to add traced cgroup")
    }

    /// Remove a cgroup inode id from the shared cgroup_filter map at runtime.
    pub fn remove_traced_cgroup(&mut self, cgroup_id: u64) -> Result<()> {
        self.proctrace
            .remove_traced_cgroup(cgroup_id)
            .context("failed to remove traced cgroup")
    }
}

/// Poller handle for the unified ring buffer thread
pub struct ProbesPoller {
    handle: Option<thread::JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
}

impl ProbesPoller {
    /// Stop the poller thread
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ProbesPoller {
    fn drop(&mut self) {
        self.stop();
    }
}
