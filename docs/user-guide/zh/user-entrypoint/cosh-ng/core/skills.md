# Skills

[English](../../../../en/user-entrypoint/cosh-ng/core/skills.md)

Skills 是可复用的操作指令，用于处理重复任务。添加 Skill 后，任务匹配时 Agent 会加载它。

## 在 cosh 中管理 Skills

```text
/skills
/skills detail <name>
/skills enable <name>
/skills disable <name>
```

名称冲突时使用 `detail` 查看最终采用的来源。被禁用的 Skill 不会提供给 Agent。

## Skill 的搜索位置

同名 Skill 按下面的顺序查找，先找到的版本生效：

1. `<workspace>/.copilot-shell/skills/`
2. `skills.custom_paths` 中的路径
3. `~/.copilot-shell/skills/`
4. Extensions 提供的 Skill 目录
5. `/usr/share/anolisa/skills/`

已有目录会被监视，文件变化后自动重新扫描。

## 创建 Skill

推荐使用 `<skill-name>/SKILL.md` 目录布局，也兼容 `<name>.md` 单文件。

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

`name` 和 `description` 必填。`allowedTools` 可省略，可以使用 YAML 列表或逗号分隔字符串。

## 添加共享目录

使用 `skills.custom_paths` 搜索团队维护的目录，无需复制文件：

```toml
[skills]
custom_paths = ["~/team-skills", "/opt/company/skills"]
```

路径支持展开 `~`、`${VAR}` 和 `$VAR`。项目路径以 Core 启动时的工作空间为基准。
