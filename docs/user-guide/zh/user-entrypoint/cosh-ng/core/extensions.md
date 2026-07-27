# 扩展系统

cosh-core 的扩展系统通过 `cosh-extension.json` 配置文件注册技能目录和钩子。扩展是一个包含配置文件的目录，可来自系统级安装或用户级安装。

## 扩展目录

| 层级 | 路径 | 说明 |
|------|------|------|
| 用户级 | `~/.copilot-shell/extensions/<name>/` | 优先级高，覆盖同名系统扩展 |
| 系统级 | `/usr/share/anolisa/extensions/<name>/` | RPM/包管理器安装 |

用户级扩展与系统级扩展同名时，用户级覆盖系统级。

## 配置文件

每个扩展目录下必须包含 `cosh-extension.json`：

```json
{
  "name": "agent-sec-core",
  "version": "0.7.0",
  "skills": "skills",
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "^(run_shell_command|shell)$",
        "hooks": [
          {
            "type": "command",
            "name": "code-scanner",
            "command": "python3 ${extensionPath}/hooks/code_scanner_hook.py",
            "description": "Scans tool code for security vulnerabilities",
            "timeout": 5000
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "name": "context-loader",
            "command": "${extensionPath}/hooks/load-context.sh"
          }
        ]
      }
    ]
  }
}
```

### 字段说明

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 扩展唯一标识 |
| `version` | string | 否 | 版本号（默认 "0.0.0"） |
| `skills` | string \| string[] | 否 | 技能目录（默认 "skills"），相对于扩展根目录 |
| `hooks` | object | 否 | 按事件分组的钩子定义 |

### 钩子组结构

```json
{
  "matcher": "^shell$",
  "sequential": true,
  "hooks": [
    {
      "type": "command",
      "name": "hook-name",
      "command": "执行命令",
      "description": "说明",
      "timeout": 5000
    }
  ]
}
```

| 字段 | 说明 |
|------|------|
| `matcher` | 正则匹配工具名（仅 PreToolUse/PostToolUse 有效） |
| `sequential` | 组内钩子是否顺序执行 |
| `hooks[].command` | 钩子执行的 shell 命令 |
| `hooks[].timeout` | 超时时间（毫秒） |

## 变量替换

配置中的字符串字段支持变量替换：

| 变量 | 替换为 |
|------|--------|
| `${extensionPath}` | 扩展目录绝对路径 |
| `${workspacePath}` | 当前工作区目录 |
| `${/}` | 路径分隔符（Linux 始终为 `/`） |

示例：`"command": "python3 ${extensionPath}/hooks/check.py --dir=${workspacePath}"`

## 安装元数据

可选文件 `cosh-extension-install.json` 记录安装来源：

```json
{
  "source": "/path/to/local/extension",
  "type": "link",
  "installed_at": "2026-07-01T10:00:00Z"
}
```

`type` 可选值：`"local"`（复制安装）或 `"link"`（符号链接）。

## 启用/禁用

通过 Registry 协议管理扩展状态：

```bash
# 列出扩展
echo '{"type":"registry_request","request_id":"r1","domain":"extensions","action":"list","params":{}}' \
  | cosh-core --registry

# 查看详情
echo '{"type":"registry_request","request_id":"r2","domain":"extensions","action":"detail","params":{"name":"agent-sec-core"}}' \
  | cosh-core --registry

# 禁用扩展
echo '{"type":"registry_request","request_id":"r3","domain":"extensions","action":"disable","params":{"name":"hook-test"}}' \
  | cosh-core --registry

# 启用扩展
echo '{"type":"registry_request","request_id":"r4","domain":"extensions","action":"enable","params":{"name":"hook-test"}}' \
  | cosh-core --registry
```

禁用状态持久化在 `~/.copilot-shell/states/extensions.json`：

```json
{ "disabled": ["hook-test"] }
```

禁用扩展后，其钩子和技能均不生效。启用时自动清除该扩展相关钩子的禁用状态。

## 加载流程

1. 扫描系统级目录（低优先级）
2. 扫描用户级目录（高优先级，同名覆盖）
3. 读取 `cosh-extension.json` 并解析
4. 执行变量替换（`${extensionPath}` 等）
5. 加载 `cosh-extension-install.json`（如存在）
6. 应用禁用状态（从 `extensions.json` 读取）
7. 按扩展名字母排序
