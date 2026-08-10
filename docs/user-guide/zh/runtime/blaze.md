# Blaze Firecracker 网络

[English](../../en/runtime/blaze.md)

Blaze 可以为每个 Firecracker sandbox 分配独立的 network namespace、tap
设备、veth pair 和地址 slot。该能力需要显式开启，默认保持关闭。

## 前置条件

Blaze daemon 必须运行在 Linux 上，并具有管理主机网络的权限。主机需要安装且
能够执行 `ip`、`sysctl` 和 `iptables`。此外还需要准备可用的 Firecracker、
guest kernel 和 root filesystem image。

当已加载的策略同时启用网络并将 Firecracker 作为候选 backend 时，Blaze 会
检查这些前置条件。网络保持关闭的策略不要求主机提供这些能力。

## 配置方法

在 workload 策略的 Firecracker 配置中设置 `enable_network`：

```toml
[select]
backend_priority = ["firecracker"]

[backend.firecracker]
enable_network = true
```

该选项仅作用于 Firecracker，默认值为 `false`。现有策略只有在显式开启后才会
改变原有行为。

## 运行行为

请求选中已启用网络的 Firecracker 策略后，sandbox 创建流程会：

1. 分配一个主机级 network slot；
2. 创建带有实例 owner 标识的 network namespace；
3. 创建 tap 和 veth 设备，并配置地址、转发和 namespace 内的 NAT；
4. 启动 Firecracker，并将 tap 设备连接到 VM。

分配与删除过程使用 `/run/lock/blaze-network.lock`，避免同一主机上的两个
Blaze daemon 进程同时选中相同 slot。Blaze 会在创建依赖设备前记录 namespace
的 owner，使未完全完成的网络配置仍能归属于对应 sandbox。

显式销毁 sandbox 时，Blaze 会先确认 backend 进程已经停止，再删除其拥有的
namespace 和设备。启动失败的补偿流程执行相同的清理。如果无法确认清理完成，
Blaze 会保留 ownership，不会将 slot 重新交给分配器，以便后续 destroy 请求
重试。

daemon 重启后，后续 destroy 请求可以根据已有记录重新识别 network slot。
Blaze 不会在后台扫描或自动重试孤立的网络资源。

## 主机集成边界

Blaze 负责配置 sandbox 本地的网络路径。主机以外的路由和 DNS 仍由主机运维方
负责。生产环境开启该选项前，需要配置所需的上游路由或地址转换，并在目标主机
环境中验证 guest 连通性。

如需关闭该能力，将 `enable_network` 设置为 `false` 或删除该配置项，再通过
正常的 instance API 销毁已经启用网络的 sandbox。

## Guest 操作

只有 sandbox 处于 `Running` 且 backend 报告兼容的 guest endpoint 时，
才能执行 guest 操作。冷启动 backend 如果报告了该 endpoint，创建流程会在
发布 `Running` 前等待 guest agent。没有 endpoint 的 backend（包括生产环境
mock fallback）会跳过等待，guest 操作返回 HTTP 409。当前从 warm pool 激活
实例时，manager 会先验证保留的 backend owner 和 storage，再发布 `Running`，
但不会再次执行 guest readiness 探测。因此 warm 路径的 `Running` 不保证 guest
endpoint 仍然可响应；第一次 guest 请求仍会执行有界连接，并可能返回 guest
错误。调用方应对第一次请求采用下文说明的重试和结果判定规则。

Guest 操作和 lifecycle 变更使用同一个 sandbox operation lock。取得锁后，
manager 会再次检查 `Running`，避免并发 lifecycle 变更后请求仍访问旧 runtime。

Sandbox 路由包括：

- `POST /v1/sandboxes/{id}/exec` — 执行一条命令；
- `POST /v1/sandboxes/{id}/read` — 读取一个文件；
- `POST /v1/sandboxes/{id}/write` — 替换一个文件。

对应的 `/v1/instances/{id}/...` 路由提供相同行为。Exec 请求格式如下：

```json
{"cmd":"uname -a","cwd":"/","env":{"LANG":"C"},"timeout":10}
```

Write 请求提供路径和 standard-base64 数据：

```json
{"path":"/tmp/input","data_b64":"aGVsbG8="}
```

Read 请求只提供 `path`。成功的文件读取结果和命令输出使用 standard base64。
Exec timeout 范围是 1 至 20 秒。Guest 路由会在读取过程中拒绝超过 22 MiB 的
HTTP envelope，文件数据解码后最多为 16 MiB。

