# SysOM 诊断类型索引（按 `service_name`）

本目录每个 **`*.md`** 描述 **该诊断在 `params` 内的专有字段**及建议用法（依据 **SysOM 诊断侧脚本与 OpenAPI 行为** 整理；实现不在本包内时以线上为准）。

## 与其它文档的分工（避免重复）

| 内容 | 在哪读 |
|------|--------|
| **诊断本机/远程、InvokeDiagnosis 请求体、`region`/`instance`、元数据补全** | [invoke-diagnosis.md](../invoke-diagnosis.md) |
| **ECS 元数据服务端点、常用路径、IMDS 说明** | [metadata-api.md](../../shared/references/metadata-api.md) |
| **precheck、凭证、三要素、场景 A-K** | [openapi-permission-guide.md](../../shared/references/openapi-permission-guide.md) |
| **本目录各 `service_name` 的字段表** | 下文索引 → 对应 `*.md` |

## 维护约定

- **OpenAPI 全量**诊断项以 **阿里云 SysOM 服务端** 注册的配置为准（本包不嵌服务端 `config` 文件）。
- **本技能文档**覆盖的 `service_name` 以 [SKILL.md](../SKILL.md) 能力表与下文「按分类索引」为准；服务端若另有诊断项，以控制台与 OpenAPI 为准。
- 专文应与 **`service_scripts`** 实现一致；与控制台不一致时以**代码与线上行为**为准。

### 运行命令时的当前目录

`cwd` 约定见 [agent-conventions.md](../agent-conventions.md)。

## 按分类索引（与 SKILL 透出表一致）

能力与 [SKILL.md](../SKILL.md) 总览表一致，下文按分类列出 **params 专文** 链接。

### 内存与 Java / Go

| service_name | 文档 |
|--------------|------|
| memgraph | [memgraph.md](./memgraph.md) |
| oomcheck | [oomcheck.md](./oomcheck.md) |
| javamem | [javamem.md](./javamem.md) |
| gomemdump（服务端若仍存在；**本技能 CLI 已不暴露**） | [gomemdump.md](./gomemdump.md) |

### IO 与磁盘（CLI：`io`）

| service_name | 文档 |
|--------------|------|
| iofsstat | [iofsstat.md](./iofsstat.md) |
| iodiagnose | [iodiagnose.md](./iodiagnose.md) |

### 网络（CLI：`net`）

| service_name | 文档 |
|--------------|------|
| packetdrop | [packetdrop.md](./packetdrop.md) |
| netjitter | [netjitter.md](./netjitter.md) |

### 负载与调度（CLI：`load`）

| service_name | 文档 |
|--------------|------|
| delay | [delay.md](./delay.md) |
| loadtask | [loadtask.md](./loadtask.md) |

## 实现源码（排障）

各诊断在 **SysOM 诊断服务** 侧通常有 `*_pre.py` 或同名脚本，经 **OpenAPI `invoke_diagnosis`** 路由到实例执行；源码不在本仓库技能包内时以 **线上行为** 为准。
