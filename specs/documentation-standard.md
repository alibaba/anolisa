# ANOLISA Documentation Standard

> Canonical reference for documentation structure, naming, bilingual conventions, and maintenance rules.
> Both human contributors and AI agents MUST follow this specification.

## Agent Reading Order

When an agent starts working on this repository:

1. `specs/` — normative rules (this directory)
2. `AGENTS.md` — development conventions and coding standards
3. `src/<component>/AGENTS.md` — scoped module rules (if working on that component)
4. `src/<component>/README.md` — component context

**Source of truth hierarchy:**

- Code > README/user-guide > design docs
- Before generating or modifying documentation: read source code, verify CLI examples against `clap`/`argparse` definitions, never document unimplemented features as available

---

## 1. Bilingual Naming Convention

| Scope | English (default) | Chinese | Notes |
|-------|-------------------|---------|-------|
| Repo root / component root | `FILE.md` | `FILE_zh.md` | No suffix = English |
| `docs/` standalone pages | `docs/FILE.md` | `docs/FILE_zh.md` | e.g. QUICKSTART, BUILDING |
| `docs/` long-form guides | `docs/{type}/en/` | `docs/{type}/zh/` | Directory-based separation |

**Cross-reference convention:**

- English file header: `[中文版](FILE_zh.md)`
- Chinese file header: `[English](FILE.md)`

**Prohibited patterns:**

- `*_CN.md` — legacy naming, must migrate to `*_zh.md`
- `*_en.md` — English is the default, no suffix needed

## 2. Repository Root Files

### 2.1 Required Files

| File | Bilingual | Purpose |
|------|-----------|---------|
| `README.md` | Yes (`README_zh.md`) | Project overview, component table, quick start, documentation index |
| `AGENTS.md` | English only | AI agent global constraints and development conventions |
| `LICENSE` | English only | Apache License 2.0 |
| `CONTRIBUTING.md` | Yes (`CONTRIBUTING_zh.md`) | General contribution guide: environment, commit rules, PR flow |
| `CHANGELOG.md` | Yes (`CHANGELOG_zh.md`) | Version changelog with component version composition |
| `CODE_OF_CONDUCT.md` | English only | Contributor Covenant (standard text) |
| `SECURITY.md` | English only | Vulnerability reporting process |
| `NOTICE` | English only | Derived works and dependency attribution |

### 2.2 File Format References

