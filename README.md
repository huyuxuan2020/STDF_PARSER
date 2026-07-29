# STDF Viewer Mac

STDF Viewer Mac 是一款用于浏览半导体测试数据的 macOS 桌面应用。它可以直接打开裸 `.stdf` / `.std` 文件，解析 STDF V4 / V4-2007 record 及字段，并以可检索、可筛选的方式展示文件内容。

## 主要功能

- 按 record type 分组浏览，查看字段名、类型、值、中文说明和文件位置。
- 检索 record type、字段名和字段值。
- 以 Part / Site 为行、测试项为列浏览 PTR、MPR 和 FTR 结果，并支持测试项筛选与导出。
- 展示良率、软 bin 和硬 bin 统计。
- 通过流式解析、批量事件和表格虚拟化支持大文件浏览。

## 技术栈

- Tauri v2
- Rust workspace 与 `stdf-core` 解析引擎
- React、TypeScript 与 Vite
