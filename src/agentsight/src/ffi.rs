//! FFI interface for AgentSight — C API for other languages
//!
//! Implements the **eventfd + read** model described in `docs/design-docs/c-ffi-api.md`:
//! AgentSight runs a background pipeline thread; completed events are pushed
//! into an `mpsc` channel and the caller is notified via `eventfd`.
//! The caller consumes events by calling `agentsight_read()` with callbacks.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::analyzer::HttpRecord;
use crate::config::AgentsightConfig;
use crate::genai::semantic::LLMCall;
use crate::unified::AgentSight;

// ===========================================================================
// Internal FFI event types (shared with unified.rs via crate::ffi)
// ===========================================================================

/// Internal event for FFI communication between pipeline and consumer.
///
/// Mutually exclusive: an HTTP exchange either becomes `Llm` (if recognised
/// as an LLM API call) or `Https` (otherwise).  See §5 in c-ffi-api.md.
pub(crate) enum FfiEvent {
    Https(HttpRecord),
    Llm(LLMCall),
}

/// Commands sent from the FFI caller thread to the background pipeline thread.
enum ProbeCommand {
    AddCgroup(u64),
    RemoveCgroup(u64),
}

/// Wraps an `mpsc::Sender<FfiEvent>` together with the `eventfd` descriptor
/// so that a single `.send()` call both enqueues the event and wakes up the
/// consumer's epoll/select loop.
pub(crate) struct FfiEventSender {
    tx: mpsc::Sender<FfiEvent>,
    eventfd: i32,
}

impl FfiEventSender {
    pub fn send(&self, event: FfiEvent) {
        if self.tx.send(event).is_ok() {
            // Write 1 to the eventfd counter to wake up the consumer.
            let val: u64 = 1;
            unsafe {
                libc::write(self.eventfd, &val as *const u64 as *const c_void, 8);
            }
        }
    }
}

// ===========================================================================
// Thread-local last-error storage
// ===========================================================================

thread_local! {
    static LAST_ERROR: std::cell::RefCell<Option<CString>> = const { std::cell::RefCell::new(None) };
}

fn set_last_error(msg: &str) {
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = CString::new(msg.replace('\0', "")).ok();
    });
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Create a `CString` from a Rust `&str`, replacing interior NUL bytes.
fn safe_cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

/// Copy a Rust string into a fixed-size `[c_char; 16]` buffer (NUL-terminated).
fn copy_process_name(name: &str) -> [c_char; 16] {
    let mut buf = [0 as c_char; 16];
    let bytes = name.as_bytes();
    let len = bytes.len().min(15);
    for i in 0..len {
        buf[i] = bytes[i] as c_char;
    }
    buf
}

/// Drain the eventfd counter so that epoll won't re-trigger.
fn drain_eventfd(fd: i32) {
    let mut buf: u64 = 0;
    unsafe {
        libc::read(fd, &mut buf as *mut u64 as *mut c_void, 8);
    }
}

// ===========================================================================
// C-compatible data structures  (§1 of c-ffi-api.md)
// ===========================================================================

/// HTTP layer data — non-LLM HTTPS traffic produces this structure.
#[repr(C)]
pub struct AgentsightHttpsData {
    pub pid: i32,
    pub process_name: [c_char; 16],
    pub timestamp_ns: u64,
    pub duration_ns: u64,
    pub method: *const c_char,
    pub path: *const c_char,
    pub status_code: u16,
    pub is_sse: u8,
    pub request_headers: *const c_char,
    pub request_headers_len: u32,
    pub request_body: *const c_char,
    pub request_body_len: u32,
    pub response_headers: *const c_char,
    pub response_headers_len: u32,
    pub response_body: *const c_char,
    pub response_body_len: u32,
}

/// LLM semantic layer data — only when the HTTP traffic is recognised as
/// an LLM API call.
#[repr(C)]
pub struct AgentsightLLMData {
    pub response_id: *const c_char,
    pub conversation_id: *const c_char,
    pub session_id: *const c_char,
    pub pid: i32,
    pub process_name: [c_char; 16],
    pub agent_name: *const c_char,
    pub container_id: *const c_char,
    pub timestamp_ns: u64,
    pub duration_ns: u64,
    pub request_url: *const c_char,
    pub provider: *const c_char,
    pub model: *const c_char,
    pub status_code: u16,
    pub is_sse: u8,
    pub finish_reason: *const c_char,
    pub llm_usage: bool,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
    pub request_messages: *const c_char,
    pub request_messages_len: u32,
    pub response_messages: *const c_char,
    pub response_messages_len: u32,
    pub tools: *const c_char,
    pub tools_len: u32,
    /// Incremental (latest-round) request messages: the same per-round
    /// increment stored in SQLite (`genai_events.input_messages`). System
    /// messages are dropped and everything from the last `user` message onward
    /// is kept (inclusive). JSON array of InputMessage.
    pub input_message_delta: *const c_char,
    pub input_message_delta_len: u32,
}

