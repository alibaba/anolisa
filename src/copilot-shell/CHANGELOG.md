# Changelog

## 2.2.0

- Added `ask` decision support for UserPromptSubmit hook. (#328)
- Added new command for Clawhub CLI. (#313)
- Added interactive Skills TUI Panel with enable/disable support. (#311)
- Added variable substitution and display control for extension TOML commands. (#291)
- Added immediate hook activation on extension install/uninstall. (#283)
- Added `ask` decision support for PreToolUse hooks. (#276)
- Added configurable status bar. (#251)
- Added `/export` command for session history. (#245)
- Fixed API key validation to skip non-Dashscope providers. (#337)
- Fixed PreToolUse ask dialog by unifying it to info type with diff preview. (#345)
- Fixed memory leak in memory management. (#309)
- Fixed extension lifecycle reliability. (#298)
- Fixed hook registry sync on extension enable/disable. (#298)
- Fixed interface crash caused by leftBottomContent of Box nested in Text in Footer. (#293)
- Fixed `/hooks install` command by removing it and adding default help. (#287)
- Fixed extension examples installation and package configuration. (#271)

## 2.1.0

- Added startup bash entry and simplified manual auth dialog. (#217)
- Added async fzf-based tab completion optimization. (#214)
- Fixed OpenAI API key and model validation via /models endpoint on auth. (#243)
- Fixed API key retention when navigating to apiKey field in auth dialog. (#241)
- Fixed node-pty native binary bundling for both linux architectures. (#232)
- Fixed stream redaction by replacing integer offset with committed text reference. (#210)
- Fixed missing fields in hook system. (#188)

## 2.0.4

- Added STS authentication support via ECS RAM role. (#161)
- Added BeforeModel, AfterModel, and BeforeToolSelection hooks. (#154)
- Added sandbox usage summary on session exit. (#137)
- Added Tab-completion for `!` shell mode. (#131)
- Fixed config-dir source unification and prevented ~/.copilot creation on startup. (#171)
- Fixed /bug command crash in headless environment. (#175)
- Fixed undefined metrics.sandbox in StatsDisplay. (#171)
- Supplement /hooks install step to post-installation guide. (#142)
- Supplement hooks documentation (index, reference, writing-hooks). (#142)

## 2.0.3

- Migrated config directory from `~/.copilot` to `~/.copilot-shell`. (#78)
- Added API key detection from configured agents with user approval on bootstrap. (#127)
- Added support for configuring multiple custom model providers. (task#80737766)
- Added global API endpoint support for Dashscope. (#133)
- Added custom skill paths support via `settings.json`. (#128)
- Added support for loading skills from extension directories with `cosh-extension.json` compatibility. (#54)
- Added `/bug` command for submitting bug reports. (#122)
- Added sandbox-guard install command with bypass approval flow. (#125)
- Added secret redaction for model output and tool results. (#100)
- Added extensible feature tip banner for first-launch guidance. (#113)
- Added built-in `/dir cd` command for in-session directory navigation. (#19)
- Added session renaming command. (task#80737766)
- Added nvm-aware Node.js detection in `cosh` wrapper script. (#72)
- Added system-level install via `Makefile` with FHS-compliant directory layout. (#72)
- Fixed 24-item limit on `@` file completion menu. (#92)
- Fixed TUI flicker on Qwen OAuth page in limited-height terminals. (#76)
- Fixed left-arrow key not wrapping from line start to previous line end. (#53)
- Fixed irrelevant info display in `/model` command. (#85)
- Fixed credentials encryption support in `settings.json`. (#90)
- Fixed test failure when running as `root` user. (#29)
- Fixed pre-commit hook working directory for lint-staged. (#90)
- Configured Husky hooks and documented pre-commit setup. (#65)

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
