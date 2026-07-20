# 安装生命周期设计

[English](install-lifecycle.md)

本文定义 `anolisa` 组件生命周期命令共享的 authority、scope、planning、
transaction 与 recovery 模型。该模型用于保证：当 raw 文件、native package、
并发命令或进程中断独立改变机器状态时，ANOLISA state 仍然真实可信。

## 设计目标

- 将 ownership 与 distribution format 分离。raw artifact 通常由 ANOLISA
  持有；RPM 安装仍以 rpmdb 为 authority。
- 每个 lifecycle 决策都基于同一 scope、同一 lock epoch 中观察到的 facts。
- 在副作用前持久化足够的 intent，使崩溃恢复具有确定性。
- 普通用户仍可看到 system 安装，但 user-scoped 命令不能因此修改 system
  state。
- 面对不明确的 package、subject 或 owner 时拒绝恢复，而不是猜测。

## Authority 模型

每个 active installation 只能有一个 `ProviderBinding`：

| Binding | 事实来源 | 修改权限 |
|---------|----------|----------|
| `Owned` | ANOLISA record 与已记录的文件 hash | ANOLISA 可以校验、替换或删除自有文件 |
| `Delegated` | native package database | ANOLISA 可以观察；native transaction 是否允许取决于 management relation |

delegated binding 还记录一种 relation：

| Relation | 含义 | 默认 uninstall |
|----------|------|----------------|
| `Managed` | ANOLISA 安装了该 package | 委托 native manager 移除 package |
| `Adopted` | 用户显式采纳了预先存在的 package | 只删除 ANOLISA record |
| `Observed` | package 可见，但没有 management consent | 只删除 ANOLISA record |

`--remove-system-package` 是单次调用中移除 adopted 或 observed package 的
显式权限，不会被持久化为新的 ownership relation。

## Scope 与可见性

user 与 system 安装拥有独立的 layout、state file、lock 和 journal directory。
修改命令只写入 `--install-mode`（或 UID 默认值）选择的 layout。

只读 state projection 有意采用更宽的范围：`list`、`status`、`doctor` 与
adapter discovery 可以组合所有可读的 user/system root。这样普通用户可以
使用和检查 system 安装，但两个 record 不会被合并，authority 也不会跨
scope 转移。

由此得到以下不变量：

1. system 安装不会阻止
   `anolisa --install-mode user install <component>`。
2. user-scoped `forget`、`uninstall`、`update` 或 `repair` 绝不修改视图中可见的
   system record。
3. user view 中两个记录都存在时，user record 为 active，system record 显示为
   shadowed；system mode 只读取 system root。
4. system 修改仍然需要底层操作要求的权限。

## Planning Pipeline

每个 lifecycle handler 都遵循同一条边界：

```text
request -> scoped facts -> pure planner -> typed steps -> executor -> record
```

facts 包含所选 scope 的 record、相关 native observation、owned-file
integrity、adapter claim、quarantine state 和 effective pending journal 状态。
planner 是纯 decision table，不能写文件、运行 dnf 或更新 state。executor
只解释自己所属的 step family。

| Intent | 关键决策 |
|--------|----------|
| `install` | 安装到所选 scope；绝不把另一个 scope 当作同一 record |
| `update` | 刷新 owned artifact，或委托 managed package update |
| `uninstall` | 只移除 binding 与 relation 授权的对象 |
| `adopt` | 将已存在的 system package 转换为 delegated-adopted record |
| `repair` | 重新观察 authority、恢复 journal 或重放已校验的 owned state |
| `forget` | 只删除所选 scope 的 record，不产生 package 或文件副作用 |

取得该 scope 的 install lock 后会重新 planning，以关闭第一次读取之后其他
lifecycle 命令改变 record、package identity、adapter claim 或 pending-journal
gate 的窗口。

## Transaction 协议

journal 创建在其所保护 record 的同一 state root。每个 step 都会在副作用前
记录为 `Planned`，执行后再转换为 terminal step status。

delegated operation 会在一次原子的 journal revision 中同时持久化 per-subject
recovery context 与首批 step。context 绑定：

- 精确的 component subject；
- native package manager 与 resolved package（如果存在）；
- 预期的 record transition。

这样崩溃不会暴露一个缺少 planned side-effect 证据的 recovery identity。
batch install/update 共用一次 native transaction，但每个组件仍有自己的
journal 和 recovery identity。

native package transaction 采用 forward-only 策略。dnf 一旦可能已经提交，
ANOLISA 就不会猜测反向操作；失败保持 `Partial`，repair 重新观察 rpmdb。
owned operation 使用逆序 compensation。backup 内容会校验 hash，并通过不跟随
destination leaf symlink 的原子替换完成恢复。

## Recovery 分类

recovery 首先为整个所选 state root 加载 `JournalInventory`。任何 entry 被消费
之前，所有 journal path、schema、state binding 和 same-root operation record
都必须通过校验。之后直接使用已经校验的内存 transaction，避免校验后再次从
路径加载。

新的 delegated journal 必须具有精确 subject 和非空原子 intent。在恢复 record
write 或 drop 前，repair 会重新校验当前 record、package identity、package
manager 与 management relation。特别是：managed record 的 package 仍存在时，
除非 journal 记录了对应 native removal，否则不能删除该 record。

重构前的 RPM install journal 通过严格受限的 legacy classifier 保持兼容。live
legacy claim 必须具有完全匹配的 install/state marker shape，以及不歧义的
package/component identity。带新 subject 却没有新 recovery context 的 journal
既不会被当作 legacy 接受，也不会被静默 replan；它会保持 pending，并返回可
操作的安全错误。

只有同一 state root 中兼容的 operation history 能证明 legacy journal 的效果
已经提交时，settled journal 才会被忽略。evidence 绝不跨越 user/system root。

## 失败策略

- 无效或不明确的 facts 在副作用前失败。
- malformed pending journal 保持 pending，等待检查。
- 已产生 native 副作用但未提交 record 时标记为 `Partial`，而不是 clean failure。
- package 仍存在时，record-only adopted/observed uninstall 仍然有效。
- managed record-only uninstall 只有在观察到 package 已不存在时才有效。
- 当前 authority 或 package binding 已变化时，recovery 绝不覆盖现有 record。

## 验证

unit test 覆盖所有 planner decision row、executor step ordering、scoped visibility
与 mutation、legacy journal classification、batch recovery、record authorization
和 owned rollback。smoke suite 使用隔离的 user/system root 与真实 RPM database
fixture 测试编译后的 CLI。
