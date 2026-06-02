//! Phase-0 LLM cache-hit SHADOW analyzer — observe-only.
//!
//! It measures, on real observed traffic, how much an exact-match LLM response
//! cache WOULD have saved, and how trustworthy the cache key is. It never serves
//! and never changes agent behaviour.
//!
//! For each completed [`LLMCall`] it:
//!   1. decides whether the call is *cacheable* (deterministic: `temperature == 0`,
//!      or an explicit opt-in marker) — non-deterministic calls are excluded;
//!   2. builds a canonical content key from the request `raw_body`
//!      (provider + model + normalized messages + params), stripping the
//!      per-call `[timestamp]` prefix agents inject into user messages;
//!   3. looks the key up in a bounded table of previously-seen requests. A miss
//!      records a baseline (the answer a cache would have served); a hit is a
//!      would-be cache hit, and the newly observed answer is compared against the
//!      stored baseline to empirically measure the FALSE-HIT rate (the whole
//!      point of Phase 0 — a bad key shows up here on real traffic).
//!
//! Reported numbers are deliberately conservative: global counters live outside
//! the bounded table, so eviction can only make a future recurrence look like a
//! first-sight miss — the hit-rate is a LOWER BOUND, never an overstatement.

use super::builder::GenAIBuilder;
use super::exporter::GenAIExporter;
use super::semantic::{GenAISemanticEvent, LLMCall, MessagePart};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Schema version of the cache key — bump on any rule change so a stale store
/// never collides with a new reader.
const KEY_SCHEMA: u32 = 2;
/// Bound on distinct keys held in memory. Once full we stop recording new
/// baselines (the run's hit-rate then becomes a strict lower bound).
const MAX_KEYS: usize = 50_000;
/// How often the background reporter logs + persists a snapshot.
const REPORT_INTERVAL: Duration = Duration::from_secs(300);
/// Cacheable-fraction below which the deterministic-only policy is near-useless.
const POLICY_FLOOR: f64 = 0.05;

const REPORT_FILE: &str = "cache_shadow_report.json";

// ─── cache key ──────────────────────────────────────────────────────────────

/// Why a call is or isn't cacheable (also drives the exclusion counters).
enum Cacheability {
    /// Cacheable — carries the canonical content key.
    Key(String),
    /// `temperature` absent — provider defaults are non-deterministic.
    TempUnknown,
    /// `temperature` present and non-zero, no opt-in.
    NonDeterministic,
    /// `error` was set on the call.
    Errored,
    /// `request.raw_body` missing.
    NoBody,
    /// `raw_body` did not parse as JSON.
    Unparseable,
}

/// SysOM nests OpenAI-shaped params in a JSON-encoded string field.
fn unwrap_params(body: &Value) -> Value {
    if let Some(Value::String(inner)) = body.get("llmParamString") {
        if let Ok(p) = serde_json::from_str::<Value>(inner) {
            return p;
        }
    }
    body.clone()
}

fn cacheability(call: &LLMCall) -> Cacheability {
    if call.error.is_some() {
        return Cacheability::Errored;
    }
    let body = match call.request.raw_body.as_deref() {
        Some(b) => b,
        None => return Cacheability::NoBody,
    };
    let raw: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Cacheability::Unparseable,
    };
    let mut params = unwrap_params(&raw);

    let temp = params.get("temperature").and_then(Value::as_f64);
    let opt_in = params
        .get("agentsight_cache")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || call.metadata.get("cache_opt_in").map(|s| s == "true").unwrap_or(false);

    let deterministic = matches!(temp, Some(t) if t == 0.0);
    if !deterministic && !opt_in {
        return if temp.is_none() {
            Cacheability::TempUnknown
        } else {
            Cacheability::NonDeterministic
        };
    }

    normalize_params(&mut params);
    let key = sha256_32(&format!(
        "v{}|{}|{}|{}",
        KEY_SCHEMA,
        call.provider.trim(),
        call.model.trim(),
        canon(&params),
    ));
    Cacheability::Key(key)
}

