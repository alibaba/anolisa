---
name: install-openclaw
version: 1.0.0
description: 在 RPM 包管理的 Linux 服务器（如 Alibaba Cloud Linux）上安装和配置 OpenClaw，接入钉钉频道与阿里云百炼（通义千问）模型。当用户需要部署 OpenClaw、配置钉钉机器人、接入 DashScope/Qwen 模型、排查 OpenClaw 网关无法启动、或遇到插件不加载等问题时使用此技能。包含完整踩坑记录与正确配置结构。
layer: application
lifecycle: usage
---

# OpenClaw 安装配置指南（钉钉 + 百炼 Qwen）

## 目录结构

```
install-openclaw/
├── SKILL.md                          # 本文件，主流程
├── scripts/
│   └── configure_openclaw_dingtalk.py   # 配置写入脚本（钉钉频道）
└── references/
    ├── troubleshooting.md            # 所有已知踩坑与排查手册
    └── dingtalk-setup-guide.md       # 钉钉开发者平台配置详细步骤
```

遇到问题时先查 `references/troubleshooting.md`，钉钉侧操作细节查 `references/dingtalk-setup-guide.md`。

---

## 第一部分：环境安装

### Step 1：安装 Node.js 22

**优先使用系统包管理器（推荐）**

```bash
# Linux 仓库自带 Node.js 22
dnf install -y nodejs
node -v && npm -v
```

**nvm 方式（系统源没有 Node.js 22 时的备选）**

```bash
touch ~/.zshrc
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh | bash
source ~/.zshrc
nvm install 22 && nvm use 22 && node -v
```

### Step 2：安装 OpenClaw

⚠️ 先检查是否安装 git

```bash
dnf install git 
```

```bash
# npm 方式安装
npm i -g openclaw
# 如果速度慢可以指定国内镜像 
npm i -g openclaw --registry=https://registry.npmmirror.com
# 备选考虑使用脚本安装
curl -fsSL https://openclaw.ai/install.sh | bash
```

检验是否安装成功

```bash
openclaw --version
```

---

## 第二部分：安装钉钉插件

```bash
# 标准安装
openclaw plugins install @soimy/dingtalk

# 国内网络超时时加镜像
NPM_CONFIG_REGISTRY=https://registry.npmmirror.com openclaw plugins install @soimy/dingtalk


```

验证：

```bash
openclaw plugins list   # 输出中应包含 dingtalk，状态为 installed
```

> 安装中断导致半安装状态的修复方法见 `references/troubleshooting.md` — 坑 7。

---

## 第三部分：收集配置参数

向用户收集以下信息：

### 必填

