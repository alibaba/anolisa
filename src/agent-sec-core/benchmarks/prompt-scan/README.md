# Prompt Scan Benchmark

评测 PromptScanner 对 Prompt 注入攻击的检测能力。

## 数据集

`prompt_injection_zh.jsonl`：攻击样本 343 条 + 良性样本 137 条，共 480 条，攻击:良性 ≈ 2.5:1。

> 数据集背景说明、各来源（Alibaba-AAIG、THU-COAI、Meta、Microsoft Research 等）的权威评级、License 及详细说明，请查看运行 benchmark 后生成的 `reports/benchmark_zh.html` 报告 **Section 00**。

### JSONL 格式

首行为元数据（`_type: "dataset_metadata"`），后续每行一条样本：

```json
{
  "id": "SS-L2-Template-001",
  "text": "...",
  "label": "jailbreak",
  "attack_type": "jailbreak",
  "sub_type": "template_wrapping",
  "description": "Level 2：简单推理越狱（使用伪装/混淆技巧嵌入恶意指令）",
  "source": "Alibaba-AAIG/Strata-Sword",
  "source_url": "https://github.com/Alibaba-AAIG/Strata-Sword",
  "source_license": "Apache-2.0",
  "language": "zh"
}
```

## 使用方法

```bash
# 在 agent-sec-core 目录下运行
make benchmark-prompt-scan

# 或 cd 到 benchmark 目录后运行脚本
cd benchmarks/prompt-scan
python3 scripts/run_benchmark.py

# 指定扫描模式
cd benchmarks/prompt-scan
python3 scripts/run_benchmark.py --mode strict
```

运行后会生成：
- `results/prompt_injection_zh.jsonl` — 每条样本的扫描结果
- `reports/benchmark_zh.html` — 完整分析报告（数据集说明、整体指标、按来源/攻击类型检出率、FN/FP 详情）

## 评测指标

> 各指标的含义、计算公式及重要性说明，请查看 `reports/benchmark_zh.html` 报告的 **Section 01**。
