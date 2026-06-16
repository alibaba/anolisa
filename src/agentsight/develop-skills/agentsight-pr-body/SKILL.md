---
name: pr-body
description: 分析当前分支的全部变更，自动生成或更新 PR 标题和正文。聚焦内容质量：解释 why、归纳 what、标注测试方式，遵循 anolisa 项目 PR 模板规范。适用于新建 PR 前生成描述，或已有 PR 需要更新描述。
---

# PR Body 生成器

## 目标

分析当前分支相对于 `main` 的全部变更（所有 commit，不仅是最新一条），生成或更新符合 `alibaba/anolisa` 规范的 PR 标题和正文。

## 触发时自动执行

### 步骤 1：收集变更信息

```bash
# 当前分支
git branch --show-current

# 全部 commit（从 main 分叉点起）
git log --oneline origin/main..HEAD

# 变更文件列表
git diff --stat origin/main..HEAD

# 完整 diff（用于分析变更内容）
git diff origin/main..HEAD

# 是否已有 PR
gh pr list --head $(git branch --show-current) --repo alibaba/anolisa --state open --json number,title,body
```

### 步骤 1.5：Preflight 检查

在分析变更前，运行以下检查并记录结果（后续自动填入 Checklist）：

```bash
# 在 agentsight 目录下执行
cargo fmt --check          # 格式检查
cargo clippy --all-targets -- -D warnings  # lint 检查
cargo test                 # 单元测试 + 集成测试
```

- 三项全部通过才继续后续步骤
- 如果 `cargo fmt --check` 失败，自动运行 `cargo fmt` 修复后重新检查
- 如果 `cargo clippy` 失败，列出告警并尝试修复，修复后重新检查
- 如果 `cargo test` 失败，停止流程，报告失败的测试用例

### 步骤 2：分析变更

从 diff 中提取以下信息：

1. **变更类型**：根据 commit type 和实际改动判断（feat/fix/docs/refactor/perf/test/ci）
2. **影响范围**：涉及哪些模块（对照 AGENTS.md Module Map）
3. **动机（Why）**：从 commit message、关联 issue、代码注释中推断为什么做这个变更
4. **内容（What）**：归纳净变更，忽略开发过程中的反复修改
5. **风险点**：是否涉及 eBPF、FFI、storage schema 等高风险区域
6. **Footprint Ladder 级别**：本次变更属于哪个级别（1-5）

### 步骤 3：生成 PR 标题

格式：`type(sight): 简要描述`

规则：
- 不超过 70 字符
- 用英文，动词开头（add/fix/update/refactor/remove）
- 描述净变更效果，不描述过程

示例：
```
feat(sight): add Go TLS uprobe for plaintext capture
fix(sight): handle qodercli wrapped SSE format
docs(sight): add scoped AGENTS.md for high-risk modules
refactor(sight): extract HTTP2 frame decoder from parser
```

### 步骤 4：生成 PR 正文

使用以下模板，**每个字段必须基于实际 diff 内容填写**，禁止使用占位符：

```markdown
## Description

<1-3 段说明，先写 why（动机），再写 what（做了什么）。>
<如果关联了 issue，说明该 issue 的背景。>

## Related Issue

closes #<issue-number>

## Type of Change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Documentation update
- [ ] Refactoring (no functional change)
- [ ] Performance improvement
- [ ] CI/CD or build changes

## Scope

- [x] `sight` (agentsight)

## Key Changes

<按模块或逻辑分组列出关键变更，每条一行：>
<- `src/parser/sse.rs`: 新增 qodercli SSE 包装格式解析>
<- `src/bpf/gotls.bpf.c`: 新增 Go crypto/tls uprobe>

## Checklist

- [x] I have read the [Contributing Guide](../CONTRIBUTING.md)
- [x/空] `cargo fmt --check` pass（基于 preflight 结果勾选）
- [x/空] `cargo clippy --all-targets -- -D warnings` pass（基于 preflight 结果勾选）
- [x/空] `cargo test` pass（基于 preflight 结果勾选）
- [ ] I have added tests that prove my fix is effective or that my feature works
- [ ] I have updated the documentation accordingly
- [x] Lock files are up to date (`Cargo.lock`)

## Testing

<具体说明如何验证这次变更，不要写"运行了 cargo test"这种泛泛的话。>
<示例：用 qodercli 发起 LLM 请求，确认 agentsight 正确解析 SSE 响应并提取 token 数。>
```

### 步骤 5：应用

**新建 PR 场景**：输出生成的 title 和 body，供 `github-issue-pr` skill 或手动 `gh pr create` 使用。

**更新已有 PR 场景**：

```bash
gh pr edit <PR-NUMBER> --repo alibaba/anolisa \
  --title "type(sight): 描述" \
  --body "$(cat <<'EOF'
<生成的 body>
EOF
)"
```

更新时注意：
- 保留已有 body 中的图片（不要删除 `![...](...)`）
- 保留人工添加的补充说明
- 只更新自动生成的部分

## 内容质量规则

1. **先写 Why 再写 What**：Description 第一段必须是动机，不是"本 PR 做了 X"
2. **描述净变更**：多次 commit 中的反复修改只看最终结果，不提开发过程
3. **具体到文件**：Key Changes 每条必须包含文件路径
4. **不说废话**：不要写 "improved code quality"、"various improvements" 等空泛描述
5. **Testing 要具体**：描述验证场景和预期结果，不要只写 "passed all tests"
6. **禁止泄露**：不包含本地绝对路径、内部 URL、密钥等敏感信息
7. **Issue 关联**：如果 commit message 中提到 issue 编号，自动填入 `closes #N`；无关联 issue 则写 `no-issue: <原因>`
