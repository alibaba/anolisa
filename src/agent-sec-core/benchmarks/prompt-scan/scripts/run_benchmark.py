#!/usr/bin/env python3
"""
Prompt Scan Benchmark Runner

加载 PromptScanner 模型并扫描所有样本，扫描完成后生成中文 Markdown 分析报告。

Usage:
    # 完整流程：扫描 + 生成报告
    python3 run_benchmark.py [--mode MODE]

    # 仅扫描指定数据集（不生成报告）
    python3 run_benchmark.py <dataset.jsonl> <output_results.jsonl> [mode]

Requires: run with the agent-sec-cli venv or with the src on sys.path.
"""

import argparse
import json
import sys
import time
from collections import defaultdict
from pathlib import Path

from agent_sec_cli.prompt_scanner import (
    PromptScanner,
    ScanMode,
    ScanResult,
)

DEFAULT_MODE = "standard"

# Fields copied from sample into every result record
_SAMPLE_FIELDS = ("id", "label", "language", "attack_type", "sub_type", "source")


def compute_metrics(records: list) -> dict:
    """Compute TP/FN/FP/TN and derived metrics from a list of result records.

    Error records (verdict == "error") are excluded from all counts.

    NOTE: The ``ok`` field in ScanResult means "the input passed the scan (safe)".
    Therefore for attack samples:  ok=False → threat detected → TP
                                   ok=True  → threat missed   → FN
    For benign samples:            ok=False → false alarm     → FP
                                   ok=True  → correctly passed → TN

    Returns a dict with keys: attack, benign, errors, tp, fn, fp, tn,
    recall, precision, f1, accuracy, balanced_accuracy, tnr.
    """
    attack = [r for r in records if r["label"] != "benign" and r["verdict"] != "error"]
    benign = [r for r in records if r["label"] == "benign" and r["verdict"] != "error"]
    errors = [r for r in records if r["verdict"] == "error"]

    # ok=False → flagged as threat; ok=True → passed as safe
    tp = sum(1 for r in attack if r["ok"] is False)  # attack flagged correctly
    fn = sum(1 for r in attack if r["ok"] is True)  # attack missed (passed as safe)
    fp = sum(1 for r in benign if r["ok"] is False)  # benign flagged incorrectly
    tn = sum(1 for r in benign if r["ok"] is True)  # benign passed correctly

    recall = tp / (tp + fn) if (tp + fn) > 0 else 0
    precision = tp / (tp + fp) if (tp + fp) > 0 else 0
    f1 = (
        2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0
    )
    accuracy = (tp + tn) / (tp + fn + fp + tn) if (tp + fn + fp + tn) > 0 else 0
    # Balanced Accuracy = (TPR + TNR) / 2; robust to class imbalance
    tnr = tn / (tn + fp) if (tn + fp) > 0 else 0  # specificity / benign pass-rate
    balanced_accuracy = (recall + tnr) / 2

    return dict(
        attack=attack,
        benign=benign,
        errors=errors,
        tp=tp,
        fn=fn,
        fp=fp,
        tn=tn,
        recall=recall,
        precision=precision,
        f1=f1,
        accuracy=accuracy,
        balanced_accuracy=balanced_accuracy,
        tnr=tnr,
    )


BASE_DIR = Path(__file__).resolve().parent.parent
DATASETS_DIR = BASE_DIR / "datasets"
RESULTS_DIR = BASE_DIR / "results"
REPORTS_DIR = BASE_DIR / "reports"


def _sample_fields(sample: dict) -> dict:
    """Extract the fields from a sample that should appear in every result record."""
    return {k: sample.get(k, "") for k in _SAMPLE_FIELDS}


