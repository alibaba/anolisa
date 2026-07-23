//! LLM cost-waste identification: Rust extracts structured candidates and
//! computes ratio metrics (agent-first: no admission pre-filtering — the LLM
//! checks admission criteria in-prompt), then fires one LLM call per
//! candidate/strategy in parallel (mirroring the perf per-strategy
//! architecture). Each call judges whether one candidate is worth optimizing;
//! verdicts are joined back and pass deterministic arbitration (cache-priority
//! net-benefit discount, noise line) to produce the final waste rows.

use anyhow::Result;

use crate::atif::AtifTrajectory;
use crate::cost::prompts::cost_identification::{build_strategy_prompt, strategy_for, STRATEGIES};
use crate::cost::{CACHED_PRICE_RATIO, NOISE_LINE};
use crate::llm::LlmClient;
use crate::types::{WasteItem, WasteReport, WasteVerdict};

/// Run full cost-waste identification: Rust candidate extraction → parallel per-candidate LLM eval → merge.
/// Parses events only once and reuses them for both cost computation and candidate extraction.
pub async fn identify_waste(
    client: &LlmClient,
    trajectory: &AtifTrajectory,
) -> Result<WasteReport> {
    let cost = crate::cost::compute_cost(trajectory)?;
    let candidates = crate::cost::extract_waste_candidates_from(&cost, trajectory)?;
    if candidates.candidates.is_empty() {
        tracing::info!("Cost: no waste candidates extracted, skipping LLM judgment");
        return Ok(WasteReport {
            items: vec![],
            considered: 0,
            dismissed: 0,
            model: candidates.model,
        });
    }

    let considered = candidates.candidates.len();
    tracing::info!(
        "Cost: {} waste candidates — evaluating {} strategies in parallel...",
        considered,
        considered
    );

    // Fire one LLM call per candidate in parallel. Candidates without a
    // strategy definition are skipped (Rust extraction and STRATEGIES share
    // the same stable id catalog, so this only guards future drift).
    let futures: Vec<_> = candidates
        .candidates
        .iter()
        .filter_map(|cand| {
            let Some(strategy) = strategy_for(&cand.id) else {
                tracing::warn!(
                    "Cost: candidate '{}' has no strategy definition, skipped",
                    cand.id
                );
                return None;
            };
            let messages = build_strategy_prompt(&candidates, cand, strategy);
            let label = format!("cost:{}", strategy.id);
            tracing::info!("Cost: strategy '{}' — sending to LLM...", strategy.id);
            Some(async move {
                let result: std::result::Result<WasteVerdict, _> = client
                    .chat_json_parsed_labeled(messages, Some(&label))
                    .await;
                (cand, strategy, result)
            })
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    let total_billed = (candidates.total_input_tokens + candidates.total_output_tokens).max(1);
    let cache_hit = candidates.metrics.m3_cache_hit_rate;

    let mut items: Vec<WasteItem> = Vec::new();
    let mut dismissed = 0usize;

    for (cand, strategy, result) in results {
        let v = match result {
            Ok(v) if v.worth_optimizing => {
                tracing::info!(
                    "Cost: strategy '{}' ✓ worth optimizing (save_ratio {:.2})",
                    cand.id,
                    v.save_ratio
                );
                v
            }
            Ok(_) => {
                tracing::info!("Cost: strategy '{}' ✗ not worth optimizing", cand.id);
                dismissed += 1;
                continue;
            }
            Err(err) => {
                tracing::warn!("Cost: strategy '{}' LLM call failed: {}", cand.id, err);
                dismissed += 1;
                continue;
            }
        };

        // save_ratio defaults to 1.0 when the LLM marked it worth but omitted a ratio.
        let ratio = if v.save_ratio > 0.0 {
            v.save_ratio.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut save_tokens = ((cand.potential_save_tokens as f64) * ratio).round() as usize;
        let mut evidence = if v.evidence.is_empty() {
            cand.facts.clone()
        } else {
            v.evidence.clone()
        };

        // 仲裁——缓存优先原则 (playbook 规则 1): when the cache hit rate is high,
        // history-editing strategies invalidate the KV cache from the edit point
        // on, so their net benefit is the full-price share of the saved tokens.
        // Below the noise line after discounting → dismissed deterministically.
        // (主手段去重——playbook 规则 2——needs no code: candidate sources are
        // disjoint by construction: assistant history vs tool results vs inputs.)
        if let Some(m3) = cache_hit {
            if m3 > 0.5 && matches!(cand.id.as_str(), "history" | "tool_output") {
                let factor = 1.0 - m3 * (1.0 - CACHED_PRICE_RATIO);
                let adjusted = (save_tokens as f64 * factor).round() as usize;
                tracing::info!(
                    "Cost: 缓存优先仲裁 '{}' — M3={:.0}%，净收益 {} → {} tok",
                    cand.id,
                    m3 * 100.0,
                    save_tokens,
                    adjusted
                );
                if (adjusted as f64 / total_billed as f64) < NOISE_LINE {
                    tracing::info!("Cost: strategy '{}' ✗ 缓存折算后低于噪声线，不报", cand.id);
                    dismissed += 1;
                    continue;
                }
                save_tokens = adjusted;
                evidence = format!(
                    "{}（已按 M3={:.0}% 缓存命中折算净收益）",
                    evidence,
                    m3 * 100.0
                );
            }
        }

        items.push(WasteItem {
            symptom: if v.symptom.is_empty() {
                cand.facts.clone()
            } else {
                v.symptom.clone()
            },
            category: cand.category.clone(),
            subtype: cand.subtype.clone(),
            optimization: cand.optimization.clone(),
            evidence,
            save_tokens,
            discount: cand.discount,
            savings_kind: cand.savings_kind.clone(),
            confidence: if v.confidence.is_empty() {
                "中".to_string()
            } else {
                v.confidence.clone()
            },
            needs_confirm: strategy.needs_confirm,
        });
    }

    // Biggest savings first (by tokens).
    items.sort_by_key(|item| std::cmp::Reverse(item.save_tokens));

    tracing::info!(
        "Cost: {}/{} candidates worth optimizing ({} strategies defined)",
        items.len(),
        considered,
        STRATEGIES.len()
    );

    Ok(WasteReport {
        items,
        considered,
        dismissed,
        model: candidates.model.clone(),
    })
}
