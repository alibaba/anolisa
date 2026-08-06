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
