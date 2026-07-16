### ✨ 新功能

- 点击 MPR 单元格可展开该 die 全部 pin 的测量结果（pin 名称、数值、单 pin P/F），支持"仅看 Fail"过滤。
- Pin 结果可一键导出带格式的 Excel。
- CSV 导出中 MPR 列改为全量统计摘要（n / min / max / avg / fail）。

### 🐛 修复

- 修复 MPR 测试项 P/F 判定只统计前 16 个 pin 的问题：现在任意一个 pin 超限都会正确判 F。
- 修复 OPT_FLAG 声明"无上下限"时占位限值被当真的问题：此前部分测试项（如 Continuity_end）会整列误判 Fail，现已按 STDF V4 规范忽略无效限值。