// ===========================================================================
// Opaque handles
// ===========================================================================

/// Configuration handle (created → configured → passed to `agentsight_new`).
/// cbindgen:no-export
pub type AgentsightConfigHandle = AgentsightConfig;

/// Main runtime handle.
/// cbindgen:no-export
pub struct AgentsightHandle {
    rx: mpsc::Receiver<FfiEvent>,
    /// Sender kept alive so the background thread's sends don't fail
    /// after start; taken (moved) into the thread in `agentsight_start`.
    tx: Option<mpsc::Sender<FfiEvent>>,
    eventfd: i32,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Config stored until `agentsight_start()` moves it into the thread.
    config: Option<AgentsightConfig>,
    /// Channel for runtime probe control commands (e.g. dynamic cgroup filter updates).
    /// Created in `agentsight_start()`; the receiver end is moved into the background thread.
    probe_cmd_tx: Option<mpsc::Sender<ProbeCommand>>,
}

// ===========================================================================
// Callback type aliases
// ===========================================================================

type HttpsCallbackFn = Option<unsafe extern "C" fn(*const AgentsightHttpsData, *mut c_void)>;
type LlmCallbackFn = Option<unsafe extern "C" fn(*const AgentsightLLMData, *mut c_void)>;

/// Flag for `agentsight_read()`: block until at least one event is available.
pub const AGENTSIGHT_READ_BLOCK: c_int = 1;

// ===========================================================================
// Temporary data holders (keep CStrings alive during callbacks)
// ===========================================================================

struct HttpsDataHolder {
    c_data: AgentsightHttpsData,
    _method: CString,
    _path: CString,
    _req_headers: CString,
    _req_body: Option<CString>,
    _resp_headers: CString,
    _resp_body: Option<CString>,
}

struct LlmDataHolder {
    c_data: AgentsightLLMData,
    _response_id: Option<CString>,
    _conversation_id: Option<CString>,
    _session_id: Option<CString>,
    _agent_name: Option<CString>,
    _container_id: Option<CString>,
    _request_url: CString,
    _provider: CString,
    _model: CString,
    _finish_reason: Option<CString>,
    _req_messages: CString,
    _resp_messages: CString,
    _tools: CString,
    _input_message_delta: CString,
}

fn build_https_data(record: &HttpRecord) -> HttpsDataHolder {
    let method = safe_cstring(&record.method);
    let path = safe_cstring(&record.path);
    let req_headers = safe_cstring(&record.request_headers);
    let req_body = record.request_body.as_ref().map(|b| safe_cstring(b));
    let resp_headers = safe_cstring(&record.response_headers);
    let resp_body = record.response_body.as_ref().map(|b| safe_cstring(b));

    let c_data = AgentsightHttpsData {
        pid: record.pid as i32,
        process_name: copy_process_name(&record.comm),
        timestamp_ns: record.timestamp_ns,
        duration_ns: record.duration_ns,
        method: method.as_ptr(),
        path: path.as_ptr(),
        status_code: record.status_code,
        is_sse: record.is_sse as u8,
        request_headers: req_headers.as_ptr(),
        request_headers_len: record.request_headers.len() as u32,
        request_body: req_body.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        request_body_len: record.request_body.as_ref().map_or(0, |s| s.len() as u32),
        response_headers: resp_headers.as_ptr(),
        response_headers_len: record.response_headers.len() as u32,
        response_body: resp_body.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        response_body_len: record.response_body.as_ref().map_or(0, |s| s.len() as u32),
    };

    HttpsDataHolder {
        c_data,
        _method: method,
        _path: path,
        _req_headers: req_headers,
        _req_body: req_body,
        _resp_headers: resp_headers,
        _resp_body: resp_body,
    }
}

