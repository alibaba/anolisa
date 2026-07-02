use anyhow::Result;
use clap::{Parser, Subcommand};
use ktuner_engine::{
    self as engine, category, detect, evaluate, rules, services, tuner, Recommendation,
};
use serde_json::json;

#[derive(Parser)]
#[command(name = "ktuner", version, about = "Deterministic kernel-tuning engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Diagnose system and output tuning recommendations
    Check {
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        conservative: bool,
    },
    /// Apply tuning recommendations
    Tune {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        conservative: bool,
        #[arg(long)]
        category: Option<String>,
    },
    /// Fix a single parameter
    Fix { param: String },
    /// Explain why a parameter should be changed
    Why { param: String },
    /// Roll back all applied changes
    Rollback,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Check {
            category: cat,
            conservative,
        } => cmd_check(cat, conservative),
        Commands::Tune {
            dry_run,
            conservative,
            category: cat,
        } => cmd_tune(dry_run, conservative, cat),
        Commands::Fix { param } => cmd_fix(&param),
        Commands::Why { param } => cmd_why(&param),
        Commands::Rollback => cmd_rollback(),
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            let out = json!({ "error": format!("{e:#}") });
            eprintln!("{}", serde_json::to_string_pretty(&out).unwrap());
            std::process::exit(2);
        }
    }
}

