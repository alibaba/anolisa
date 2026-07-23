# ANOLISA Blaze

[English](README.md)

面向 AI Agent 工作负载的单机 sandbox 编排 daemon。

Blaze 通过 HTTP API 管理 sandbox 实例的完整生命周期，支持策略驱动的后端选择。
它提供 warm pool 预分配、多后端回退（Firecracker → Bubblewrap → Mock）以及
Prometheus 指标导出，设计为 E2B 类编排平台的单机执行代理。

## 特性

- **HTTP API** — Unix domain socket (`/run/blaze/api.sock`) + TCP (`:14159`)
- **策略驱动后端选择** — workload class → 后端优先级列表
- **生命周期状态机** — 8 种状态（Pending → Creating → Running → Paused → Checkpointed → Reset → Warm → Destroyed）
- **Warm pool 管理** — 预热实例 + 基于 TTL 的 GC
- **模板注册表** — 内存中模板追踪，支持空闲驱逐
- **内核 hook 注册** — 前/后置 hook 状态追踪
- **Prometheus 指标** — 请求计数、实例 gauge、池大小
- **Spawner 后端** — FirecrackerSpawner、BubblewrapSpawner、MockSpawner

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
curl -X POST --unix-socket /run/blaze/api.sock http://localhost/v1/instances \
  -H 'Content-Type: application/json' \
  -d '{"workload_class":"agent-rl","image_digest":"sha256:..."}'
```

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
```

## API 端点

| 方法 | 路径 | 说明 |
|--------|------|-------------|
| GET | `/v1/health` | 健康检查 |
| GET | `/v1/instances` | 列出所有实例 |
| POST | `/v1/instances` | 创建新 sandbox 实例 |
| GET | `/v1/instances/{id}` | 获取实例详情 |
| POST | `/v1/instances/{id}/checkpoint` | 对实例做 checkpoint |
| POST | `/v1/instances/{id}/reset` | 将实例重置到 checkpoint |
| POST | `/v1/instances/{id}/destroy` | 销毁实例 |
| GET | `/v1/pools` | 列出 warm pool |
| GET | `/v1/pools/{backend}/{class}` | 获取 pool 状态 |
| POST | `/v1/pools/{backend}/{class}/drain` | 排空 pool |
| PUT | `/v1/pools/{backend}/{class}/sizing` | 调整 pool 大小 |
| GET | `/v1/templates` | 列出模板 |
| GET | `/v1/templates/{id}` | 查看模板详情 |
| POST | `/v1/templates/gc` | 触发模板 GC |
| GET | `/v1/policies` | 列出已加载策略 |
| GET | `/v1/hooks` | 列出内核 hook |
| GET | `/v1/metrics` | Prometheus 指标 |
| POST | `/v1/admin/reload` | 热加载策略 |

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

## 许可证

Apache-2.0
