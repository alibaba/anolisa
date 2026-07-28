//! Cost analyzer — pure computation + LLM waste identification.
//!
//! Pure computation: walks ATIF steps and computes content size breakdown,
//! redundant call detection, per-step token replay model, and cost findings.
//! Also extracts structured waste candidates for the LLM to judge.
//!
//! LLM layer (`llm` module): sends candidates to the LLM for waste judgment,
//! then joins verdicts back to produce the final WasteReport.

pub mod llm;
mod prompts;

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::atif::{observation_looks_like_error, AtifStep, AtifTrajectory};
use crate::types::{
    CostFinding, CostHeadroom, CostRatioMetrics, CostSegment, CostStats, LlmCall,
    RedundantCallGroup, WasteCandidate, WasteCandidateSet,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum call count to flag a group as redundant.
const REDUNDANT_MIN_COUNT: usize = 3;

/// Maximum command signature characters for grouping.
const CMD_SIG_CHARS: usize = 80;

/// char→token ratio version tag, for auditability.
const TOKEN_RATIO_VERSION: &str = "v1-cjk1.5-latin4";

/// Prefix-caching threshold: static region must exceed this to be worth caching.
const PREFIX_CACHE_MIN: usize = 2000;
/// History-prune threshold: accumulated assistant history in tokens.
const HISTORY_PRUNE_MIN: usize = 2000;
/// History-prune threshold: minimum step index (0-based) before pruning applies.
const HISTORY_PRUNE_MIN_STEP: usize = 5;
/// Fraction of accumulated assistant history considered prunable/summarizable.
const HISTORY_PRUNE_FRAC: f64 = 0.6;
/// Tool-trim threshold: a single tool output above this (tokens) is oversized.
const TOOL_TRIM_MIN: usize = 2000;
/// Fraction of an oversized tool output considered truncatable.
const TOOL_TRIM_FRAC: f64 = 0.7;
/// A single user message above this (tokens) is a Prompt-Compression candidate.
const USER_LARGE_MIN: usize = 1500;
/// How many top offenders to surface as evidence per candidate.
const EVIDENCE_TOP_N: usize = 3;
/// Max characters for an evidence snippet (UTF-8 safe).
const SNIPPET_CHARS: usize = 180;
/// Fraction of an oversized user prompt considered compressible.
const PROMPT_COMPRESS_FRAC: f64 = 0.6;

// ── Playbook v1.1 admission gates (ratio-based, see
// docs/design/cost/optimization-playbook.md). Candidate admission uses only
// relative shares — absolute token thresholds above are kept solely for the
// per-step flame-chart fields (cacheable/trimmable/…). ──

/// 统一噪声线: cited in prompts as a reference line and enforced only in
/// cache-discount arbitration (agent-first: no Rust-side candidate filtering).
pub(crate) const NOISE_LINE: f64 = 0.03;
/// Cached tokens bill at roughly this fraction of the full input price.
pub(crate) const CACHED_PRICE_RATIO: f64 = 0.25;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute cost statistics from an ATIF trajectory (pure computation, no LLM).
pub fn compute_cost(trajectory: &AtifTrajectory) -> Result<CostStats> {
    if trajectory.steps.is_empty() {
        return Ok(CostStats {
            total_events: 0,
            total_chars: 0,
            breakdown: vec![],
            redundant_calls: vec![],
            findings: vec![],
            calls: vec![],
            model: String::new(),
            token_ratio_version: String::new(),
            usage_steps: 0,
            total_real_input_tok: 0,
            total_real_output_tok: 0,
            total_real_cached_tok: 0,
            headroom: CostHeadroom::default(),
        });
    }

    let total_events = trajectory.steps.len();

    // Accumulators for content categories.
    let mut tool_result_chars: usize = 0;
    let mut tool_input_chars: usize = 0;
    let mut thinking_chars: usize = 0;
    let mut text_chars: usize = 0;
    let mut user_input_chars: usize = 0;
    let mut system_chars: usize = 0;
    let mut total_chars: usize = 0;

    // Track tool calls for redundancy detection.
    // Key: (tool_name, cmd_signature)
    let mut tool_sig_counts: HashMap<(String, String), usize> = HashMap::new();

    for step in &trajectory.steps {
        match step.source.as_str() {
            "agent" => {
                if let Some(r) = step.reasoning_content.as_deref() {
                    let chars = r.chars().count();
                    thinking_chars += chars;
                    total_chars += chars;
                }
                if let Some(m) = step.message.as_deref() {
                    let chars = m.chars().count();
                    text_chars += chars;
                    total_chars += chars;
                }
                for call in step.calls() {
                    let chars = call.arguments.to_string().chars().count();
                    tool_input_chars += chars;
                    total_chars += chars;
                    let sig = call.command_summary(CMD_SIG_CHARS);
                    *tool_sig_counts
                        .entry((call.function_name.clone(), sig))
                        .or_insert(0) += 1;
                }
                for result in step.results() {
                    let chars = result.content.as_deref().unwrap_or("").chars().count();
                    tool_result_chars += chars;
                    total_chars += chars;
                }
            }
            "user" => {
                let chars = step.message.as_deref().unwrap_or("").chars().count();
                user_input_chars += chars;
                total_chars += chars;
            }
            "system" => {
                let chars = step.message.as_deref().unwrap_or("").chars().count();
                system_chars += chars;
                total_chars += chars;
            }
            _ => {}
        }
    }

    // Build breakdown sorted by chars descending.
    let mut segments = vec![
        ("工具返回", tool_result_chars),
        ("工具入参", tool_input_chars),
        ("思考", thinking_chars),
        ("回复正文", text_chars),
        ("用户输入", user_input_chars),
        ("系统/元信息", system_chars),
    ];
    segments.sort_by_key(|seg| std::cmp::Reverse(seg.1));

    let breakdown: Vec<CostSegment> = segments
        .into_iter()
        .map(|(label, chars)| CostSegment {
            label: label.to_string(),
            chars,
            pct: if total_chars > 0 {
                (chars as f64 / total_chars as f64) * 100.0
            } else {
                0.0
            },
        })
        .collect();

    // Redundancy detection: find tool signatures called >= REDUNDANT_MIN_COUNT times.
    let mut redundant_calls: Vec<RedundantCallGroup> = tool_sig_counts
        .iter()
        .filter(|(_, &count)| count >= REDUNDANT_MIN_COUNT)
        .map(|((name, sig), &count)| {
            let avg_result_chars = if tool_result_chars > 0 && !tool_sig_counts.is_empty() {
                tool_result_chars / tool_sig_counts.values().sum::<usize>().max(1)
            } else {
                0
            };
            let wasted = (count - 1) * avg_result_chars;
            RedundantCallGroup {
                name: name.clone(),
                cmd_sig: sig.clone(),
                count,
                wasted_chars: wasted,
            }
        })
        .collect();
    redundant_calls.sort_by_key(|group| std::cmp::Reverse(group.count));

    // Generate findings.
    let mut findings = Vec::new();

    // Finding: tool result dominance.
    if total_chars > 0 && tool_result_chars as f64 / total_chars as f64 > 0.5 {
        let pct = (tool_result_chars as f64 / total_chars as f64 * 100.0).round() as usize;
        findings.push(CostFinding {
            severity: "high".to_string(),
            html: format!(
                "工具返回占上下文的 <b>{}%</b>，是最大的体积来源。需检查是否有大体量输出未做摘要。",
                pct
            ),
        });
    }

    // Finding: redundant calls.
    if !redundant_calls.is_empty() {
        let top = &redundant_calls[0];
        findings.push(CostFinding {
            severity: "high".to_string(),
            html: format!(
                "<b>{}×</b> 近乎相同的 <code>{}</code> {} 调用（占全部 {} 次工具的 {}%），是重复执行的主要泄漏点。",
                top.count,
                top.cmd_sig.chars().take(40).collect::<String>(),
                top.name,
                tool_sig_counts.values().sum::<usize>(),
                if tool_sig_counts.values().sum::<usize>() > 0 {
                    (top.count * 100) / tool_sig_counts.values().sum::<usize>()
                } else { 0 },
            ),
        });
    }

    // Finding: thinking vs text ratio.
    if thinking_chars > text_chars * 5 && thinking_chars > 1000 {
        findings.push(CostFinding {
            severity: "mid".to_string(),
            html: format!(
                "思考内容 (<b>{}</b> 字符) 是回复正文的 {} 倍，可能存在冗长推理链。",
                thinking_chars,
                thinking_chars
                    .checked_div(text_chars)
                    .unwrap_or(thinking_chars),
            ),
        });
    }

    // Per-step replay model: the token flame chart data.
    let system_tokens = estimate_tokens_from_chars(system_chars);
    let calls = compute_llm_calls(trajectory, system_tokens);
    let model = trajectory.model_name();
    let headroom = compute_headroom(&calls);

    Ok(CostStats {
        total_events,
        total_chars,
        breakdown,
        redundant_calls,
        findings,
        usage_steps: calls
            .iter()
            .filter(|c| c.real_prompt_tokens.is_some())
            .count(),
        total_real_input_tok: calls.iter().filter_map(|c| c.real_prompt_tokens).sum(),
        total_real_output_tok: calls.iter().filter_map(|c| c.real_completion_tokens).sum(),
        total_real_cached_tok: calls.iter().filter_map(|c| c.real_cached_tokens).sum(),
        calls,
        model,
        token_ratio_version: TOKEN_RATIO_VERSION.to_string(),
        headroom,
    })
}

// ---------------------------------------------------------------------------
// Per-step token replay model (drives the token flame chart)
// ---------------------------------------------------------------------------

/// Language-aware char→token estimation (no tokenizer).
/// CJK ≈ 1.5 chars/token; Latin/code ≈ 4 chars/token.
fn estimate_tokens(s: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for c in s.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    (cjk as f64 / 1.5 + other as f64 / 4.0).round() as usize
}

/// Whether a char belongs to a CJK / fullwidth block (denser token packing).
fn is_cjk(c: char) -> bool {
    let u = c as u32;
    (0x4E00..=0x9FFF).contains(&u)      // CJK Unified Ideographs
        || (0x3400..=0x4DBF).contains(&u) // CJK Ext A
        || (0x3040..=0x30FF).contains(&u) // Hiragana + Katakana
        || (0xAC00..=0xD7AF).contains(&u) // Hangul syllables
        || (0xF900..=0xFAFF).contains(&u) // CJK compatibility
        || (0xFF00..=0xFFEF).contains(&u) // Fullwidth forms
}

/// Real token counts from ATIF step metrics (normalized across API styles).
///
/// `prompt_tokens` semantics differ per provider: Anthropic excludes the
/// cached prefix while OpenAI includes it. When cached > prompt we assume
/// exclusive semantics and add the cached portion back to get the full
/// billed prompt size.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UsageBlock {
    prompt_total: Option<u64>,
    completion: Option<u64>,
    cached: Option<u64>,
}

