# Skills

[中文版](../../../../zh/user-entrypoint/cosh-ng/core/skills.md)

Skills are reusable instructions for recurring operating tasks. Add a Skill,
then let the Agent load it when the task matches.

## Manage Skills in cosh

```text
/skills
/skills detail <name>
/skills enable <name>
/skills disable <name>
```

Use `detail` to check which source won when names collide. Disabled Skills are
not offered to the Agent.

## Where Skills are loaded

The first matching name wins, in this order:

1. `<workspace>/.copilot-shell/skills/`
2. Paths in `skills.custom_paths`
3. `~/.copilot-shell/skills/`
4. Skill directories from Extensions
5. `/usr/share/anolisa/skills/`

Existing directories are watched and rescanned after changes.

## Create a Skill

The preferred layout is `<skill-name>/SKILL.md`; a flat `<name>.md` file is
also supported.

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

`name` and `description` are required. `allowedTools` is optional and may be a
YAML list or a comma-separated string.

## Add shared directories

Use `skills.custom_paths` to search team-maintained directories without copying
their files:

```toml
[skills]
custom_paths = ["~/team-skills", "/opt/company/skills"]
```

Paths expand `~`, `${VAR}`, and `$VAR`. Project paths are relative to the
workspace where Core starts.
