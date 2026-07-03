# Skill System

The cosh-core skill system allows defining reusable Agent skills via Markdown files. LLM can proactively discover and invoke registered skills during conversations.

## Skill Search Paths

Skills are searched in the following directories by priority (highest to lowest):

| Level | Path | Description |
|-------|------|-------------|
| Project | `<project>/.copilot-shell/skills/` | Project-level skills |
| Custom | config.toml `skills.custom_paths` | Custom paths |
| User | `~/.copilot-shell/skills/` | User-level skills |
| Extension | Extension `skill_dirs` declaration | Registered via extensions |
| System | `/usr/share/anolisa/skills/` | System-level (RPM installed) |

Skills with the same name are overridden by priority (Project > Custom > User > Extension > System).

## Skill File Format

A skill is a Markdown file (`.md`) containing YAML frontmatter and body:

```markdown
---
name: check-disk
description: Check disk usage and provide recommendations
---

# Check Disk Usage

1. Run `df -h` to get mount point utilization
2. Issue warnings for partitions with usage above 80%
3. Run `du -sh /var/log` to check log directory size
```

## Configuration

```toml
[skills]
custom_paths = ["~/my-skills", "/opt/team-skills"]
```

## Runtime Behavior

1. On startup, `SkillManager` scans all paths and builds a skill cache
2. The skill list is injected into the system prompt's `# Available Skills` section
3. LLM invokes skills via the `skill` tool (passing the skill name)
4. Skill content is injected into the conversation context as a system message
5. File system watcher automatically detects new/modified skill files (hot reload)

## Disabling Skills

Managed via the state file `~/.copilot-shell/states/skills.json`:

```json
{ "disabled": ["dangerous-skill"] }
```

Disabled skills do not appear in the LLM's visible list.

## Difference from copilot-shell

The cosh-core skill system mirrors copilot-shell's skill discovery logic but is implemented in pure Rust. The same skill files can be loaded by both simultaneously.
