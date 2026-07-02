# 贡献指南

## 开发环境

| 要求 | 版本 |
|------|------|
| Rust toolchain | stable（`rust-toolchain.toml` 管理） |
| Rust 最低版本 | 1.74 |
| 组件 | rustfmt + clippy |

```bash
cd src/cosh-ng
rustup show   # 确认 toolchain 已就绪
```

## 构建

```bash
# 完整构建（所有 5 个 crate）
cargo build --workspace

# 发布构建
cargo build --workspace --release

# 单独构建某个二进制
cargo build --bin cosh-cli
cargo build --bin cosh-core
cargo build --bin cosh-shell
```

## 代码质量检查

提交前必须通过以下全部检查：

```bash
# 格式检查
cargo fmt --all -- --check

# Clippy（warnings 视为错误）
cargo clippy --all-targets --locked -- -D warnings

# 测试
cargo test --locked

# 文档构建（修改 pub API 时）
cargo doc --workspace --no-deps
```

## 工作空间结构

```
cosh-ng/
├── Cargo.toml              # workspace 配置
├── rust-toolchain.toml     # stable + rustfmt + clippy
└── crates/
    ├── cosh-types/         # 纯类型，零副作用
    ├── cosh-platform/      # 平台抽象（发行版检测、后端路由）
    ├── cosh-cli/           # CLI 入口
    ├── cosh-core/          # Agent 核心
    └── cosh-shell/         # 交互终端
```

## 依赖管理

- 所有依赖版本在 `[workspace.dependencies]` 统一声明
- 子 crate 通过 `dep = { workspace = true }` 引用
- 添加新依赖前检查是否已存在等价 crate
- 不允许未经讨论升级主版本号

## 代码规范

### 模块组织

使用 Rust 2018+ 推荐的文件布局，**不使用 `mod.rs`**：

```
# 正确
src/extension.rs        # 父模块
src/extension/          # 子模块目录
    config.rs
    manager.rs

# 错误 — 不使用
src/extension/mod.rs
```

### 错误处理

| 场景 | 方式 |
|------|------|
| 库 crate | `thiserror` 枚举 |
| 二进制 | `anyhow::Result` |
| 不可达路径 | `unreachable!()` + 注释 |
| 禁止 | `unwrap()` / `expect()` / `panic!()` |

### 注释

- `///` 用于所有 pub 项
- `//` 仅解释 *为什么*，不重复类型签名
- 首行为独立摘要，祈使句或名词短语
- 不允许 `TODO` 无 owner、注释掉的旧代码

### Clippy

- 默认 deny 所有 warnings
- 确需忽略时使用最窄范围的 `#[allow(clippy::xxx)]` + 注释说明

## 提交规范

格式：`type(scope): imperative description`

- 类型：feat / fix / refactor / docs / test / ci / chore
- scope：`cosh-ng` 对应 `cosh`（如改动跨多个 crate）
- 50 字符内，英文，祈使语气，首字母小写，无句号
- 需要 `Signed-off-by` trailer

```bash
git commit \
  --trailer "Assisted-by: Qoder:1.7.0" \
  --trailer "Signed-off-by: $(git config user.name) <$(git config user.email)>" \
  -m 'feat(cosh): add registry list action for hooks'
```

## PR 流程

1. 从最新 main 分支切出特性分支
2. 遵循分支命名：`feature/cosh/<short-desc>`
3. 确保所有检查通过后推送
4. PR 标题遵循 commit message 格式
5. 填写 PR 模板（Description / Testing / Related Issue）
