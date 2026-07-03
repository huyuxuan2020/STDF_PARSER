# DTR 文本解析与导出 — 设计文档

日期:2026-07-04
状态:已实现(自主会话:设计决策按代码库现有模式选定,待用户复核)

## 背景

解析器对 DTR (Datalog Text Record, REC_TYP=50/REC_SUB=30) 一直是登记不展开
(`parser.rs` 中 `(50, 10) | (50, 30)` 直接返回空字段),明细页选中 DTR 记录时
字段详情只有"该 record 无数据字段"的空态。用户需要:在 DTR 字段处提供一个
"解析"入口,点击后解析全部 DTR 文本,成功后可下载为 txt。

## 需求

1. 明细页选中 DTR 记录时,字段详情面板提供"解析 DTR 文本"按钮。
2. 点击后扫描整份文件,提取所有 DTR 的 TEXT_DAT,按文件顺序每条一行。
3. 解析成功后显示条数,并出现"下载 TXT"按钮;经保存对话框写到用户所选路径。
4. 解析失败显示错误并可重试;下载成功有"已保存 ✓"反馈。

## 方案选型

- **A. 按需流式重扫(选定)**:点击解析时用 `with_input_reader` 重新流式读取
  源文件(.stdf/.std/.gz/.zip 同一条路径),只解码 (50,30) 的 TEXT_DAT,边扫边写入
  `temp_workspace_dir()/{session_id}.dtr.txt`,返回条数。下载时把临时文件复制到
  用户所选路径。不碰解析热路径、不增大 SQLite 索引,内存占用有界;代价是点击时
  重读一遍文件(顺序 IO,无字段解码/无建库,远快于首次解析,配 spinner 可接受)。
- B. 首次解析时就展开 DTR 并入库:导出瞬时完成,但海量 DTR 文件(百万行 × ≤255B)
  会拖慢解析、撑大索引库 —— 初版特意跳过 GDR/DTR 就是为此。放弃。
- C. 裸文件用 SQLite 里的 offset 随机 seek:对稀疏 DTR 的大裸文件更快,但压缩流
  仍需顺序扫,得维护两条代码路径。v1 不做,慢了再加。

## 设计

### stdf-core

- `parser.rs`:新增 `pub fn decode_dtr_text(payload: &[u8]) -> String`,
  与 `FieldCursor::cn()` 同语义:首字节为长度,UTF-8 lossy,长度越界按实际
  payload 截断,空 payload 得空串。
- `sessions.rs`:
  - `SessionManager::parse_dtr_text(&self, session_id) -> Result<DtrParseResult, String>`
    (`DtrParseResult { session_id, count }`):流式扫描记录头,非 DTR 记录用
    `io::copy(take, sink)` 跳过 payload,DTR 记录解码后写一行到临时 txt。
    文件尾部截断(中止的 lot)与首次解析同样宽容:停止扫描,保留已提取内容。
  - `SessionManager::save_dtr_text(&self, session_id, path) -> Result<(), String>`:
    复制临时 txt 到目标路径;未解析过则报"请先解析"。
  - 会话逐出(打开新文件)时连同删除旧会话的 `.dtr.txt`;启动清理本就整目录删除。

### src-tauri

- 新命令 `parse_dtr_text`、`save_dtr_text`,均 `(async)`(扫描是 IO/CPU 密集,
  不能占 WebKit 主线程),注册进 invoke_handler。

### 前端

- `types.ts`:`DtrParseResult`;`StdfApi` 增加 `parseDtrText` / `saveTxtDialog` /
  `saveDtrText`。
- `api.ts`:三个方法的 Tauri 实现(save 对话框 filter 为 txt)。
- `App.tsx`:
  - 会话级状态 `dtrParse`(idle / parsing / done / error + count + message +
    saving/saved),换文件时重置。
  - `FieldDetailPanel` 中 `record_type === "DTR"` 时,主体渲染 `DtrTextCard`
    替代空态:说明文案 + [解析 DTR 文本];解析中 spinner;完成后"共 N 条" +
    [下载 TXT](沿用 CSV 导出的默认文件名规则,`{去扩展名}_DTR.txt`);
    错误内联显示可重试。头部(类型说明、翻页、状态胶囊)保持不变。
- `test/fixtures.ts`:mock 增加 DTR 组/记录与三个新方法。

### 输出格式

每条 DTR 一行、原文写出,行尾 `\n`;不加序号/偏移前缀,保证 datalog 文本可直接
阅读与 grep。

## 测试

- Rust:裸 stdf 与 zip 输入解析出相同 txt(含中文、非法 UTF-8 lossy、空文本);
  条数正确;先 save 后 parse 报错;save 复制内容一致。
- 前端:选中 DTR 记录出现解析卡片;解析→显示条数→下载调用 saveTxtDialog/
  saveDtrText 且默认名 `demo-1_DTR.txt`;解析失败显示错误与重试。

## 范围外

GDR 展开、DTR 文本内联预览、解析进度条(用不定态 spinner)。