fn build_llm_data(call: &LLMCall) -> LlmDataHolder {
    let response_id = call.metadata.get("response_id").map(|s| safe_cstring(s));
    let conversation_id = call
        .metadata
        .get("conversation_id")
        .map(|s| safe_cstring(s));
    let session_id = call.metadata.get("session_id").map(|s| safe_cstring(s));
    let agent_name = call.agent_name.as_ref().map(|s| safe_cstring(s));
    let container_id =
        crate::container::extract_container_id_cached(call.pid as u32).map(|s| safe_cstring(&s));

    // Construct request_url from metadata
    let server_addr = call
        .metadata
        .get("server.address")
        .cloned()
        .unwrap_or_default();
    let server_port = call
        .metadata
        .get("server.port")
        .cloned()
        .unwrap_or_default();
    let path = call.metadata.get("path").cloned().unwrap_or_default();
    let url = if server_port.is_empty() {
        format!("https://{server_addr}{path}")
    } else {
        format!("https://{server_addr}:{server_port}{path}")
    };
    let request_url = safe_cstring(&url);

    let provider = safe_cstring(&call.provider);
    let model = safe_cstring(&call.model);

    let status_code: u16 = call
        .metadata
        .get("status_code")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let is_sse: bool = call.metadata.get("is_sse").is_some_and(|s| s == "true");

    let finish_reason = call
        .response
        .messages
        .first()
        .and_then(|m| m.finish_reason.as_ref())
        .map(|s| safe_cstring(s));

    let (llm_usage, input_tokens, output_tokens, total_tokens, cache_creation, cache_read) =
        match &call.token_usage {
            Some(u) => (
                true,
                u.input_tokens,
                u.output_tokens,
                u.total_tokens,
                u.cache_creation_input_tokens.unwrap_or(0),
                u.cache_read_input_tokens.unwrap_or(0),
            ),
            None => (false, 0, 0, 0, 0, 0),
        };

    let req_messages_json = serde_json::to_string(&call.request.messages).unwrap_or_default();
    let resp_messages_json = serde_json::to_string(&call.response.messages).unwrap_or_default();
    let req_messages = safe_cstring(&req_messages_json);
    let resp_messages = safe_cstring(&resp_messages_json);

    // Incremental (latest-round) input messages: the same per-round increment
    // stored in SQLite (`genai_events.input_messages`). Drops system messages
    // and keeps everything from the last `user` message onward.
    let input_delta = crate::genai::semantic::latest_round_input_messages(&call.request.messages);
    let input_message_delta_json = serde_json::to_string(&input_delta).unwrap_or_default();
    let input_message_delta = safe_cstring(&input_message_delta_json);

    let tools_json = call
        .request
        .tools
        .as_ref()
        .map(|tools| serde_json::to_string(tools).unwrap_or_default())
        .unwrap_or_else(|| "[]".to_string());
    let tools = safe_cstring(&tools_json);

    let c_data = AgentsightLLMData {
        response_id: response_id.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        conversation_id: conversation_id.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        session_id: session_id.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        pid: call.pid,
        process_name: copy_process_name(&call.process_name),
        agent_name: agent_name.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        container_id: container_id.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        timestamp_ns: call.start_timestamp_ns,
        duration_ns: call.duration_ns,
        request_url: request_url.as_ptr(),
        provider: provider.as_ptr(),
        model: model.as_ptr(),
        status_code,
        is_sse: is_sse as u8,
        finish_reason: finish_reason.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        llm_usage,
        input_tokens,
        output_tokens,
        total_tokens,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
        request_messages: req_messages.as_ptr(),
        request_messages_len: req_messages_json.len() as u32,
        response_messages: resp_messages.as_ptr(),
        response_messages_len: resp_messages_json.len() as u32,
        tools: tools.as_ptr(),
        tools_len: tools_json.len() as u32,
        input_message_delta: input_message_delta.as_ptr(),
        input_message_delta_len: input_message_delta_json.len() as u32,
    };

    LlmDataHolder {
        c_data,
        _response_id: response_id,
        _conversation_id: conversation_id,
        _session_id: session_id,
        _agent_name: agent_name,
        _container_id: container_id,
        _request_url: request_url,
        _provider: provider,
        _model: model,
        _finish_reason: finish_reason,
        _req_messages: req_messages,
        _resp_messages: resp_messages,
        _tools: tools,
        _input_message_delta: input_message_delta,
    }
}

