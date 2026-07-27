# 贡献指南

[English](CONTRIBUTING.md)

欢迎参与贡献！本文档介绍参与项目的基本流程。

> **写完代码了？** 让你的 AI 助手帮忙：*「读一下 AGENTS.md，帮我生成 commit message 和 PR 描述。」*

## 开发环境

### 前置依赖

- **Node.js** >= 20.0.0（copilot-shell）
- **Python** >= 3.12.0（os-skills）
- **Rust** stable 工具链（Rust 组件，部分仅限 Linux）
- **uv**（os-skills 的 Python 包管理器）
- **clang** & **libbpf**（编译 agentsight eBPF C 代码）

### 快速开始

```bash
# 克隆仓库
git clone https://github.com/alibaba/anolisa.git
cd anolisa

# 一键构建：安装依赖 + 构建 + 安装到系统（推荐）
./scripts/build-all.sh

# 仅构建选定组件
./scripts/build-all.sh --component cosh --component sec-core

# 运行统一测试
./tests/run-all-tests.sh
```

完整的构建选项和依赖安装说明，请参阅 [docs/BUILDING_zh.md](docs/BUILDING_zh.md)。

### 组件独立开发

每个组件有自己的构建流程：

- **copilot-shell**：
  ```bash
  cd src/copilot-shell
  make deps   # npm install + husky hooks
  make build
  ```

- **os-skills**：`cd src/os-skills` — Skill 定义是静态资源，无需编译

- **agent-sec-core**（仅 Linux）：`cd src/agent-sec-core && make build-sandbox`

- **agentsight**（仅 Linux，可选）：`cd src/agentsight && make build`

- **SkillFS**（仅 Linux）：`cd src/skillfs && cargo build --workspace`

## 构建与测试命令

```bash
# 统一构建（推荐——自动处理依赖、构建和安装）
./scripts/build-all.sh                                        # 所有默认组件
./scripts/build-all.sh --no-install                           # 仅构建，跳过安装
./scripts/build-all.sh --ignore-deps                          # 跳过依赖安装
./scripts/build-all.sh --component cosh --component sec-core  # 选定组件

# 统一测试
./tests/run-all-tests.sh             # 所有组件
./tests/run-all-tests.sh --filter shell   # 仅 copilot-shell
./tests/run-all-tests.sh --filter sec     # 仅 agent-sec-core

# 单组件
cd src/copilot-shell && make lint && make test
cd src/agent-sec-core && pytest tests/integration-test/ tests/unit-test/
cd src/agentsight && cargo test
cd src/skillfs && cargo test --workspace
```

## 贡献流程

1. **先开 Issue** — 在写代码前讨论你的方案。
2. **Fork 并创建分支** — 基于 `main` 创建功能分支。
3. **编写代码** — 遵循已有代码风格和规范。
4. **编写测试** — 确保变更有充分的测试覆盖。
5. **运行预检** — 提交前对受影响的组件进行 lint 和测试：
   ```bash
   # copilot-shell
   cd src/copilot-shell && make lint && make test
   # agent-sec-core
   cd src/agent-sec-core && pytest tests/
   # agentsight
   cd src/agentsight && cargo clippy -- -D warnings && cargo test
   # SkillFS
   cd src/skillfs && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
   ```
6. **提交 PR** — 关联 Issue 并提供清晰的描述。

### Commit 规范

遵循 [Conventional Commits](https://www.conventionalcommits.org/)：

```
feat(cosh): add --json flag to config command
fix(sec-core): handle sandbox escape edge case
docs(docs): update installation guide
```

**scope 是必填项**，必须为以下之一：

| Scope | 覆盖范围 |
|-------|---------|
| `cosh` | `src/copilot-shell/` |
| `sec-core` | `src/agent-sec-core/` |
| `skill` | `src/os-skills/` |
| `sight` | `src/agentsight/` |
| `tokenless` | `src/tokenless/` |
| `ckpt` | `src/ws-ckpt/` |
| `memory` | `src/agent-memory/` |
| `anolisa` | `src/anolisa/` |
| `skillfs` | `src/skillfs/` |
| `ci` | `.github/workflows/` |
| `docs` | `docs/` 或文档更新 |
| `deps` | 依赖版本升级（lock 文件） |
| `chore` | 其他维护（配置、脚本、工具） |

### 分支命名

内部贡献者请遵循以下约定：

```
feature/<scope>/<short-desc>    例如 feature/cosh/json-output
fix/<scope>/<short-desc>        例如 fix/sec-core/sandbox-escape
hotfix/<scope>/<short-desc>     例如 hotfix/skill/broken-load
```

**Fork 贡献者**：分支命名不作限制——CI 只会给出建议，不会阻止你的 PR。

### CI 检查说明

| 检查项 | 级别 | 修复方法 |
|--------|------|---------|
| Commit scope 缺失 | **错误**（阻止合并） | 给每个 commit message 加 `(scope)`，如 `fix(cosh): ...` |
| Commit scope 不在允许列表 | 警告 | 使用上方列表中的 scope |
| PR 标题格式 | 警告 | 遵循 `type(scope): description` 格式 |
| 分支名不符合约定 | 警告 | 遵循 `feature/<scope>/<desc>`——Fork 不要求 |
| PR 未关联 Issue | 警告 | 在 PR 描述中添加 `closes #<n>` 或 `no-issue: <reason>` |
| CI 测试失败 | **错误**（阻止合并） | 在请求 review 前修复失败的测试 |

## 代码风格

- **TypeScript**：ESLint + Prettier（配置在 copilot-shell）
- **Python**：Ruff + Black（配置在 os-skills）
- **Rust**：`cargo fmt` + `cargo clippy`

## 许可证

参与贡献即表示你同意你的贡献将以 Apache License 2.0 进行许可。
