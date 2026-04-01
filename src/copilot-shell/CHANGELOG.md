# Changelog

## 2.0.1

- Renamed OpenAI authentication label to "BaiLian (OpenAI Compatible)" for clarity.
- Fixed login shell stdin drain to prevent unwanted input echo.
- Removed ripgrep unavailable warning message.

## 2.0.0

- Synced upstream `qwen-code` to v0.9.0 and rebranded to **Copilot Shell**.
- Bumped version directly to 2.0.0 (skipping 1.x, which was used by a previous `OS Copilot` release).
- Integrated Skill-OS online remote skill discovery with priority-based fallback (Project > User > Extension > Remote).
- Added `/skills remote` and `/skills cache clear` commands for remote skill management.
- Added `/bash` interactive shell mode
- Added `-c` argument support for inline bash commands.
- Added PTY mode for `sudo` command support.
- Added hooks system with PreToolUse event for intercepting tool calls before execution.
- Added new model provider named Aliyun
- Added nested startup detection warning banner.
- Added system-wide skill path (`/usr/share`) support.
- Removed original Gemini sandbox.
- Fixed skill frontmatter parsing for YAML special characters (`|`, `&`, `>`).
- Fixed login escaped character echo issue in ECS workbench.
- Fixed Linux headless environment browser open failure when auth with Qwen OAuth.
- Fixed Qwen OAuth authentication, replay, and UI rendering issues.
- Fixed exception handling when adding workspace directories.
- Fixed user query start with unix path being misidentified as command.
- Fixed API key display explicitly.
- Fixed Chinese i18n for `/resume` command.
- Improved `?` hint visibility — hidden while user is typing.
- Miscellaneous UI, branding, CI, and build improvements.
