# agent-sec-core Development Standards

本仓库包含多个组件，请根据你要修改的模块查阅对应章节：

| 组件 | 语言 | 路径 | 章节 |
|------|------|------|------|
| agent-sec-cli | Python + Rust | agent-sec-cli/ | [agent-sec-cli](#agent-sec-cli) |
| hermes-plugin | Python (stdlib) | hermes-plugin/ | [hermes-plugin](#hermes-plugin) |
| cosh-extension | Python (hooks) | cosh-extension/ | [cosh-extension](#cosh-extension) |
| openclaw-plugin | TypeScript | openclaw-plugin/ | [openclaw-plugin](#openclaw-plugin) |
| linux-sandbox | Rust | linux-sandbox/ | [linux-sandbox](#linux-sandbox) |
| skills | Shell/Python | skills/ | [skills](#skills) |

---

## agent-sec-cli

### 1. 项目概述

agent-sec-cli 是面向 AI Agent 的安全 CLI 工具，提供系统加固、沙箱策略生成、资产完整性验证、代码安全扫描、提示词安全检测和安全事件追踪等功能。

**关键目录结构：**

```
agent-sec-cli/
├── src/agent_sec_cli/        # 主 Python 包
│   ├── cli.py                # 统一 CLI 入口
│   ├── asset_verify/         # 资产完整性验证（GPG 签名）
│   ├── code_scanner/         # 代码安全扫描
│   ├── prompt_scanner/       # 提示词安全检测（ML 分类器）
│   ├── sandbox/              # 沙箱策略生成
│   ├── security_events/      # 安全事件日志
│   ├── security_middleware/  # 统一中间件层（路由+后端）
│   └── skill_ledger/         # 技能账本管理
├── src/lib.rs                # Rust 原生模块入口（PyO3）
├── pyproject.toml            # 构建配置 + lint/格式化配置
├── Cargo.toml                # Rust 依赖
└── uv.lock                   # 依赖锁定文件
tests/                        # 测试目录（位于 agent-sec-core/ 下）
├── unit-test/                # 单元测试
├── integration-test/         # 集成测试
└── e2e/                      # 端到端测试
```

### 2. 环境准备

- **Python 版本**: 严格固定 `3.11.6`（`pyproject.toml` 中 `requires-python = "==3.11.6"`）
- **包管理器**: [uv](https://docs.astral.sh/uv/)，管理依赖和虚拟环境
- **Rust 构建**: [maturin](https://www.maturin.rs/)，编译 PyO3 原生扩展为 `.so`
- **初始化环境**:

```bash
cd agent-sec-cli && uv sync
```

> uv 会自动创建 `.venv` 并安装所有依赖（含 dev group）。

### 3. 依赖管理

| 场景 | 命令 | 说明 |
|------|------|------|
| 安装所有依赖（含 dev） | `uv sync` | 自动创建 .venv 并安装 |
| 仅安装运行时依赖 | `uv sync --no-group dev` | 生产环境用 |
| 添加运行时依赖 | `uv add <pkg>` | 自动更新 pyproject.toml 和 uv.lock |
| 添加 dev 依赖 | `uv add --group dev <pkg>` | 写入 [dependency-groups].dev |
| 添加可选依赖 | `uv add --optional <group> <pkg>` | 写入 [project.optional-dependencies]，如 `uv add --optional pgpy pgpy` |
| 删除依赖 | `uv remove <pkg>` | 同时清理 pyproject.toml 和 uv.lock |
| 更新单个依赖 | `uv lock --upgrade-package <pkg>` | 仅升级指定包 |
| 更新所有依赖 | `uv lock --upgrade` | 重新解析所有版本 |
| 运行命令 | `uv run <cmd>` | 在 .venv 环境中执行 |
| 运行测试 | `make test-python` | 从 agent-sec-core 目录执行 |
| 构建 wheel | `make build-cli` | maturin + Python 3.11 |

> **重要**: 修改依赖后务必提交更新后的 `pyproject.toml` 和 `uv.lock`。

### 4. 代码格式化

使用 **black + isort** 进行代码格式化（配置在 `agent-sec-cli/pyproject.toml`）：

- `line-length = 100`
- `target-version = py311`
- `isort` profile = "black"

```bash
# 从 agent-sec-core 目录执行
make python-code-pretty
```

> 格式化排除 `dev-tools/backend-skill/templates/` 目录（含 Jinja 模板）。

### 5. 静态检查 (ruff lint)

使用 [ruff](https://docs.astral.sh/ruff/) 进行静态检查（仅 lint，不做格式化）。

**启用规则：**

| 规则 | 说明 |
|------|------|
| F | pyflakes — 未使用 import、未定义变量等逻辑错误 |
| E, W | pycodestyle — PEP 8 编码风格（E501 行超长已 ignore） |
| I | isort — import 排序 |
| TID252 | 禁止相对导入 |
| PLC0415 | 禁止函数体内导入 |
| ANN001 | 函数参数必须标注类型 |
| ANN201 | 公有函数必须标注返回类型 |
| ANN202 | 私有函数必须标注返回类型 |
| S602 | 禁止 subprocess shell=True |
| S605 | 禁止 os.system() |
| S606 | 禁止 os.popen() |
| S108 | 禁止硬编码 /tmp 路径 |
| PLW1510 | subprocess.run() 必须指定 check |
| SIM115 | open() 必须使用 with |
| B006 | 禁止可变默认参数 |
| B008 | 禁止默认参数中调用函数 |

**已禁用规则：**

| 规则 | 原因 |
|------|------|
| PTH (pathlib 强制) | 存量代码中 os.path 使用过多，暂不启用，待后续逐步治理 |
| E501 (行超长) | 由格式化工具自动处理 |

**豁免规则：**

| 作用范围 | 豁免规则 | 原因 |
|----------|----------|------|
| `tests/**` | ANN（类型注解） | 测试代码标注类型收益低 |
| `tests/**` | S（安全规则） | 测试需构造危险输入验证防护逻辑 |
| ML lazy import 行 | PLC0415 | torch/transformers 等重型依赖延迟加载，用 `# noqa: PLC0415` 豁免 |

**命令：**

```bash
# 全量检查（从 agent-sec-core 目录）
make python-lint

# 增量检查（仅报告相对 upstream/main 变更行的违规，含未提交修改）
make python-lint-ci

# 自定义对比分支
make python-lint-ci COMPARE_BRANCH=origin/main
```

> `python-lint-ci` 对比范围包含 committed + staged + unstaged 变更，无需先 commit。

### 6. 导入规范

- **绝对导入**: 所有 import 使用绝对路径 `from agent_sec_cli.xxx import yyy`
- **禁止相对导入**: `from .xxx import` 或 `from ..xxx import` 一律禁止
- **禁止动态导入**: `importlib.import_module()` 和 `__import__()` 禁止使用
- **禁止函数体内导入**: 所有 import 必须在文件头部

**例外 — ML 延迟加载：** 对于重型 ML 依赖（torch、transformers、modelscope），允许在实际推理时才导入，需添加行内注释：

```python
def predict(self, text: str) -> float:
    import torch  # noqa: PLC0415 - lazy import: only needed when running ML inference
    from transformers import AutoModel  # noqa: PLC0415
    ...
```

### 7. 类型注解

- 所有函数/方法必须标注**参数类型**和**返回类型**
- 使用 Python 3.11 原生语法：`dict[str, Any]`、`str | None`、`list[int]`
- 无需 `from __future__ import annotations`
- `tests/` 目录下所有文件豁免类型注解要求

```python
# 正确
def process(name: str, count: int, items: list[str]) -> dict[str, Any]:
    ...

# 错误 — 缺少类型标注
def process(name, count, items):
    ...
```

### 8. 编码风格

**通用规范：**

- 空函数/抽象方法使用 `pass` 占位，不使用 `...`（Ellipsis）
- 数据类优先使用 `pydantic`
- 路径操作优先使用 `pathlib.Path`，而非 `os.path`
- 禁止使用可变对象（`[]`、`{}`、`set()`）作为函数默认参数（B006）
- 禁止在默认参数中调用函数（B008），如 `def f(x=time.time())` 是错误写法

**Import 规范：**

- import 排序由 isort 自动管理（I）
- 禁止相对导入（TID252）：使用 `from agent_sec_cli.xxx import yyy`
- 禁止函数体内导入（PLC0415）：所有 import 放在文件顶部

**类型标注：**

- 函数参数必须标注类型（ANN001）
- 公有函数必须标注返回类型（ANN201）
- 私有函数必须标注返回类型（ANN202）

**安全规范：**

- 禁止 `subprocess` 使用 `shell=True`（S602）
- 禁止使用 `os.system()`（S605）
- 禁止使用 `os.popen()`（S606）
- 禁止硬编码 `/tmp` 路径（S108），应使用 `tempfile` 模块
- `subprocess.run()` 必须显式指定 `check` 参数（PLW1510）
- `open()` 必须使用 `with` 上下文管理器（SIM115）

### 9. 测试

- **框架**: pytest
- **测试目录结构**:
  - `tests/unit-test/` — 单元测试
  - `tests/integration-test/` — 集成测试
  - `tests/e2e/` — 端到端测试
- **测试文件放置**: 统一放在 `tests/` 目录下，不放入 `agent-sec-cli/` 内部
- **e2e 测试要求**: 必须同时支持两种调用方式：
  1. **二进制 CLI 调用**（subprocess）：`subprocess.run(["agent-sec-cli", "scan-code", "--code", code, "--language", "bash"], ...)`
  2. **Python 模块回退**：`subprocess.run(["python", "-m", "agent_sec_cli.cli", "scan-code", ...], ...)`

  两种方式均以字符串数组传参（不经 shell 解析），保障参数完整性。

**常用命令（从 agent-sec-core 目录执行）：**

```bash
make test-python           # 运行单元 + 集成 + CLI e2e 测试
make test-python-coverage  # 运行测试并生成覆盖率报告
```

### 10. 构建

```bash
make build-cli             # 构建 wheel（maturin + Python 3.11）
make export-requirements   # 从 uv.lock 导出 requirements.txt
```

- Rust 原生扩展通过 PyO3 编译为 `_native.cpython-311-*.so`，随 wheel 分发
- 构建产物位于 `agent-sec-cli/target/wheels/`
- **非 .py 文件打包**: 新增的非 Python 文件（如 `.yaml`、`.conf`、`.asc`、`.json` 等）如果需要随 wheel 分发，必须在 `pyproject.toml` 的 `[tool.maturin].include` 中添加对应路径：

```toml
[tool.maturin]
include = [
    "src/agent_sec_cli/asset_verify/config.conf",
    "src/agent_sec_cli/asset_verify/trusted-keys/*.asc",
    "src/agent_sec_cli/code_scanner/rules/**/*.yaml",
    "src/agent_sec_cli/prompt_scanner/rules/*.yaml",
    # 新增资源文件在此添加
]
```

### 11. CI 检查项

| 检查项 | 范围 | 失败行为 |
|--------|------|----------|
| black + isort 格式化 | 全量代码 | 存在未格式化代码则 CI 失败 |
| ruff lint（增量） | 仅 PR 变更行 | **不卡点**，违规以 warning 显示在 CI Summary |
| pytest --cov | 全量测试 | 测试失败则 CI 失败 |
| 增量代码覆盖率 | 仅 PR 变更行 | 新增/修改代码覆盖率 < 80% 则 CI 失败 |
| uv lock --check | 依赖锁文件 | uv.lock 与 pyproject.toml 不同步则 CI 失败 |

> Lint 检查仅在 PR 触发时对增量代码检查，不检查历史代码。违规信息显示在 PR 的 Job Summary 区域。
> 增量覆盖率门禁仅在 PR 触发，要求本次 PR 新增/修改的代码行中被测试覆盖的比例 ≥ 80%。

---

## hermes-plugin

### 1. 项目概述

hermes-plugin 是面向 [Hermes Agent](https://hermes-agent.nousresearch.com/) 的安全插件，通过 Hook 机制拦截危险操作，底层调用 agent-sec-cli 进行安全扫描。

**设计原则：**

- **Fail-open** — 任何异常都不阻塞 agent 运行，hook 内部捕获所有异常返回 `None` 放行
- **零运行时依赖** — 仅使用 Python 3.11 标准库（tomllib、json、subprocess、logging、dataclasses）
- **可配置行为** — 默认 observe（仅日志），需显式 `enable_block = true` 才阻断

**目录结构：**

```
hermes-plugin/
├── scripts/
│   └── deploy.sh             # 部署脚本
├── src/                      # 运行时文件（部署到 ~/.hermes/plugins/）
│   ├── plugin.yaml           # Hermes 插件 manifest
│   ├── __init__.py           # register(ctx) 入口
│   ├── config.toml           # 能力开关与参数
│   ├── registry.py           # 能力注册器 + safe-wrap
│   ├── cli_runner.py         # agent-sec-cli subprocess 封装
│   └── capabilities/
│       ├── __init__.py       # 能力清单
│       ├── base.py           # AgentSecCoreCapability 抽象基类
│       ├── code_scan.py      # Code Scanner 实现
│       └── pii_scan.py       # PII Checker 实现
└── README.md                 # 开发指南
tests/unit-test/hermes-plugin/ # 单元测试（位于 agent-sec-core/tests/unit-test/ 下）
```

### 2. 导入规范

Hermes 以包形式加载插件，模块间**必须使用相对导入**：

```python
# 正确：相对导入
from .registry import load_config              # 同级模块
from .capabilities import ALL_CAPABILITIES     # 同级子包
from ..cli_runner import call_agent_sec_cli    # 上级模块（在子包中）

# 错误：裸名导入（插件目录不在 sys.path）
# from registry import load_config
```

**依赖分层（无循环依赖）：**

- 底层：`cli_runner.py`（纯 stdlib，无内部依赖）
- 中间层：`registry.py`（纯 stdlib）
- 基类层：`capabilities/base.py`（依赖 registry）
- 实现层：`capabilities/*.py`（继承 base，依赖 cli_runner）
- 顶层：`__init__.py`（依赖 capabilities、registry）

### 3. 编码风格

| 规范 | 要求 |
|------|------|
| 格式化 | black + isort（同 agent-sec-cli） |
| lint | 不适用 ruff（stdlib-only 项目，规则不兼容） |
| 日志 | `logging.getLogger("agent-sec-core")`，f-string 格式 |
| 类型注解 | 不强制（非 ruff 管辖） |
| 注释 | 英文 |

### 4. 新增 Capability

1. 在 `src/capabilities/` 下新建 `xxx.py`
2. 继承 `AgentSecCoreCapability`，定义 `id`、`name`（基类通过 `@property` + `@abstractmethod` 强制），实现 `_on_register()`、`get_hooks_define()` 和回调方法
3. 在 `capabilities/__init__.py` 中导入并加入 `ALL_CAPABILITIES`
4. 在 `config.toml` 中添加对应配置段 `[capabilities.<id>]`（`enabled` 和 `timeout` 必填）

```python
from .base import AgentSecCoreCapability


class MyCapability(AgentSecCoreCapability):
    id = "my-cap"
    name = "My Capability"

    def _on_register(self, config: dict) -> None:
        self._my_option = config.get("my_option", "default")

    def get_hooks_define(self) -> dict:
        return {"pre_tool_call": self._on_pre_tool_call}

    def _on_pre_tool_call(self, tool_name, args, **kwargs):
        ...
```

### 5. 可用 Hook

| Hook | 触发时机 | 回调签名 | 阻断方式 |
|------|----------|----------|----------|
| `pre_tool_call` | 工具执行前 | `(tool_name, args, **kwargs)` | 返回 `{"action": "block", "message": str}` |
| `post_tool_call` | 工具执行后 | `(tool_name, result, **kwargs)` | 无阻断 |
| `pre_llm_call` | LLM 调用前 | `(messages, **kwargs)` | 注入 context |
| `transform_llm_output` | 最终回复交付前 | `(response_text, session_id, **kwargs)` | 替换最终回复 |

### 6. 配置（config.toml）

```toml
[capabilities.code-scan]
enabled = true          # 是否注册该能力（必填）
timeout = 10            # agent-sec-cli 子进程超时（秒，必填）
enable_block = false    # false=observe(仅日志), true=block(阻断)

[capabilities.pii-scan-user-input]
enabled = true
timeout = 10
include_low_confidence = false
warning_ttl_seconds = 300
```

- `enabled = false` → 能力完全不注册
- `enable_block = false` → 检测到风险时仅记 WARNING 日志，不阻断工具调用
- `enable_block = true` → 检测到 deny/warn 时阻断工具调用
- `pii-scan-user-input` 仅扫描本轮用户输入，warning-only，不扫描 tool output

### 7. 测试

```bash
# 从 agent-sec-core 目录执行
uv run --project agent-sec-cli pytest tests/unit-test/hermes-plugin/ -v
```

### 8. 部署

```bash
./hermes-plugin/scripts/deploy.sh
```

`deploy.sh` 会将 `src/` 目录内容复制到 `~/.hermes/plugins/agent-sec-core-hermes-plugin/`。

---

## cosh-extension

> TODO: 待补充

---

## openclaw-plugin

> TODO: 待补充

---

## linux-sandbox

> TODO: 待补充

---

## skills

> TODO: 待补充

---

## User-Facing Documentation Guidelines

### Authoring Protocol

Every factual assertion in this section and in user-facing docs MUST be verified against source code before writing. Specifically:

1. **Enum/value-set claims** — read the defining source file that declares the enum
2. **External dependency sources** — grep for download/fetch calls in the relevant module
3. **Config field names** — read the config-loading function for that specific capability
4. **Module/section counts** — count actual headings in the user guide, never rely on memory
5. **Cross-file consistency** — if this file prescribes ordering/structure, verify the user guide matches before commit
6. **Intra-file consistency** — overview tables, section headings, and enumeration lists within the same document must agree

Do NOT write guidelines from design intent or mental models. Write them AFTER verifying the implementation.

### Value Proposition
- Lead with "all-local, zero Token cost" — addresses the common misconception that runtime security = expensive API calls or performance overhead.
- The three-layer defense framing (pre-execution prevention → runtime detection → kernel-level containment) helps users understand why multiple modules exist.

### Content Decisions
- Eight modules in overview table (Sandbox is architecture-only, no dedicated usage section). Seven usage sections: Prompt Scanner, Code Scanner, Skill Ledger, PII Checker, Security Baseline, Observability, Security Events. Do not merge them.
- Agent integration order in docs: CLI (always available) → OpenClaw plugin → Hermes plugin → cosh hook (auto-loaded, no user action needed). This reflects manual-effort-first ordering.
- `loongshield` may be mentioned alongside `agent-sec-cli harden` — loongshield is an Alinux system component users already know; `agent-sec-cli harden` is ANOLISA's unified entry point wrapping it.
- ML model warmup: state that models come from ModelScope (Llama-Prompt-Guard-2-86M). Never reference internal model registries.

### Gotchas to Warn About
- Code Scanner verdict enum defines `pass` / `warn` / `deny` / `error`. Built-in rules currently produce `warn` or `pass`; `deny` and `error` are available for custom/LLM-driven rules. Do not invent levels outside this enum (no "critical", no "info").
- Skill Ledger has exactly 6 states: pass / none / drifted / warn / deny / tampered. The state table must always appear in full when documenting Skill Ledger.
- Default plugin behavior is observe-only (fail-open). Users must explicitly enable blocking — always document both modes.

### Terminology
- "Security Baseline" not "hardening scan" (the feature name in CLI is `harden`, but user docs should call the concept "Security Baseline")
- "Skill Ledger" not "skill integrity" or "skill verification" (the latter was v0.3 naming, now superseded)
