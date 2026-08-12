# 生命周期状态一致性

[English](lifecycle-state-consistency.md)

Blaze 必须完整重建已经持久化的 sandbox 清单，才能开始资源恢复或提供 API
请求。本设计说明 daemon 如何协调生命周期状态写入、校验启动清单，以及如何在
不暴露部分结果的前提下发布该清单。

这一协议不改变 HTTP API、配置项或持久化 JSON 格式。

## 概念与持有对象

**state root** 是 `daemon.state_dir` 配置的目录。每条已经持久化的 sandbox
记录都保存在该目录下一个以规范 UUID 命名的目录中，其中的 `state.json`
保存重启时使用的生命周期记录。

`StateStore` 是生命周期记录持久化的受支持入口。它会在自身存续期间一直保留
已经打开的 state-root 目录对象，而不是重新打开配置路径。对于每个活动
sandbox，它还会保留已经打开的 UUID 目录对象；后续记录和运行目录操作都从
这些已经打开的对象派生。

**启动清单**包含每个 UUID 所属目录中通过校验的生命周期记录。另一个
retained-owner map 保存 daemon 后续执行生命周期和 backend 操作时必须继续使用
的已打开 UUID 目录。

## 写入协调

production daemon 会在扫描生命周期记录前，对已经打开的 state root 取得非阻塞
排他 advisory lock。另一个遵守相同协议的 Blaze daemon 必须等到前一个 daemon
释放 lock 后，才能使用同一个 state root 启动。

在单个 daemon 内，启动扫描会在完整扫描和发布过程中持续持有 `StateStore` 的
run-directory map lock。生命周期持久化也通过 `StateStore` 进入该 map，因此
受支持的进程内 writer 不能在启动清单构建期间发布或释放 owner。每个 sandbox
的记录写入还使用独立的 writer lock。

这两类 lock 的职责不同：state-root lock 协调遵守协议的 daemon 进程，
run-directory map lock 协调单个 daemon 内的 writer。

## 启动发布流程

启动按照以下顺序执行：

1. 打开配置的 state root，取得 advisory lock，并保留这个已经打开的目录对象。
2. 枚举 UUID 所属条目，在私有的 instance map 和 retained-owner map 中构建
   结果。每个 UUID 条目必须满足：
   - 目录名是规范的小写、带连字符 UUID；
   - 条目本身是目录而不是链接或其他文件系统对象，并且与枚举时观察到的是
     同一个目录对象；
   - `state.json` 是只有一个硬链接的普通文件，并且相对于该目录打开，而不是
     通过可能已经被替换的路径打开；
   - 记录内的 sandbox ID 与目录名一致；
   - `Destroyed` 记录没有活动 operation，并且 backend ownership 为
     `NotStarted` 或 `Stopped`。
3. 完成第二次规范 UUID 名称枚举，并将完整集合与首次扫描结果比较。
4. 第二次枚举完成后，逐个确认保留的 UUID 目录和 `state.json` 仍与首次扫描
   接受的对象一致。
5. 只有全部检查通过后，才发布 retained-owner map，并将 instance map 返回给
   `ServerState`。
6. 处理已经接受的 sandbox 记录，随后绑定配置的 Unix 和 TCP API listener。

名称集合比较必须在对象复验开始前完成。这个顺序可以避免较早的 owner 已经通过
检查，而最终目录枚举仍在处理后续 UUID 条目。

## 失败行为

UUID 记录缺失、格式错误、类型异常、使用别名或内部状态不一致，都会使 daemon
停止启动。如果最终名称集合比较或对象复验发现 owner 或记录新增、删除或替换，
启动也会停止。扫描不会发布部分 retained-owner map，daemon 也不会打开 API
listener。

Blaze 会保留被拒绝的 UUID 目录及其 `state.json`，供运维人员检查和修复。
已有的状态发布 staging 条目清理流程与拒绝记录的处理相互独立。

完整清单通过校验后，启动恢复会分别处理每个非终态 sandbox。单个 sandbox
清理失败时可以保留为 `RecoveryRequired`，但不会把已经通过校验的清单变成
部分清单。

## 一致性边界

本协议覆盖通过 `StateStore` 写入生命周期状态、并参与 state-root advisory
lock 的 daemon 进程。advisory lock 不会阻止无关进程直接修改该目录；有限次数
的目录扫描也无法针对这种 writer 提供原子快照。

绕过 state-root lock 的直接修改不在支持范围内。该路径的进一步隔离由
[#2459](https://github.com/alibaba/anolisa/issues/2459) 跟踪。

## 维护约束

后续生命周期状态改动必须保持以下规则：

- production 生命周期写入必须经过 `StateStore`；
- 必须在 inventory 扫描前取得 state-root owner，并在 request handler 仍可能
  写入生命周期状态期间持续持有；
- 启动过程必须持有 run-directory map lock，直到完整清单被接受或拒绝；
- 必须先完成最终 UUID 枚举，再复验保留对象；
- 所有清单检查完成前，request handler 不能观察到任何一个启动 map。
