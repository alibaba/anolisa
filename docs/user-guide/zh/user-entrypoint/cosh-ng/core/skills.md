# Skills

[English](../../../../en/user-entrypoint/cosh-ng/core/skills.md)

Skills 把反复使用的操作方法整理成 Agent 可以按需加载的指令。Agent 平时只读取名称和
简介，任务需要时才加载完整内容。

## 在 cosh 中使用 Skills

```text
/skills                     列出最终生效的 Skills
/skills detail <name>       查看一个 Skill 及其来源层级
/skills enable <name>       让被禁用的 Skill 重新可用
/skills disable <name>      对 Agent 隐藏一个 Skill
```

## 搜索顺序

多个位置存在同名 Skill 时，排在前面的版本生效。

| 优先级 | 位置 |
|---:|---|
| 1 | `<workspace>/.copilot-shell/skills/` |
| 2 | `skills.custom_paths` 中的路径 |
| 3 | `~/.copilot-shell/skills/` |
| 4 | Extensions 提供的 Skill 目录 |
| 5 | `/usr/share/anolisa/skills/` |

运行时会监视已有的搜索目录，文件变化后自动刷新 Skill 缓存。

## Skill 格式

推荐使用 `<skill-name>/SKILL.md` 目录布局。

```markdown
---
name: service-health
description: Inspect a systemd service and summarize actionable evidence
allowedTools:
  - shell
---

# Service health

Inspect status and recent logs before proposing a change. Ask for approval
before restarting the service.
```

`name` 和 `description` 用于识别 Skill。`allowedTools` 可省略，可以使用 YAML 列表或
逗号分隔字符串。系统仍兼容旧的 `<name>.md` 单文件格式，但推荐使用目录布局，以便携带
其他资源。

## 配置

共享只读目录可以直接加入搜索路径，无需复制到用户目录。

```toml
[skills]
custom_paths = ["~/team-skills", "/opt/company/skills"]
```

项目 Skills 和项目配置都以启动工作区为基准解析。存在同名 Skill 时，使用
`/skills detail <name>` 查看最终采用的版本和来源。
