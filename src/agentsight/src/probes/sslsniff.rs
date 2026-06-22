// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2023 Yusheng Zheng
//
// SSL/TLS sniffer built on libbpf-rs.
// Exposes a `SslSniff` struct with a builder-style API.

use crate::config;
use anyhow::{Context, Result};
use libbpf_rs::{
    Link, RingBufferBuilder, UprobeOpts,
    skel::{OpenSkel, SkelBuilder},
};
use procfs::process::Process;

use super::shared_maps::{MapKind, SharedMaps};
use std::{
    collections::{HashMap, HashSet},
    fs,
    mem::{self, MaybeUninit},
    path::Path,
    slice,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

// ─── Generated skeleton ───────────────────────────────────────────────────────
#[allow(
    non_camel_case_types,
    non_upper_case_globals,
    dead_code,
    non_snake_case
)]
pub mod bpf {
    include!(concat!(env!("OUT_DIR"), "/sslsniff.skel.rs"));
    include!(concat!(env!("OUT_DIR"), "/sslsniff.rs"));
}
use bpf::*;

// ─── Constants ────────────────────────────────────────────────────────────────
const MAX_BUF_SIZE: usize = bpf::MAX_BUF_SIZE as usize;
const POLL_TIMEOUT_MS: u64 = 100;

/// User-space SslEvent - lightweight version of BPF probe_SSL_data_t
///
/// Unlike the BPF version which has a 512KB fixed-size buffer, this struct
/// only stores the actual data received, significantly reducing memory usage.
#[derive(Debug, Clone)]
pub struct SslEvent {
    pub source: u32,
    pub timestamp_ns: u64,
    pub delta_ns: u64,
    pub pid: u32,
    pub tid: u32,
    pub uid: u32,
    pub len: u32,
    pub rw: i32,
    pub comm: String,
    /// Actual data buffer (only contains received data, not full 512KB)
    pub buf: Vec<u8>,
    pub is_handshake: bool,
    pub ssl_ptr: u64,
}

impl SslEvent {
    /// Create SslEvent from a raw ring-buffer sample of VARIABLE length.
    ///
    /// SSL records are tiered: the BPF side reserves only
    /// `offsetof(probe_SSL_data_t, buf) + <tier>` bytes, so a sample is the
    /// header prefix followed by `buf_size` payload bytes — NOT the full
    /// fixed-size struct. We therefore (1) gate on the header size, (2) read each
    /// scalar field from the prefix at its real (bindgen) offset — NOT via a
    /// full-struct cast, which is UB on a short sample and would also put the
    /// 4 MiB `buf` array on the stack if materialized — and (3) slice the payload
    /// by the `buf_size` FIELD, never by `data.len()` (which over-counts for any
    /// still-full-size record such as tcpsniff's 4 MiB reservation).
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        type R = bpf::probe_SSL_data_t;
        let hdr = mem::offset_of!(R, buf);
        if data.len() < hdr {
            return None;
        }
        // Build the byte arrays directly (no `.unwrap()`; see AGENTS.md §0). Every `off`
        // is a header field offset < `hdr`, and the guard above proves `data.len() >= hdr`,
        // so each index is in-bounds (`buf` is the last struct member).
        let u32_at = |off: usize| {
            u32::from_ne_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        };
        let u64_at = |off: usize| {
            u64::from_ne_bytes([
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
                data[off + 4],
                data[off + 5],
                data[off + 6],
                data[off + 7],
            ])
        };
        let i32_at = |off: usize| {
            i32::from_ne_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        };

        let len = u32_at(mem::offset_of!(R, len)) as usize;
        let buf_size = (u32_at(mem::offset_of!(R, buf_size)) as usize)
            .min(MAX_BUF_SIZE)
            .min(data.len() - hdr);
        // Warn only on a genuine over-cap capture. The `len > MAX_BUF_SIZE` guard is
        // defense-in-depth: every EVENT_SOURCE_SSL producer shares this header and
        // bpf_ringbuf_reserve does not zero the reservation, so a producer that omits
        // `truncated` cannot trip a false warning on stale ring bytes.
        if i32_at(mem::offset_of!(R, truncated)) != 0 && len > MAX_BUF_SIZE {
            log::warn!(
                "SSL payload exceeded {}-byte capture cap; captured {} of {} bytes (pid={})",
                MAX_BUF_SIZE,
                buf_size,
                len,
                u32_at(mem::offset_of!(R, pid)),
            );
        }
        let buf = data[hdr..hdr + buf_size].to_vec();

        let comm_off = mem::offset_of!(R, comm);
        let mut comm_arr = [0u8; 16];
        comm_arr.copy_from_slice(&data[comm_off..comm_off + 16]);

