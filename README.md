# STDF Viewer Mac

Mac 版 STDF 字段浏览器，技术栈为 Tauri v2 + Rust + React + TypeScript。

## 功能状态

- 支持打开裸 `.stdf` / `.std` 文件。
- Rust 后端流式解析 STDF V4 / V4-2007 record。
- 前端按 record type 分组浏览，右侧展示字段详情。
- 字段列包含字段名、类型、值、中文说明、offset/length。
- 支持后端分页搜索 record type、字段名、字段值。
- 已针对大文件做事件批处理，避免解析 40MB+ STDF 时前端黑屏。

## STDF 解析覆盖

当前解析器位于 `stdf-core/src/parser.rs`，已按 STDF V4 规格表覆盖以下标准 record：

`FAR/ATR/MIR/MRR/PCR/HBR/SBR/PMR/PGR/PLR/RDR/SDR/WIR/WRR/WCR/PIR/PRR/TSR/PTR/MPR/FTR/BPS/EPS/GDR/DTR`

已支持的 STDF 类型包括：

`U*1/U*2/U*4`、`I*1/I*2/I*4`、`R*4/R*8`、`C*1`、`B*1`、`C*n`、`B*n`、`D*n`、`N*1`、`V*n`，以及 count-driven arrays。

说明：

- 可选尾字段被省略时会显示为空值，不标记错误。
- 必填字段或已经开始读取的变长字段截断时，record 会标记为 `error`。
- 数组和 bitfield 采用 `count/bits + preview` 的摘要显示，不逐元素展开，避免大文件卡顿。
- 第一版不做 PTR/MPR/FTR 默认值继承计算，只显示当前 record 中实际存在或按规格省略的字段。

## 常用命令

安装依赖：

```bash
npm install
```

启动前端开发服务：

```bash
npm run dev
```

运行前端测试：

```bash
npm test
```

运行 Rust core 测试：

```bash
cargo test -p stdf-core
```

运行真实 STDF 样例回归测试：

```bash
cargo test -p stdf-core parser_reads_real_customer_sample_without_standard_raw_records -- --ignored --nocapture
```

如果样例路径变化，可指定：

```bash
STDF_SAMPLE_PATH="/path/to/sample.stdf" cargo test -p stdf-core parser_reads_real_customer_sample_without_standard_raw_records -- --ignored --nocapture
```

构建前端：

```bash
npm run build
```

构建 Mac app：

```bash
PATH="/opt/homebrew/opt/rustup/bin:$PATH" npm run tauri -- build --bundles app
```

打包 DMG 安装包（自带拖拽到 Applications 的安装背景）：

```bash
npm run dmg          # 未签名，本机测试用
npm run dmg:signed   # 正式：Developer ID 签名 + 公证（需已配置签名身份与 notary profile）
```

> DMG 背景由 `packaging/dmg/install-background.py` 生成并由上面脚本嵌入；不要用 `tauri build --bundles dmg`（项目未配置 dmg 背景，会丢失安装界面）。

构建产物位置：

```text
target/release/bundle/macos/STDF Parser.app
target/release/bundle/dmg/STDF_Parser_0.1.1_aarch64_developer_id.dmg
```

## 回归测试样例

大文件回归基准测试通过环境变量 `STDF_SAMPLE_PATH` 指向本机任意 STDF 文件运行（默认 `#[ignore]`，不设变量则跳过）。仓库内不包含任何样例数据。

## 后续开发建议

- 优先把解析结果落到 SQLite 或更细粒度索引，降低超大 STDF 的内存压力。
- 给字段详情增加数组字段的展开/复制能力，但保持默认摘要显示。
- 增加 CSV/JSON/Excel 导出前，先明确导出是按 record、按 test item，还是按 part 维度。
- 如果要做 bin map、良率统计、参数分布图，建议另开数据模型层，不要塞进当前字段浏览器解析层。