| File | Format standard | Root-specific rule |
|------|----------------|-------------------|
| README | First sentence = one-line positioning; then 2–4 sentences expanding scope | Root adds component table + documentation index |
| CONTRIBUTING | Structured sections: prerequisites, build, test, PR checklist | Root covers general flow; component covers local build/test only |
| CHANGELOG | [Keep a Changelog](https://keepachangelog.com/) format (Added / Changed / Fixed) | Root MUST prepend a "Component Versions" table before highlights |

**CHANGELOG writing rules:**

1. Each user-perceivable change requires an entry in the affected component's CHANGELOG
2. One sentence per bullet — max 25 English words / 40 Chinese characters
3. User perspective — describe the behavior change, not the code change
4. No internal jargon — command names and config keys are fine; kernel APIs and syscalls are not
5. One bullet, one change — do not combine unrelated changes
6. Skip invisible changes — pure refactors, test infra, CI tweaks do not belong
7. Key entries should reference the implementing PR

**Root CHANGELOG structural requirement** — each version entry starts with:

```markdown
## [0.3.0] - 2026-08-01

### Component Versions

| Component | Version |
|-----------|--------|
| copilot-shell | 2.2.0 |
| agent-sec-core | 0.5.0 |
| ...

### Highlights
- ...
```

Chinese version (`CHANGELOG_zh.md`) mirrors the same structure.

## 3. docs/ Directory Structure

```
docs/
├── QUICKSTART.md              # Cross-component quick start (English)
├── QUICKSTART_zh.md           # Cross-component quick start (Chinese)
├── BUILDING.md                # Build from source (English)
├── BUILDING_zh.md             # Build from source (Chinese)
├── user-guide/
│   ├── en/                    # User manual (English)
│   │   ├── README.md          # Index page
│   │   ├── installation.md
│   │   └── {capability}/      # Organized by capability domain
│   └── zh/                    # User manual (Chinese, mirrors en/)
│       └── ...
└── developer-guide/
    ├── en/                    # Developer documentation (English)
    │   └── {component}/       # Organized by component name
    └── zh/                    # Developer documentation (Chinese)
        └── ...
```

### 3.1 What Goes Where

| Content type | Location | NOT here |
|-------------|----------|----------|
| User-facing how-to | `docs/user-guide/{en,zh}/` | Component root |
| Developer architecture / IPC / hooks | `docs/developer-guide/{en,zh}/` | Component root |
| Component design docs | `src/<component>/docs/` | `docs/` top level |
| Cross-component quick start | `docs/QUICKSTART.md` | README |
| Build instructions | `docs/BUILDING.md` | README |

### 3.2 Design Documents

Design documents live **only** in the component's own directory:

```
src/<component>/docs/design/    ← component-specific design docs
```

Design docs are **never** placed at:
- Repository root
- `docs/` top level
- `docs/user-guide/` or `docs/developer-guide/`

## 4. Component-Level Files

### 4.1 Required Files (every component)

| File | Bilingual | Purpose |
|------|-----------|---------|
| `README.md` | Yes (`README_zh.md`) | Entry point: positioning, use cases, install, usage, relationships |
| `CHANGELOG.md` | Yes (`CHANGELOG_zh.md`) | User-perceivable changes per release |

### 4.2 Optional Files

| File | Bilingual | When needed | Rule |
|------|-----------|-------------|------|
| `CONTRIBUTING.md` | **If exists, `CONTRIBUTING_zh.md` is also required** | When component has specific build/test/lint process | Must NOT repeat root-level general rules |
| `AGENTS.md` | English only | When component has non-trivial AI-agent constraints | Specific to module scope |
| `LICENSE` | English only | When component uses a different license than root | e.g. MIT sub-component |
| `docs/` | — | For design documents, internal protocol specs | NOT for user-guide content |

### 4.3 Files NOT Needed at Component Level

These are inherited from the repository root:

- `CODE_OF_CONDUCT.md`
- `SECURITY.md`
- `NOTICE` (maintained centrally at root)

### 4.4 README Opening Paragraph Convention

Every component `README.md` MUST open with:

1. **First sentence**: one-line positioning statement (reusable verbatim in indexes and tables)
2. **Remainder of paragraph**: 2–4 sentences expanding scope, differentiators, and target users

Example:
```markdown
# AgentSight

[中文版](README_zh.md)

eBPF-based observability tool for AI Agents on Linux, providing zero-intrusion
monitoring of LLM API calls, token consumption, and process behavior.
AgentSight captures kernel-level events without modifying agent code...
```

### 4.5 Differentiation from Root Level

| Dimension | Root | Component |
|-----------|------|-----------|
| README | Panoramic — what project, what problem, what's included, how to start | Focused — what this component does, who uses it, how to install & use |
| CONTRIBUTING | General — environment, commit format, PR flow, branch naming | Specific — this component's build/test/lint commands, special deps |
| CHANGELOG | Aggregated — version composition + highlights, links to component CHANGELOGs | Detailed — every user-perceivable entry for this component |

### 4.6 User Guide Writing Standards

**Installation priority** (all component docs MUST follow this order):

1. `anolisa install <component>` — always first (agentsight / agent-sec-core require `sudo` system mode)
2. RPM package (`yum install`) — alternative for Alinux users
3. Source build — developers only, always last

**Content boundaries:**

- Only document components whose source code exists in `src/`. No code = no docs.
- Cloud-specific configuration (SLS endpoints, AK/SK auth, security groups) belongs to cloud vendor docs
- Never document planned-but-unimplemented features as available

**Framing:**

- Open with a value proposition ("why install this?"), not architecture description
- Cross-component integration stories belong in user-guide (e.g. "install AgentSight + Tokenless, savings appear in Dashboard")

**Language rules for bilingual docs:**

- `en/` and `zh/` MUST be semantically equivalent
- Technical terms keep English form in Chinese docs (eBPF, Token, CLI)
- Command examples identical across languages; only prose differs

**Entry point convention:** each component directory in `docs/user-guide/` uses `QUICKSTART.md` as its entry point.

## 5. PR Documentation Update Rules

When a PR introduces any of the following changes, documentation MUST be updated in the same PR:

| Change type | Required doc update |
|-------------|-------------------|
| New/modified CLI command or flag | Component README + user-guide |
| New/modified config option | Component README + user-guide |
| User-perceivable feature or fix | Component CHANGELOG |
| Installation method change | docs/QUICKSTART + component README |
| Architecture or protocol change | Component `docs/design/` |
| New component added | Root README + NOTICE (if applicable) |

---

*Last updated: 2026-07-04*
