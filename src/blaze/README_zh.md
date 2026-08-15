# ANOLISA Blaze

[English](README.md)

面向 AI Agent 工作负载的单机 sandbox 编排 daemon。

Blaze 通过 HTTP API 管理 sandbox 实例的完整生命周期，支持策略驱动的后端选择。
它提供 warm pool 预分配、多后端回退（Firecracker → Bubblewrap → Mock）以及
Prometheus 指标导出，设计为 E2B 类编排平台的单机执行代理。

## 特性

- **HTTP API** — Unix domain socket (`/run/blaze/api.sock`) + TCP (`:14159`)
- **策略驱动后端选择** — workload class → 后端优先级列表
- **生命周期状态机** — 9 种状态：Pending、Creating、Running、Paused、
  Checkpointed、RecoveryRequired、Reset、Warm 和 Destroyed
- **Guest 操作** — 对提供 guest endpoint 的运行中后端执行有界命令和文件传输
- **Warm pool 管理** — 预热实例 + 基于 TTL 的 GC
- **Template catalog** — 有界导入并原子发布可复用 artifact
- **内核 hook 注册** — 前/后置 hook 状态追踪
- **Prometheus 指标** — 请求计数、实例 gauge、池大小
- **Spawner 后端** — FirecrackerSpawner、BubblewrapSpawner、MockSpawner
- **可选 VM 网络** — 每台 Firecracker VM 独立使用 netns、tap、veth 和 NAT

## 快速开始

```bash
# 构建
cd src/blaze
cargo build --release

# 运行 daemon（开发环境：覆盖 policy.dir 使用本地示例）
sudo ./target/release/blazed daemon start --config examples/config.toml
# 注意：默认配置设置 policy.dir = /etc/anolisa/blaze/policies。
# 源码开发测试时，创建符号链接或覆盖：
#   sudo mkdir -p /etc/anolisa/blaze
#   sudo ln -s $(pwd)/examples/policies /etc/anolisa/blaze/policies

# 健康检查
curl --unix-socket /run/blaze/api.sock http://localhost/v1/health

# 创建 sandbox
curl -X POST --unix-socket /run/blaze/api.sock http://localhost/v1/sandboxes \
  -H 'Content-Type: application/json' \
  -d '{"workload_class":"agent-tool","image_digest":"sha256:..."}'
```

快速开始使用关闭 Firecracker guest transport 的示例策略，因此没有兼容
guest agent 的镜像不会等待 guest 就绪。只有镜像运行了对应 agent 时才应
启用该 transport。

## 配置

daemon 读取 TOML 配置文件（默认：`/etc/anolisa/blaze/config.toml`）
以及包含按 workload class 划分的策略文件的策略目录。

```
/etc/anolisa/blaze/
├── config.toml
└── policies/
    ├── agent-rl.toml
    └── agent-tool.toml
```

参见 `src/blaze/examples/` 获取带注释的示例配置。

### VM 资源配置

Blaze 使用三层回退链解析 vCPU 和内存设置：

1. **后端特定**（`[backend.firecracker].vcpus` / `.memory`）— 最高优先级
2. **策略级**（`[vm].vcpus` / `[vm].memory`）— 跨后端共享
3. **代码默认值**（1 vCPU, 256 MiB）— 未指定时的兜底

策略文件示例：

```toml
[vm]
vcpus = 2
memory = "512Mi"

[backend.firecracker]
vcpus = 4        # 仅对 Firecracker 覆盖 [vm].vcpus
memory = "1Gi"   # 仅对 Firecracker 覆盖 [vm].memory
enable_network = false
```

设置 `enable_network = true` 后，每台 Firecracker VM 会获得独立的网络
slot。显式销毁 sandbox 和启动失败补偿会在进程确认终止后删除对应的 netns、
tap 和 veth。daemon 重启后再次销毁时可以根据记录恢复清理，但不会在后台
自动扫描。slot 创建和删除使用主机级锁，避免多个 daemon 同时分配相同的主机
设备名。加载的 Firecracker 策略启用该选项时，backend probe 还会检查所需
命令和主机权限；网络关闭时跳过这些检查。上游路由和 DNS 仍由主机运维方
配置。

### 存储配置

`[storage]` 部分控制 sandbox 存储后端：