/// Dispatch a single FFI event to the appropriate callback.
///
/// # Safety
/// Caller must ensure the callback function pointers and user-data pointers
/// are valid for the duration of the call.
unsafe fn dispatch_event(
    event: FfiEvent,
    http_cb: HttpsCallbackFn,
    http_ud: *mut c_void,
    llm_cb: LlmCallbackFn,
    llm_ud: *mut c_void,
) {
    match event {
        FfiEvent::Https(record) => {
            if let Some(cb) = http_cb {
                let holder = build_https_data(&record);
                unsafe { cb(&holder.c_data, http_ud) };
            }
        }
        FfiEvent::Llm(call) => {
            if let Some(cb) = llm_cb {
                let holder = build_llm_data(&call);
                unsafe { cb(&holder.c_data, llm_ud) };
            }
        }
    }
}

// ===========================================================================
// C API functions  (§2 of c-ffi-api.md)
// ===========================================================================

// ---- Error handling ----

/// Return the last error description, or NULL if no error.
/// The pointer is valid until the next API call on the same thread.
#[unsafe(no_mangle)]
pub extern "C" fn agentsight_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ref().map_or(ptr::null(), |s| s.as_ptr()))
}

// ---- Configuration ----

/// Create a new configuration with default values.
#[unsafe(no_mangle)]
pub extern "C" fn agentsight_config_new() -> *mut AgentsightConfigHandle {
    Box::into_raw(Box::new(AgentsightConfig::default()))
}

/// Set the verbose flag (0 = off, 1 = on).
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_config_set_verbose(
    cfg: *mut AgentsightConfigHandle,
    verbose: c_int,
) {
    if !cfg.is_null() {
        unsafe { (*cfg).verbose = verbose != 0 };
    }
}

/// Set the log file path (NULL → stderr).
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_config_set_log_path(
    cfg: *mut AgentsightConfigHandle,
    path: *const c_char,
) {
    if cfg.is_null() {
        return;
    }
    unsafe {
        (*cfg).log_path = if path.is_null() {
            None
        } else {
            Some(CStr::from_ptr(path).to_string_lossy().to_string())
        };
    }
}

/// Add a cmdline rule (allowlist or denylist).
/// * `rule` — NULL-terminated array of C strings (glob patterns).
/// * `agent_name` — agent name for allow=1; ignored for allow=0 (may be NULL).
/// * `allow` — 1 = whitelist (attach), 0 = blacklist (don't attach).
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_config_add_cmdline_rule(
    cfg: *mut AgentsightConfigHandle,
    rule: *const *const c_char,
    agent_name: *const c_char,
    allow: c_int,
) {
    if cfg.is_null() || rule.is_null() {
        return;
    }
    let c = unsafe { &mut *cfg };

    // Collect patterns from NULL-terminated array
    let mut patterns = Vec::new();
    let mut i = 0usize;
    loop {
        let ptr = unsafe { *rule.add(i) };
        if ptr.is_null() {
            break;
        }
        let s = unsafe { CStr::from_ptr(ptr).to_string_lossy().to_string() };
        if !s.is_empty() {
            patterns.push(s);
        }
        i += 1;
    }

    if patterns.is_empty() {
        return;
    }

    let agent_name = if agent_name.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(agent_name).to_string_lossy().to_string() })
    };

    c.cmdline_rules.push(crate::config::CmdlineRule {
        patterns,
        agent_name,
        allow: allow != 0,
    });
}

/// Add an HTTPS rule (domain glob pattern for SSL/TLS probe attachment).
/// * `rule` — domain glob pattern (e.g. "*.openai.com").
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_config_add_https(
    cfg: *mut AgentsightConfigHandle,
    rule: *const c_char,
) {
    if cfg.is_null() || rule.is_null() {
        return;
    }
    let c = unsafe { &mut *cfg };
    let s = unsafe { CStr::from_ptr(rule).to_string_lossy().to_string() };
    if !s.is_empty() {
        c.https_rules.push(crate::config::HttpsRule { pattern: s });
    }
}

