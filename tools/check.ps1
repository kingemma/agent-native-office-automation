# 驭工 OA · 本机类型检查（Windows PowerShell 版，无需 Visual Studio C++ 工作负载）
#
# 用法：
#   .\tools\check.ps1                    # cargo check --workspace
#   .\tools\check.ps1 -p oa-core         # 透传任意 cargo check 参数
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Mingw = Join-Path $Root 'tools\mingw-bin\mingw64\bin'
$Toolchain = 'stable-x86_64-pc-windows-gnu'
$cliArgs = @($args)

if (-not (Test-Path (Join-Path $Mingw 'gcc.exe'))) {
    Write-Error "未找到 MinGW-w64 GCC：$Mingw\gcc.exe（该目录由 MSYS2 binutils/gcc/crt/headers 包解压合并而来）"
}

$sysroot = (& rustup run $Toolchain rustc --print sysroot).Trim()
$self = Join-Path $sysroot 'lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained'
$env:PATH = "$Mingw;$self;$env:PATH"

Set-Location $Root
if ($cliArgs.Count -eq 0) { $cliArgs = @('--workspace') }
& cargo "+$Toolchain" check @cliArgs
exit $LASTEXITCODE
