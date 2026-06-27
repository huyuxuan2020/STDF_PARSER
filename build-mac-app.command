#!/bin/bash
# STDF Viewer — one-click macOS app builder.
# Double-click this file in Finder, or run it from Terminal.
set -e
cd "$(dirname "$0")"

echo "============================================================"
echo " STDF Parser — 构建 macOS App"
echo " 项目目录: $(pwd)"
echo "============================================================"

# Put the Rust toolchain on PATH (Homebrew rustup, then cargo fallback)
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

if ! command -v npm >/dev/null 2>&1; then
  echo "❌ 找不到 npm。请先安装 Node.js (https://nodejs.org) 后重试。"
  echo ""; echo "按回车键关闭。"; read _; exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "❌ 找不到 cargo/rust。请先安装 Rust (https://rustup.rs) 后重试。"
  echo ""; echo "按回车键关闭。"; read _; exit 1
fi

echo ""
echo "==> [1/2] 安装前端依赖 (npm install)…"
npm install

echo ""
echo "==> [2/2] 构建 App (首次构建会编译 Rust，可能需要几分钟)…"
npm run tauri -- build --bundles app

APP="target/release/bundle/macos/STDF Parser.app"
echo ""
if [ -d "$APP" ]; then
  echo "✅ 构建完成！App 位置："
  echo "   $(pwd)/$APP"
  echo "   （已在 Finder 中为你定位，可直接拖到“应用程序”文件夹）"
  open -R "$APP" || true
else
  echo "⚠️ 构建结束，但没找到预期的 .app。"
  echo "   请检查 target/release/bundle/macos/ 目录。"
fi
echo ""
echo "按回车键关闭。"
read _