/// Add an HTTP capture target for plain HTTP traffic (tcpsniff probe).
///
/// * `target` — string that is auto-detected:
///   - `":8080"`          → port-only endpoint
///   - `"10.0.0.1"`       → IP-only endpoint
///   - `"10.0.0.1:8080"`  → IP+port endpoint
///   - `"model-svc.default.svc"` → domain (DNS-resolved at runtime)
///
/// Returns 0 on success, <0 on error (call `agentsight_last_error()`).
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_config_add_http(
    cfg: *mut AgentsightConfigHandle,
    target: *const c_char,
) -> c_int {
    if cfg.is_null() || target.is_null() {
        set_last_error("NULL config or target");
        return -1;
    }
    let c = unsafe { &mut *cfg };
    let s = unsafe { CStr::from_ptr(target).to_string_lossy().to_string() };
    if s.is_empty() {
        set_last_error("Empty HTTP target string");
        return -1;
    }
    match s.parse::<crate::config::TcpTarget>() {
        Ok(t) => c.http_targets.push(crate::config::HttpTarget::Endpoint(t)),
        Err(_) => c.http_targets.push(crate::config::HttpTarget::Domain(s)),
    }
    0
}

/// Load configuration from a JSON string. Rules are appended to existing ones.
/// Returns 0 on success, <0 on parse error.
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_config_load_config(
    cfg: *mut AgentsightConfigHandle,
    json_str: *const c_char,
) -> c_int {
    if cfg.is_null() || json_str.is_null() {
        return -1;
    }
    let c = unsafe { &mut *cfg };
    let json = unsafe { CStr::from_ptr(json_str).to_string_lossy() };

    match c.load_from_json(&json) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e);
            -1
        }
    }
}

/// Free the configuration handle.
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_config_free(cfg: *mut AgentsightConfigHandle) {
    if !cfg.is_null() {
        unsafe { drop(Box::from_raw(cfg)) };
    }
}

// ---- Lifecycle ----

/// Create a new AgentSight handle.  Does NOT start the pipeline yet.
/// Returns NULL on failure (call `agentsight_last_error()` for details).
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_new(cfg: *mut AgentsightConfigHandle) -> *mut AgentsightHandle {
    // Create eventfd
    let efd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if efd < 0 {
        set_last_error("Failed to create eventfd");
        return ptr::null_mut();
    }

    let config = if cfg.is_null() {
        AgentsightConfig::default()
    } else {
        unsafe { (*cfg).clone() }
    };

    let (tx, rx) = mpsc::channel();
    let running = Arc::new(AtomicBool::new(false));

    Box::into_raw(Box::new(AgentsightHandle {
        rx,
        tx: Some(tx),
        eventfd: efd,
        running,
        thread: None,
        config: Some(config),
        probe_cmd_tx: None,
    }))
}

/// Start the background pipeline thread.  Returns 0 on success, <0 on error.
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_start(h: *mut AgentsightHandle) -> c_int {
    if h.is_null() {
        set_last_error("NULL handle");
        return -1;
    }
    let handle = unsafe { &mut *h };

    // Take config and sender (can only start once)
    let config = match handle.config.take() {
        Some(c) => c,
        None => {
            set_last_error("agentsight_start called more than once");
            return -1;
        }
    };
    let tx = match handle.tx.take() {
        Some(t) => t,
        None => {
            set_last_error("agentsight_start: sender already consumed");
            return -1;
        }
    };

    let running = handle.running.clone();
    running.store(true, Ordering::SeqCst);
    let eventfd = handle.eventfd;

    // Probe control channel: caller thread sends commands; background thread drains them.
    let (probe_cmd_tx, probe_cmd_rx) = mpsc::channel::<ProbeCommand>();
    handle.probe_cmd_tx = Some(probe_cmd_tx);

    handle.thread = Some(std::thread::spawn(move || {
        ffi_background_thread(config, tx, eventfd, running, probe_cmd_rx);
    }));

    0
}