fn parse_usage(step: &AtifStep) -> Option<UsageBlock> {
    let m = step.metrics?;
    let prompt = m.prompt_tokens.map(u64::from);
    let completion = m.completion_tokens.map(u64::from);
    let cached = m.cached_tokens.map(u64::from);
    if prompt.is_none() && completion.is_none() {
        return None;
    }
    let prompt_total = prompt.map(|p| match cached {
        Some(c) if c > p => p + c,
        _ => p,
    });
    Some(UsageBlock {
        prompt_total,
        completion,
        cached,
    })
}

/// Per-turn metadata kept aside to compute `removable_turn` in a second pass.
struct TurnMeta {
    primary_sig: String, // "name|cmd_sig" of the first tool call in the step
    all_calls_errored: bool,
    has_tool: bool,
}

/// Accumulating builder for the replay model.
struct CallBuilder {
    calls: Vec<LlmCall>,
    metas: Vec<TurnMeta>,
    origin: DateTime<Utc>,
    /// Static region (system prompt + skill/tool definitions) in tokens.
    /// Replayed every turn; drives Prefix-Caching candidate.
    static_region: usize,
    // history accumulated *before* the current step (the replayed context)
    hist_user: usize,
    hist_assistant: usize,
    hist_tool: usize,
    hist_trimmable: usize, // cumulative truncatable tokens from oversized tool outputs
}

