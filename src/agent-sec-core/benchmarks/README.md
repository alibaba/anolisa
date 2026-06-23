# AgentSecCore Benchmarks

本目录存放各安全扫描能力的评测基准（Benchmark）。

## 目录结构

```
benchmarks/
├── README.md                    # 本文件
├── prompt-scan/                 # Prompt 注入检测（单轮 + 多轮）
│   ├── README.md                # 单轮 benchmark 说明
│   ├── README_multiturn.md      # 多轮 benchmark 说明
│   ├── datasets/                # 评测数据集（multiturn_ 前缀 = 多轮）
│   ├── scripts/                 # 评测脚本（run_benchmark_multiturn.py = 多轮）
│   ├── results/                 # 扫描结果输出
│   └── reports/                 # 评测报告（HTML）
├── code-scan/                   # (规划中) 代码安全扫描 benchmark
└── ...
```

各子目录详见自身 README：

- [`prompt-scan/README.md`](./prompt-scan/README.md) — 单轮 prompt 注入检测（L1–L3），关注「这段输入是否含恶意指令」
- [`prompt-scan/README_multiturn.md`](./prompt-scan/README_multiturn.md) — 多轮意图识别（L4），关注「铺垫期 → target 的渐进式越狱链路是否能在泄露前被拦下」

## 快速运行

```bash
# 多轮 benchmark 快速验证（每边 50 条 + 洗牌，约 5 分钟）
make benchmark-prompt-scan-multiturn-smoke

# 多轮 benchmark 全量（2400 条）
make benchmark-prompt-scan-multiturn
```
