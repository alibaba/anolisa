# Skills

[中文版](../../../../zh/user-entrypoint/cosh-ng/core/skills.md)

Skills turn repeatable operating knowledge into reusable Agent instructions.
The Agent sees compact Skill metadata and loads the full instructions only when
the task needs them.

## Use Skills from cosh

```text
/skills                     list effective Skills
/skills detail <name>       show one Skill and its source level
/skills enable <name>       make a disabled Skill available
/skills disable <name>      hide a Skill from the Agent
```

## Search order

When multiple levels contain the same name, the first level wins:

| Priority | Location |
|---:|---|
| 1 | `<workspace>/.copilot-shell/skills/` |
| 2 | Paths in `skills.custom_paths` |
| 3 | `~/.copilot-shell/skills/` |
| 4 | Skill directories contributed by extensions |
| 5 | `/usr/share/anolisa/skills/` |

The runtime watches existing search directories and refreshes the Skill cache
after filesystem changes.

## Skill format

The preferred layout is `<skill-name>/SKILL.md`:

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

`name` and `description` identify the Skill. `allowedTools` is optional and can
be a YAML list or comma-separated string. A legacy flat `<name>.md` file is
still read, but directory layout is preferred because it can contain supporting
resources.

## Configuration

Add shared read-only directories without copying them into the user store:

```toml
[skills]
custom_paths = ["~/team-skills", "/opt/company/skills"]
```

Project Skills and project configuration are evaluated relative to the
workspace sent by cosh-shell. Use `/skills detail <name>` when you need to
confirm which level won a name conflict.