impl CallBuilder {
    fn new(origin: DateTime<Utc>, static_region: usize) -> Self {
        Self {
            calls: Vec::new(),
            metas: Vec::new(),
            origin,
            static_region,
            hist_user: 0,
            hist_assistant: 0,
            hist_tool: 0,
            hist_trimmable: 0,
        }
    }

    /// Finalize one agent step into an `LlmCall`, then fold its output into history.
    fn finalize(
        &mut self,
        turn_ts: DateTime<Utc>,
        turn_output: usize,
        label: Option<String>,
        meta: TurnMeta,
        usage: Option<UsageBlock>,
    ) {
        let step_id = self.calls.len();

        // Static region: system prompt + skill/tool definitions (injected by caller).
        let system_prompt = self.static_region;
        let skill_definitions = 0;
        let tool_definitions = 0;
        let static_region = system_prompt + skill_definitions + tool_definitions;
        let injected_context = 0;

        // Payload-layer optimizable amounts (threshold rules).
        let cacheable = if static_region > PREFIX_CACHE_MIN {
            static_region
        } else {
            0
        };
        let history_prunable =
            if self.hist_assistant > HISTORY_PRUNE_MIN && step_id >= HISTORY_PRUNE_MIN_STEP {
                (self.hist_assistant as f64 * HISTORY_PRUNE_FRAC).round() as usize
            } else {
                0
            };
        let trimmable = self.hist_trimmable;
        let prunable = if injected_context > 0 {
            (injected_context as f64 * 0.5).round() as usize
        } else {
            0
        };

        let secs = (turn_ts - self.origin).num_seconds().max(0);
        let time = format!("{:02}:{:02}", secs / 60, secs % 60);

        // Real completion is billing truth when available; estimate is the fallback.
        let real_completion = usage.and_then(|u| u.completion);
        let output_tokens = real_completion.map(|v| v as usize).unwrap_or(turn_output);

        self.calls.push(LlmCall {
            step_id,
            time,
            label: label.unwrap_or_else(|| "text-only".to_string()),
            system_prompt,
            skill_definitions,
            tool_definitions,
            user_messages: self.hist_user,
            assistant_messages: self.hist_assistant,
            tool_results: self.hist_tool,
            injected_context,
            output_tokens,
            cacheable,
            history_prunable,
            trimmable,
            prunable,
            removable_turn: false,
            real_prompt_tokens: usage.and_then(|u| u.prompt_total),
            real_completion_tokens: real_completion,
            real_cached_tokens: usage.and_then(|u| u.cached),
        });
        self.metas.push(meta);

        // This turn's output becomes replayed assistant history for later steps.
        self.hist_assistant += output_tokens;
    }
}

/// Walk ATIF steps and compute the replay breakdown. Each agent step is one
/// LLM call; its observation (tool results) becomes replayed tool history for
/// the following steps.
fn compute_llm_calls(traj: &AtifTrajectory, static_region: usize) -> Vec<LlmCall> {
    let origin = traj.origin_ts().unwrap_or_default();
    let mut b = CallBuilder::new(origin, static_region);

    for step in &traj.steps {
        match step.source.as_str() {
            "agent" => {
                let mut turn_output = 0usize;
                if let Some(r) = step.reasoning_content.as_deref() {
                    turn_output += estimate_tokens(r);
                }
                if let Some(m) = step.message.as_deref() {
                    turn_output += estimate_tokens(m);
                }
                let mut label: Option<String> = None;
                let mut primary_sig = String::new();
                for call in step.calls() {
                    // Tool-call args are model-generated output, replayed as history.
                    turn_output += estimate_tokens(&call.arguments.to_string());
                    if label.is_none() {
                        label = Some(call.function_name.clone());
                    }
                    if primary_sig.is_empty() {
                        primary_sig = format!(
                            "{}|{}",
                            call.function_name,
                            call.command_summary(CMD_SIG_CHARS)
                        );
                    }
                }
                let has_tool = !step.calls().is_empty();
                let all_calls_errored = has_tool
                    && !step.results().is_empty()
                    && step
                        .results()
                        .iter()
                        .all(|r| observation_looks_like_error(r.content.as_deref().unwrap_or("")));

                let turn_ts = step.end_ts().unwrap_or(origin);
                b.finalize(
                    turn_ts,
                    turn_output,
                    label,
                    TurnMeta {
                        primary_sig,
                        all_calls_errored,
                        has_tool,
                    },
                    parse_usage(step),
                );

                // Tool results become replayed history for subsequent steps.
                for result in step.results() {
                    let toks = estimate_tokens(result.content.as_deref().unwrap_or(""));
                    b.hist_tool += toks;
                    if toks > TOOL_TRIM_MIN {
                        b.hist_trimmable += (toks as f64 * TOOL_TRIM_FRAC).round() as usize;
                    }
                }
            }
            "user" => {
                b.hist_user += estimate_tokens(step.message.as_deref().unwrap_or(""));
            }
            _ => {} // system steps feed the static region, already injected
        }
    }

    // Second pass: orchestration-layer heuristic for removable_turn.
    // A turn is flagged when it duplicates the previous turn's primary action
    // (spinning) or every tool call in it errored (failed retry).
    for i in 0..b.calls.len() {
        let m = &b.metas[i];
        let dup = i > 0 && !m.primary_sig.is_empty() && b.metas[i - 1].primary_sig == m.primary_sig;
        b.calls[i].removable_turn = dup || (m.has_tool && m.all_calls_errored);
    }

    calibrate_calls(&mut b.calls);

    b.calls
}