| 参数 | 说明 | 来源 |
|------|------|------|
| `--dingtalk-client-id` | 钉钉应用 AppKey | [钉钉开发者后台](https://open-dev.dingtalk.com/) |
| `--dingtalk-client-secret` | 钉钉应用 AppSecret | 同上 |
| `--dashscope-api-key` | 百炼 API Key | [百炼控制台](https://dashscope.console.aliyun.com/) |

### 可选（有默认值）

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--model-id` | `qwen3-max-2026-01-23` | 模型版本 |
| `--dm-policy` | `open` | 私聊策略：`open` / `pairing` / `allowlist` |
| `--group-policy` | `open` | 群聊策略：`open` / `allowlist` |
| `--message-type` | `markdown` | 消息类型：`markdown` / `card` |
| `--dingtalk-robot-code` | - | 机器人代码（通常与 AppKey 相同）|
| `--dingtalk-corp-id` | - | 企业 ID |
| `--dingtalk-agent-id` | - | 应用 ID |

---

## 第四部分：执行配置脚本

脚本位于 `scripts/configure_openclaw_dingtalk.py`，直接读写 `~/.openclaw/openclaw.json`（deep_merge 策略，不覆盖已有 `plugins.entries` / `installs` / `gateway.auth` 等系统字段）。

```bash
python3 /path/to/scripts/configure_openclaw_dingtalk.py \
  --dingtalk-client-id "dingxxxxxx" \
  --dingtalk-client-secret "your-secret" \
  --dashscope-api-key "sk-xxxxxxxx" \
  --model-id "qwen3-max-2026-01-23" \
  --dm-policy "open" \
  --group-policy "open" \
  --message-type "markdown"
```

卡片模式额外参数：

```bash
  --message-type "card" \
  --card-template-id "xxxxx-xxxxx-xxxxx.schema" \
  --card-template-key "content"
```

脚本写入的配置节：`plugins`、`models`、`agents`、`channels`、`messages`、`commands`、`session`、`gateway.mode`、`skills.load`。

---

## 第五部分：初始化 Gateway（首次安装必做）

安装 gateway

```bash
openclaw gateway install
```

运行 doctor 生成 `gateway.auth.token`：

```bash
openclaw doctor --fix
```

> **重要**：`doctor --fix` 只写 `gateway.auth.token`，**不写 `gateway.mode`**。
> 配置脚本已自动写入 `gateway.mode: "local"`，但如果跳过了脚本直接到这步，网关会启动失败。
> 详见 `references/troubleshooting.md` — 坑 1。

doctor 完成后确认 `openclaw.json` 中同时存在：

```json
"gateway": {
  "mode": "local",
  "auth": { "mode": "token", "token": "..." }
}
```

---

## 第六部分：启动并验证

```bash
openclaw gateway restart
ss -tlnp | grep 18789          # 应看到端口监听
openclaw gateway status
```

如果端口不监听，立即运行：

```bash
openclaw gateway run --verbose 2>&1 | head -5   # 查看实际报错
```

常见原因和修复方法见 `references/troubleshooting.md`。

---

## 第七部分：钉钉开发者平台配置

配置脚本执行完成后，告知用户完成以下钉钉侧设置：

1. 访问 [钉钉开发者后台](https://open-dev.dingtalk.com/) 创建**企业内部应用**
2. 添加「机器人」能力，消息接收模式选 **Stream 模式**
3. 权限管理中开启：`Card.Instance.Write`、`Card.Streaming.Write`、机器人消息发送、媒体文件上传
4. 发布应用

详细步骤见 `references/dingtalk-setup-guide.md`。

---

## 服务管理速查

| 操作 | 命令 |
|------|------|
| 重启 | `openclaw gateway restart` |
| 停止 | `openclaw gateway stop` |
| 状态 | `openclaw gateway status` |
| 健康检查 | `openclaw gateway health` |
| 前台调试 | `openclaw gateway run --verbose` |
| 更新插件 | `openclaw plugins update dingtalk` |
| 更新插件（国内）| `NPM_CONFIG_REGISTRY=https://registry.npmmirror.com openclaw plugins update dingtalk` |

---

## openclaw.json 完整参考结构

```json
{
  "meta": { "lastTouchedVersion": "...", "lastTouchedAt": "..." },
  "wizard": { "lastRunAt": "...", "lastRunVersion": "...", "lastRunCommand": "doctor", "lastRunMode": "local" },
  "gateway": {
    "mode": "local",
    "auth": { "mode": "token", "token": "..." }
  },
  "plugins": {
    "enabled": true,
    "allow": ["dingtalk"],
    "entries": { "dingtalk": { "enabled": true } },
    "installs": { "dingtalk": { "source": "npm", "spec": "@soimy/dingtalk", ... } }
  },
  "models": {
    "mode": "merge",
    "providers": {
      "bailian": {
        "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "apiKey": "sk-...",
        "api": "openai-completions",
        "models": [{ "id": "qwen3-max-2026-01-23", "name": "qwen3-max-thinking", "reasoning": false, "input": ["text"], "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 }, "contextWindow": 262144, "maxTokens": 65536 }]
      }
    }
  },
  "agents": {
    "defaults": {
      "model": { "primary": "bailian/qwen3-max-2026-01-23" },
      "models": { "bailian/qwen3-max-2026-01-23": { "alias": "qwen3-max-thinking" } },
      "maxConcurrent": 4,
      "subagents": { "maxConcurrent": 8 }
    }
  },
  "channels": {
    "dingtalk": {
      "enabled": true,
      "clientId": "dingxxxxxx",
      "clientSecret": "...",
      "dmPolicy": "open",
      "groupPolicy": "open",
      "debug": false,
      "messageType": "markdown",
      "allowFrom": ["*"]
    }
  },
  "messages": { "ackReactionScope": "group-mentions" },
  "commands": { "native": "auto", "nativeSkills": "auto", "restart": true, "ownerDisplay": "raw" },
  "session": { "dmScope": "per-channel-peer" },
  "skills": {
    "load": {
      "extraDirs": ["/usr/share/anolisa/skills"]
    }
  }
}
```