/// Background thread: creates AgentSight and runs the event loop.
///
/// This function creates AgentSight *inside* the thread to avoid `Send`
/// constraints on eBPF objects.
fn ffi_background_thread(
    config: AgentsightConfig,
    tx: mpsc::Sender<FfiEvent>,
    eventfd: i32,
    running: Arc<AtomicBool>,
    probe_cmd_rx: mpsc::Receiver<ProbeCommand>,
) {
    let sender = FfiEventSender { tx, eventfd };

    let mut sight = match AgentSight::new(config) {
        Ok(s) => s,
        Err(e) => {
            log::error!("agentsight background thread: AgentSight::new failed: {e}");
            return;
        }
    };

    // Install FFI event sender on the AgentSight instance.
    sight.set_ffi_sender(sender);

    // Event loop controlled by the external running flag.
    while running.load(Ordering::SeqCst) {
        // Drain probe commands (non-blocking)
        while let Ok(cmd) = probe_cmd_rx.try_recv() {
            match cmd {
                ProbeCommand::AddCgroup(id) => {
                    if let Err(e) = sight.add_traced_cgroup(id) {
                        log::warn!("add_traced_cgroup({id}) failed: {e}");
                    } else {
                        log::info!("Added cgroup_id {id} to BPF filter");
                    }
                }
                ProbeCommand::RemoveCgroup(id) => {
                    if let Err(e) = sight.remove_traced_cgroup(id) {
                        log::warn!("remove_traced_cgroup({id}) failed: {e}");
                    } else {
                        log::info!("Removed cgroup_id {id} from BPF filter");
                    }
                }
            }
        }
        if sight.try_process().is_none() {
            // No event available — flush any timed-out pending GenAI events
            sight.flush_expired_pending_genai();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // Shutdown: flush pending events.
    sight.shutdown();
}

/// Stop the background pipeline thread.  Returns 0 on success.
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_stop(h: *mut AgentsightHandle) -> c_int {
    if h.is_null() {
        set_last_error("NULL handle");
        return -1;
    }
    let handle = unsafe { &mut *h };
    handle.running.store(false, Ordering::SeqCst);

    // Wait for background thread to finish.
    if let Some(th) = handle.thread.take() {
        let _ = th.join();
    }
    0
}

/// Free the handle.  Must be called after `agentsight_stop()`.
/// The eventfd is closed automatically via the `Drop` impl.
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_free(h: *mut AgentsightHandle) {
    if !h.is_null() {
        unsafe { drop(Box::from_raw(h)) };
    }
}

impl Drop for AgentsightHandle {
    fn drop(&mut self) {
        // Close the eventfd managed by this handle.
        if self.eventfd >= 0 {
            unsafe { libc::close(self.eventfd) };
            self.eventfd = -1;
        }
        // Join the background thread if still running.
        if let Some(th) = self.thread.take() {
            self.running.store(false, Ordering::SeqCst);
            let _ = th.join();
        }
    }
}

/// Return a static version string (e.g. "0.2.2").
#[unsafe(no_mangle)]
pub extern "C" fn agentsight_version() -> *const c_char {
    static VERSION_CSTR: std::sync::OnceLock<CString> = std::sync::OnceLock::new();
    VERSION_CSTR
        .get_or_init(|| CString::new(env!("CARGO_PKG_VERSION")).unwrap())
        .as_ptr()
}

// ---- Event notification ----

/// Return the eventfd descriptor.  The caller may register it with
/// epoll/select.  Returns <0 if eventfd is not supported.
/// The fd is managed internally — the caller must NOT close it.
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_get_eventfd(h: *mut AgentsightHandle) -> c_int {
    if h.is_null() {
        return -1;
    }
    unsafe { (*h).eventfd }
}

// ---- Data reading ----

/// Process available events via callbacks.
///
/// * `http_cb` / `llm_cb`: callbacks for HTTP / LLM events (NULL = ignore).
/// * `flags`: 0 = non-blocking, `AGENTSIGHT_READ_BLOCK` = block until ≥1 event.
///
/// Returns the number of events processed, 0 if none, <0 on error.
///
/// # Safety
///
/// `cfg` / `h` must be a valid pointer returned by the corresponding
/// `_new()` function, or null (which is handled gracefully).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_read(
    h: *mut AgentsightHandle,
    http_cb: HttpsCallbackFn,
    http_ud: *mut c_void,
    llm_cb: LlmCallbackFn,
    llm_ud: *mut c_void,
    flags: c_int,
) -> c_int {
    if h.is_null() {
        return -1;
    }
    let handle = unsafe { &*h };
    let mut count: c_int = 0;

    // Blocking mode: wait for at least one event.
    if flags & AGENTSIGHT_READ_BLOCK != 0 {
        match handle.rx.recv() {
            Ok(event) => {
                unsafe { dispatch_event(event, http_cb, http_ud, llm_cb, llm_ud) };
                count += 1;
            }
            Err(_) => return -1,
        }
    }

    // Non-blocking drain of remaining (or all) events.
    while let Ok(event) = handle.rx.try_recv() {
        unsafe { dispatch_event(event, http_cb, http_ud, llm_cb, llm_ud) };
        count += 1;
    }

    // Drain the eventfd counter to prevent stale wakeups.
    drain_eventfd(handle.eventfd);

    count
}

// ---- Dynamic cgroup filter control ----

/// Add a cgroup inode ID to the BPF cgroup_filter map at runtime.
/// Returns 0 on success, -1 on failure (check agentsight_last_error()).
/// Requires agentsight_start() to have been called first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_add_traced_cgroup(
    h: *mut AgentsightHandle,
    cgroup_id: u64,
) -> c_int {
    if h.is_null() {
        set_last_error("null handle");
        return -1;
    }
    let handle = unsafe { &*h };
    match &handle.probe_cmd_tx {
        Some(tx) => {
            if tx.send(ProbeCommand::AddCgroup(cgroup_id)).is_err() {
                set_last_error("probe command channel closed");
                -1
            } else {
                0
            }
        }
        None => {
            set_last_error("handle not started or probe_cmd_tx not initialized");
            -1
        }
    }
}