/// Calibrate estimated context categories against real usage (真值定总量、估算定比例).
///
/// 1. The minimum surplus of (real prompt − estimated context) across usage-carrying
///    steps is constant per step → attributed to the unmeasured static region
///    (system prompt / tool definitions).
/// 2. Each usage-carrying step's categories are then scaled so their sum equals
///    `real_prompt_tokens`; threshold-derived amounts are re-derived on calibrated
///    values. Steps without usage keep pure estimates.
fn calibrate_calls(calls: &mut [LlmCall]) {
    let est_sum = |c: &LlmCall| {
        c.system_prompt
            + c.skill_definitions
            + c.tool_definitions
            + c.user_messages
            + c.assistant_messages
            + c.tool_results
            + c.injected_context
    };

    let static_floor = calls
        .iter()
        .filter_map(|c| {
            c.real_prompt_tokens
                .map(|rp| (rp as i64 - est_sum(c) as i64).max(0))
        })
        .min()
        .unwrap_or(0) as usize;

    for c in calls.iter_mut() {
        let Some(rp) = c.real_prompt_tokens else {
            continue;
        };
        let rp = rp as usize;

        c.system_prompt += static_floor;
        let est = est_sum(c);
        if est > 0 {
            let scale = rp as f64 / est as f64;
            for f in [
                &mut c.system_prompt,
                &mut c.skill_definitions,
                &mut c.tool_definitions,
                &mut c.user_messages,
                &mut c.assistant_messages,
                &mut c.tool_results,
                &mut c.injected_context,
            ] {
                *f = (*f as f64 * scale).round() as usize;
            }
            c.trimmable = (c.trimmable as f64 * scale).round() as usize;
            // Absorb rounding drift into the largest dynamic category.
            let drift = rp as i64 - est_sum(c) as i64;
            let largest = [
                &mut c.assistant_messages,
                &mut c.tool_results,
                &mut c.user_messages,
                &mut c.system_prompt,
            ]
            .into_iter()
            .max_by_key(|f| **f);
            if let Some(f) = largest {
                *f = (*f as i64 + drift).max(0) as usize;
            }
        } else {
            // Nothing measurable at all → whole context is the static region.
            c.system_prompt = rp;
        }

        // Re-derive threshold rules on calibrated values.
        let static_region = c.system_prompt + c.skill_definitions + c.tool_definitions;
        c.cacheable = if static_region > PREFIX_CACHE_MIN {
            static_region
        } else {
            0
        };
        c.history_prunable =
            if c.assistant_messages > HISTORY_PRUNE_MIN && c.step_id >= HISTORY_PRUNE_MIN_STEP {
                (c.assistant_messages as f64 * HISTORY_PRUNE_FRAC).round() as usize
            } else {
                0
            };
    }
}

// ---------------------------------------------------------------------------
// Waste candidate extraction (Rust supplies structured candidates → LLM judges)
// ---------------------------------------------------------------------------

/// Total context tokens (prompt_tokens) of one step.
fn ctx_total(c: &LlmCall) -> usize {
    c.system_prompt
        + c.skill_definitions
        + c.tool_definitions
        + c.user_messages
        + c.assistant_messages
        + c.tool_results
        + c.injected_context
}

/// Compute playbook M-class ratio metrics from the replay model (pure Rust).
/// These drive candidate admission and are handed to every strategy prompt.
fn compute_ratio_metrics(cost: &CostStats) -> CostRatioMetrics {
    let calls = &cost.calls;
    if calls.is_empty() {
        return CostRatioMetrics::default();
    }
    let total_input: usize = calls.iter().map(ctx_total).sum();
    let total_output: usize = calls.iter().map(|c| c.output_tokens).sum();
    let share = |part: usize, whole: usize| {
        if whole > 0 {
            part as f64 / whole as f64
        } else {
            0.0
        }
    };

    // M1: static prefix replayed every step / billed input.
    let static_sum: usize = calls
        .iter()
        .map(|c| c.system_prompt + c.skill_definitions + c.tool_definitions)
        .sum();

    // M3 uses billing truth only; None when the trajectory carries no usage.
    let m3_cache_hit_rate = if cost.total_real_input_tok > 0 {
        Some(cost.total_real_cached_tok as f64 / cost.total_real_input_tok as f64)
    } else {
        None
    };

    // M7: replayed history (user + assistant + tool results) / step input, peak.
    let m7_history_peak_share = calls
        .iter()
        .map(|c| {
            share(
                c.user_messages + c.assistant_messages + c.tool_results,
                ctx_total(c),
            )
        })
        .fold(0.0, f64::max);

    // M14 via the char breakdown — thinking is output-side, chars are a fair proxy.
    let seg_chars = |label: &str| {
        cost.breakdown
            .iter()
            .find(|s| s.label == label)
            .map(|s| s.chars)
            .unwrap_or(0)
    };
    let thinking_chars = seg_chars("思考");
    let text_chars = seg_chars("回复正文");
    let m14_thinking_ratio = if text_chars > 0 {
        thinking_chars as f64 / text_chars as f64
    } else if thinking_chars > 0 {
        99.0 // all thinking, no body text — capped instead of infinity
    } else {
        0.0
    };

    // M15: tool-call steps / all steps (label falls back to "text-only").
    let tool_steps = calls.iter().filter(|c| c.label != "text-only").count();

    // M16: retry/backtrack churn (full turn incl. replay) / total bill.
    let churn: usize = calls
        .iter()
        .filter(|c| c.removable_turn)
        .map(|c| ctx_total(c) + c.output_tokens)
        .sum();

    CostRatioMetrics {
        m1_prefix_share: share(static_sum, total_input),
        m3_cache_hit_rate,
        m7_history_peak_share,
        m14_thinking_ratio,
        m15_tool_step_share: share(tool_steps, calls.len()),
        m16_churn_share: share(churn, total_input + total_output),
    }
}

