#!/usr/bin/env bash
# 驭工 OA · 本机类型检查（Windows，无需 Visual Studio C++ 工作负载）
#
# 背景：本机默认 Rust 工具链是 MSVC，但缺 Windows SDK 导入库与 CRT，
# link.exe 无法链接 build script / proc-macro 的 host 可执行文件。
# 本脚本切到 GNU host 工具链，并把 tools/mingw-bin 里的 MinGW-w64 GCC
# 加入 PATH，使 raw-dylib(dlltool + as) 等依赖汇编器的环节能通过。
#
# 用法：
#   ./tools/check.sh                     # 等价于 cargo check --workspace
#   ./tools/check.sh -p oa-core          # 透传任意 cargo check 参数
#   ./tools/check.sh --all-targets       # 同上
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MINGW="$ROOT/tools/mingw-bin/mingw64/bin"
TOOLCHAIN="stable-x86_64-pc-windows-gnu"

if [ ! -x "$MINGW/gcc.exe" ]; then
  echo "ERROR: 未找到 MinGW-w64 GCC：$MINGW/gcc.exe" >&2
  echo "       该目录由 MSYS2 的 binutils + gcc + crt + headers 包解压合并而来，" >&2
  echo "       是本机不装 Visual Studio C++ 工作负载时编译 C 依赖的必要条件。" >&2
  exit 1
fi

SYSROOT="$(rustup run "$TOOLCHAIN" rustc --print sysroot)"
SELF="$(cygpath -u "$SYSROOT")/lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained"
export PATH="$MINGW:$SELF:$PATH"

cd "$ROOT"
if [ "$#" -eq 0 ]; then
  set -- --workspace
fi
exec cargo "+$TOOLCHAIN" check "$@"