fn cmd_check(cat: Option<String>, conservative: bool) -> Result<i32> {
    if let Some(ref c) = cat {
        category::validate_category(c)?;
    }
    let info = detect::gather_system_info()?;
    let eval = evaluate(&info)?;
    let workload = engine::classify(&info);
    let runtime_env = detect::detect_runtime_env();
    let detected_services = services::detect_services(&info);

    let mut recs = eval.recommendations.clone();
    if let Some(ref c) = cat {
        recs = category::filter_by_category(recs, c);
    }
    if conservative {
        recs.retain(|r| r.confidence == rules::Confidence::High);
    }

    let score = eval.score();
    let counts = category::RecCounts::from_recs(&recs);
    let total_weight: usize = recs
        .iter()
        .map(|r| match r.confidence {
            rules::Confidence::High => 3,
            rules::Confidence::Medium => 2,
        })
        .sum();
    let predicted_score = (score + total_weight).min(100);

    let recs_json: Vec<serde_json::Value> = recs
        .iter()
        .map(|r| {
            json!({
                "param": r.param,
                "current": r.current_value,
                "recommended": r.recommended_value,
                "reason": r.reason,
                "confidence": format!("{:?}", r.confidence).to_lowercase(),
                "category": format!("{:?}", r.category).to_lowercase(),
                "subcategory": category::param_subcategory(&r.param),
                "writable": r.writable,
            })
        })
        .collect();

    let output = json!({
        "score": score,
        "predicted_score": predicted_score,
        "total_checked": eval.total_checked,
        "recommendations": recs_json,
        "counts": {
            "performance": counts.perf,
            "security": counts.sec,
            "high_confidence": counts.high,
            "writable": counts.writable,
        },
        "system": {
            "kernel": info.kernel_version,
            "cpu_cores": info.cpu_cores,
            "memory_gb": info.memory_total_gb,
            "numa_nodes": info.numa_nodes,
        },
        "environment": format!("{runtime_env}"),
        "workload": format!("{workload}"),
        "services": detected_services,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);

    let code = if recs.is_empty() { 0 } else { 1 };
    Ok(code)
}

fn cmd_tune(dry_run: bool, conservative: bool, cat: Option<String>) -> Result<i32> {
    if !dry_run {
        let is_root = unsafe { libc::geteuid() } == 0;
        if !is_root {
            anyhow::bail!("tune requires root (sudo ktuner tune)");
        }
    }

    if let Some(ref c) = cat {
        category::validate_category(c)?;
    }
    let (_info, eval) = gather()?;
    let score_before = eval.score();
    let mut recs = eval.recommendations;
    if let Some(ref c) = cat {
        recs = category::filter_by_category(recs, c);
    }
    if conservative {
        recs.retain(|r| r.confidence == rules::Confidence::High);
    }
    recs.retain(|r| r.writable && !category::is_runtime_dangerous(&r.param));

    if recs.is_empty() {
        let output = json!({ "status": "optimal", "applied": 0 });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(0);
    }

    if dry_run {
        let recs_json: Vec<serde_json::Value> = recs.iter().map(rec_json).collect();
        let output = json!({ "dry_run": true, "would_apply": recs_json });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(0);
    }

    let applied = tuner::apply_quiet(&recs)?;
    let (_, eval_after) = gather()?;
    let score_after = eval_after.score();

    let output = json!({
        "applied": applied,
        "score_before": score_before,
        "score_after": score_after,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(0)
}

fn cmd_fix(param: &str) -> Result<i32> {
    let is_root = unsafe { libc::geteuid() } == 0;
    if !is_root {
        anyhow::bail!("fix requires root (sudo ktuner fix {param})");
    }
    let (_, eval) = gather()?;
    let rec = eval
        .recommendations
        .iter()
        .find(|r| r.param == param)
        .ok_or_else(|| anyhow::anyhow!("parameter not found or already optimal: {param}"))?;
    if !rec.writable {
        anyhow::bail!("parameter {param} is read-only in this environment");
    }
    if category::is_runtime_dangerous(&rec.param) {
        anyhow::bail!(
            "parameter {param} is dangerous to write at runtime, persist to /etc/sysctl.d instead"
        );
    }
    tuner::apply_one(rec)?;
    let (_, eval_after) = gather()?;
    let output = json!({
        "fixed": param,
        "previous": rec.current_value,
        "applied": rec.recommended_value,
        "score_after": eval_after.score(),
        "remaining": eval_after.recommendations.len(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(0)
}

fn cmd_why(param: &str) -> Result<i32> {
    let (_, eval) = gather()?;
    let normalized = param.replace('/', ".").to_lowercase();
    if let Some(rec) = eval
        .recommendations
        .iter()
        .find(|r| r.param == param || r.param == normalized)
    {
        let output = json!({
            "param": rec.param,
            "current": rec.current_value,
            "recommended": rec.recommended_value,
            "reason": rec.reason,
            "confidence": format!("{:?}", rec.confidence).to_lowercase(),
            "category": format!("{:?}", rec.category).to_lowercase(),
            "subcategory": category::param_subcategory(&rec.param),
            "writable": rec.writable,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(1);
    }
    let path = tuner::param_to_path(&normalized);
    if std::path::Path::new(&path).exists() {
        let val = std::fs::read_to_string(&path).unwrap_or_default();
        let output = json!({ "param": normalized, "current": val.trim(), "status": "optimal" });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(0);
    }
    anyhow::bail!("parameter not found: {param}")
}

fn cmd_rollback() -> Result<i32> {
    let is_root = unsafe { libc::geteuid() } == 0;
    if !is_root {
        anyhow::bail!("rollback requires root (sudo ktuner rollback)");
    }
    let outcome = tuner::rollback_quiet()?;
    let status = tuner::classify_rollback(&outcome);
    let output = json!({
        "restored": outcome.restored,
        "failed": outcome.failed,
        "skipped": outcome.skipped,
        "status": format!("{status:?}"),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(0)
}

fn gather() -> Result<(detect::SystemInfo, rules::EvalResult)> {
    let info = detect::gather_system_info()?;
    let eval = evaluate(&info)?;
    Ok((info, eval))
}

fn rec_json(r: &Recommendation) -> serde_json::Value {
    json!({
        "param": r.param,
        "current": r.current_value,
        "recommended": r.recommended_value,
        "reason": r.reason,
        "confidence": format!("{:?}", r.confidence).to_lowercase(),
        "category": format!("{:?}", r.category).to_lowercase(),
    })
}
