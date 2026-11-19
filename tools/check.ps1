# 驭工 OA · 本机类型检查（Windows / macOS / Linux 通用，需要 PowerShell 7）
#
# 用法：
#   pwsh -File ./tools/check.ps1                 # cargo check --workspace
#   pwsh -File ./tools/check.ps1 -p oa-core      # 透传任意 cargo check 参数
#
# Windows 上本机 MSVC 缺 Windows SDK/CRT，统一切 GNU 工具链并注入 tools/mingw-bin；
# macOS/Linux 用 rustup 默认工具链即可，系统依赖见 README「开发环境」。
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$OnWindows = if (Test-Path Variable:IsWindows) { $IsWindows } else { $true }
$cliArgs = @($args)

if ($OnWindows) {
    $Mingw = Join-Path $Root 'tools/mingw-bin/mingw64/bin'
    $Toolchain = 'stable-x86_64-pc-windows-gnu'
    if (-not (Test-Path (Join-Path $Mingw 'gcc.exe'))) {
        throw "未找到 MinGW-w64 GCC：$Mingw/gcc.exe（该目录由 MSYS2 binutils/gcc/crt/headers 包解压合并而来）"
    }
    $sysroot = (& rustup run $Toolchain rustc --print sysroot).Trim()
    $self = Join-Path $sysroot 'lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained'
    $env:PATH = "$Mingw;$self;$env:PATH"
    $cargo = @("+$Toolchain", 'check')
}
else {
    $cargo = @('check')
}

Set-Location $Root
if ($cliArgs.Count -eq 0) { $cliArgs = @('--workspace') }
& cargo @cargo @cliArgs
exit $LASTEXITCODE
