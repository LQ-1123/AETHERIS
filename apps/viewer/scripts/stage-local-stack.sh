#!/usr/bin/env bash
# 组装"本地完整栈"资源：pacsd + PostgreSQL 14（含 brew 依赖库，install_name_tool 改相对路径）。
# 输出到 src-tauri/local-stack/（gitignore 忽略），由 tauri 打包进 Resources/local-stack/。
# 目标机无需安装任何东西：postgres 通过 @executable_path/@loader_path 找到包内库，
# 编译内嵌的绝对路径被打补丁成相对路径（pgsql 与 pgdata 平级的布局）。
set -euo pipefail

cd "$(dirname "$0")/.."          # apps/viewer
REPO_ROOT="$(cd ../.. && pwd)"
SRC_TAURI="$PWD/src-tauri"
DEST="$SRC_TAURI/local-stack"
PG="${POSTGRES_HOME:-/opt/homebrew/opt/postgresql@14}"
BREW="${HOMEBREW_PREFIX:-/opt/homebrew}"
PATCHER="$PWD/scripts/patch-postgres-paths.py"
# postgres 二进制依赖的 brew 包（otool -L postgres 实测得出）
PG_DEPS=(lz4 openssl@3 krb5 icu4c@77 readline libarchive xz zstd libb2)

# 非 macOS（如 Windows CI）：只建空目录占位（tauri resources 需要路径存在）。
# Windows 侧服务栈由 aetheris-launcher + Inno Setup 安装包提供，不在这里组装。
if [ "$(uname -s)" != "Darwin" ]; then
  mkdir -p "$DEST"
  echo "==> 非 macOS，跳过本地栈暂存（Windows 由安装包提供）"
  exit 0
fi

echo "==> 编译 pacsd（macOS release）"
export PATH="$HOME/.cargo/bin:$BREW/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
cargo build --release -p pacsd --manifest-path "$REPO_ROOT/Cargo.toml"

echo "==> 组装 local-stack"
rm -rf "$DEST"
mkdir -p "$DEST/postgres/bin" "$DEST/postgres/lib" "$DEST/postgres/share"
cp "$REPO_ROOT/target/release/pacsd" "$DEST/pacsd"

cp -R "$PG/bin/." "$DEST/postgres/bin/"
mkdir -p "$DEST/postgres/lib/postgresql@14"
cp "$PG/lib/postgresql@14/"*.dylib "$DEST/postgres/lib/" 2>/dev/null || true
cp "$PG/lib/postgresql@14/"*.so "$DEST/postgres/lib/postgresql@14/" 2>/dev/null || true
if [ -d "$PG/share/postgresql@14" ]; then
  cp -R "$PG/share/postgresql@14" "$DEST/postgres/share/postgresql@14"
else
  cp -R "$PG/share/postgresql" "$DEST/postgres/share/postgresql"
fi
for dep in "${PG_DEPS[@]}"; do
  cp -L "$BREW/opt/$dep/lib/"*.dylib "$DEST/postgres/lib/" 2>/dev/null || true
done
chmod -R u+w "$DEST"

echo "==> 逐文件处理：改依赖路径 + 移除旧签名 + 路径补丁 + ad-hoc 签名"
fix_libs() {
  local file="$1" prefix="$2"
  for old in $(otool -L "$file" | awk '/\/opt\/homebrew\//{print $1}'); do
    install_name_tool -change "$old" "${prefix}$(basename "$old")" "$file"
  done
  for rp in $(otool -l "$file" | awk '/path \/opt\/homebrew/{print $2}'); do
    install_name_tool -rpath "$rp" "${prefix%/}" "$file" 2>/dev/null || true
  done
}

process_one() {
  local file="$1" prefix="$2" do_patch="$3"
  [ -f "$file" ] || return 0
  fix_libs "$file" "$prefix"
  # 补丁会写入 __LINKEDIT，先移除旧签名避免破坏签名 blob（codesign internal error）
  codesign --remove-signature "$file" 2>/dev/null || true
  if [ "$do_patch" = 1 ]; then
    python3 "$PATCHER" "$file" 2>/dev/null || true
  fi
  # install_name_tool/补丁会破坏原签名；未签名二进制在 Apple Silicon 上会被 SIGKILL
  codesign --force -s - "$file" 2>/dev/null || true
}

for f in "$DEST"/postgres/bin/*; do
  [ -x "$f" ] && process_one "$f" "@executable_path/../lib/" 1
done
for f in "$DEST"/postgres/lib/*.dylib; do
  process_one "$f" "@loader_path/" 0
done
for f in "$DEST"/postgres/lib/postgresql@14/*.so; do
  process_one "$f" "@loader_path/../" 0
done
# pacsd：macOS 26 起系统不再自带 libarchive（本机链接的是 brew 的），
# 打进来并改写引用到 @executable_path/../lib（本地栈根部的 lib 软链指向 postgres/lib）
if [ -f "$DEST/pacsd" ]; then
  # @executable_path = local-stack/，lib 软链在 local-stack/lib（指向 postgres/lib）
  install_name_tool -change /opt/homebrew/opt/libarchive/lib/libarchive.13.dylib "@executable_path/lib/libarchive.13.dylib" "$DEST/pacsd" 2>/dev/null || true
  codesign --remove-signature "$DEST/pacsd" 2>/dev/null || true
  codesign --force -s - "$DEST/pacsd" 2>/dev/null || true
fi

echo "==> 完成:"
du -sh "$DEST"
