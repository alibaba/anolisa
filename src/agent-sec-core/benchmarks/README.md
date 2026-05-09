# AgentSecCore Benchmarks

本目录存放各安全扫描能力的评测基准（Benchmark）。

## 目录结构

```
benchmarks/
├── README.md               # 本文件
├── prompt-scan/            # Prompt 注入检测 benchmark
│   ├── datasets/           # 评测数据集（JSONL 格式）
│   ├── scripts/            # 评测脚本
│   ├── results/            # 扫描结果输出
│   └── reports/            # 评测报告（HTML）
├── code-scan/              # (规划中) 代码安全扫描 benchmark
└── ...
```
