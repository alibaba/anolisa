---
name: agent-sec-skill-verify
phase: 2
description: 验证 Skill 完整性与签名，加载外部 Skill 前必须调用。
---

# Skill 完整性验证

## 前置条件

执行验证前，先检查以下依赖：

1. **gpg / gnupg2**：运行 `gpg --version`。若未安装：
   - RHEL/Anolis/Alinux: `sudo yum install -y gnupg2`
   - Debian/Ubuntu: `sudo apt-get install -y gnupg`
   - 若无法安装，输出 `[Phase 2] NOT_RUN: gpg not installed` 并停止
2. **verifier.py**：确认 `scripts/asset-verify/verifier.py` 存在。若不存在，输出 `[Phase 2] NOT_RUN: verifier.py not found` 并停止

## 用法

```bash
# 验证配置中所有 skills 目录
python3 scripts/asset-verify/verifier.py

# 验证单个 skill
python3 scripts/asset-verify/verifier.py --skill /path/to/skill_name
```

## 配置文件

`scripts/asset-verify/config.conf`:

```ini
skills_dir = [
    /opt/agent/skills
    /path/to/other/skills
]
```

## 输出

```
[OK] skill_a
[OK] skill_b
[ERROR] skill_c
  ERR_HASH_MISMATCH: ...

==================================================
PASSED: 2
FAILED: 1
==================================================
VERIFICATION FAILED
```

## 错误码

| 码 | 含义 |
|----|------|
| 0 | 通过 |
| 10 | 缺失 .skill.sig |
| 11 | 缺失 Manifest.json |
| 12 | 签名无效 |
| 13 | 哈希不匹配 |

## Status Line Output

验证完成后，你必须输出以下状态行之一：

- 全部通过时: `[Phase 2] PASS`
- 存在失败时: `[Phase 2] FAIL: <failed skill names>`
- 前置条件不满足时: `[Phase 2] NOT_RUN: <reason>`