/// Remove a cgroup inode ID from the BPF cgroup_filter map at runtime.
/// Returns 0 on success, -1 on failure (check agentsight_last_error()).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agentsight_remove_traced_cgroup(
    h: *mut AgentsightHandle,
    cgroup_id: u64,
) -> c_int {
    if h.is_null() {
        set_last_error("null handle");
        return -1;
    }
    let handle = unsafe { &*h };
    match &handle.probe_cmd_tx {
        Some(tx) => {
            if tx.send(ProbeCommand::RemoveCgroup(cgroup_id)).is_err() {
                set_last_error("probe command channel closed");
                -1
            } else {
                0
            }
        }
        None => {
            set_last_error("handle not started or probe_cmd_tx not initialized");
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_cfg() -> AgentsightConfig {
        let mut cfg = AgentsightConfig::default();
        cfg.cmdline_rules.clear();
        cfg.https_rules.clear();
        cfg
    }

    #[test]
    fn test_load_json_basic() {
        let mut cfg = new_cfg();
        let json = r#"{"verbose":1,"log_path":"/tmp/test.log"}"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert!(cfg.verbose);
        assert_eq!(cfg.log_path, Some("/tmp/test.log".to_string()));
    }

    #[test]
    fn test_load_json_cmdline_allow_and_deny() {
        let mut cfg = new_cfg();
        let json = r#"{
            "cmdline": {
                "allow": [
                    {"rule": ["node", "*claude*"], "agent_name": "Claude Code"}
                ],
                "deny": [
                    {"rule": ["node", "*webpack*"]}
                ]
            }
        }"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert_eq!(cfg.cmdline_rules.len(), 2);
        assert!(cfg.cmdline_rules[0].allow);
        assert_eq!(
            cfg.cmdline_rules[0].agent_name,
            Some("Claude Code".to_string())
        );
        assert!(!cfg.cmdline_rules[1].allow);
        assert!(cfg.cmdline_rules[1].agent_name.is_none());
    }

    #[test]
    fn test_load_json_https_rules() {
        let mut cfg = new_cfg();
        let json = r#"{
            "https": [
                {"rule": ["*.openai.com", "*.anthropic.com"]}
            ]
        }"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert_eq!(cfg.https_rules.len(), 2);
        assert_eq!(cfg.https_rules[0].pattern, "*.openai.com");
        assert_eq!(cfg.https_rules[1].pattern, "*.anthropic.com");
    }

    #[test]
    fn test_load_json_invalid() {
        let mut cfg = new_cfg();
        let json = r#"{ invalid json }"#;
        assert!(cfg.load_from_json(json).is_err());
    }

    #[test]
    fn test_load_json_appends_to_existing() {
        let mut cfg = new_cfg();
        // First load
        let json1 = r#"{"cmdline":{"allow":[{"rule":["node"],"agent_name":"Agent1"}]}}"#;
        assert!(cfg.load_from_json(json1).is_ok());
        assert_eq!(cfg.cmdline_rules.len(), 1);

        // Second load should append
        let json2 = r#"{"cmdline":{"allow":[{"rule":["python3"],"agent_name":"Agent2"}]}}"#;
        assert!(cfg.load_from_json(json2).is_ok());
        assert_eq!(cfg.cmdline_rules.len(), 2);
        assert_eq!(cfg.cmdline_rules[1].agent_name, Some("Agent2".to_string()));
    }

    #[test]
    fn test_load_json_empty_rule_skipped() {
        let mut cfg = new_cfg();
        let json = r#"{
            "cmdline": {
                "allow": [
                    {"rule": [], "agent_name": "Skipped"},
                    {"rule": ["node"], "agent_name": "Kept"}
                ]
            }
        }"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert_eq!(cfg.cmdline_rules.len(), 1);
        assert_eq!(cfg.cmdline_rules[0].agent_name, Some("Kept".to_string()));
    }

    #[test]
    fn test_safe_cstring_replaces_nul() {
        let s = "hel\0lo";
        let c = safe_cstring(s);
        assert_eq!(c.to_str().unwrap(), "hello");
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn test_copy_process_name_truncate() {
        let name = "very_long_process_name_that_exceeds_16";
        let buf = copy_process_name(name);
        assert_eq!(buf[15], 0); // NUL-terminated
        // First 15 chars should match
        for i in 0..15 {
            assert_eq!(buf[i] as u8, name.as_bytes()[i]);
        }
    }

    #[test]
    fn test_load_json_cmdline_allow() {
        let mut cfg = new_cfg();
        let json = r#"{
            "cmdline": {
                "allow": [
                    {"rule": ["*python*", "*hermes*"], "agent_name": "Hermes"},
                    {"rule": ["node*", "*copilot-shell*"], "agent_name": "Cosh"}
                ]
            }
        }"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert_eq!(cfg.cmdline_rules.len(), 2);
        assert_eq!(cfg.cmdline_rules[0].agent_name, Some("Hermes".to_string()));
        assert!(cfg.cmdline_rules[0].allow);
        assert_eq!(cfg.cmdline_rules[1].agent_name, Some("Cosh".to_string()));
    }

    #[test]
    fn test_load_json_cmdline_with_deny() {
        let mut cfg = new_cfg();
        let json = r#"{
            "cmdline": {
                "allow": [{"rule": ["node", "*claude*"], "agent_name": "Claude Code"}],
                "deny": [{"rule": ["node", "*webpack*"]}]
            }
        }"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert_eq!(cfg.cmdline_rules.len(), 2);
        assert!(cfg.cmdline_rules[0].allow);
        assert!(!cfg.cmdline_rules[1].allow);
    }

    #[test]
    fn test_load_json_all_fields() {
        let mut cfg = new_cfg();
        let json = r#"{
            "verbose": 1,
            "cmdline": {
                "allow": [{"rule": ["node", "*claude*"], "agent_name": "Claude Code"}]
            },
            "https": [{"rule": ["*.openai.com"]}]
        }"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert!(cfg.verbose);
        assert_eq!(cfg.cmdline_rules.len(), 1);
        assert_eq!(cfg.https_rules.len(), 1);
    }

    #[test]
    fn test_load_json_http_endpoint() {
        let mut cfg = new_cfg();
        let json = r#"{
            "http": [
                {"rule": [":8080", "10.0.0.1:9090"]}
            ]
        }"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert_eq!(cfg.http_targets.len(), 2);
        match &cfg.http_targets[0] {
            crate::config::HttpTarget::Endpoint(t) => {
                assert_eq!(t.ip, None);
                assert_eq!(t.port, Some(8080));
            }
            _ => panic!("expected Endpoint"),
        }
        match &cfg.http_targets[1] {
            crate::config::HttpTarget::Endpoint(t) => {
                assert_eq!(t.ip, Some(std::net::Ipv4Addr::new(10, 0, 0, 1)));
                assert_eq!(t.port, Some(9090));
            }
            _ => panic!("expected Endpoint"),
        }
    }

    #[test]
    fn test_load_json_http_domain() {
        let mut cfg = new_cfg();
        let json = r#"{
            "http": [
                {"rule": ["model-svc.default.svc", "*.internal.com"]}
            ]
        }"#;
        assert!(cfg.load_from_json(json).is_ok());
        assert_eq!(cfg.http_targets.len(), 2);
        match &cfg.http_targets[0] {
            crate::config::HttpTarget::Domain(d) => assert_eq!(d, "model-svc.default.svc"),
            _ => panic!("expected Domain"),
        }
        match &cfg.http_targets[1] {
            crate::config::HttpTarget::Domain(d) => assert_eq!(d, "*.internal.com"),
            _ => panic!("expected Domain"),
        }
    }
}
