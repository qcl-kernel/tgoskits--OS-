# 作品相关

AxVisor 混合系统比赛作品的独立材料仓库，包含技术报告、可复现图表、机器可读证据摘要和选定原始 CSV/串口摘要。

## 入口

- [比赛技术报告](报告/比赛技术报告.md)
- [复现说明](报告/复现说明.md)
- [证据索引](报告/证据索引/证据索引.md)
- `figures/` 可直接预览的 PNG 图表与架构图
- `figures/overall_system_overview.png` 三项任务整体链路总览
- `data/` 证据摘要和选定原始数据

## 重新生成图表

```bash
python3 scripts/build_figures.py
python3 scripts/build_overview.py
python3 scripts/svg_to_png.py
```

前两条命令先生成图表中间文件，第三条命令统一转换为标准 RGB PNG，转换过程中产生的 SVG 中间文件会自动删除；PNG 可直接用普通图片查看器和 Markdown 预览。脚本会检查 `Noto Sans CJK SC` 中文字体，避免生成黑框。报告中的数据和图片关系如下：CSV 是任务一逐样本原始数据，JSON 是统计汇总、拓扑和 PASS 结果，LOG 是任务二通信压测与故障现场记录；`build_figures.py` 用这些数据生成任务一/二图表，`build_overview.py` 生成三项任务的整体链路图，PNG 只负责展示结果。任务三在总览图中表示目标链路，并明确标注尚未形成真实闭环证据。

报告明确记录了任务一实时性闭环和任务三 YOLO 联动的未完成部分，没有把计划材料当成实测结果。