        Some(Self {
            source: u32_at(mem::offset_of!(R, source)),
            timestamp_ns: config::ktime_to_unix_ns(u64_at(mem::offset_of!(R, timestamp_ns))),
            delta_ns: u64_at(mem::offset_of!(R, delta_ns)),
            pid: u32_at(mem::offset_of!(R, pid)),
            tid: u32_at(mem::offset_of!(R, tid)),
            uid: u32_at(mem::offset_of!(R, uid)),
            len: len as u32,
            rw: i32_at(mem::offset_of!(R, rw)),
            comm: Self::parse_comm(&comm_arr),
            buf,
            is_handshake: i32_at(mem::offset_of!(R, is_handshake)) != 0,
            ssl_ptr: u64_at(mem::offset_of!(R, ssl_ptr)),
        })
    }

    /// Parse comm from the BPF struct field (layout matches C `char comm[16]`; generated
    /// bindings may use `[i8; 16]` or `[u8; 16]` depending on target / libbpf-cargo version).
    fn parse_comm<T>(comm: &[T; 16]) -> String {
        debug_assert_eq!(mem::size_of::<T>(), 1);
        let bytes = unsafe { slice::from_raw_parts(comm.as_ptr() as *const u8, 16) };
        let bytes: Vec<u8> = bytes.iter().copied().take_while(|&b| b != 0).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Get payload as string (if valid UTF-8)
    pub fn payload(&self) -> Option<&str> {
        std::str::from_utf8(&self.buf).ok()
    }

    /// Check if payload starts with "HTTP" (case-insensitive) without converting to string
    /// This is useful for detecting HTTP responses without UTF-8 validation overhead
    pub fn is_http(&self) -> bool {
        self.is_http_request() || self.is_http_response()
    }

    pub fn is_http_request(&self) -> bool {
        const METHODS: &[&[u8]] = &[
            b"GET ", b"POST", b"PUT ", b"DELE", b"HEAD", b"OPTI", b"PATC",
        ];
        METHODS.iter().any(|m| self.buf.starts_with(m))
    }

    pub fn is_http_response(&self) -> bool {
        self.buf.starts_with(b"HTTP")
    }

    /// Check if payload is an HTTP/2 connection preface
    pub fn is_http2_preface(&self) -> bool {
        self.buf.starts_with(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
    }

    /// Heuristic check for HTTP/2 binary frame data
    pub fn is_http2_frame(&self) -> bool {
        if self.buf.len() < 9 {
            return false;
        }
        // Parse 3-byte frame length
        let length =
            ((self.buf[0] as usize) << 16) | ((self.buf[1] as usize) << 8) | (self.buf[2] as usize);
        // Frame type must be a known type (0..=9)
        let frame_type = self.buf[3];
        if frame_type > 9 {
            return false;
        }
        // Reserved bit of stream ID must be 0
        if self.buf[5] & 0x80 != 0 {
            return false;
        }
        // Frame payload must fit in the buffer
        9 + length <= self.buf.len()
    }

    /// Check if payload is HTTP/2 (preface or binary frames)
    pub fn is_http2(&self) -> bool {
        self.is_http2_preface() || self.is_http2_frame()
    }

    /// Get comm as a String
    pub fn comm_str(&self) -> String {
        self.comm.clone()
    }

    /// Get the SSL connection pointer for connection tracking
    pub fn ssl_ptr(&self) -> u64 {
        self.ssl_ptr
    }

    /// Get the connection ID (pid, ssl_ptr) for unique connection identification
    pub fn connection_id(&self) -> (u32, u64) {
        (self.pid, self.ssl_ptr)
    }

    /// Get buf_size (convenience method for compatibility)
    pub fn buf_size(&self) -> u32 {
        self.buf.len() as u32
    }
}

// ─── Main struct ──────────────────────────────────────────────────────────────
pub struct SslSniff {
    // We store the skel behind a Box so we can hold it alongside the
    // links without lifetime trouble.  The MaybeUninit holds the
    // OpenObject allocation that the skeleton borrows from.
    _open_object: Box<MaybeUninit<libbpf_rs::OpenObject>>,
    skel: Box<SslsniffSkel<'static>>,
    _links: Vec<Link>,
    traced_files: HashSet<u64>,
    /// Maps pid -> inodes that were attached for this pid.
    /// Used to clean up traced_files when the process exits.
    pid_inodes: HashMap<u32, Vec<u64>>,
    // Channel for user-space SslEvent (lightweight, no need for Box)
    tx: crossbeam_channel::Sender<SslEvent>,
    rx: crossbeam_channel::Receiver<SslEvent>,
}

/// Maps sslsniff reuses from the shared bundle: ring buffer + process filter.
const SHARED_MAPS: &[MapKind] = &[MapKind::Rb, MapKind::TracedProcesses];

impl SslSniff {
    /// Create a new SslSniff with its own (unshared) maps.
    pub fn new() -> Result<Self> {
        Self::build(None)
    }

    /// Create a new SslSniff that reuses the shared ring buffer and process filter.
    pub fn new_with_shared(shared: &SharedMaps) -> Result<Self> {
        Self::build(Some(shared))
    }

    /// Open + load the skeleton (optionally reusing shared maps) and build `Self`.
    fn build(shared: Option<&SharedMaps>) -> Result<Self> {
        // ── Open + load skeleton ───────────────────────────────────────
        let mut builder = SslsniffSkelBuilder::default();
        builder.obj_builder.debug(config::verbose());
        // Keep MaybeUninit on the heap so its address is stable.
        let open_object = Box::new(MaybeUninit::<libbpf_rs::OpenObject>::uninit());
        let mut open_skel = builder.open().context("failed to open BPF object")?;

        // Reuse shared maps when running under the unified manager.
        if let Some(shared) = shared {
            shared
                .reuse_into(SHARED_MAPS, open_skel.open_object_mut())
                .context("failed to reuse shared maps for sslsniff")?;
        }

        let skel = open_skel.load().context("failed to load BPF object")?;

        // SAFETY: skel borrows open_object which lives in a Box<MaybeUninit>
        // on the heap.  We pin both together inside Self and never move either,
        // so the 'static lifetime cast is sound for the lifetime of Self.
        let skel =
            unsafe { Box::from_raw(Box::into_raw(Box::new(skel)) as *mut SslsniffSkel<'static>) };

        let (tx, rx) = crossbeam_channel::unbounded();
        Ok(Self {
            _open_object: open_object,
            skel,
            _links: Vec::new(),
            traced_files: HashSet::default(),
            pid_inodes: HashMap::default(),
            tx,
            rx,
        })
    }

    /// Attach SSL probes to a running process by reading its `/proc/<pid>/maps`.
    ///
    /// Detects which SSL libraries the process has mapped (OpenSSL, GnuTLS, NSS,
    /// or BoringSSL embedded in a binary), attaches uprobes, and skips any
    /// library whose inode has already been traced (dedup via `traced_files`).
    pub fn attach_process(&mut self, pid: i32) -> Result<()> {
        let libs = ssl_libs_from_maps(pid)?;
        if libs.is_empty() {
            log::warn!("[attach_process] pid={pid}: no SSL libraries found in maps");
            return Ok(());
        }

        // Debug: print all libs found
        log::debug!(
            "[attach_process] pid={pid}: found {} libs: {:?}",
            libs.len(),
            libs.iter()
                .map(|(p, i, k)| (p.as_str(), *i, format!("{k:?}")))
                .collect::<Vec<_>>()
        );

        let mut attached_inodes = Vec::new();
        for (path, inode, kind) in libs {
            // Skip libraries whose inode we already traced.
            // Now using pid=-1 for global attach, so each library only needs to be attached once.
            if !self.traced_files.insert(inode) {
                log::debug!("[attach_process] pid={pid}: skipping already-traced {path}");
                continue;
            }

            log::debug!("[attach_process] pid={pid}: attaching {kind:?} → {path}");

            // uprobe attach requires host filesystem paths, not /proc/{pid}/root/... paths.
            // canonicalize resolves /proc/{pid}/root symlink to the real host path,
            // handling both container (overlay rootfs) and non-container (/ symlink) cases.
            // Falls back to original path if the process has already exited.
            let uprobe_path = std::fs::canonicalize(&path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.clone());

            let result = match kind {
                // Use pid=-1 for global attach (all processes), avoiding per-process duplicate attaches
                SslLibKind::OpenSsl => attach_openssl(&mut self.skel, &uprobe_path, -1),
                SslLibKind::GnuTls => attach_gnutls(&mut self.skel, &uprobe_path, -1),
                SslLibKind::Nss => attach_nss(&mut self.skel, &uprobe_path, -1),
                SslLibKind::Boring => {
                    match attach_boringssl_by_symbol(&mut self.skel, &uprobe_path, -1) {
                        Ok(ls) => Ok(ls),
                        Err(sym_err) => {
                            log::debug!(
                                "[attach_process] pid={pid}: BoringSSL symbol attach failed for {path} ({sym_err:#}), falling back to byte-pattern"
                            );
                            match find_boringssl_offsets(&path) {
                                Some(off) => attach_boringssl_by_offset(
                                    &mut self.skel,
                                    &uprobe_path,
                                    &off,
                                    false,
                                    -1,
                                ),
                                None => {
                                    log::warn!(
                                        "[attach_process] pid={pid}: BoringSSL detection failed for {path} (no SSL_* in .dynsym and no byte-pattern match), skipping"
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                }
            };

            match result {
                Ok(ls) => {
                    self._links.extend(ls);
                    attached_inodes.push(inode);
                }
                Err(e) => {
                    // Attach failed: remove inode from traced_files so retries can succeed
                    self.traced_files.remove(&inode);
                    eprintln!("Warning: attach_process pid={pid} {path}: {e:#}");
                }
            }
        }

        // Record inodes attached for this pid so we can clean up on process exit
        if !attached_inodes.is_empty() {
            self.pid_inodes.insert(pid as u32, attached_inodes);
        }

        Ok(())
    }

    /// Clean up per-pid bookkeeping when a traced process exits.
    ///
    /// Inodes are intentionally kept in `traced_files` because uprobes are
    /// attached globally (`pid=-1`). The existing Links remain valid for all
    /// processes using the same library, so re-attaching would only create
    /// duplicate fds.
    pub fn detach_process(&mut self, pid: u32) {
        if let Some(inodes) = self.pid_inodes.remove(&pid) {
            log::debug!(
                "[detach_process] pid={pid}: removed pid_inodes entry ({} inodes, kept in traced_files)",
                inodes.len()
            );
        }
    }

    /// Spawn a background thread that polls the BPF ring buffer and sends
    /// decoded [`SslEvent`]s through an internal channel.
    ///
    /// Returns a [`SslPoller`] handle.  Drop it (or call [`SslPoller::stop`])
    /// to signal the poll thread to exit.
    pub fn run(&self) -> Result<SslPoller> {
        let tx = self.tx.clone();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_inner = Arc::clone(&stop_flag);

        // We need the ring-buffer map fd, which is owned by `self.skel`.
        // Build the RingBuffer here (on the calling thread) then move it
        // into the poll thread.
        let mut rb_builder = RingBufferBuilder::new();
        let binding = self.skel.maps();
        rb_builder
            .add(binding.rb(), move |data: &[u8]| {
                // SSL records are variable-length (tiered reservation): decode by
                // header prefix + buf_size, not a full-struct cast.
                if let Some(event) = SslEvent::from_bytes(data) {
                    let _ = tx.send(event);
                }
                0
            })
            .context("failed to add ring buffer")?;
        let rb = rb_builder.build().context("failed to build ring buffer")?;

        let handle = thread::Builder::new()
            .name("sslsniff-poll".into())
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
                            eprintln!("sslsniff poll error: {e:#}");
                            break;
                        }
                    }
                }
            })
            .context("failed to spawn poll thread")?;

        Ok(SslPoller {
            handle: Some(handle),
            stop_flag,
        })
    }

    /// Receive the next [`SslEvent`] from the background poll thread.
    ///
    /// Blocks until an event arrives or the sender is disconnected.
    pub fn recv(&self) -> Option<SslEvent> {
        self.rx.recv().ok()
    }

    /// Non-blocking variant of [`recv`](Self::recv).
    pub fn try_recv(&self) -> Option<SslEvent> {
        self.rx.try_recv().ok()
    }
}

// ─── Poll thread handle ─────────────────────────────────────────────────────

/// Handle returned by [`SslSniff::run`].
///
/// The background poll thread runs until this handle is dropped or
/// [`SslPoller::stop`] is called explicitly.
pub struct SslPoller {
    handle: Option<thread::JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
}

impl SslPoller {
    /// Signal the poll thread to stop and wait for it to finish.
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for SslPoller {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ─── BoringSSL pattern detection ─────────────────────────────────────────────

struct BoringSslOffsets {
    ssl_write: usize,
    ssl_read: usize,
    ssl_do_handshake: usize,
}

fn find_pattern(haystack: &[u8], pattern: &[u8]) -> Option<usize> {
    if pattern.is_empty() || pattern.len() > haystack.len() {
        return None;
    }
    haystack.windows(pattern.len()).position(|w| w == pattern)
}

/// Find all occurrences of `pattern` in `haystack`.
fn find_all_patterns(haystack: &[u8], pattern: &[u8]) -> Vec<usize> {
    if pattern.is_empty() || pattern.len() > haystack.len() {
        return Vec::new();
    }
    let mut results = Vec::new();
    let mut pos = 0;
    while pos + pattern.len() <= haystack.len() {
        if let Some(off) = find_pattern(&haystack[pos..], pattern) {
            results.push(pos + off);
            pos += off + 1;
        } else {
            break;
        }
    }
    results
}

fn find_boringssl_offsets(path: &str) -> Option<BoringSslOffsets> {
    // BoringSSL function prologue byte patterns (x86_64).
    // These are stable across versions because they represent the fixed
    // parameter-saving and state-setup logic of the POSIX SSL API.
    const HANDSHAKE_PAT: &[u8] = &[
        0x55, 0x48, 0x89, 0xe5, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x53, 0x48, 0x83,
        0xec, 0x28, 0x49, 0x89, 0xfc, 0x48, 0x8b, 0x47, 0x30,
    ];
    const READ_PAT: &[u8] = &[
        0x55, 0x48, 0x89, 0xe5, 0x41, 0x57, 0x41, 0x56, 0x53, 0x50, 0x48, 0x83, 0xbf, 0x98, 0x00,
        0x00, 0x00, 0x00, 0x74,
    ];
    const WRITE_PAT: &[u8] = &[
        0x55, 0x48, 0x89, 0xe5, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x53, 0x48, 0x83,
        0xec, 0x18, 0x41, 0x89, 0xd7, 0x49, 0x89, 0xf6, 0x48, 0x89, 0xfb,
    ];
    // Maximum distance between SSL_read and SSL_write in the same compilation unit.
    const ADJACENCY_THRESHOLD: usize = 0x1000; // 4KB
    let verbose = config::verbose();

    let data = fs::read(path).ok()?;

    // --- SSL_read: expect unique match ---
    let read_matches = find_all_patterns(&data, READ_PAT);
    if read_matches.is_empty() {
        if verbose {
            eprintln!("BoringSSL: SSL_read pattern not found in {path}");
        }
        return None;
    }
    let read_off = if read_matches.len() == 1 {
        read_matches[0]
    } else {
        if verbose {
            eprintln!(
                "BoringSSL: SSL_read pattern has {} matches, expected 1",
                read_matches.len()
            );
        }
        return None;
    };

    // --- SSL_do_handshake: expect unique match ---
    let hs_matches = find_all_patterns(&data, HANDSHAKE_PAT);
    if hs_matches.is_empty() {
        if verbose {
            eprintln!("BoringSSL: SSL_do_handshake pattern not found in {path}");
        }
        return None;
    }
    // Pick the match closest to (and before) SSL_read.
    let hs_off = if hs_matches.len() == 1 {
        hs_matches[0]
    } else {
        // Multiple matches: choose the one closest before read_off.
        match hs_matches.iter().filter(|&&o| o < read_off).next_back() {
            Some(&o) => o,
            None => {
                if verbose {
                    eprintln!(
                        "BoringSSL: SSL_do_handshake has {} matches, none before SSL_read",
                        hs_matches.len()
                    );
                }
                return None;
            }
        }
    };

    // --- SSL_write: adjacency verification ---
    let write_matches = find_all_patterns(&data, WRITE_PAT);
    if write_matches.is_empty() {
        if verbose {
            eprintln!("BoringSSL: SSL_write pattern not found in {path}");
        }
        return None;
    }
    // Pick the first match after SSL_read within ADJACENCY_THRESHOLD.
    let wr_off = write_matches
        .iter()
        .filter(|&&o| o > read_off && o - read_off < ADJACENCY_THRESHOLD)
        .copied()
        .next()
        .or_else(|| {
            if verbose {
                eprintln!(
                    "BoringSSL: SSL_write has {} matches but none within {}B after SSL_read ({:#x})",
                    write_matches.len(),
                    ADJACENCY_THRESHOLD,
                    read_off
                );
            }
            None
        })?;

    log::debug!("BoringSSL detected in {path}:");
    log::debug!("  SSL_do_handshake: {hs_off:#x}");
    log::debug!("  SSL_read:         {read_off:#x}");
    log::debug!("  SSL_write:        {wr_off:#x}");

    Some(BoringSslOffsets {
        ssl_write: wr_off,
        ssl_read: read_off,
        ssl_do_handshake: hs_off,
    })
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// SSL library kind detected from `/proc/<pid>/maps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SslLibKind {
    /// libssl.so  (OpenSSL / LibreSSL)
    OpenSsl,
    /// libgnutls.so
    GnuTls,
    /// libnspr4.so (NSS / Firefox)
    Nss,
    /// BoringSSL embedded in binary (e.g. Node.js, Chrome)
    Boring,
}

/// Classify a mapped file path into an `SslLibKind`, if it is an SSL library.
fn classify_ssl_lib(path: &str) -> Option<SslLibKind> {
    // Strip " (deleted)" suffix that the kernel appends when the backing file
    // has been unlinked while the process is still running.
    let raw_path = path.strip_suffix(" (deleted)").unwrap_or(path);
    let name = Path::new(raw_path).file_name()?.to_string_lossy();
    if name.starts_with("libssl.so") || name.starts_with("libssl-") {
        return Some(SslLibKind::OpenSsl);
    }
    if name.starts_with("libgnutls.so") || name.starts_with("libgnutls-") {
        return Some(SslLibKind::GnuTls);
    }
    if name.starts_with("libnspr4.so") || name.starts_with("libnspr4-") {
        return Some(SslLibKind::Nss);
    }
    // BoringSSL is typically linked statically into a binary (node, chrome, etc.).
    // Detect common binary names that are known to embed BoringSSL.
    if matches!(
        name.as_ref(),
        "node"
            | "nodejs"
            | "bun"
            | "deno"
            | "chrome"
            | "chromium"
            | "google-chrome"
            | "google-chrome-stable"
            | "claude.exe"
    ) {
        return Some(SslLibKind::Boring);
    }
    // uv Python statically links OpenSSL into the binary. The ELF .symtab contains
    // SSL_write/SSL_read/SSL_do_handshake as LOCAL symbols, so attach_openssl()
    // (symbol-name uprobe) works directly. Only match python3.<ver> (with version
    // suffix) to avoid matching bare "python3" symlinks from system Python.
    if name.starts_with("python3.") {
        return Some(SslLibKind::OpenSsl);
    }
    None
}

/// Parse `/proc/<pid>/maps` via `procfs` and return `(absolute_path, inode, SslLibKind)`
/// for every SSL-related library found.
///
/// Each unique inode is returned at most once.
fn ssl_libs_from_maps(pid: i32) -> Result<Vec<(String, u64, SslLibKind)>> {
    let proc = Process::new(pid).with_context(|| format!("failed to open /proc/{pid}"))?;
    let maps = proc
        .maps()
        .with_context(|| format!("failed to read /proc/{pid}/maps"))?;

    let mut seen_inodes: HashSet<u64> = HashSet::new();
    let mut results: Vec<(String, u64, SslLibKind)> = Vec::new();

    for entry in maps.iter() {
        // Only care about file-backed mappings.
        let path_str = match &entry.pathname {
            procfs::process::MMapPath::Path(p) => p.to_string_lossy().into_owned(),
            _ => continue,
        };
        // inode comes from the memory map entry's inode field.
        let inode = entry.inode;
        if inode == 0 || seen_inodes.contains(&inode) {
            continue;
        }
        if let Some(kind) = classify_ssl_lib(&path_str) {
            seen_inodes.insert(inode);
            // When the backing file has been unlinked (" (deleted)" in maps),
            // the filesystem path no longer exists.  Fall back to /proc/<pid>/exe
            // which the kernel keeps accessible as long as the process is alive.
            let path_str = if path_str.ends_with(" (deleted)") {
                format!("/proc/{pid}/exe")
            } else {
                format!("/proc/{pid}/root{path_str}")
            };
            results.push((path_str, inode, kind));
        }
    }

    Ok(results)
}

// ─── uprobe helpers ───────────────────────────────────────────────────────────

#[allow(clippy::field_reassign_with_default)]
fn make_sym_opts(sym: &str, retprobe: bool) -> UprobeOpts {
    let mut o = UprobeOpts::default();
    o.func_name = sym.to_string();
    o.retprobe = retprobe;
    o
}

macro_rules! up {
    ($prog:expr, $pid:expr, $path:expr, $sym:expr) => {
        $prog
            .attach_uprobe_with_opts($pid, $path, 0, make_sym_opts($sym, false))
            .with_context(|| format!("uprobe {}@{}", $sym, $path))
    };
}
macro_rules! ur {
    ($prog:expr, $pid:expr, $path:expr, $sym:expr) => {
        $prog
            .attach_uprobe_with_opts($pid, $path, 0, make_sym_opts($sym, true))
            .with_context(|| format!("uretprobe {}@{}", $sym, $path))
    };
}
macro_rules! up_off {
    ($prog:expr, $pid:expr, $path:expr, $off:expr) => {
        $prog
            .attach_uprobe(false, $pid, $path, $off)
            .with_context(|| format!("uprobe offset {:#x}@{}", $off, $path))
    };
}
macro_rules! ur_off {
    ($prog:expr, $pid:expr, $path:expr, $off:expr) => {
        $prog
            .attach_uprobe(true, $pid, $path, $off)
            .with_context(|| format!("uretprobe offset {:#x}@{}", $off, $path))
    };
}

fn attach_openssl(skel: &mut SslsniffSkel<'_>, lib: &str, pid: i32) -> Result<Vec<Link>> {
    Ok(vec![
        up!(skel.progs_mut().probe_SSL_rw_enter(), pid, lib, "SSL_write")?,
        ur!(
            skel.progs_mut().probe_SSL_write_exit(),
            pid,
            lib,
            "SSL_write"
        )?,
        up!(skel.progs_mut().probe_SSL_rw_enter(), pid, lib, "SSL_read")?,
        ur!(skel.progs_mut().probe_SSL_read_exit(), pid, lib, "SSL_read")?,
        up!(
            skel.progs_mut().probe_SSL_write_ex_enter(),
            pid,
            lib,
            "SSL_write_ex"
        )?,
        ur!(
            skel.progs_mut().probe_SSL_write_ex_exit(),
            pid,
            lib,
            "SSL_write_ex"
        )?,
        up!(
            skel.progs_mut().probe_SSL_read_ex_enter(),
            pid,
            lib,
            "SSL_read_ex"
        )?,
        ur!(
            skel.progs_mut().probe_SSL_read_ex_exit(),
            pid,
            lib,
            "SSL_read_ex"
        )?,
        up!(
            skel.progs_mut().probe_SSL_do_handshake_enter(),
            pid,
            lib,
            "SSL_do_handshake"
        )?,
        ur!(
            skel.progs_mut().probe_SSL_do_handshake_exit(),
            pid,
            lib,
            "SSL_do_handshake"
        )?,
    ])
}

fn attach_gnutls(skel: &mut SslsniffSkel<'_>, lib: &str, pid: i32) -> Result<Vec<Link>> {
    Ok(vec![
        up!(
            skel.progs_mut().probe_SSL_rw_enter(),
            pid,
            lib,
            "gnutls_record_send"
        )?,
        ur!(
            skel.progs_mut().probe_SSL_write_exit(),
            pid,
            lib,
            "gnutls_record_send"
        )?,
        up!(
            skel.progs_mut().probe_SSL_rw_enter(),
            pid,
            lib,
            "gnutls_record_recv"
        )?,
        ur!(
            skel.progs_mut().probe_SSL_read_exit(),
            pid,
            lib,
            "gnutls_record_recv"
        )?,
    ])
}

fn attach_nss(skel: &mut SslsniffSkel<'_>, lib: &str, pid: i32) -> Result<Vec<Link>> {
    Ok(vec![
        up!(skel.progs_mut().probe_SSL_rw_enter(), pid, lib, "PR_Write")?,
        ur!(
            skel.progs_mut().probe_SSL_write_exit(),
            pid,
            lib,
            "PR_Write"
        )?,
        up!(skel.progs_mut().probe_SSL_rw_enter(), pid, lib, "PR_Send")?,
        ur!(skel.progs_mut().probe_SSL_write_exit(), pid, lib, "PR_Send")?,
        up!(skel.progs_mut().probe_SSL_rw_enter(), pid, lib, "PR_Read")?,
        ur!(skel.progs_mut().probe_SSL_read_exit(), pid, lib, "PR_Read")?,
        up!(skel.progs_mut().probe_SSL_rw_enter(), pid, lib, "PR_Recv")?,
        ur!(skel.progs_mut().probe_SSL_read_exit(), pid, lib, "PR_Recv")?,
    ])
}

fn attach_boringssl_by_symbol(
    skel: &mut SslsniffSkel<'_>,
    lib: &str,
    pid: i32,
) -> Result<Vec<Link>> {
    Ok(vec![
        up!(skel.progs_mut().probe_SSL_rw_enter(), pid, lib, "SSL_write")?,
        ur!(
            skel.progs_mut().probe_SSL_write_exit(),
            pid,
            lib,
            "SSL_write"
        )?,
        up!(skel.progs_mut().probe_SSL_rw_enter(), pid, lib, "SSL_read")?,
        ur!(skel.progs_mut().probe_SSL_read_exit(), pid, lib, "SSL_read")?,
        up!(
            skel.progs_mut().probe_SSL_do_handshake_enter(),
            pid,
            lib,
            "SSL_do_handshake"
        )?,
        ur!(
            skel.progs_mut().probe_SSL_do_handshake_exit(),
            pid,
            lib,
            "SSL_do_handshake"
        )?,
    ])
}

fn attach_boringssl_by_offset(
    skel: &mut SslsniffSkel<'_>,
    lib: &str,
    off: &BoringSslOffsets,
    handshake: bool,
    pid: i32,
) -> Result<Vec<Link>> {
    let mut links = vec![
        up_off!(
            skel.progs_mut().probe_SSL_rw_enter(),
            pid,
            lib,
            off.ssl_write
        )?,
        ur_off!(
            skel.progs_mut().probe_SSL_write_exit(),
            pid,
            lib,
            off.ssl_write
        )?,
        up_off!(
            skel.progs_mut().probe_SSL_rw_enter(),
            pid,
            lib,
            off.ssl_read
        )?,
        ur_off!(
            skel.progs_mut().probe_SSL_read_exit(),
            pid,
            lib,
            off.ssl_read
        )?,
    ];
    if handshake {
        links.push(up_off!(
            skel.progs_mut().probe_SSL_do_handshake_enter(),
            pid,
            lib,
            off.ssl_do_handshake
        )?);
        links.push(ur_off!(
            skel.progs_mut().probe_SSL_do_handshake_exit(),
            pid,
            lib,
            off.ssl_do_handshake
        )?);
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a raw ring-buffer sample: the header prefix (each field written at its
    /// real bindgen offset) followed by `payload`, with buf_size/len/truncated/
    /// is_handshake set explicitly. Each scalar header field gets a DISTINCT
    /// sentinel (pid≠tid≠uid, a marker delta_ns/comm) so any field/offset swap among
    /// the adjacent u32s or on rw/comm/is_handshake fails a test. `tail_pad` appends
    /// bytes AFTER the payload to simulate a still-full-size record (e.g. tcpsniff's
    /// 4 MiB reservation) whose data.len() exceeds buf_size.
    fn make_record(
        payload: &[u8],
        buf_size: u32,
        len: u32,
        truncated: i32,
        tail_pad: usize,
        is_handshake: i32,
    ) -> Vec<u8> {
        type R = bpf::probe_SSL_data_t;
        let hdr = std::mem::offset_of!(R, buf);
        let mut v = vec![0u8; hdr + payload.len() + tail_pad];
        let put_u32 = |v: &mut [u8], off: usize, val: u32| {
            v[off..off + 4].copy_from_slice(&val.to_ne_bytes())
        };
        let put_u64 = |v: &mut [u8], off: usize, val: u64| {
            v[off..off + 8].copy_from_slice(&val.to_ne_bytes())
        };
        let put_i32 = |v: &mut [u8], off: usize, val: i32| {
            v[off..off + 4].copy_from_slice(&val.to_ne_bytes())
        };
        put_u32(&mut v, std::mem::offset_of!(R, source), 2); // EVENT_SOURCE_SSL
        put_u64(&mut v, std::mem::offset_of!(R, timestamp_ns), 0);
        put_u64(&mut v, std::mem::offset_of!(R, delta_ns), 0xDEAD_BEEF);
        put_u32(&mut v, std::mem::offset_of!(R, pid), 1234);
        put_u32(&mut v, std::mem::offset_of!(R, tid), 5678);
        put_u32(&mut v, std::mem::offset_of!(R, uid), 4321);
        put_u32(&mut v, std::mem::offset_of!(R, len), len);
        put_u32(&mut v, std::mem::offset_of!(R, buf_size), buf_size);
        put_i32(&mut v, std::mem::offset_of!(R, rw), 1);
        put_i32(&mut v, std::mem::offset_of!(R, is_handshake), is_handshake);
        put_i32(&mut v, std::mem::offset_of!(R, truncated), truncated);
        put_u64(&mut v, std::mem::offset_of!(R, ssl_ptr), 0xABCD);
        let comm_off = std::mem::offset_of!(R, comm);
        v[comm_off..comm_off + 4].copy_from_slice(b"node");
        v[hdr..hdr + payload.len()].copy_from_slice(payload);
        v
    }

    #[test]
    fn from_bytes_decodes_small_record() {
        // A small (16 KiB-tier) record — the whole point of the fix. Under the old
        // `data.len() >= size_of::<full struct>()` (~4 MiB) gate this sample was
        // DROPPED, so reverting to that gate makes this test fail.
        let payload = b"POST /v1/messages HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let rec = make_record(payload, payload.len() as u32, payload.len() as u32, 0, 0, 0);
        assert!(
            rec.len() < MAX_BUF_SIZE,
            "record is far smaller than the full struct"
        );
        let ev = SslEvent::from_bytes(&rec).expect("small record must decode");
        assert_eq!(&ev.buf[..], &payload[..]);
        assert_eq!(ev.len as usize, payload.len());
        assert_eq!(ev.ssl_ptr, 0xABCD);
        // Each header scalar decodes from its OWN offset (distinct sentinels: a
        // pid/tid offset swap, or an unset uid/delta_ns/rw/comm, fails here).
        assert_eq!(ev.pid, 1234);
        assert_eq!(ev.tid, 5678, "tid decodes from its own offset, not pid's");
        assert_eq!(ev.uid, 4321);
        assert_eq!(ev.delta_ns, 0xDEAD_BEEF);
        assert_eq!(ev.rw, 1);
        assert_eq!(ev.comm, "node");
        assert!(!ev.is_handshake);
    }

    #[test]
    fn from_bytes_decodes_handshake_header_only() {
        // A handshake record carries NO payload: the BPF reserves header-only, so
        // data.len() == offset_of!(buf) EXACTLY. The decoder must ACCEPT this
        // boundary (a `<`→`<=` off-by-one at the header-length check would reject it)
        // and decode is_handshake. Complements the reject-at-hdr-1 test below, so the
        // header boundary is pinned on both sides.
        let rec = make_record(&[], 0, 0, 0, 0, 1);
        assert_eq!(
            rec.len(),
            std::mem::offset_of!(bpf::probe_SSL_data_t, buf),
            "handshake record is exactly the header prefix"
        );
        let ev = SslEvent::from_bytes(&rec).expect("header-only handshake must decode");
        assert!(ev.is_handshake, "is_handshake decodes true");
        assert!(ev.buf.is_empty(), "no payload");
    }

    #[test]
    fn from_bytes_uses_buf_size_field_not_data_len() {
        // A still-full-size record (e.g. tcpsniff's 4 MiB reservation): data carries
        // many bytes after the payload, but buf_size says only `n` are real. The
        // decoder MUST take buf_size bytes, never data.len()-hdr — otherwise it would
        // read the padding tail. Reverting to a data.len()-derived length fails this.
        let payload = b"hi there";
        let rec = make_record(
            payload,
            payload.len() as u32,
            payload.len() as u32,
            0,
            4096,
            0,
        );
        let ev = SslEvent::from_bytes(&rec).expect("full-size record must decode");
        assert_eq!(
            &ev.buf[..],
            &payload[..],
            "buf is the buf_size bytes, not the padded tail"
        );
    }

    #[test]
    fn from_bytes_rejects_short_header() {
        // A sample shorter than the header prefix is rejected (no UB, no full cast).
        let hdr = std::mem::offset_of!(bpf::probe_SSL_data_t, buf);
        assert!(SslEvent::from_bytes(&vec![0u8; hdr - 1]).is_none());
    }

    #[test]
    fn from_bytes_decodes_truncated_record() {
        // A truncated record (payload clamped to the cap): buf_size < len, truncated=1,
        // len > MAX_BUF_SIZE. The decoder still returns the captured bytes; len reports
        // the true size. Also exercises the warn guard's positive case.
        let captured = vec![b'x'; 64];
        let rec = make_record(&captured, captured.len() as u32, 9_000_000, 1, 0, 0);
        let ev = SslEvent::from_bytes(&rec).expect("truncated record must still decode");
        assert_eq!(ev.buf.len(), 64);
        assert_eq!(
            ev.len, 9_000_000,
            "len reports the true (pre-truncation) size"
        );
    }

    #[test]
    fn from_bytes_clamps_oversized_buf_size_to_available() {
        // Defense: a buf_size larger than the bytes present must not read past the
        // sample (clamped to data.len()-hdr).
        let payload = b"abc";
        let rec = make_record(payload, 1000, 1000, 0, 0, 0);
        let ev = SslEvent::from_bytes(&rec).expect("decode");
        assert_eq!(
            ev.buf.len(),
            payload.len(),
            "buf_size clamped to available bytes"
        );
    }

    #[test]
    fn uprobe_path_canonicalize_resolves_real_path() {
        // Test canonicalize-based path resolution for uprobe attach.
        // canonicalize follows symlinks, resolving /proc/{pid}/root/... to host paths.

        // Case 1: Real path on host filesystem is resolved to itself
        let host_path = "/usr/lib/x86_64-linux-gnu/libssl.so.3";
        if std::path::Path::new(host_path).exists() {
            let resolved = std::fs::canonicalize(host_path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| host_path.to_string());
            // canonicalize of an existing absolute path returns the real path
            assert!(
                !resolved.contains("/proc/"),
                "resolved path should not contain /proc/ prefix: {resolved}"
            );
        }

        // Case 2: /proc/self/root/... resolves to host path (self is always valid)
        let self_root_path = "/proc/self/root/etc/hosts";
        if std::path::Path::new(self_root_path).exists() {
            let resolved = std::fs::canonicalize(self_root_path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| self_root_path.to_string());
            assert_eq!(
                resolved, "/etc/hosts",
                "/proc/self/root/etc/hosts should resolve to /etc/hosts"
            );
        }

        // Case 3: Non-existent path falls back to original (simulates exited process)
        let dead_proc_path = "/proc/999999999/root/usr/lib/libssl.so";
        let resolved = std::fs::canonicalize(dead_proc_path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| dead_proc_path.to_string());
        assert_eq!(
            resolved, dead_proc_path,
            "non-existent path should fall back to original"
        );
    }
}
