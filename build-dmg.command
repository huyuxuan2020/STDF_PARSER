#!/bin/bash
# STDF Parser — 一键打包未签名 DMG（无需苹果验证）。
# 在 Finder 中双击此文件，或在终端运行。
set -e
cd "$(dirname "$0")"

echo "============================================================"
echo " STDF Parser — 打包 DMG 安装包（未签名 / 测试用）"
echo " 项目目录: $(pwd)"
echo "============================================================"

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

if ! command -v npm >/dev/null 2>&1; then
  echo "❌ 找不到 npm。请先安装 Node.js (https://nodejs.org) 后重试。"
  echo ""; echo "按回车键关闭。"; read _; exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "❌ 找不到 cargo/rust。请先安装 Rust (https://rustup.rs) 后重试。"
  echo ""; echo "按回车键关闭。"; read _; exit 1
fi

APP="target/release/bundle/macos/STDF Parser.app"
if [ -d "$APP" ]; then
  echo ""
  echo "检测到已构建的 App。是否跳过重新编译，直接打包？"
  echo "  [回车] = 跳过编译，快速打包    [n] = 重新编译再打包"
  read -r ans
  if [ "$ans" = "n" ] || [ "$ans" = "N" ]; then
    bash packaging/build-dmg.sh
  else
    SKIP_APP_BUILD=1 bash packaging/build-dmg.sh
  fi
else
  echo ""
  echo "==> 未发现已构建的 App，将先编译再打包（首次编译 Rust 可能需要几分钟）…"
  bash packaging/build-dmg.sh
fi

DMG="target/release/bundle/dmg/STDF_Parser_0.1.1_aarch64.dmg"
echo ""
if [ -f "$DMG" ]; then
  echo "✅ 打包完成！DMG 位置："
  echo "   $(pwd)/$DMG"
  open -R "$DMG" || true
else
  echo "⚠️ 打包结束，但没找到预期的 DMG。请查看上方日志。"
fi

echo ""
echo "按回车键关闭。"
read _
