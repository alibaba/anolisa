# AGENTS.md

本文件约定本仓库中 AI 协作者编写代码时遵循的工程规范。

## 注释原则（Rust）

遵循 Rust 官方风格指南与 API Guidelines，写「让读者更快理解意图」的注释，而不是把代码翻译成自然语言。

### 1. 注释类型与位置

- **`//!` 模块级文档**：放在文件/模块顶部，一两句话说清楚这个模块负责什么、何时使用。
- **`///` 文档注释**：用于一切公开 (`pub`) 的 item —— struct、enum、trait、函数、方法、重要字段、变体。这些会进入 `cargo doc`。
- **`//` 普通注释**：仅用于实现内部需要解释「为什么这么写」的地方。
- 不要在私有的、内部显而易见的辅助函数上堆 `///`。

### 2. 写「为什么」，不写「是什么」

- 类型名、字段名、函数名已经说明 *是什么*；注释应补充 *为什么* 和 *不变量*。
  - 好：`// 序列化为 untagged，因为多数 provider 不返回 type 字段`
  - 坏：`// 这是一个枚举，表示助手内容`
- 描述**不变量**（例如 `NonEmptyVec` 保证至少一个元素）、**前置条件**、**副作用**、**与外部协议的约定**（如 provider 需要原样回传的字段）。
- 凡能从签名/类型/命名直接读出的事实，不要重复。

### 3. 简洁优先

- 一行能说清的不写两行；trivial setter保持无注释或一句话足矣。
- 避免礼貌性废话：「This function returns ...」「这是一个用于 ...」。直接用祈使句或名词短语开头：「Returns ...」「Builds ...」「构造 ...」。
- 第一行是摘要句，独立成行；需要细节时空一行再展开。

### 4. 链接与交叉引用

- 用 intra-doc link 引用其它 item。
- 在父类型上提到子字段时用 `` [`Field`](Self::field) ``，让 rustdoc 渲染成跳转链接。

### 5. 文档约定段落

按需使用 rustdoc 约定的段落标题，缺失时不要硬凑：

- `# Errors` —— 返回 `Result` 的函数：列出失败条件。
- `# Panics` —— 可能 panic 的函数：列出触发条件。
- `# Safety` —— `unsafe fn`：调用方必须保证的不变量。
- `# Examples` —— 公共 API 的典型用法，写在 ```` ```rust ```` 代码块里，能被 `cargo test --doc` 跑过。

### 6. 不变量与协议字段

- 涉及序列化/外部协议的字段（`#[serde(...)]`、provider id、signature 等），注释要说明它在线协议中的角色，以及为什么需要保留/原样回传。
- 使用 `#[serde(skip_serializing_if = ...)]`、`flatten`、`untagged` 等非默认属性时，注释说明动机。

### 7. 禁止事项

- 不写「TODO（无 owner、无上下文）」；要写就带原因和处理条件。
- 不留注释掉的旧代码；用 git 历史。
- 不写时间戳、作者名、变更日志类注释；交给 VCS。
- 不写「修复了 issue #123」之类引用，PR 描述里写。
- 不在注释里复述类型签名。

### 8. 校验

- 提交前跑 `cargo check` 与 `cargo doc --no-deps` 确保无 broken intra-doc link。
- 公共 API 可在 crate 顶部开启 `#![warn(missing_docs)]` 强制覆盖。

## 工作区结构与 crate 职责

## 模块组织：no `mod.rs` 风格

统一采用 Rust 2018+ 推荐的非 `mod.rs` 布局：父模块用同名 `.rs` 文件，子模块放进同名目录。

理由：避免一个仓库里出现一堆同名的 `mod.rs`，文件树和编辑器标签更易辨识；与 `rustfmt`、`cargo new` 默认行为一致。新增模块时不要创建任何 `mod.rs`，CR 中遇到也要求改掉。

## 依赖管理

- 所有第三方依赖在 `[workspace.dependencies]` 中统一声明版本；子 crate 用 `dep_name = { workspace = true }` 引用，绝不在子 crate 里写死版本号。
- 新增依赖前先 grep `Cargo.toml` 看是否已有等价库（例如已有 `serde_json` 就别再引入 `simd-json`）。
- 不擅自升级已声明依赖的大版本（major），有需要时单独提出讨论。
- feature flag 在 workspace 声明处集中开启，子 crate 不要重复 `features = [...]`，除非真的需要扩展。

## 错误处理

- **库 crate**：用 `thiserror` 定义具名 `enum` 错误类型；每个 crate 自己的错误枚举不要跨 crate 复用，而是用 `#[from]` 包装上游错误。
- **二进制**：可以使用 `anyhow::Result` 简化错误传播。
- 库代码里禁止 `unwrap()` / `expect()` / `panic!()`，除非能在注释里证明该条件由类型系统保证不可能发生（这种情况优先用 `unreachable!()` 并写明原因）。
- 错误信息面向开发者：包含失败的上下文与变量值，避免「something went wrong」式空话。
- 优先用 `?` 传播；不要用 `match` + 立即 `return Err(...)` 重写已经能 `?` 的代码。

## 提交前检查

提交/打开 PR 前，工作区根目录运行以下命令，全部通过才算完成：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
cargo test --workspace
```

- 修改了公共 API 或 doc 注释时，额外跑 `cargo doc --workspace --no-deps` 检查 intra-doc link。
- clippy 警告默认拒绝；确有理由放过的，用 `#[allow(clippy::xxx)]` 在最小作用域上加，并写注释说明原因。
- 不要为了让检查通过而注释掉测试或删掉断言；查根因。