/// Map a volatile tool-call id to a stable positional alias (`tc0`, `tc1`, …),
/// shared between an assistant tool_call and its matching tool result so the
/// call↔response linkage is preserved while server-random ids are erased.
fn alias_id(alias: &mut HashMap<String, String>, id: &mut String) {
    let next = alias.len();
    let a = alias
        .entry(id.clone())
        .or_insert_with(|| format!("tc{next}"))
        .clone();
    *id = a;
}

/// Strip request-volatile fields, normalize user-message text, and canonicalize
/// server-random tool-call ids, all in place.
fn normalize_params(v: &mut Value) {
    if let Value::Object(m) = v {
        for k in [
            "stream",
            "stream_options",
            "user",
            "metadata",
            "id",
            "request_id",
            "agentsight_cache",
        ] {
            m.remove(k);
        }
        if let Some(Value::Array(msgs)) = m.get_mut("messages") {
            // Per-request alias map — turns random call_*/toolu_* ids into stable
            // positional refs so identical multi-turn replays hash the same.
            let mut alias: HashMap<String, String> = HashMap::new();
            for msg in msgs.iter_mut() {
                if let Value::Object(mo) = msg {
                    let is_user = mo.get("role").and_then(Value::as_str) == Some("user");
                    if is_user {
                        if let Some(c) = mo.get_mut("content") {
                            strip_user_content(c);
                        }
                    }
                    // OpenAI: tool-role message carries a tool_call_id.
                    if let Some(Value::String(id)) = mo.get_mut("tool_call_id") {
                        alias_id(&mut alias, id);
                    }
                    // OpenAI: assistant message carries tool_calls[].id.
                    if let Some(Value::Array(tcs)) = mo.get_mut("tool_calls") {
                        for tc in tcs.iter_mut() {
                            if let Value::Object(tco) = tc {
                                if let Some(Value::String(id)) = tco.get_mut("id") {
                                    alias_id(&mut alias, id);
                                }
                            }
                        }
                    }
                    // Anthropic: content blocks carry tool_use.id / tool_result.tool_use_id.
                    if let Some(Value::Array(blocks)) = mo.get_mut("content") {
                        for b in blocks.iter_mut() {
                            if let Value::Object(bo) = b {
                                if let Some(Value::String(id)) = bo.get_mut("id") {
                                    alias_id(&mut alias, id);
                                }
                                if let Some(Value::String(id)) = bo.get_mut("tool_use_id") {
                                    alias_id(&mut alias, id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Apply `strip_user_query_prefix` to a user message's content (string form, or
/// the `text` fields of a content-parts array).
fn strip_user_content(c: &mut Value) {
    match c {
        Value::String(s) => *s = GenAIBuilder::strip_user_query_prefix(s),
        Value::Array(parts) => {
            for p in parts.iter_mut() {
                if let Value::Object(po) = p {
                    if let Some(Value::String(t)) = po.get_mut("text") {
                        *t = GenAIBuilder::strip_user_query_prefix(t);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Canonical, key-sorted, compact serialization of a JSON value so that
/// reordered object keys and equivalent number forms (`0`/`0.0`, `0.1`/`1e-1`)
/// collapse to one string. Array element order is preserved.
fn canon(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", quote(k), canon(&m[*k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(a) => {
            format!("[{}]", a.iter().map(canon).collect::<Vec<_>>().join(","))
        }
        Value::String(s) => quote(s),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else if let Some(f) = n.as_f64() {
                // shortest round-trip; collapses 0.0->0, 1.0->1, 1e-1->0.1
                format!("{f}")
            } else {
                n.to_string()
            }
        }
    }
}

fn quote(s: &str) -> String {
    // Reuse serde's string escaping for an unambiguous, stable encoding.
    Value::String(s.to_string()).to_string()
}

fn sha256_32(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(32);
    for b in &digest[..16] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Volatile per-call fields in a raw LLM response envelope (must not affect the
/// answer fingerprint).
const RESPONSE_ENVELOPE_VOLATILE: [&str; 5] =
    ["id", "created", "system_fingerprint", "request_id", "object"];

/// Canonical fingerprint of the response a cache would have served.
///
/// Built from the already-reduced `response.messages` as a kind-tagged, role- and
/// message-delimited canonical structure (so e.g. a Text part and a ToolCall part
/// can never concatenate into the same bytes). Falls back to the raw body with
/// the volatile envelope fields stripped, so two deterministic answers that
/// differ only in `id`/`created`/`system_fingerprint` are NOT scored as a false
/// hit.
fn answer_fingerprint(call: &LLMCall) -> String {
    if call.response.messages.is_empty() {
        return match call.response.raw_body.as_deref() {
            Some(b) => match serde_json::from_str::<Value>(b) {
                Ok(mut v) => {
                    if let Value::Object(m) = &mut v {
                        for k in RESPONSE_ENVELOPE_VOLATILE {
                            m.remove(k);
                        }
                    }
                    sha256_32(&canon(&v))
                }
                Err(_) => sha256_32(b), // unparseable: hash verbatim (rare)
            },
            None => sha256_32(""),
        };
    }

    let msgs: Vec<Value> = call
        .response
        .messages
        .iter()
        .map(|msg| {
            let parts: Vec<Value> = msg
                .parts
                .iter()
                .filter_map(|part| match part {
                    MessagePart::Text { content } => {
                        Some(serde_json::json!({"k": "text", "v": content}))
                    }
                    MessagePart::ToolCall { name, arguments, .. } => {
                        Some(serde_json::json!({"k": "call", "n": name, "a": arguments}))
                    }
                    MessagePart::ToolCallResponse { response, .. } => {
                        Some(serde_json::json!({"k": "resp", "r": response}))
                    }
                    // Reasoning is excluded from the primary answer fingerprint.
                    MessagePart::Reasoning { .. } => None,
                })
                .collect();
            serde_json::json!({"role": msg.role, "parts": parts})
        })
        .collect();
    sha256_32(&canon(&Value::Array(msgs)))
}

/// Tokens a would-be cache hit saves. Prefers the structured `token_usage`, but
/// the analyzer only populates that for streamed responses, so fall back to
/// parsing the `usage` object out of the response body (the data is present for
/// non-streaming completions too — only the upstream wiring is missing).
fn saved_tokens(call: &LLMCall) -> (u64, u64) {
    if let Some(u) = &call.token_usage {
        return (u.input_tokens as u64, u.output_tokens as u64);
    }
    if let Some(body) = call.response.raw_body.as_deref() {
        if let Some(u) = crate::analyzer::token::TokenParser.parse_data(body) {
            return (u.input_tokens, u.output_tokens);
        }
    }
    (0, 0)
}

// ─── analyzer state ───────────────────────────────────────────────────────────

#[derive(Default, Clone, Serialize)]
struct Counters {
    total_calls: u64,
    cacheable: u64,
    temp_unknown: u64,
    nondeterministic: u64,
    errored: u64,
    no_body: u64,
    unparseable: u64,
    unique_keys: u64,
    hits: u64,
    hits_exact: u64,
    output_tokens_saved: u64,
    input_tokens_saved: u64,
    duration_ns_saved: u64,
    keys_dropped: u64, // baselines not recorded because the table was full
}

struct Inner {
    /// cache key → fingerprint of the answer a cache would have served (fixed at
    /// first sight). Bounded by MAX_KEYS.
    seen: HashMap<String, String>,
    counters: Counters,
}

/// Observe-only cache-hit analyzer. Registered as a [`GenAIExporter`] in the CLI
/// trace path only (it is not wired into the FFI event path, where
/// `enable_cache_analysis` cannot currently be set).
pub struct CacheAnalyzer {
    inner: Mutex<Inner>,
    report_dir: PathBuf,
    finalized: AtomicBool,
}

impl CacheAnalyzer {
    pub fn new(report_dir: PathBuf) -> Self {
        Self {
            inner: Mutex::new(Inner {
                seen: HashMap::new(),
                counters: Counters::default(),
            }),
            report_dir,
            finalized: AtomicBool::new(false),
        }
    }

    /// Record one completed LLM call.
    fn observe(&self, call: &LLMCall) {
        let mut guard = self.inner.lock().unwrap();
        // Bind a single &mut Inner so `seen` and `counters` are disjoint field
        // borrows (field access through the MutexGuard's Deref is not disjoint).
        let inner = &mut *guard;
        inner.counters.total_calls += 1;

        let key = match cacheability(call) {
            Cacheability::Key(k) => k,
            Cacheability::TempUnknown => {
                inner.counters.temp_unknown += 1;
                return;
            }
            Cacheability::NonDeterministic => {
                inner.counters.nondeterministic += 1;
                return;
            }
            Cacheability::Errored => {
                inner.counters.errored += 1;
                return;
            }
            Cacheability::NoBody => {
                inner.counters.no_body += 1;
                return;
            }
            Cacheability::Unparseable => {
                inner.counters.unparseable += 1;
                return;
            }
        };

        inner.counters.cacheable += 1;
        let fp = answer_fingerprint(call);

        match inner.seen.get(&key) {
            Some(baseline_fp) => {
                let exact = *baseline_fp == fp;
                inner.counters.hits += 1;
                if exact {
                    inner.counters.hits_exact += 1;
                    // Tokens/latency this duplicate call cost — a cache would have
                    // avoided them. Counted only for byte-identical answers.
                    let (in_tok, out_tok) = saved_tokens(call);
                    inner.counters.input_tokens_saved += in_tok;
                    inner.counters.output_tokens_saved += out_tok;
                    inner.counters.duration_ns_saved += call.duration_ns;
                }
            }
            None => {
                if inner.seen.len() >= MAX_KEYS {
                    inner.counters.keys_dropped += 1;
                } else {
                    inner.seen.insert(key, fp);
                    inner.counters.unique_keys += 1;
                }
            }
        }
    }

    /// Build a point-in-time report from the current counters.
    pub fn snapshot(&self) -> ShadowReport {
        let c = self.inner.lock().unwrap().counters.clone();
        ShadowReport::from_counters(&c)
    }

    /// Write the final report once (idempotent — safe under run()+Drop double call).
    pub fn finalize(&self) {
        if self.finalized.swap(true, Ordering::SeqCst) {
            return;
        }
        let report = self.snapshot();
        log::info!("[cache-shadow] FINAL {}", report.one_line());
        report.persist(&self.report_dir);
    }
}

impl GenAIExporter for CacheAnalyzer {
    fn name(&self) -> &str {
        "cache-shadow"
    }

    fn export(&self, events: &[GenAISemanticEvent]) {
        for ev in events {
            if let GenAISemanticEvent::LLMCall(call) = ev {
                self.observe(call);
            }
        }
    }
}

/// Spawn the detached periodic reporter (mirrors the genai-stale-scanner thread).
pub fn spawn_reporter(analyzer: Arc<CacheAnalyzer>) {
    let spawned = std::thread::Builder::new()
        .name("genai-cache-shadow-reporter".to_string())
        .spawn(move || loop {
            std::thread::sleep(REPORT_INTERVAL);
            let report = analyzer.snapshot();
            log::info!("[cache-shadow] {}", report.one_line());
            report.persist(&analyzer.report_dir);
        });
    if let Err(e) = spawned {
        log::warn!(
            "[cache-shadow] failed to spawn reporter thread: {e}; \
             periodic snapshots disabled (finalize() still writes the final report)"
        );
    }
}

// ─── report ───────────────────────────────────────────────────────────────────

/// A trustworthy, fully-caveated snapshot of the shadow analysis.
#[derive(Debug, Clone, Serialize)]
pub struct ShadowReport {
    pub total_calls: u64,
    pub cacheable_calls: u64,
    /// cacheable / total
    pub cacheable_fraction: f64,
    pub excluded_temperature_unknown: u64,
    pub excluded_nondeterministic: u64,
    pub excluded_errored: u64,
    pub excluded_no_body: u64,
    pub excluded_unparseable: u64,
    pub unique_keys: u64,
    pub would_be_hits: u64,
    /// hits / total_calls — the real ceiling on infra savings (headline).
    pub hit_rate_all: f64,
    /// hits / cacheable_calls — conditional on cacheability.
    pub hit_rate_cacheable: f64,
    /// 1 - hits_exact/hits — fraction of would-be hits that would have served a
    /// byte-different answer (the key-precision self-check).
    pub false_hit_rate: f64,
    /// Savings counted ONLY for byte-identical (exact) would-be hits. WOULD-BE
    /// upper bound — a local cache is not zero-latency.
    pub output_tokens_saved: u64,
    pub input_tokens_saved: u64,
    pub duration_ms_saved: u64,
    pub keys_dropped: u64,
    pub caveats: Vec<String>,
}

impl ShadowReport {
    fn from_counters(c: &Counters) -> Self {
        let rate = |num: u64, den: u64| if den == 0 { 0.0 } else { num as f64 / den as f64 };
        let cacheable_fraction = rate(c.cacheable, c.total_calls);

        let mut caveats = vec![
            "temperature=0 does NOT guarantee identical model outputs (float \
             non-associativity, MoE routing, server batching, model drift); a \
             non-zero false_hit_rate is expected even with a correct key"
                .to_string(),
            "normalization is partial: only user-message [timestamp] prefixes and \
             tool-call ids are canonicalized. Volatile content injected into the \
             system prompt (current date, cwd, git branch) is hashed verbatim, so \
             an identical task seen across days/sessions can register as a \
             first-sight miss — hit_rate is a LOWER bound for this reason too"
                .to_string(),
        ];
        if c.total_calls > 0 && cacheable_fraction < POLICY_FLOOR {
            caveats.push(format!(
                "POLICY_NEAR_USELESS: only {:.1}% of calls are cacheable under the \
                 deterministic-only policy",
                cacheable_fraction * 100.0
            ));
        }
        if c.keys_dropped > 0 {
            caveats.push(format!(
                "TRUNCATED: {} baselines dropped (table cap {}); hit-rate is a LOWER bound",
                c.keys_dropped, MAX_KEYS
            ));
        }

        ShadowReport {
            total_calls: c.total_calls,
            cacheable_calls: c.cacheable,
            cacheable_fraction,
            excluded_temperature_unknown: c.temp_unknown,
            excluded_nondeterministic: c.nondeterministic,
            excluded_errored: c.errored,
            excluded_no_body: c.no_body,
            excluded_unparseable: c.unparseable,
            unique_keys: c.unique_keys,
            would_be_hits: c.hits,
            hit_rate_all: rate(c.hits, c.total_calls),
            hit_rate_cacheable: rate(c.hits, c.cacheable),
            false_hit_rate: if c.hits == 0 {
                0.0
            } else {
                1.0 - rate(c.hits_exact, c.hits)
            },
            output_tokens_saved: c.output_tokens_saved,
            input_tokens_saved: c.input_tokens_saved,
            duration_ms_saved: c.duration_ns_saved / 1_000_000,
            keys_dropped: c.keys_dropped,
            caveats,
        }
    }

    /// One-line log summary — always carries the key caveats so log-only readers
    /// cannot miss them.
    pub fn one_line(&self) -> String {
        format!(
            "calls={} cacheable={} ({:.1}%) would_be_hits={} hit_rate_all={:.1}% \
             hit_rate_cacheable={:.1}% false_hit_rate={:.1}% tokens_saved(in/out)={}/{} \
             saved_ms={}{}{}",
            self.total_calls,
            self.cacheable_calls,
            self.cacheable_fraction * 100.0,
            self.would_be_hits,
            self.hit_rate_all * 100.0,
            self.hit_rate_cacheable * 100.0,
            self.false_hit_rate * 100.0,
            self.input_tokens_saved,
            self.output_tokens_saved,
            self.duration_ms_saved,
            if self.keys_dropped > 0 { " [TRUNCATED]" } else { "" },
            if self.total_calls > 0 && self.cacheable_fraction < POLICY_FLOOR {
                " [POLICY_NEAR_USELESS]"
            } else {
                ""
            },
        )
    }

    /// Atomically write the full JSON report (temp file + rename).
    pub fn persist(&self, dir: &std::path::Path) {
        let json = match serde_json::to_string_pretty(self) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("[cache-shadow] serialize report failed: {e}");
                return;
            }
        };
        let final_path = dir.join(REPORT_FILE);
        // Per-writer-unique temp name so the reporter thread and finalize() never
        // share a temp file (which would tear the write / break temp+rename).
        static PERSIST_SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = PERSIST_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = dir.join(format!("{REPORT_FILE}.{}.tmp", seq));
        if let Some(parent) = final_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&tmp_path, json).and_then(|_| std::fs::rename(&tmp_path, &final_path)) {
            log::warn!("[cache-shadow] persist report failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai::semantic::{LLMRequest, LLMResponse, OutputMessage, TokenUsage};

    fn call_with(raw_body: &str, answer: &str) -> LLMCall {
        LLMCall {
            call_id: "c".into(),
            start_timestamp_ns: 0,
            end_timestamp_ns: 0,
            duration_ns: 1_000_000,
            provider: "openai".into(),
            model: "gpt-4".into(),
            request: LLMRequest {
                messages: vec![],
                temperature: None,
                max_tokens: None,
                frequency_penalty: None,
                presence_penalty: None,
                top_p: None,
                top_k: None,
                seed: None,
                stop_sequences: None,
                stream: false,
                tools: None,
                raw_body: Some(raw_body.to_string()),
            },
            response: LLMResponse {
                messages: vec![OutputMessage {
                    role: "assistant".into(),
                    parts: vec![MessagePart::Text { content: answer.to_string() }],
                    name: None,
                    finish_reason: Some("stop".into()),
                }],
                streamed: false,
                raw_body: None,
            },
            token_usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            error: None,
            pid: 1,
            process_name: "p".into(),
            agent_name: None,
            metadata: HashMap::new(),
        }
    }

    fn key_of(body: &str) -> Option<String> {
        match cacheability(&call_with(body, "x")) {
            Cacheability::Key(k) => Some(k),
            _ => None,
        }
    }

    #[test]
    fn gate_temp_zero_cacheable() {
        assert!(key_of(r#"{"model":"gpt-4","temperature":0,"messages":[]}"#).is_some());
    }

    #[test]
    fn gate_temp_missing_excluded() {
        assert!(key_of(r#"{"model":"gpt-4","messages":[]}"#).is_none());
        assert!(matches!(
            cacheability(&call_with(r#"{"messages":[]}"#, "x")),
            Cacheability::TempUnknown
        ));
    }

    #[test]
    fn gate_temp_nonzero_excluded() {
        assert!(matches!(
            cacheability(&call_with(r#"{"temperature":0.7,"messages":[]}"#, "x")),
            Cacheability::NonDeterministic
        ));
    }

    #[test]
    fn gate_opt_in_overrides_temperature() {
        assert!(key_of(r#"{"temperature":1.0,"agentsight_cache":true,"messages":[]}"#).is_some());
    }

    #[test]
    fn gate_no_body() {
        let mut c = call_with("{}", "x");
        c.request.raw_body = None;
        assert!(matches!(cacheability(&c), Cacheability::NoBody));
    }

    #[test]
    fn gate_unparseable() {
        assert!(matches!(
            cacheability(&call_with("not json", "x")),
            Cacheability::Unparseable
        ));
    }

    #[test]
    fn key_stable_and_32_hex() {
        let b = r#"{"temperature":0,"messages":[{"role":"user","content":"hi"}]}"#;
        let k = key_of(b).unwrap();
        assert_eq!(k.len(), 32);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(key_of(b).unwrap(), k); // deterministic
    }

    #[test]
    fn key_ignores_reordered_keys_and_number_form() {
        let a = key_of(r#"{"temperature":0,"top_p":1,"messages":[]}"#).unwrap();
        let b = key_of(r#"{"messages":[],"top_p":1.0,"temperature":0.0}"#).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn key_ignores_stream_and_user() {
        let a = key_of(r#"{"temperature":0,"messages":[],"stream":false,"user":"u1"}"#).unwrap();
        let b = key_of(r#"{"temperature":0,"messages":[],"stream":true,"user":"u2"}"#).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn key_collapses_user_timestamp_prefix() {
        let a = key_of(
            r#"{"temperature":0,"messages":[{"role":"user","content":"[Tue 2026-03-31 17:19 GMT+8] do X"}]}"#,
        )
        .unwrap();
        let b = key_of(
            r#"{"temperature":0,"messages":[{"role":"user","content":"[Wed 2026-04-01 09:00 GMT+8] do X"}]}"#,
        )
        .unwrap();
        assert_eq!(a, b, "injected [timestamp] prefix must not change the key");
    }

    #[test]
    fn key_differs_on_real_content() {
        let a = key_of(r#"{"temperature":0,"messages":[{"role":"user","content":"do X"}]}"#).unwrap();
        let b = key_of(r#"{"temperature":0,"messages":[{"role":"user","content":"do Y"}]}"#).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn key_unwraps_sysom_llmparamstring() {
        // SysOM nests the real params in a JSON string; temperature lives inside.
        let body = r#"{"llmParamString":"{\"temperature\":0,\"messages\":[]}"}"#;
        assert!(key_of(body).is_some());
    }

    #[test]
    fn analyzer_miss_then_exact_hit() {
        let a = CacheAnalyzer::new(std::env::temp_dir());
        let body = r#"{"temperature":0,"messages":[{"role":"user","content":"hi"}]}"#;
        a.observe(&call_with(body, "answer"));
        a.observe(&call_with(body, "answer"));
        let r = a.snapshot();
        assert_eq!(r.total_calls, 2);
        assert_eq!(r.cacheable_calls, 2);
        assert_eq!(r.unique_keys, 1);
        assert_eq!(r.would_be_hits, 1);
        assert_eq!(r.false_hit_rate, 0.0);
        assert_eq!(r.output_tokens_saved, 20);
        assert_eq!(r.input_tokens_saved, 10);
    }

    #[test]
    fn analyzer_detects_false_hit_on_divergent_answer() {
        let a = CacheAnalyzer::new(std::env::temp_dir());
        let body = r#"{"temperature":0,"messages":[{"role":"user","content":"hi"}]}"#;
        a.observe(&call_with(body, "answer-1"));
        a.observe(&call_with(body, "answer-2")); // same key, different answer
        let r = a.snapshot();
        assert_eq!(r.would_be_hits, 1);
        assert!(r.false_hit_rate > 0.0, "divergent answer must register a false hit");
        // No savings credited for a divergent would-be hit.
        assert_eq!(r.output_tokens_saved, 0);
    }

    #[test]
    fn analyzer_excludes_nondeterministic() {
        let a = CacheAnalyzer::new(std::env::temp_dir());
        a.observe(&call_with(r#"{"temperature":0.7,"messages":[]}"#, "x"));
        let r = a.snapshot();
        assert_eq!(r.cacheable_calls, 0);
        assert_eq!(r.excluded_nondeterministic, 1);
        assert_eq!(r.would_be_hits, 0);
    }

    #[test]
    fn report_always_carries_determinism_caveat() {
        let a = CacheAnalyzer::new(std::env::temp_dir());
        a.observe(&call_with(r#"{"temperature":0,"messages":[]}"#, "x"));
        let r = a.snapshot();
        assert!(r.caveats.iter().any(|c| c.contains("temperature=0 does NOT guarantee")));
    }

    #[test]
    fn key_collapses_tool_call_ids() {
        // Same multi-turn tool-use exchange, only the server-random ids differ.
        let a = key_of(
            r#"{"temperature":0,"messages":[{"role":"assistant","tool_calls":[{"id":"call_AAA","type":"function","function":{"name":"f","arguments":"{}"}}]},{"role":"tool","tool_call_id":"call_AAA","content":"ok"}]}"#,
        )
        .unwrap();
        let b = key_of(
            r#"{"temperature":0,"messages":[{"role":"assistant","tool_calls":[{"id":"call_BBB","type":"function","function":{"name":"f","arguments":"{}"}}]},{"role":"tool","tool_call_id":"call_BBB","content":"ok"}]}"#,
        )
        .unwrap();
        assert_eq!(a, b, "server-random tool-call ids must not change the key");
    }

    #[test]
    fn key_differs_on_tool_name() {
        let a = key_of(
            r#"{"temperature":0,"messages":[{"role":"assistant","tool_calls":[{"id":"call_A","function":{"name":"f","arguments":"{}"}}]}]}"#,
        )
        .unwrap();
        let b = key_of(
            r#"{"temperature":0,"messages":[{"role":"assistant","tool_calls":[{"id":"call_A","function":{"name":"g","arguments":"{}"}}]}]}"#,
        )
        .unwrap();
        assert_ne!(a, b, "different tool name must change the key");
    }

    #[test]
    fn fingerprint_distinguishes_text_from_toolcall() {
        // Old bug: concatenating parts with no delimiter let a single Text part
        // "callget_weather" collide with [Text "call", ToolCall "get_weather"].
        let a = CacheAnalyzer::new(std::env::temp_dir());
        let body = r#"{"temperature":0,"messages":[{"role":"user","content":"hi"}]}"#;
        a.observe(&call_with(body, "callget_weather"));
        let mut c2 = call_with(body, "x");
        c2.response.messages = vec![OutputMessage {
            role: "assistant".into(),
            parts: vec![
                MessagePart::Text { content: "call".into() },
                MessagePart::ToolCall { id: None, name: "get_weather".into(), arguments: None },
            ],
            name: None,
            finish_reason: None,
        }];
        a.observe(&c2);
        let r = a.snapshot();
        assert_eq!(r.would_be_hits, 1);
        assert!(
            r.false_hit_rate > 0.0,
            "a text answer and a tool call must not share a fingerprint"
        );
    }

    #[test]
    fn fingerprint_ignores_response_envelope() {
        // Empty reduced messages -> raw_body fallback; volatile id/created must
        // NOT make two deterministic answers look divergent.
        let a = CacheAnalyzer::new(std::env::temp_dir());
        let body = r#"{"temperature":0,"messages":[{"role":"user","content":"hi"}]}"#;
        let mut c1 = call_with(body, "x");
        c1.response.messages = vec![];
        c1.response.raw_body =
            Some(r#"{"id":"chatcmpl-A","created":111,"choices":[{"message":{"content":"hi"}}]}"#.into());
        a.observe(&c1);
        let mut c2 = call_with(body, "x");
        c2.response.messages = vec![];
        c2.response.raw_body =
            Some(r#"{"id":"chatcmpl-B","created":222,"choices":[{"message":{"content":"hi"}}]}"#.into());
        a.observe(&c2);
        let r = a.snapshot();
        assert_eq!(r.would_be_hits, 1);
        assert_eq!(
            r.false_hit_rate, 0.0,
            "volatile id/created in the envelope must not count as a false hit"
        );
    }

    #[test]
    fn saved_tokens_falls_back_to_response_usage() {
        // The analyzer leaves token_usage None for non-streaming responses; we
        // must recover the savings figure from the response body's usage object.
        let body = r#"{"temperature":0,"messages":[{"role":"user","content":"hi"}]}"#;
        let mut c = call_with(body, "answer");
        c.token_usage = None;
        c.response.raw_body = Some(
            r#"{"choices":[{"message":{"content":"answer"}}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}"#
                .into(),
        );
        let (input, output) = saved_tokens(&c);
        assert_eq!((input, output), (7, 3));
    }
}
