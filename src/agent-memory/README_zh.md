# Agent Memory

[English](README.md)

面向 AI Agent 的 CMA 风格持久化文件系统记忆服务，基于 MCP 协议。提供沙箱化文件工具、BM25 + 向量混合检索、自动捕获/召回、git 版本控制和 tar.gz 快照。Agent Memory 是 [ANOLISA](../../README_zh.md) 的记忆组件。仅支持 Linux。

## 特性

- **文件形态记忆** — 通过 37 个 MCP 工具以文件系统语义读写记忆；命名空间隔离与路径沙箱（openat2 RESOLVE_BENEATH）
- **混合语义检索** — BM25 关键词 + 稠密向量嵌入，通过倒数排名融合（RRF）组合；时间衰减排序
- **自动捕获与召回** — 对话结束时自动提取观察，下一次 prompt 构建前注入相关记忆
- **记忆聚合** — 从会话审计日志中自动提取原子事实
- **版本控制与快照** — 可选 git 自动提交 + tar.gz 快照，支持文件级和挂载级回滚
- **安全** — 注入内容的 prompt 注入检测与密钥/PII 脱敏
- **跨会话任务** — 跨会话保存/恢复/关闭任务及完整上下文

## 快速开始

### 安装

```bash
# 推荐
anolisa install agent-memory

# 或通过 RPM（Alinux）
sudo yum install agent-memory
```

### 集成（MCP 客户端）

添加到 MCP 配置（Claude Code、Cursor 等）：

```json
{
  "mcpServers": {
    "agent-memory": {
      "command": "/usr/bin/agent-memory",
      "args": [],
      "env": {
        "USER_ID": "alice",
        "MEMORY_PROFILE": "advanced"
      }
    }
  }
}
```

## 架构

单进程 Tokio 异步运行时，通过 stdio JSON-RPC 2.0 暴露 37 个 MCP 工具：

- **Tier A**（11 工具）：文件操作
- **Tier B**（6 工具）：结构化检索
- **Tier C**（7 工具）：治理（快照、版本控制、聚合）
- **主权**（13 工具）：关于、遗忘、同意、导入导出、任务、梦境合成

## 许可证

Apache License 2.0 — 详见 [LICENSE](LICENSE)。
