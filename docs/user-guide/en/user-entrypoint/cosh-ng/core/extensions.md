# Extension System

The cosh-core extension system registers skill directories and hooks via `cosh-extension.json` configuration files. An extension is a directory containing a configuration file, which can come from system-level or user-level installations.

## Extension Directories

| Level | Path | Description |
|-------|------|-------------|
| User-level | `~/.copilot-shell/extensions/<name>/` | Higher priority, overrides same-name system extensions |
| System-level | `/usr/share/anolisa/extensions/<name>/` | RPM/package manager installed |

When user-level and system-level extensions share the same name, user-level overrides system-level.

## Configuration File

Each extension directory must contain `cosh-extension.json`:

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

### Field Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Extension unique identifier |
| `version` | string | No | Version number (default "0.0.0") |
| `skills` | string \| string[] | No | Skill directory (default "skills"), relative to extension root |
| `hooks` | object | No | Hook definitions grouped by event |

### Hook Group Structure

```json
{
  "matcher": "^shell$",
  "sequential": true,
  "hooks": [
    {
      "type": "command",
      "name": "hook-name",
      "command": "execution command",
      "description": "description",
      "timeout": 5000
    }
  ]
}
```

| Field | Description |
|-------|-------------|
| `matcher` | Regex matching tool names (only effective for PreToolUse/PostToolUse) |
| `sequential` | Whether hooks in the group execute sequentially |
| `hooks[].command` | Shell command the hook executes |
| `hooks[].timeout` | Timeout in milliseconds |

## Variable Substitution

String fields in configuration support variable substitution:

| Variable | Replaced With |
|----------|--------------|
| `${extensionPath}` | Extension directory absolute path |
| `${workspacePath}` | Current workspace directory |
| `${/}` | Path separator (always `/` on Linux) |

Example: `"command": "python3 ${extensionPath}/hooks/check.py --dir=${workspacePath}"`

## Installation Metadata

Optional file `cosh-extension-install.json` records the installation source:

```json
{
  "source": "/path/to/local/extension",
  "type": "link",
  "installed_at": "2026-07-01T10:00:00Z"
}
```

`type` options: `"local"` (copy install) or `"link"` (symbolic link).

## Enable/Disable

Manage extension state via the Registry protocol:

```bash
# List extensions
echo '{"type":"registry_request","request_id":"r1","domain":"extensions","action":"list","params":{}}' \
  | cosh-core --registry

# View details
echo '{"type":"registry_request","request_id":"r2","domain":"extensions","action":"detail","params":{"name":"agent-sec-core"}}' \
  | cosh-core --registry

# Disable extension
echo '{"type":"registry_request","request_id":"r3","domain":"extensions","action":"disable","params":{"name":"hook-test"}}' \
  | cosh-core --registry

# Enable extension
echo '{"type":"registry_request","request_id":"r4","domain":"extensions","action":"enable","params":{"name":"hook-test"}}' \
  | cosh-core --registry
```

Disabled state is persisted in `~/.copilot-shell/states/extensions.json`:

```json
{ "disabled": ["hook-test"] }
```

After disabling an extension, its hooks and skills are no longer effective. Enabling automatically clears the disabled state of hooks related to that extension.

## Loading Flow

1. Scan system-level directory (lower priority)
2. Scan user-level directory (higher priority, overrides same names)
3. Read `cosh-extension.json` and parse
4. Perform variable substitution (`${extensionPath}`, etc.)
5. Load `cosh-extension-install.json` (if present)
6. Apply disabled state (read from `extensions.json`)
7. Sort by extension name alphabetically
