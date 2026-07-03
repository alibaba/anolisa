# AI Analysis

cosh-shell can automatically or on-demand invoke the AI adapter to analyze failure causes and provide suggestions when a command failure is detected.

## Analysis Modes

Switch via `/mode analysis <mode>` or configure `shell.analysis_mode`:

| Mode | Description |
|------|-------------|
| `smart` | Smart mode: severe errors show action cards, general errors show hints |
| `auto` | Auto mode: immediately auto-analyze on detected failure |
| `manual` | Manual mode: show action cards, wait for user confirmation before analysis |

## Failure Classification

cosh-shell performs semantic analysis on command exit codes and output, classifying failures:

| Classification | Example | Description |
|---------------|---------|-------------|
| `CommandNotFound` | `command not found` | Command does not exist |
| `PermissionDenied` | `Permission denied` | Insufficient permissions |
| `BuildOrTestFailure` | `error[E0308]` | Compilation/test error |
| `AbnormalSignal` | SIGSEGV | Abnormal signal termination |
| `GenericRuntimeFailure` | Non-zero exit code | General runtime error |
| `UsageOrHelp` | `Usage:` output | Usage error |
| `UnknownFailure` | Other | Unclassified failure |

The following classifications are considered "not real failures" and do not trigger analysis:
- `Success` — Actually succeeded
- `InteractiveCancel` — User actively cancelled
- `UserInterrupt` — Ctrl+C
- `PipelineNormal` — Normal pipeline exit code
- `ProviderOrInternalArtifact` — Exit code from internal tools

## Analysis Disposition Matrix

| Failure Classification | Auto Mode | Smart Mode | Manual Mode |
|-----------------------|-----------|------------|-------------|
| CommandNotFound / PermissionDenied / AbnormalSignal / BuildOrTestFailure | Auto-analyze | Action card | Action card |
| GenericRuntimeFailure | Auto-analyze | Hint | Action card |
| UnknownFailure | Action card | Hint | Hint |
| UsageOrHelp | Hint | Silent | Silent |

Disposition type descriptions:
- **Auto-analyze** — Immediately invoke AI adapter for analysis
- **Action card** — Render interactive card, user can choose "Analyze" or "Skip"
- **Hint** — Display brief hint, user can trigger analysis via slash command
- **Silent** — Log only, do not disturb user

## Analysis Flow

```
Command execution failed (exit code ≠ 0)
       │
       ▼
  Failure semantic classification
       │
       ▼
  Disposition decision (based on analysis mode)
       │
       ├── AutoAnalyze → Directly start Agent analysis
       ├── ActionCard  → Render action card → Wait for user confirmation
       ├── Hint        → Display brief hint
       └── SilentRecord → Silent log
```

## Agent Analysis Process

1. Collect failure context: command text, exit code, output excerpt (up to 8KB)
2. Construct prompt and send to AI adapter (cosh-core)
3. Adapter streams back analysis results
4. cosh-shell renders analysis content in Markdown format
5. User can Ctrl+C to cancel during analysis

## Configuration

```toml
[shell]
# Analysis mode: smart | auto | manual
analysis_mode = "smart"
```

Runtime switching:

```
/mode analysis smart
/mode analysis auto
/mode analysis manual
```