/// Aggregate per-step replay model into a headroom summary (token-based only).
/// USD savings are not computed — pricing varies too much across models/providers.
fn compute_headroom(calls: &[LlmCall]) -> CostHeadroom {
    if calls.is_empty() {
        return CostHeadroom::default();
    }

    let payload_deletable_tok = calls
        .iter()
        .map(|c| c.history_prunable + c.trimmable + c.prunable)
        .sum::<usize>();
    let payload_cacheable_tok = calls.iter().map(|c| c.cacheable).sum::<usize>();

    let orch_savable_tok = calls
        .iter()
        .filter(|c| c.removable_turn)
        .map(|c| ctx_total(c) + c.output_tokens)
        .sum::<usize>();

    let total_input_tok = calls.iter().map(ctx_total).sum::<usize>();
    let total_output_tok = calls.iter().map(|c| c.output_tokens).sum::<usize>();

    let total_save_tok = payload_deletable_tok + payload_cacheable_tok + orch_savable_tok;
    let pct = if total_input_tok + total_output_tok > 0 {
        (total_save_tok as f64 / (total_input_tok + total_output_tok) as f64) * 100.0
    } else {
        0.0
    };

    CostHeadroom {
        payload_deletable_tok,
        payload_cacheable_tok,
        orch_savable_tok,
        total_input_tok,
        total_output_tok,
        pct,
        headroom_compressed_tok: 0,
        headroom_save_pct: 0.0,
    }
}

/// UTF-8 safe truncation to `n` chars, collapsing newlines for compactness.
fn trunc(s: &str, n: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > n {
        format!("{}…", flat.chars().take(n).collect::<String>())
    } else {
        flat
    }
}

/// Whether a tool call command looks like a backtrack / dead-end reversal.
fn is_backtrack_cmd(cmd: &str) -> bool {
    let c = cmd.to_lowercase();
    [
        "git checkout",
        "git reset",
        "git revert",
        "git stash",
        "git restore",
        "回退",
        "撤销",
    ]
    .iter()
    .any(|k| c.contains(k))
}

/// Convert a char count to estimated tokens (used for system region injection).
fn estimate_tokens_from_chars(chars: usize) -> usize {
    // Conservative: treat all as Latin (4 chars/tok) — system prompts are mostly code/English.
    (chars as f64 / 4.0).round() as usize
}

/// Build the structured waste candidates for a trajectory. Pure computation:
/// groups the trajectory into per-sub-type candidates with billing-caliber
/// `potential_save_tokens` and short evidence, ready for LLM judgment.
pub fn extract_waste_candidates(trajectory: &AtifTrajectory) -> Result<WasteCandidateSet> {
    let cost = compute_cost(trajectory)?;
    extract_waste_candidates_from(&cost, trajectory)
}