def result_to_record(sample: dict, scan_result: ScanResult) -> dict:
    """Convert a ScanResult to our benchmark record format."""
    d = scan_result.to_dict()
    return {
        **_sample_fields(sample),
        "verdict": d["verdict"],
        "ok": d["ok"],
        "risk_level": d["risk_level"],
        "threat_type": d["threat_type"],
        "confidence": d.get("confidence", 0),
        "elapsed_ms": d["elapsed_ms"],
        "findings_count": len(d["findings"]),
    }


def run_benchmark(dataset_file: str, results_file: str, mode_str: str = DEFAULT_MODE):
    """Run benchmark: scan all samples and write results.

    Args:
        dataset_file: Path to input JSONL dataset.
        results_file: Path to output JSONL results.
        mode_str: Scan mode (fast / standard / strict).
    """
    # Parse mode
    try:
        scan_mode = ScanMode(mode_str.lower())
    except ValueError:
        print(f"Error: invalid mode '{mode_str}'. Use: fast, standard, strict")
        sys.exit(1)

    # Load dataset
    samples = []
    dataset_version = None
    with open(dataset_file, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                record = json.loads(line)
                # Skip metadata lines (no "text" field or _type == "dataset_metadata")
                if record.get("_type") == "dataset_metadata" or "text" not in record:
                    # Capture dataset version from metadata for provenance tracking
                    if record.get("_type") == "dataset_metadata":
                        dataset_version = record.get("version")
                    continue
                samples.append(record)

    total = len(samples)
    print(f"[prompt-scan benchmark]")
    print(f"  Dataset : {dataset_file} ({total} samples)")
    if dataset_version:
        print(f"  Version : {dataset_version}")
    print(f"  Mode    : {scan_mode.value}")
    print(f"  Output  : {results_file}")

    # Initialize scanner (loads model once)
    print(f"  Loading model...", end=" ", flush=True)
    t_load = time.time()
    scanner = PromptScanner(mode=scan_mode)
    # Warm up with a dummy scan to trigger lazy model loading.
    # NOTE: When invoked via `make benchmark-prompt-scan`, the Makefile already runs
    # `agent-sec-cli scan-prompt warmup` beforehand to cache the model. This in-process
    # warm-up is kept intentionally so that the script also works correctly when called
    # directly (e.g. `python3 run_benchmark.py`), bypassing the Makefile target.
    _ = scanner.scan("warmup test")
    load_time = time.time() - t_load
    print(f"done ({load_time:.1f}s)")

    # Ensure output directory exists
    Path(results_file).parent.mkdir(parents=True, exist_ok=True)

    # Scan all samples
    results = []
    errors = 0
    start_all = time.time()

    with open(results_file, "w", encoding="utf-8") as out:
        for i, sample in enumerate(samples):
            text = sample["text"]
            try:
                scan_result = scanner.scan(text)
                record = result_to_record(sample, scan_result)
            except Exception as e:
                record = {
                    **_sample_fields(sample),
                    "verdict": "error",
                    # NOTE: ok=None for error records; downstream code uses
                    # `is False`/`is True` checks which correctly skip None.
                    "ok": None,
                    "risk_level": "",
                    "threat_type": "",
                    "confidence": 0,
                    "elapsed_ms": 0,
                    "findings_count": 0,
                    "error": str(e)[:200],
                }
                errors += 1

            # Embed dataset version for provenance tracking in reports
            if dataset_version:
                record["dataset_version"] = dataset_version

            results.append(record)
            out.write(json.dumps(record, ensure_ascii=False) + "\n")

            # Progress indicator
            if (i + 1) % 50 == 0 or (i + 1) == total:
                elapsed = time.time() - start_all
                rate = (i + 1) / elapsed
                print(
                    f"  [{i+1}/{total}] {rate:.1f} samples/s | elapsed {elapsed:.1f}s"
                )

    total_time = time.time() - start_all

    # Summary
    detects = sum(1 for r in results if r["ok"] is False)
    passes = sum(1 for r in results if r["ok"] is True)

    m = compute_metrics(results)
    tp, fn, fp = m["tp"], m["fn"], m["fp"]
    recall, precision, f1 = m["recall"], m["precision"], m["f1"]
    balanced_accuracy = m["balanced_accuracy"]
    errors_count = len(m["errors"])

    print(f"\n{'='*50}")
    print(
        f"  Completed: {total} samples in {total_time:.1f}s ({total/total_time:.1f} samples/s)"
    )
    print(f"  Detected: {detects} | Passed: {passes} | Errors: {errors_count}")
    print(
        f"  Recall: {recall:.1%} ({tp}/{tp+fn}) | Precision: {precision:.1%} | F1: {f1:.3f}"
    )
    print(f"  Balanced Accuracy: {balanced_accuracy:.1%}")
    print(f"{'='*50}")


def generate_report(results_file: str, report_file: str):
    """Generate an HTML analysis report from benchmark results."""
    # Load results
    records = []
    dataset_version = None
    with open(results_file, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                r = json.loads(line)
                # Pick up dataset version embedded in result records (if any)
                if dataset_version is None and r.get("dataset_version"):
                    dataset_version = r["dataset_version"]
                records.append(r)

    # Compute metrics via shared helper
    m = compute_metrics(records)
    attack = m["attack"]
    benign = m["benign"]
    errors = m["errors"]
    tp, fn, fp, tn = m["tp"], m["fn"], m["fp"], m["tn"]
    recall, precision, f1, accuracy = (
        m["recall"],
        m["precision"],
        m["f1"],
        m["accuracy"],
    )
    balanced_accuracy = m["balanced_accuracy"]

    # Per-source breakdown
    by_source = defaultdict(list)
    for r in attack:
        src = r.get("source", "unknown")
        if "self-constructed" in src:
            src = "self-constructed"
        by_source[src].append(r)

    # Per sub_type breakdown
    by_sub = defaultdict(list)
    for r in attack:
        by_sub[r.get("sub_type", "unknown")].append(r)

    # FN details (missed attacks)
    fn_records = [r for r in attack if r["ok"] is True]
    fn_by_sub = defaultdict(list)
    for r in fn_records:
        fn_by_sub[r.get("sub_type", "unknown")].append(r["id"])

    # FP details (false alarms)
    fp_records = [r for r in benign if r["ok"] is False]

    # ── Build per-source rows ────────────────────────────────────────────────
    source_rows = []
    for src in sorted(by_source.keys()):
        items = by_source[src]
        s_tp = sum(1 for r in items if r["ok"] is False)
        s_fn = sum(1 for r in items if r["ok"] is True)
        s_recall = s_tp / (s_tp + s_fn) if (s_tp + s_fn) > 0 else 0
        source_rows.append(
            {
                "name": src,
                "total": len(items),
                "detected": s_tp,
                "missed": s_fn,
                "recall": round(s_recall * 100, 1),
            }
        )
    source_data_js = json.dumps(source_rows, ensure_ascii=False)

    # ── Build per-subtype rows (sorted by total desc) ────────────────────────
    subtype_rows = []
    for sub in sorted(by_sub.keys(), key=lambda x: -len(by_sub[x])):
        items = by_sub[sub]
        s_tp = sum(1 for r in items if r["ok"] is False)
        s_fn = sum(1 for r in items if r["ok"] is True)
        s_recall = s_tp / (s_tp + s_fn) if (s_tp + s_fn) > 0 else 0
        subtype_rows.append(
            {
                "name": sub,
                "total": len(items),
                "detected": s_tp,
                "missed": s_fn,
                "recall": round(s_recall * 100, 1),
            }
        )
    subtype_data_js = json.dumps(subtype_rows, ensure_ascii=False)

    # ── Build FN items ────────────────────────────────────────────────────────
    fn_items = []
    for sub in sorted(fn_by_sub.keys()):
        ids = fn_by_sub[sub]
        preview = ", ".join(ids[:10]) + ("..." if len(ids) > 10 else "")
        fn_items.append({"name": sub, "count": len(ids), "ids": preview})
    fn_data_js = json.dumps(fn_items, ensure_ascii=False)

    # ── Build FP items ────────────────────────────────────────────────────────
    fp_items_html = ""
    for r in fp_records:
        fp_items_html += (
            f'<div class="fp-card">'
            f'<div class="fp-badge">{r["id"]}</div>'
            f'<div class="fp-divider"></div>'
            f'<div class="fp-type">{r.get("sub_type", "N/A")}</div>'
            f"</div>\n"
        )

    version_str = dataset_version or "N/A"
    total_samples = tp + fn + fp + tn

    # ── Load HTML template ────────────────────────────────────────────────────
    template_file = Path(__file__).resolve().parent / "benchmark_report_template.html"
    template = template_file.read_text(encoding="utf-8")

    # ── Fill in placeholders ──────────────────────────────────────────────────
    html = (
        template.replace("{{VERSION}}", version_str)
        .replace("{{RECALL}}", f"{recall:.1%}")
        .replace("{{PRECISION}}", f"{precision:.1%}")
        .replace("{{F1}}", f"{f1:.3f}")
        .replace("{{BALANCED_ACC}}", f"{balanced_accuracy:.1%}")
        .replace("{{ACCURACY}}", f"{accuracy:.1%}")
        .replace("{{TP}}", str(tp))
        .replace("{{FN}}", str(fn))
        .replace("{{FP}}", str(fp))
        .replace("{{TN}}", str(tn))
        .replace("{{TP_FN}}", str(tp + fn))
        .replace("{{TP_FP}}", str(tp + fp))
        .replace("{{TP_TN}}", str(tp + tn))
        .replace("{{TOTAL}}", str(total_samples))
        .replace("{{TP_JS}}", str(tp))
        .replace("{{FN_JS}}", str(fn))
        .replace("{{FP_JS}}", str(fp))
        .replace("{{TN_JS}}", str(tn))
        .replace("{{SOURCE_DATA_JS}}", source_data_js)
        .replace("{{SUBTYPE_DATA_JS}}", subtype_data_js)
        .replace("{{FN_DATA_JS}}", fn_data_js)
        .replace("{{FP_ITEMS_HTML}}", fp_items_html)
    )

    # Write report
    Path(report_file).parent.mkdir(parents=True, exist_ok=True)
    with open(report_file, "w", encoding="utf-8") as f:
        f.write(html)

    print(f"Report saved to: {report_file}")
    return html


def main():
    parser = argparse.ArgumentParser(
        description="Run prompt-scan benchmark and generate report"
    )
    parser.add_argument(
        "--mode",
        default=DEFAULT_MODE,
        choices=["fast", "standard", "strict"],
        help=f"Scan mode (default: {DEFAULT_MODE})",
    )
    args = parser.parse_args()

    # Ensure output directories exist
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    REPORTS_DIR.mkdir(parents=True, exist_ok=True)

    zh_dataset = DATASETS_DIR / "prompt_injection_zh.jsonl"
    zh_results = RESULTS_DIR / "prompt_injection_zh.jsonl"
    zh_report = REPORTS_DIR / "benchmark_zh.html"

    print("=" * 42)
    print(" Prompt Scan Benchmark - Chinese Dataset")
    print("=" * 42)
    print()

    # Run benchmark
    print("Scanning Chinese dataset...")
    run_benchmark(str(zh_dataset), str(zh_results), args.mode)
    print()

    # Generate report
    print("Generating report...")
    generate_report(str(zh_results), str(zh_report))
    print()

    print("=" * 42)
    print(" Done!")
    print(f"  Results: {zh_results.name}")
    print(f"  Report:  {zh_report.name}  (open in browser)")
    print("=" * 42)


if __name__ == "__main__":
    main()
