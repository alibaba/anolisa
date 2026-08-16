# 生命周期状态一致性与兼容性

[English](lifecycle-state-consistency.md)

Blaze 有两个相互关联的生命周期边界。提供请求服务前，它必须完整重建已经持久化
的 sandbox 清单，且不能暴露部分结果。提供请求服务期间，它只通过沙箱命名空间
提供生命周期和客户机操作，并在预留的复用容量操作改变所有权之前将其拒绝。已停用
的 `Reset`、`Warm` 和 `start_path = "warm"` 值继续可解析，以便启动恢复清理
包含这些值的非终态记录。

本设计定义这两个边界。清单发布流程不改变 HTTP API、配置项或持久化 JSON 格式。
管理 API 章节定义沙箱命名空间以及预留的复用容量边界。

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
清理失败时可以在内存中保留为 `RecoveryRequired`，但不会把已经通过校验的清单
变成部分清单。Blaze 会尝试持久化恢复状态；如果这次写入也失败，启动恢复会报告
附加错误，持久化记录仍可能保留先前的状态。

## 管理 API 与可复用状态边界

生命周期和客户机操作注册在 `/v1/sandboxes` 下。操作式重置、检查点和销毁
路径不注册，并返回 `404 Not Found`。规范的销毁入口仍是
`DELETE /v1/sandboxes/{id}`。
检查点捕获由独立设计定义。

以下保留的管理路由同样返回 `501 Not Implemented`，并且不会管理复用容量：

- `GET /v1/pools`；
- `GET /v1/pools/{backend}/{class}`；
- `POST /v1/pools/{backend}/{class}/drain`；
- `PUT /v1/pools/{backend}/{class}/sizing`。

为了保持响应兼容，`GET /v1/health` 会继续返回 `storage_pool` 对象；文件存储
提供者报告的就绪、容量、待处理和隔离槽位数量均为零。因为没有注册重置路由，
监控接口不发布重置计数。复用容量没有受支持的成功路径，因此资源池命中和未命中
计数也继续保持缺失。

新建 sandbox 始终记录 `start_path = "cold"`。生命周期状态转换不能进入 `Reset`
或 `Warm`，因此没有受支持的路径可以产生或重新启用可复用 sandbox。Blaze 继续
解析旧版本写入的 `Reset`、`Warm` 和 `start_path = "warm"`，唯一目的是让启动
恢复释放这些记录拥有的资源。完整清单通过校验后，启动恢复会销毁每一条这样的
非终态记录。清理成功后记录进入 `Destroyed`。清理失败后，内存记录保留为
`RecoveryRequired`，并尝试持久化该状态；持久化失败会被报告，磁盘上的记录可能
仍是先前状态。启动恢复会继续处理其他已通过校验的记录。新建请求绝不会选择或
重新启用旧版记录。

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
- 所有清单检查完成前，request handler 不能观察到任何一个启动 map；
- 未注册的沙箱操作式路由必须在读取或改变 sandbox 状态前返回 `404`；
- 资源池管理请求必须在生命周期、运行环境或存储所有权发生变化前被拒绝；
- 生命周期操作不能进入或重新启用 `Reset` 或 `Warm`；旧值只能用于清理。
