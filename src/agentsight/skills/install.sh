#!/usr/bin/env bash
set -euo pipefail

# AgentSight Skills Installer
# 将 agentsight 提供的 skill 安装到指定 AI coding agent 的 skills 目录
#
# 用法:
#   ./skills/install.sh              # 自动检测已安装的 agent
#   ./skills/install.sh claude       # 安装到 Claude Code
#   ./skills/install.sh qoder        # 安装到 Qoder
#   ./skills/install.sh codex        # 安装到 Codex
#   ./skills/install.sh all          # 安装到所有已检测到的 agent

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

AGENT_DIRS=(
  "claude:.claude/skills"
  "qoder:.qoder/skills"
  "codex:.codex/skills"
)

list_skills() {
  find "$SCRIPT_DIR" -maxdepth 2 -name 'SKILL.md' -printf '%h\n' | xargs -I{} basename {} | sort
}

install_to() {
  local agent="$1"
  local target_dir=""

  for entry in "${AGENT_DIRS[@]}"; do
    local name="${entry%%:*}"
    local dir="${entry#*:}"
    if [[ "$name" == "$agent" ]]; then
      target_dir="$PROJECT_ROOT/$dir"
      break
    fi
  done

  if [[ -z "$target_dir" ]]; then
    echo "错误: 未知的 agent 类型 '$agent'"
    echo "支持: claude, qoder, codex"
    return 1
  fi

  local count=0
  for skill_dir in "$SCRIPT_DIR"/*/; do
    local skill_name
    skill_name="$(basename "$skill_dir")"
    [[ -f "$skill_dir/SKILL.md" ]] || continue

    mkdir -p "$target_dir/$skill_name"
    cp "$skill_dir/SKILL.md" "$target_dir/$skill_name/SKILL.md"

    # 复制 skill 目录下的其他文件（scripts、references 等）
    find "$skill_dir" -mindepth 1 -not -name 'SKILL.md' -not -type d | while read -r f; do
      local rel="${f#$skill_dir}"
      mkdir -p "$target_dir/$skill_name/$(dirname "$rel")"
      cp "$f" "$target_dir/$skill_name/$rel"
    done

    count=$((count + 1))
  done

  echo "✓ 已安装 $count 个 skill 到 $target_dir"
}

detect_agents() {
  local found=()
  for entry in "${AGENT_DIRS[@]}"; do
    local name="${entry%%:*}"
    local dir="${entry#*:}"
    if [[ -d "$PROJECT_ROOT/$dir" ]] || [[ -d "$PROJECT_ROOT/.${name}" ]]; then
      found+=("$name")
    fi
  done
  echo "${found[@]}"
}

main() {
  local skills
  skills="$(list_skills)"
  echo "AgentSight Skills Installer"
  echo "可用 skill: $skills"
  echo ""

  if [[ $# -eq 0 ]]; then
    local agents
    agents="$(detect_agents)"
    if [[ -z "$agents" ]]; then
      echo "未检测到已安装的 AI coding agent"
      echo "用法: $0 <claude|qoder|codex|all>"
      exit 1
    fi
    echo "检测到: $agents"
    for agent in $agents; do
      install_to "$agent"
    done
  elif [[ "$1" == "all" ]]; then
    for entry in "${AGENT_DIRS[@]}"; do
      local name="${entry%%:*}"
      install_to "$name"
    done
  else
    install_to "$1"
  fi
}

main "$@"
