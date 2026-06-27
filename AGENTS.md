# 项目协作说明

这个目录是 STDF Viewer Mac 的正式开发副本。

## 项目定位

目标是做一个 Mac 桌面小程序：打开裸 `.stdf/.std` 后，可靠解析 STDF V4/V4-2007 的 record 和字段，并以“左侧 record 树 + 右侧字段详情”的方式浏览。

## 技术栈

- Tauri v2
- Rust workspace
- React + TypeScript + Vite
- 核心解析 crate：`stdf-core`

## 重要约束

- 第一版只支持裸 `.stdf/.std`，不支持 `.gz/.zip`。
- `StdfV4Parser.exe` 只作为功能参考，不作为运行依赖。
- 不要回退已实现的 batch event / progress throttle，大文件解析会依赖它避免 UI 黑屏。
- 解析器应以 STDF V4 文档为准，字段顺序不能凭直觉调整。
- 可选尾字段省略时显示空值；真正截断时标记 record 为 `error`。
- 数组和 bitfield 默认摘要显示，不要默认展开为海量字段行。

## 验证基线

修改解析器后至少运行：

```bash
cargo test -p stdf-core
npm test
npm run build
```

涉及真实 STDF 兼容性时运行：

```bash
cargo test -p stdf-core parser_reads_real_customer_sample_without_standard_raw_records -- --ignored --nocapture
```

发布或交付 app 前运行：

```bash
PATH="/opt/homebrew/opt/rustup/bin:$PATH" npm run tauri -- build --bundles app
```

## 交付偏好

- 只有用户明确要求“打包 / 生成 app / 生成 dmg / 发布 / 交付安装包”时，才运行 `tauri build` 或生成 `.app/.dmg`。普通功能修改、bugfix、UI 调整后，只运行必要验证，不主动打包。
- DMG 必须通过项目脚本构建：测试用 `npm run dmg`（= `packaging/build-dmg.sh`，未签名），正式交付用 `npm run dmg:signed`（= `packaging/build-signed-dmg.sh`，签名+公证）。不要手工复用 `target/release/bundle/*` 里的临时 staging 目录作为长期来源。
- **不要**用 `tauri build --bundles dmg` 或手工 create-dmg 出 DMG —— `tauri.conf.json` 未配置 dmg 背景，那样不会带这个安装界面。
- DMG 安装界面必须保留项目内 `packaging/dmg/` 固化的拖拽安装样式：左侧 `STDF Parser.app`，右侧 `Applications`，背景图中间有拖动箭头/提示。背景图由 `packaging/dmg/install-background.py` 自动生成（png + @2x + 视网膜 tiff）并由上述脚本嵌入；要改样式就改这个脚本，不要手改图片或在别处复制。
- 正式签名公证后的 DMG 内只放 `STDF Parser.app`、`Applications` symlink、`.background/install-background.tiff` 和 Finder 布局 `.DS_Store`；不要再放 `打不开时请看.txt`、`修复并打开.command` 或任何 `xattr -cr` 临时绕过说明。
- 每次 `.app` 或 `.dmg` 内容发生变化后，都必须重新 Developer ID 签名、公证 notarize、staple，并验证 Gatekeeper；不能复用旧票据或旧签名。
- 签名身份与 notary profile 通过环境变量 `SIGNING_IDENTITY` / `NOTARY_PROFILE` 提供（见 `packaging/build-signed-dmg.sh`）；仓库内不写入任何签名身份、团队 ID、Apple ID 或 app-specific password。
- 每次完成代码或文档更改后，最终回复里都要放一个简短的“运行 App”卡片，包含项目目录链接和推荐运行命令，方便直接打开目录或复制命令。
- 如果本轮已经启动了 dev server，在“运行 App”卡片里附上当前本地 URL。
- 只有本轮确实完成 app 打包后，才额外放一个“打包产物”卡片，包含可点击的产物目录和主要文件链接。

## 本地说明

这个副本没有包含 `node_modules`、`target`、`dist` 等可再生成目录。新对话接手后，先在本目录执行 `npm install`。
