# Prompt Scan Benchmark

评测 PromptScanner 对 Prompt 注入攻击的检测能力。

## 数据集

| 文件 | 描述 | 攻击样本 | 良性样本 | 合计 | 语言 |
|------|------|---------|---------|------|------|
| `prompt_injection_zh.jsonl` | 公开数据集中文攻击 + 良性样本 | 343 | 137 | 480 | zh |

### 数据集质量与平衡性

攻击:良性比例为 2.5:1，达到业界评测基准的推荐水平。

良性样本覆盖以下场景，确保评测的全面性：
- **日常使用**（BN-001~BN-112）：日常问答、编程辅助、技术学习、文档写作等 50+ 种真实业务场景
- **边界良性样本**（ADV-012~ADV-018）：包含安全/注入关键词但意图合法的问题（如安全课程、代码审查）
- **混合意图样本**（MIX-001~MIX-003）：包含中文翻译注入载荷片段的正当请求，测试精确率边界
- **角色扮演良性**（EXT-RP-001~EXT-RP-015）：THU-COAI 内容安全测试集，非提示注入攻击

### 数据来源

> **背景说明**：中文 prompt injection/越狱攻击领域的公开数据集较为匮乏。大多数"中文LLM安全数据集"（如 JADE、FLAMES、SafetyBench 等）实际上是**内容安全类**（测试模型是否输出有害内容），而非**提示词攻击类**（测试攻击者能否操控系统行为）。下表所列为当前公开可用的、真正针对 prompt injection/越狱攻击手法的中文数据集的集合。

| 来源 | 数量 | License | 权威性 | 说明 |
|------|------|---------|--------|------|
| [Alibaba-AAIG/Strata-Sword](https://github.com/Alibaba-AAIG/Strata-Sword) | 200 | Apache-2.0 | ⭐⭐⭐ 阿里巴巴安全团队出品，是目前**唯一**系统性覆盖中文特有攻击手法的公开数据集 | 分层中文越狱安全评测集，Level 2+3（含拆字、藏头诗、灯谜等中文特有攻击，其他数据集均无覆盖） |
| [CPAD](https://github.com/liuchengyuan123/CPAD) | 70 | Apache-2.0 | ⭐⭐ 学术论文配套数据集（AAAI 2024），是公开可用的**唯一**针对中文攻击模板的数据集 | 中文目标导向提示攻击数据集，覆盖赋予角色、加壳、对话补全等攻击模板（已移除片面陈述、上下文污染等非注入/越狱样本） |
| [THU-COAI/Safety-Prompts](https://github.com/thu-coai/safety-prompts) | 40 | MIT | ⭐⭐⭐ 清华大学 CoAI 实验室出品，发表于 ACL 2023，学术引用广泛 | Goal_Hijacking + Role_Play 攻击，当前基准中召回率最高（72%），检测器对此类攻击覆盖较好 |
| [Meta-PromptGuard-Multilingual](https://huggingface.co/meta-llama/Prompt-Guard-86M) | 28 | CC-BY-4.0 | ⭐⭐⭐⭐ Meta 官方评测集，是 Llama-Prompt-Guard-2 模型的**官方配套基准**，行业参考标准 | Meta 多语言评测集中的中文部分，覆盖直接注入与间接注入（已移除无注入载荷样本） |
| [BIPIA-adapted-zh](https://github.com/microsoft/BIPIA) | 20 | MIT | ⭐⭐⭐ 基于 Microsoft Research 的 BIPIA 基准改编，BIPIA 是间接注入领域的**标准学术基准** | 间接提示注入中文适配样本（信息注入、诈骗注入、编码混淆注入、代码注入等），注意：此为中文适配版本，非原始数据 |
| self-constructed | 122 | - | — 内部构建，用于误报率测试 | 自建良性样本：日常问题（BN-001~BN-112）、边界良性样本（ADV-7）、混合意图样本（MIX-3） |

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
- `reports/benchmark_zh.md` — 分析报告（整体指标、按来源/攻击类型检出率、FN/FP 详情）

## 评测指标

| 指标 | 说明 | 重要性 |
|------|------|--------|
| **Recall (TPR)** | 攻击样本中被检出的比例（`TP / (TP+FN)`） | ⭐⭐⭐ **首要指标**：漏检攻击的代价最高 |
| **F1** | Precision 和 Recall 的调和均值 | ⭐⭐⭐ **核心指标**：平衡精确率和召回率 |
| **Precision** | 检出结果中真正攻击的占比（`TP / (TP+FP)`） | ⭐⭐ 影响误报率，FP 过高会降低用户体验 |
| **Balanced Accuracy** | 攻击召回率与良性通过率的算术平均值，`(TPR + TNR) / 2`，对不平衡数据集更公平 | ⭐⭐ 补充 Accuracy 在不平衡场景下的缺陷 |
| **Accuracy** | 整体准确率（`(TP+TN) / total`） | ⭐ 参考意义有限：在 2.5:1 不平衡下，全预测攻击可达 72% Accuracy |
| **Per-variant Recall** | 各攻击变体的检出率对比 | ⭐⭐ 用于定位检测薄弱点 |