Exec 或 write 在送达前失败时，可以由调用方决定重试；送达前超时使用
`"code": "guest_timeout"`。如果已经开始送达，但 daemon 无法确定结果，
返回 HTTP 504 和 `"code": "guest_outcome_unknown"`；此时应先核对 guest
状态，不能自动重放。Read 不改变 guest 状态。输入过大时返回 HTTP 413；
read 响应过大时返回 HTTP 502 和
`"code": "guest_response_too_large"`。

每个请求都会在单请求上限内完整缓冲。该上限不限制所有并发请求的总量，调用方
还需要控制 guest 操作并发数。当前不支持文件流式传输、交互式终端和会话复用。

可选 TCP listener 目前没有 daemon 级访问边界。在
[issue #2223](https://github.com/alibaba/anolisa/issues/2223) 解决前，生产配置应
保持 `listen.http_addr` 关闭。Daemon 停止时也不会等待全部 HTTP handler 或
释放所有 runtime owner，因此正在执行的请求可能看到连接关闭。

## 存储制品同步

Blaze 可以定期持久化 running sandbox 中已经写入的宿主机制品和目录元数据。
该 worker 默认关闭；只有配置同步周期后，现有部署的行为才会改变。

### 配置方法

在 daemon 配置中设置同步周期和单个 sandbox 的执行时限：

```toml
[storage]
sync_interval = "30s"
sync_timeout = "10s"
```

`sync_interval = "disabled"` 会关闭周期 worker。`sync_timeout` 限制
scheduler 等待单个完整 provider attempt 的时间，包括重建 storage slot 和
同步该 slot。

每次 storage-provider 同步调用会持久化本次调用可见、且已经写入的字节与目录
元数据。并发发生的制品更新可能在本次或后续 attempt 中变为可见。

### 运行行为

每轮 sweep 会选择处于 running 状态且仍持有完整 storage slot 的 sandbox。它会
对 operation lock 已被占用的 sandbox 直接推迟本轮处理，而不等待该 lock，使
sweep 可以继续处理后续 sandbox。Lifecycle 变更、guest 请求和存储制品同步共用这把
lock。取得可用 lock 后，worker 会在调用 storage provider 前再次检查 lifecycle
状态。如果取得 lock 后记录仍为 `Running`，但保留了未完成的 operation 或非
running 的 backend ownership，该记录属于不一致状态，会记为失败而不是推迟。
第一次 sweep 会在完整的配置周期过去后启动，而不是在 worker 启动时立即执行。
定时器错过的 tick 会被跳过而不是排队，避免慢速 sweep 累积任务。

已经返回的失败只影响对应 sandbox。Blaze 会保留 storage slot 的 ownership，
且不改变 lifecycle 状态，因此后续 sweep 或 destroy 仍可重试。如果文件系统
操作在 deadline 到达时无法停止，它会继续持有 sandbox operation lock 和唯一
的同步许可直至完成；后续 attempt 会被推迟，而不会累积更多 blocking 任务。
在此期间到达的 guest 和 lifecycle 操作会等待 provider 工作完成；
`sync_timeout` 只限制 scheduler 的等待时间，不限制这些操作的等待时间。

service loop 停止时，Blaze 会取消并等待周期 scheduler 退出。无法取消的
provider 工作会继续由对应 sandbox lock 持有直至完成；daemon 级连接排空和
runtime 清理仍属于独立职责。

## Template Catalog

Blaze 可以原子发布运维人员准备的 runtime artifact，并通过 daemon API 提供
其 metadata。`/v1/templates` 是唯一面向运维人员的 template 资源；发布条目
目前不会让 sandbox create 自动选择或启动它。

后续 sandbox create 支持会从同一个 catalog 解析可选的 template name；运维
人员不需要配置或监控另一套进程内 registry。

### 配置方法

catalog 目录有默认值，但只有配置 import root 后才会启用导入：

```toml
[template]
dir = "/var/lib/blaze/templates"
import_root = "/var/lib/blaze/template-imports"
max_files = 32
max_bytes = 274877906944
max_metadata_bytes = 1048576
max_total_bytes = 1099511627776
max_entries = 128
```

两个根目录必须使用绝对路径，彼此不能重叠，也不能与 Blaze 的 image、instance、
policy 根目录、`[backends]` 中配置的任一 executable 路径、本次启动
打开 daemon 配置文件时捕获的解析位置、该文件的配置路径或配置的
`daemon.socket` 路径以及宿主机网络协调路径
`/run/lock/blaze-network.lock` 重叠，也不能与宿主机上两种常见的命名网络空间
目录 `/var/run/netns` 和 `/run/netns` 重叠。
`[backends]` 中的相对路径会在启动时根据 daemon 的工作目录解析一次；目录边界
检查、backend probe 和 sandbox launch 随后复用该绝对路径。如果配置的 backend 路径
是符号链接，则该链接的配置位置及其解析目标都不能进入 template catalog ownership。
daemon 配置路径为符号链接时遵循相同规则：配置的链接位置与已打开文件的解析位置
都不能进入 template catalog ownership。
template catalog 根目录不能包含符号链接组件。在 Linux 上，Blaze 启动时会解析
路径中已经存在的部分，并根据 mount table 比较其底层文件系统位置，避免符号
链接或 bind mount 别名绕过目录边界。Blaze 会保留已打开的配置文件，并在捕获
的解析位置重复核对其身份，因此重定向配置路径不能换入另一个配置文件。发现
重叠时，启动会在修改 catalog
权限或扫描 catalog 条目之前拒绝继续。
template catalog 根目录可以像默认配置一样使用 `daemon.state_dir` 下的非 UUID
子目录，但不能接管 state root，也不能进入 sandbox UUID 子树。
如果 catalog 根目录尚不存在，Blaze 会保留路径中最深的现有父目录，并从该目录
创建缺失的路径段。如果计划创建的路径段在检查期间出现，启动会在修改该对象权限
之前停止。policy 条目边界检查遵循 `policy.on_load_error`：`warn` 模式下的条目
发现失败与 policy 加载一样使用空 policy engine；成功发现的 policy 目标仍受边界
保护。Blaze 通过 `PATH` 找到的宿主机辅助程序也受保护，检查同时覆盖程序的配置
位置和解析目标。
Blaze 会保留启动时打开并验证过的 import root 目录。之后替换配置路径不会改变
源目录查找的起点。

### 导入与查询

以下请求会发布 `import_root` 下的一个源目录：

```http
POST /v1/templates/import
Content-Type: application/json

{"name":"runtime-base","source":"runtime-base","description":"base runtime"}
```

`source` 必须是相对路径，不能跳转父目录或经过链接。源目录必须包含顶层普通文件
`vmstate.snap`、`mem.bin` 和 `rootfs.ext4`；可选的 `template.json` 必须是
JSON object。源目录和文件必须属于 daemon 用户，且不能允许 group 或其他用户
写入。嵌套目录、链接和特殊文件都会被拒绝。
已发布文件只能有一个硬链接，catalog 条目和 staging 目录也必须留在 catalog
根目录所在的挂载点。发现不满足这些边界的数据时，Blaze 会停止处理，不会修改或
继续遍历这些数据。
启动扫描或 list/get 读取 artifact 前，Blaze 会先在不取得可读句柄的情况下判型，
并在读取前重新核对对象身份。Linux 上的可读句柄来自已经固定的判型对象，因此替换
目录项不能把读取重定向到另一个对象。

`GET /v1/templates` 用于列出按名称排序的轻量摘要，
`GET /v1/templates/{name}` 用于读取一个条目的完整 metadata。列表读取会
逐条校验并释放完整 metadata，且任一时刻最多保留一个列表响应；在其 body 释放
前，并发列表请求返回 `503 Service Unavailable`。单项查询使用独立上限，任一时刻
最多保留一个完整单项响应；在该 body 释放前，其他单项查询返回
`503 Service Unavailable`。目标名称已存在或同名导入正在进行时返回
`409 Conflict`。

### 发布、上限与恢复

Blaze 在检查输入时执行单条目的文件数和字节数上限，并在复制到私有 staging
目录前预留 catalog 字节和一个 `max_entries` slot。复制后会再次检查源文件
身份，同步完整条目，再通过不覆盖现有目标的 rename 发布。因此读取方只会
看到“没有条目”或完整条目，名称摘要 list 响应也不会物化超过配置数量的条目。

导入失败时会删除 staging 数据；这也包括 staging 目录已创建、但后续打开或
校验失败的情况。如果无法确认清理完成或发布结果已持久化，后续导入会被拒绝，
直到修复 catalog 并重启 daemon。启动时会验证已发布条目，并删除中断导入遗留
且归 daemon 所有的 staging 目录。在执行扫描或清理前，daemon 会在已打开的
catalog 根目录上取得并持续持有独占锁；使用同一 catalog 的第二个 daemon 会在
检查或清理仍在使用的 staging 目录前直接失败。正常关闭时会拒绝新导入、取消
正在复制的任务，并等待相关文件句柄关闭。

API 只校验 artifact 结构，不证明 snapshot 能在特定 backend 上启动。当前的
sandbox create 不接受 template name，catalog 也尚未提供删除或引用跟踪。
