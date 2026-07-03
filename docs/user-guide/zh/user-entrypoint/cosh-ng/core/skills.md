# 技能系统

cosh-core 的技能系统允许通过 Markdown 文件定义可复用的 Agent 技能。LLM 可在对话中主动发现和调用已注册的技能。

## 技能搜索路径

技能按优先级从高到低在以下目录中搜索：

| 层级 | 路径 | 说明 |
|------|------|------|
| Project | `<project>/.copilot-shell/skills/` | 项目级技能 |
| Custom | config.toml `skills.custom_paths` | 自定义路径 |
| User | `~/.copilot-shell/skills/` | 用户级技能 |
| Extension | 扩展 `skill_dirs` 声明 | 通过扩展注册 |
| System | `/usr/share/anolisa/skills/` | 系统级（RPM 安装） |

同名技能按优先级覆盖（Project > Custom > User > Extension > System）。

## 技能文件格式

技能是一个 Markdown 文件（`.md`），包含 YAML frontmatter 和正文：

```markdown
---
name: check-disk
description: 检查磁盘使用情况并给出建议
---

# 检查磁盘使用情况

1. 执行 `df -h` 获取挂载点使用率
2. 对使用率超过 80% 的分区发出警告
3. 执行 `du -sh /var/log` 检查日志目录大小
```

## 配置

```toml
[skills]
custom_paths = ["~/my-skills", "/opt/team-skills"]
```

## 运行时行为

1. 启动时 `SkillManager` 扫描所有路径，构建技能缓存
2. 技能列表注入系统提示词的 `# Available Skills` 段
3. LLM 通过 `skill` 工具调用技能（传入技能名称）
4. 技能内容以 system message 形式注入对话上下文
5. 文件系统监听器自动检测新增/修改的技能文件（热加载）

## 禁用技能

通过状态文件 `~/.copilot-shell/states/skills.json` 管理：

```json
{ "disabled": ["dangerous-skill"] }
```

被禁用的技能不会出现在 LLM 的可见列表中。

## 与 copilot-shell 的区别

cosh-core 的技能系统镜像了 copilot-shell 的技能发现逻辑，但实现为纯 Rust。同一套技能文件可同时被两者加载。