/// Build waste candidates from pre-computed cost stats and the trajectory.
/// This avoids redundant computation when called together with `compute_cost`.
pub(crate) fn extract_waste_candidates_from(
    cost: &CostStats,
    trajectory: &AtifTrajectory,
) -> Result<WasteCandidateSet> {
    let calls = &cost.calls;
    if calls.is_empty() {
        return Ok(WasteCandidateSet::default());
    }

    let total_steps = calls.len();
    let total_output_tokens: usize = calls.iter().map(|c| c.output_tokens).sum();
    let total_input_tokens: usize = calls.iter().map(ctx_total).sum();

    // Aggregates from the replay model (billing caliber, incl. replay).
    let hist_tok: usize = calls.iter().map(|c| c.history_prunable).sum();
    let trim_tok: usize = calls.iter().map(|c| c.trimmable).sum();
    let cache_tok: usize = calls.iter().map(|c| c.cacheable).sum();

    // Peak accumulated assistant history.
    let (mut peak_hist, mut peak_hist_step) = (0usize, 0usize);
    for (i, c) in calls.iter().enumerate() {
        if c.assistant_messages > peak_hist {
            peak_hist = c.assistant_messages;
            peak_hist_step = i;
        }
    }

    // Second pass over ATIF steps for evidence: tool outputs, large user
    // inputs, backtracks. The agent-step ordinal is the replay step index.
    let mut tool_outputs: Vec<(usize, String, usize, String)> = Vec::new(); // step, name, tokens, snippet
    let mut user_inputs: Vec<(usize, usize, String)> = Vec::new(); // step, tokens, snippet
    let mut backtracks: Vec<(usize, String)> = Vec::new(); // step, cmd snippet

    let mut turn_idx: i64 = -1;
    for step in &trajectory.steps {
        match step.source.as_str() {
            "agent" => {
                turn_idx += 1;
                let step_no = turn_idx.max(0) as usize;
                for call in step.calls() {
                    let cmd = call.command_summary(CMD_SIG_CHARS);
                    if is_backtrack_cmd(&cmd) {
                        backtracks.push((step_no, trunc(&cmd, 80)));
                    }
                }
                for result in step.results() {
                    let text = result.content.as_deref().unwrap_or("");
                    let toks = estimate_tokens(text);
                    if toks >= TOOL_TRIM_MIN {
                        let name = result
                            .source_call_id
                            .as_deref()
                            .and_then(|id| {
                                step.calls()
                                    .iter()
                                    .find(|c| c.tool_call_id == id)
                                    .map(|c| c.function_name.clone())
                            })
                            .or_else(|| step.calls().first().map(|c| c.function_name.clone()))
                            .unwrap_or_else(|| "unknown".to_string());
                        tool_outputs.push((step_no, name, toks, trunc(text, SNIPPET_CHARS)));
                    }
                }
            }
            "user" => {
                let text = step.message.as_deref().unwrap_or("");
                let toks = estimate_tokens(text);
                if toks >= USER_LARGE_MIN {
                    let step_no = turn_idx.max(0) as usize;
                    user_inputs.push((step_no, toks, trunc(text, SNIPPET_CHARS)));
                }
            }
            _ => {}
        }
    }
    tool_outputs.sort_by_key(|out| std::cmp::Reverse(out.2));

    // Agent-first: Rust only computes ratio metrics as evidence; every candidate
    // with underlying data goes to the LLM, which checks the admission criteria
    // itself against the metrics block in its prompt.
    let metrics = compute_ratio_metrics(cost);
    let total_billed = total_input_tokens + total_output_tokens;
    let bill_share = |tok: usize| {
        if total_billed > 0 {
            tok as f64 / total_billed as f64
        } else {
            0.0
        }
    };

    let mut candidates: Vec<WasteCandidate> = Vec::new();

    // ── 上下文臃肿 ──

    // 前缀缓存 (playbook #1): savings are a price discount — prefix replay ×
    // (1 − cached price). 判据直接给命中率序列（对齐 perf 的 prefix_cache）：
    // 持续为零/偏低即未在享受缓存；无 usage 本身就是"可能未开启 caching"的信号。
    let cache_save = (cache_tok as f64 * (1.0 - CACHED_PRICE_RATIO)).round() as usize;
    if cache_tok > 0 {
        let hit_seq: Vec<String> = calls
            .iter()
            .filter_map(|c| match c.real_prompt_tokens {
                Some(p) if p > 0 => Some(format!(
                    "{:.0}%",
                    c.real_cached_tokens.unwrap_or(0) as f64 / p as f64 * 100.0
                )),
                _ => None,
            })
            .take(20)
            .collect();
        let hit_desc = if hit_seq.is_empty() {
            "无 usage 数据（可能未开启 prompt caching，这本身即适用信号）".to_string()
        } else {
            format!("每步命中率 [{}]", hit_seq.join(", "))
        };
        candidates.push(WasteCandidate {
            id: "fixed_overhead".into(),
            category: "上下文臃肿".into(),
            subtype: "固定开销".into(),
            optimization: "前缀缓存".into(),
            potential_save_tokens: cache_save,
            discount: true,
            save_share: bill_share(cache_save),
            savings_kind: "折价".into(),
            steps: vec![],
            facts: format!(
                "总体缓存命中率 {}；{}；静态前缀每步约 {} tok，{} 步逐轮重发",
                match metrics.m3_cache_hit_rate {
                    Some(v) => format!("{:.0}% (M3)", v * 100.0),
                    None => "未知".to_string(),
                },
                hit_desc,
                cache_tok / total_steps.max(1),
                total_steps
            ),
            snippet: String::new(),
        });
    }

    // 历史消息裁剪 (playbook #5): 判据直接给每步历史占比序列；
    // M8 旧内容引用率交由 LLM 判断。
    if hist_tok > 0 {
        let hist_seq: Vec<String> = calls
            .iter()
            .take(20)
            .map(|c| {
                format!(
                    "{:.0}%",
                    c.assistant_messages as f64 / ctx_total(c).max(1) as f64 * 100.0
                )
            })
            .collect();
        candidates.push(WasteCandidate {
            id: "history".into(),
            category: "上下文臃肿".into(),
            subtype: "历史消息".into(),
            optimization: "历史消息裁剪".into(),
            potential_save_tokens: hist_tok,
            discount: false,
            save_share: bill_share(hist_tok),
            savings_kind: "可省".into(),
            steps: vec![peak_hist_step],
            facts: format!(
                "每步历史占比 [{}]；峰值 {:.0}% (M7)，{} tok (step {})，O(n) 累积并逐轮重放",
                hist_seq.join(", "),
                metrics.m7_history_peak_share * 100.0,
                peak_hist,
                peak_hist_step
            ),
            snippet: String::new(),
        });
    }

    // 工具输出截断 (playbook #7): 单条 M9 与 S1 死重占比均由 LLM 对照指标判断。
    if trim_tok > 0 && !tool_outputs.is_empty() {
        let steps: Vec<usize> = tool_outputs
            .iter()
            .take(EVIDENCE_TOP_N)
            .map(|t| t.0)
            .collect();
        let facts = tool_outputs
            .iter()
            .take(EVIDENCE_TOP_N)
            .map(|(s, n, tk, _)| format!("step {} {} {} tok", s, n, fmt_k(*tk)))
            .collect::<Vec<_>>()
            .join("；");
        let top_m9 = tool_outputs
            .first()
            .map(|(step, _, toks, _)| bill_share(toks * total_steps.saturating_sub(*step + 1)))
            .unwrap_or(0.0);
        candidates.push(WasteCandidate {
            id: "tool_output".into(),
            category: "上下文臃肿".into(),
            subtype: "工具输出多".into(),
            optimization: "工具输出截断".into(),
            potential_save_tokens: trim_tok,
            discount: false,
            save_share: bill_share(trim_tok),
            savings_kind: "可省".into(),
            steps,
            facts: format!(
                "最大单条 M9 重放占比 {:.0}%；超长工具返回 top{}：{}",
                top_m9 * 100.0,
                tool_outputs.len().min(EVIDENCE_TOP_N),
                facts
            ),
            snippet: tool_outputs
                .first()
                .map(|t| t.3.clone())
                .unwrap_or_default(),
        });
    }

    // 动态注入裁剪 (playbook #4) removed: injected_context was never captured
    // from raw trajectories (hardcoded 0), so the candidate could not fire and the
    // strategy carried no practical value. Re-add only if injection markers
    // (<system-reminder> etc.) get parsed out of user messages.

    // 提示词压缩 (playbook #11): 单条 M13 重放占比由 LLM 对照指标判断。
    if !user_inputs.is_empty() {
        let up_potential: usize = user_inputs
            .iter()
            .map(|(s, t, _)| {
                ((*t as f64) * PROMPT_COMPRESS_FRAC * total_steps.saturating_sub(*s) as f64).round()
                    as usize
            })
            .sum();
        let steps: Vec<usize> = user_inputs.iter().map(|u| u.0).collect();
        let top_m13 = user_inputs
            .iter()
            .map(|(s, t, _)| bill_share(t * total_steps.saturating_sub(*s)))
            .fold(0.0, f64::max);
        candidates.push(WasteCandidate {
            id: "user_prompt".into(),
            category: "上下文臃肿".into(),
            subtype: "用户提示词".into(),
            optimization: "提示词压缩".into(),
            potential_save_tokens: up_potential,
            discount: false,
            save_share: bill_share(up_potential),
            savings_kind: "可省".into(),
            steps,
            facts: format!(
                "{} 段超长用户输入（最大 {} tok），最大单条 M13 重放占比 {:.0}%",
                user_inputs.len(),
                fmt_k(user_inputs.iter().map(|u| u.1).max().unwrap_or(0)),
                top_m13 * 100.0
            ),
            snippet: user_inputs.first().map(|u| u.2.clone()).unwrap_or_default(),
        });
    }

    // 推理预算调低 (playbook #12) removed: M14 is a chars-based proxy (no
    // per-step thinking tokens), the S3 difficulty judgment is subjective, and
    // lowering reasoning effort risks accuracy for a one-off (non-replayed)
    // output cost — evidence too weak for an actionable verdict. Thinking share
    // stays observable in the breakdown and findings.

    // ── 步骤冗余（预防性节省：防下次会话复发，非本次可回收）──

    // 无效轮次消除 (playbook #14, merged retry + backtrack): both are wasted
    // turns — spinning in place vs. exploring a dead end — with identical
    // disposal (attribute first, then fix skill/prompt or archive the lesson).
    // One candidate carries both signal lists so the LLM can cross-reference
    // them (backtracks are often preceded by retries).
    let removable: Vec<&LlmCall> = calls.iter().filter(|c| c.removable_turn).collect();
    if !removable.is_empty() || !backtracks.is_empty() {
        // Union of wasted steps; a backtrack turn may also be flagged removable.
        let mut steps: Vec<usize> = removable.iter().map(|c| c.step_id).collect();
        steps.extend(backtracks.iter().map(|b| b.0));
        steps.sort_unstable();
        steps.dedup();
        let potential: usize = steps
            .iter()
            .filter_map(|s| calls.iter().find(|c| c.step_id == *s))
            .map(|c| ctx_total(c) + c.output_tokens)
            .sum();
        let mut parts = vec![format!(
            "M16 空转账单占比 {:.0}%",
            metrics.m16_churn_share * 100.0
        )];
        if !removable.is_empty() {
            let labels = removable
                .iter()
                .take(6)
                .map(|c| c.label.clone())
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!(
                "{} 轮疑似重复/报错重试（{}）",
                removable.len(),
                labels
            ));
        }
        if !backtracks.is_empty() {
            parts.push(format!(
                "{} 处回退信号（{}）",
                backtracks.len(),
                backtracks
                    .iter()
                    .take(3)
                    .map(|b| b.1.clone())
                    .collect::<Vec<_>>()
                    .join(" / ")
            ));
        }
        parts.push("整轮重放".to_string());
        candidates.push(WasteCandidate {
            id: "churn".into(),
            category: "步骤冗余".into(),
            subtype: "无效轮次".into(),
            optimization: "先归因：优化 Skill/提示词或沉淀经验".into(),
            potential_save_tokens: potential,
            discount: false,
            save_share: bill_share(potential),
            savings_kind: "预防".into(),
            steps,
            facts: parts.join("；"),
            snippet: String::new(),
        });
    }

    // TODO(#opt-model-routing): 模型路由 (playbook #13) — M15 is computed, but
    // "routable steps have low M14" needs per-step thinking tokens; deferred
    // until those exist.

    // Agent-first: no pre-filtering — rank by expected bill-share so the biggest
    // savings surface first; the 3% noise line is cited in the prompt for the
    // LLM to weigh and enforced only in cache-discount arbitration.
    candidates.sort_by(|a, b| {
        b.save_share
            .partial_cmp(&a.save_share)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(WasteCandidateSet {
        model: cost.model.clone(),
        total_steps,
        total_input_tokens,
        total_output_tokens,
        metrics,
        candidates,
    })
}

/// Compact token formatter for facts strings (e.g. 3.4k).
fn fmt_k(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn traj(steps_json: &str) -> AtifTrajectory {
        AtifTrajectory::from_json(&format!(
            r#"{{"schema_version":"ATIF-v1.6","session_id":"s1",
                "agent":{{"name":"a","version":"1","model_name":"claude-x"}},"steps":{steps_json}}}"#
        ))
        .unwrap()
    }

    /// Replay model: per-step categories must accumulate monotonically.
    #[test]
    fn test_replay_monotonic() {
        let cost = compute_cost(&traj(
            r#"[
            {"step_id":1,"source":"user","timestamp":"2026-07-02T06:30:00.000Z","message":"review this"},
            {"step_id":2,"source":"agent","timestamp":"2026-07-02T06:30:02.000Z",
             "tool_calls":[{"tool_call_id":"c1","function_name":"Bash","arguments":{"command":"ls"}}],
             "observation":{"results":[{"source_call_id":"c1","content":"a\nb\nc"}]}},
            {"step_id":3,"source":"agent","timestamp":"2026-07-02T06:30:05.000Z","message":"done"}
        ]"#,
        ))
        .unwrap();
        assert_eq!(cost.calls.len(), 2);
        // Step 0 sees no prior assistant/tool history; step 1 sees step 0's output + the tool result.
        assert_eq!(cost.calls[0].assistant_messages, 0);
        assert!(cost.calls[1].assistant_messages >= cost.calls[0].assistant_messages);
        assert!(cost.calls[1].tool_results > 0);
        assert_eq!(cost.calls[0].label, "Bash");
        assert_eq!(cost.calls[1].label, "text-only");
        assert_eq!(cost.model, "claude-x");
    }

    #[test]
    fn test_empty_trajectory() {
        let cost = compute_cost(&traj("[]")).unwrap();
        assert_eq!(cost.total_events, 0);
        assert_eq!(cost.total_chars, 0);
        assert!(cost.breakdown.is_empty());
    }

    #[test]
    fn test_waste_candidates_tool_output() {
        // A big tool return replayed across several steps → tool_output candidate.
        let big = "x ".repeat(6000); // ~3k tokens (latin/4), over the 2k trim threshold
        let t = traj(&format!(
            r#"[
            {{"step_id":1,"source":"user","timestamp":"2026-07-02T06:30:00.000Z","message":"go"}},
            {{"step_id":2,"source":"agent","timestamp":"2026-07-02T06:30:01.000Z",
             "tool_calls":[{{"tool_call_id":"c1","function_name":"Read","arguments":{{"file_path":"/big"}}}}],
             "observation":{{"results":[{{"source_call_id":"c1","content":"{big}"}}]}}}},
            {{"step_id":3,"source":"agent","timestamp":"2026-07-02T06:30:03.000Z","message":"ok"}}
        ]"#
        ));
        let set = extract_waste_candidates(&t).unwrap();
        let tool = set.candidates.iter().find(|c| c.id == "tool_output");
        assert!(
            tool.is_some(),
            "expected tool_output candidate, got {:?}",
            set.candidates.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        let tool = tool.unwrap();
        assert!(tool.potential_save_tokens > 0);
        assert_eq!(tool.optimization, "工具输出截断");
        assert!(tool.facts.contains("Read"));
        assert!(set.total_input_tokens > 0);
    }

    #[test]
    fn test_waste_candidates_empty() {
        let set = extract_waste_candidates(&traj("[]")).unwrap();
        assert!(set.candidates.is_empty());
    }

    /// Oversized user steps must fire the Prompt-Compression candidate.
    #[test]
    fn test_waste_candidates_user_prompt() {
        let big = "x ".repeat(4000); // ~2k tokens, over the 1.5k USER_LARGE_MIN
        let t = traj(&format!(
            r#"[
            {{"step_id":1,"source":"user","timestamp":"2026-07-02T06:30:00.000Z","message":"{big}"}},
            {{"step_id":2,"source":"agent","timestamp":"2026-07-02T06:30:01.000Z","message":"ok"}},
            {{"step_id":3,"source":"user","timestamp":"2026-07-02T06:30:02.000Z","message":"next"}},
            {{"step_id":4,"source":"agent","timestamp":"2026-07-02T06:30:03.000Z","message":"done"}}
        ]"#
        ));
        let set = extract_waste_candidates(&t).unwrap();
        let up = set.candidates.iter().find(|c| c.id == "user_prompt");
        assert!(
            up.is_some(),
            "expected user_prompt candidate, got {:?}",
            set.candidates.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        let up = up.unwrap();
        assert_eq!(up.optimization, "提示词压缩");
        assert!(up.potential_save_tokens > 0);
        assert!(!up.snippet.is_empty());
    }

    /// Usage metrics: categories calibrated to real prompt total,
    /// output_tokens taken from real completion. `cached > prompt` implies
    /// Anthropic-style exclusive semantics (cache added back).
    #[test]
    fn test_usage_calibration_exclusive_semantics() {
        let cost = compute_cost(&traj(
            r#"[
            {"step_id":1,"source":"user","timestamp":"2026-07-02T06:30:00.000Z","message":"review this"},
            {"step_id":2,"source":"agent","timestamp":"2026-07-02T06:30:02.000Z",
             "metrics":{"prompt_tokens":100,"completion_tokens":50,"cached_tokens":8000},
             "tool_calls":[{"tool_call_id":"c1","function_name":"Bash","arguments":{"command":"ls"}}],
             "observation":{"results":[{"source_call_id":"c1","content":"a\nb\nc"}]}},
            {"step_id":3,"source":"agent","timestamp":"2026-07-02T06:30:05.000Z",
             "metrics":{"prompt_tokens":200,"completion_tokens":30,"cached_tokens":8000},
             "message":"done"}
        ]"#,
        ))
        .unwrap();
        assert_eq!(cost.calls.len(), 2);
        assert_eq!(cost.usage_steps, 2);
        // prompt_total = prompt + cached (exclusive semantics detected)
        assert_eq!(cost.calls[0].real_prompt_tokens, Some(8100));
        assert_eq!(cost.calls[1].real_prompt_tokens, Some(8200));
        assert_eq!(cost.calls[0].real_cached_tokens, Some(8000));
        // output = real completion, not estimate
        assert_eq!(cost.calls[0].output_tokens, 50);
        assert_eq!(cost.calls[1].output_tokens, 30);
        // calibrated categories sum exactly to real prompt total
        for c in &cost.calls {
            let sum = c.system_prompt
                + c.skill_definitions
                + c.tool_definitions
                + c.user_messages
                + c.assistant_messages
                + c.tool_results
                + c.injected_context;
            assert_eq!(sum, c.real_prompt_tokens.unwrap() as usize);
        }
        // static floor absorbed the unmeasured system prompt → cacheable fires
        assert!(cost.calls[0].system_prompt > PREFIX_CACHE_MIN);
        assert!(cost.calls[0].cacheable > 0);
        // aggregates
        assert_eq!(cost.total_real_input_tok, 8100 + 8200);
        assert_eq!(cost.total_real_output_tok, 80);
        assert_eq!(cost.total_real_cached_tok, 16000);
    }

    /// OpenAI-style usage: prompt_tokens already includes the cached prefix
    /// (cached <= prompt → no adjustment).
    #[test]
    fn test_usage_calibration_inclusive_semantics() {
        let cost = compute_cost(&traj(
            r#"[
            {"step_id":1,"source":"user","timestamp":"2026-07-02T06:30:00.000Z","message":"go"},
            {"step_id":2,"source":"agent","timestamp":"2026-07-02T06:30:02.000Z",
             "metrics":{"prompt_tokens":5000,"completion_tokens":40,"cached_tokens":4000},
             "message":"ok"}
        ]"#,
        ))
        .unwrap();
        assert_eq!(cost.calls.len(), 1);
        assert_eq!(cost.calls[0].real_prompt_tokens, Some(5000));
        assert_eq!(cost.calls[0].real_completion_tokens, Some(40));
        assert_eq!(cost.calls[0].real_cached_tokens, Some(4000));
        assert_eq!(cost.calls[0].output_tokens, 40);
    }

    /// No usage in the trajectory → pure estimates, zeroed aggregates.
    #[test]
    fn test_usage_absent_fallback() {
        let cost = compute_cost(&traj(
            r#"[
            {"step_id":1,"source":"user","timestamp":"2026-07-02T06:30:00.000Z","message":"go"},
            {"step_id":2,"source":"agent","timestamp":"2026-07-02T06:30:02.000Z","message":"ok"}
        ]"#,
        ))
        .unwrap();
        assert_eq!(cost.usage_steps, 0);
        assert_eq!(cost.total_real_input_tok, 0);
        assert!(cost.calls[0].real_prompt_tokens.is_none());
    }
}
