# agent-sec-core Development Standards

本仓库包含多个组件，请根据你要修改的模块查阅对应章节：

| 组件 | 语言 | 路径 | 章节 |
|------|------|------|------|
| agent-sec-cli | Python + Rust | agent-sec-cli/ | [agent-sec-cli](#agent-sec-cli) |
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