```toml
[storage]
provider = "file"       # 存储 provider 选择。当前支持："file"、"auto"。
                        # "auto" 按优先级探测可用 provider（当前等同于 "file"）。
                        # 其他值将记录告警并回退到 file。
images_dir = "/var/lib/blaze/images"
# pool_size = 0           # [Reserved] 预热存储槽位数（尚未启用）
# prefork = false         # [Reserved] 是否在槽位中预启动 VM（尚未启用）
sync_interval = "disabled" # 设置正数 duration 后持久化 slot 中已经写入的制品。
sync_timeout = "30s"       # scheduler 等待 slot 重建与制品同步的最长时间。
```

`file` provider 使用标准文件系统操作管理 sandbox 存储。`auto` 按优先级探测可用 provider（当前等同于 `file`）。无法识别的值将记录告警并回退到 `file`。
现有存储配置字段、HTTP API 和正常生命周期行为保持不变。文件存储提供程序现在会
拒绝包含 `..` 或符号链接组件的实例根路径、解析后位置重叠的存储根、会覆盖守护进程
状态目录或进入沙箱 UUID 子树的实例根（包括通过别名重叠），以及已经由另一个
守护进程占用的实例根。首次启动时仍会自动创建缺失的实例根，但用于发布目录的每个
父目录及最终实例根必须由超级用户或守护进程的有效用户所有，并且不得允许组用户或
其他用户写入。已有路径中的共享可写祖先只有在设置粘滞位、且下一级组件由超级用户或
守护进程的有效用户所有时才会被接受。如果原子发布时目标名称已被占用，或者已发布的
存储槽在分配完成前被替换，操作会失败；当前守护进程不会采用或删除替换目录。该清理
禁令不会跨守护进程重启保留，重启前必须检查或恢复路径。超级用户和与守护进程使用
相同有效用户的进程属于主机管理信任边界；更改存储路径前应停止 Blaze。
如果 Blaze 运行期间配置路径被替换，新的存储分配和重建会失败关闭。释放操作以及
已经完成重建的同步操作仍作用于启动时打开的原存储根。
完整行为见[文件存储兼容性与安全检查](../../docs/user-guide/zh/runtime/blaze.md#文件存储兼容性与安全检查)。
启用周期同步后，已经返回的 provider 失败不会中断后续 sandbox。如果 provider
在 deadline 到达时仍无法停止文件系统操作，该操作会继续持有 sandbox operation
lock 和唯一的同步许可直至完成；后续同步会被推迟而不会不断累积。service loop
结束时，worker 会停止调度新任务。

[存储制品同步用户指南](../../docs/user-guide/zh/runtime/blaze.md#存储制品同步)进一步说明
配置、选择、重试和 worker 关闭行为。

## API 端点

| 方法 | 路径 | 说明 |
|--------|------|-------------|
| GET | `/v1/health` | 健康检查 |
| GET | `/v1/sandboxes` | 列出所有 sandbox |
| POST | `/v1/sandboxes` | 创建 sandbox |
| GET | `/v1/sandboxes/{id}` | 获取 sandbox 详情 |
| DELETE | `/v1/sandboxes/{id}` | 销毁 sandbox |
| POST | `/v1/sandboxes/{id}/exec` | 执行 guest 命令 |
| POST | `/v1/sandboxes/{id}/read` | 读取 guest 文件 |
| POST | `/v1/sandboxes/{id}/write` | 替换 guest 文件 |
| GET | `/v1/instances` | 列出 sandbox 的兼容入口 |
| POST | `/v1/instances` | 创建 sandbox 的兼容入口 |
| GET | `/v1/instances/{id}` | 获取 sandbox 详情的兼容入口 |
| DELETE | `/v1/instances/{id}` | 销毁 sandbox 的兼容入口 |
| POST | `/v1/instances/{id}/destroy` | 保留的销毁 action |
| POST | `/v1/instances/{id}/exec` | Guest 命令兼容入口 |
| POST | `/v1/instances/{id}/read` | Guest 文件读取兼容入口 |
| POST | `/v1/instances/{id}/write` | Guest 文件写入兼容入口 |
| POST | `/v1/instances/{id}/checkpoint` | 记录 checkpoint 状态 |
| POST | `/v1/instances/{id}/reset` | 记录 reset 并返回 warm pool |
| GET | `/v1/pools` | 列出 warm pool |
| GET | `/v1/pools/{backend}/{class}` | 获取 pool 状态 |
| POST | `/v1/pools/{backend}/{class}/drain` | 排空 pool |
| PUT | `/v1/pools/{backend}/{class}/sizing` | 调整 pool 大小 |
| GET | `/v1/templates` | 列出已发布 template 的名称 |
| GET | `/v1/templates/{name}` | 查看已发布 template 的 metadata |
| POST | `/v1/templates/import` | 从配置的导入根目录发布 template |
| GET | `/v1/policies` | 列出已加载策略 |
| GET | `/v1/hooks` | 列出内核 hook |
| GET | `/v1/metrics` | Prometheus 指标 |
| POST | `/v1/admin/reload` | 热加载策略 |

`/v1/templates` 是唯一面向运维人员的 template catalog。导入条目目前不会让
sandbox create 自动选择它；后续 create 支持会从同一个 catalog 解析可选名称。
配置方法、接受的 artifact、上限和发布规则参见
[Template catalog 用户指南](../../docs/user-guide/zh/runtime/blaze.md#template-catalog)。

### 生命周期管理与恢复

创建和销毁会在修改存储或后端资源之前记录当前操作。创建成功后状态为
`Running`，销毁成功后状态为 `Destroyed`。如果失败补偿不能释放全部已有
资源，sandbox 会保留为可查询的 `RecoveryRequired`，后续可以再次执行销毁。

daemon 启动时，Blaze 会先校验完整的生命周期清单。只有清单完整且一致时，
daemon 才会逐个处理未结束的 sandbox。后续逐项恢复期间，如果单个 sandbox
清理失败，该 sandbox 会保留为 `RecoveryRequired`，但不会阻止其他已通过校验的
记录继续处理，也不会阻止 API 启动。

清单校验采用 fail-closed 策略。如果 UUID 所属条目不是规范命名的目录，
`state.json` 缺失、不可读、是符号链接或目录、存在其他
硬链接，记录内的 sandbox ID 与目录名不同，或者 `Destroyed` 记录仍保留活动操作
或可能仍存活的后端所有权，daemon 都会在打开 API 监听器前停止。Blaze 不会自动
修复或删除这些记录。接受这份清单前，Blaze 还会确认每个 UUID 名称和其中的
`state.json` 仍然指向刚才读取的对象；具体流程是先完成第二次规范 UUID 名称
枚举并比较完整集合，再逐一复验保留的目录和记录。如果第二次枚举发现条目新增
或删除，或者后续对象检查发现保留对象消失或被替换，daemon 会停止启动。这一
一致性合同面向 Blaze 状态写入者：生产 store 持有 state root advisory lock，扫描
也会持有进程内 ownership map 锁直至发布。绕过 state root 锁直接修改文件的外部
进程不在支持范围内。

写入协调、清单发布和失败边界参见
[生命周期状态一致性设计](docs/design/lifecycle-state-consistency_zh.md)。

操作记录只保存操作类型和开始时间，不记录每个资源步骤是否已经完成。中断的
创建会被清理而不是从原位置继续，重启后也不会接管先前的后端进程。恢复失败
后目前没有后台循环自动重试。checkpoint 和 reset 接口保持原有的元数据状态
变化；这里的恢复流程没有增加后端 snapshot 或 restore 操作。

### Guest 操作

当 backend 提供兼容的 guest endpoint 时，运行中的 sandbox 可以执行有界
命令和文件传输。生产环境的 mock fallback 不会声明该能力。请求格式、上限、
就绪检查、错误处理和当前关闭边界参见
[Blaze 用户指南](../../docs/user-guide/zh/runtime/blaze.md#guest-操作)。

#### 健康检查

`GET /v1/health` 返回 daemon 状态，包含存储池就绪信息：

```json
{
  "status": "ok",
  "version": "0.3.0",
  "storage_pool": { "ready": 0, "capacity": 0, "pending": 0 }
}
```

## 项目结构

```
src/blaze/
├── crates/
│   ├── blaze-core/   # 库：策略、生命周期、池、模板、内核、配置
│   └── blazed/       # 二进制：daemon、API server、spawner、指标
├── examples/         # config.toml、policies/
├── dist/             # blazed.service、blaze.spec、tmpfiles
└── manifests/        # 组件元数据
```

## 环境要求

- Rust 1.88+（参见 `src/blaze/rust-toolchain.toml`）
- 具有 root 权限的 Linux 主机（sandbox 后端需要）
- 启用 VM 网络时需要 `ip`、`iptables`、`sysctl` 和 netns 管理权限

## 许可证

Apache-2.0
