# Changelog

## 0.6.0

- Added anolisa component contract (component.toml, Makefile, RPM spec). (#1159)
- Added OpenClaw bootstrap guidance to `install-openclaw` skill. (#1051)
- Added model endpoint preflight before gateway startup in `install-openclaw` skill. (#1031)
- Added static knowledge base update script for `anolisa-guide` skill. (#1010)
- Added `anolisa-guide` skill. (#849)
- Fixed Aliyun mirror fallback for uv and qwenpaw install. (#968)
- Fixed dashscope proxy URL to new Anthropic endpoint in `install-claude-code` skill. (#858)
- Renamed `copaw` to `qwenpaw` across os-skills. (#968)

## 0.5.0

- Added `anolisa-register` skill. (#829)

## 0.4.0

- Added auto-install tokenless plugin support for agent install skills. (#731)
- Added OpenClaw dependency precheck. (#719)
- Improved OpenClaw non-interactive setup. (#687)
- Added Hermes adapter runner. (#617)
- Added standalone ANOLISA adapter entry. (#549)
- Fixed OpenClaw state dir handling normalization. (#641)
- Improved Makefile install paths and contract. (#541)

## 0.3.0

- Added `hermes-agent-install` skill. (#353)
- Added `clawhub-skill-mng` skill with npm install support and YAML description matching. (#315)
- Fixed AgentSight custom db path issue, using default paths instead. (#366)
- Fixed AgentSight token savings query support. (#355)
- Fixed AgentSight interruption CLI and aligned `conversation_id` naming. (#334)

## 0.2.2

- Support enable AgentSight dashboard in `agentsight` skill. (#222)

## 0.2.1

- Upgraded `xlsx` skill with MiniMax open-source implementation. (#218)
- Updated skill descriptions from "suitable for alinux4" to "rpm-base linux". (#182)

## 0.2

- Added `humanizer`, `image-gen`, `pdf-reader`, and `xlsx` skills. (#178)
- Added `cosh-guide` skill. (#23)
- Support net/io/load diagnostic capabilities to `sysom-diagnosis` skill. (#163)
