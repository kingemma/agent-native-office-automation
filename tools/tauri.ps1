# 驭工 OA · Tauri 桌面/移动端启动器（Windows / macOS / Linux 通用）
#
# 用法：
#   pwsh -File ./tools/tauri.ps1 dev     # 开发模式（REST 8787 + 前端 5173 + 桌面窗口）
#   pwsh -File ./tools/tauri.ps1 build   # 打包当前平台安装包
#   pwsh -File ./tools/tauri.ps1 info    # 环境自检
#
# 平台差异只集中在本脚本里：
#   - Windows：默认 MSVC 工具链缺 Windows SDK/CRT，自动切 GNU 工具链并注入
#     tools/mingw-bin 的 MinGW-w64 GCC（与 tools/check.ps1 同一套约定）；
#   - macOS / Linux：直接用 rustup 默认工具链，只需要 Xcode CLT / webkit2gtk 等系统依赖；
#   - 结束服务端进程统一走 Process.Kill(entireProcessTree)，不再依赖 taskkill.exe。
#
# dev 会顺带拉起 oa-server：桌面窗口走 invoke 不依赖它，但同一个 Vite 端口也能被
# 浏览器/局域网设备直接打开（浏览器模式需要这个常驻 REST 服务）。
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CliArgs)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
# PowerShell 5.1 没有 $IsWindows，按 Windows 处理（5.1 本来就跑不了本脚本的中文注释，仅作兜底）
$OnWindows = if (Test-Path Variable:IsWindows) { $IsWindows } else { $true }

$CliName = if ($OnWindows) { 'tauri.cmd' } else { 'tauri' }
$Cli = Join-Path $Root "frontend/node_modules/.bin/$CliName"
if (-not (Test-Path $Cli)) {
    throw "未找到 Tauri CLI：$Cli（请先在 frontend 目录执行 npm install）"
}

if ($OnWindows) {
    $Mingw = Join-Path $Root 'tools/mingw-bin/mingw64/bin'
    $Toolchain = 'stable-x86_64-pc-windows-gnu'
    if (Test-Path (Join-Path $Mingw 'gcc.exe')) {
        $sysroot = (& rustup run $Toolchain rustc --print sysroot).Trim()
        $env:PATH = "$Mingw;$sysroot/lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained;$env:PATH"
        $env:RUSTUP_TOOLCHAIN = $Toolchain
    }
    else {
        Write-Warning "未找到 MinGW-w64 GCC（$Mingw），将使用默认 Rust 工具链"
    }
}

$OaPort = 8787
$ServerExe = if ($OnWindows) {
    Join-Path $Root 'target/debug/oa-server.exe'
}
else {
    Join-Path $Root 'target/debug/oa-server'
}
$ServerOut = Join-Path $Root '.oa-server.out.log'
$ServerErr = Join-Path $Root '.oa-server.err.log'

function Test-OaServerUp {
    try {
        $r = Invoke-RestMethod -Uri "http://127.0.0.1:$OaPort/api/health" -TimeoutSec 2
        return [bool]$r.ok
    }
    catch {
        return $false
    }
}

function Stop-OaServer {
    param($Proc)
    if ($Proc -and -not $Proc.HasExited) {
        # 整个进程树一起结束：cargo/子进程在三大平台上的父子关系不一致，逐个 kill 不可靠
        $Proc.Kill($true)
    }
}

function Start-OaServer {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw '未找到 cargo，请确认 Rust 工具链已加入 PATH'
    }
    # 先编译再直接运行二进制：比 `cargo run` 多一层 shell 更可控，三平台都能准确结束进程
    Write-Host "→ 编译 oa-server（首次约 1-3 分钟）"
    Push-Location $Root
    try {
        & cargo build -p oa-server
        if ($LASTEXITCODE -ne 0) { throw "cargo build -p oa-server 失败（exit $LASTEXITCODE）" }
    }
    finally {
        Pop-Location
    }
    if (-not (Test-Path $ServerExe)) { throw "未找到服务端二进制：$ServerExe" }

    Write-Host "→ 启动 oa-server（端口 $OaPort）"
    $spArgs = @{
        FilePath               = $ServerExe
        WorkingDirectory       = $Root
        PassThru               = $true
        RedirectStandardOutput = $ServerOut
        RedirectStandardError  = $ServerErr
    }
    if ($OnWindows) { $spArgs.WindowStyle = 'Hidden' }
    $proc = Start-Process @spArgs

    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline) {
        if (Test-OaServerUp) {
            Write-Host "→ oa-server 就绪：http://127.0.0.1:$OaPort（输出 $ServerOut）"
            return $proc
        }
        if ($proc.HasExited) {
            throw "oa-server 启动失败（exit $($proc.ExitCode)），详见 $ServerErr"
        }
        Start-Sleep -Milliseconds 500
    }
    Stop-OaServer $proc
    throw "等待 oa-server 就绪超时（60s），详见 $ServerErr"
}

Push-Location $Root
$serverProc = $null
try {
    # 只对 dev 生效：build 出的是静态产物 + 桌面壳，不需要额外 REST 进程
    if ($CliArgs.Count -gt 0 -and $CliArgs[0] -eq 'dev') {
        if (Test-OaServerUp) {
            Write-Host "→ 复用已在运行的 oa-server：http://127.0.0.1:$OaPort"
        }
        else {
            $serverProc = Start-OaServer
        }
    }
    & $Cli @CliArgs
    exit $LASTEXITCODE
}
finally {
    if ($serverProc) {
        Write-Host '→ 结束 oa-server'
        Stop-OaServer $serverProc
    }
    Pop-Location
}
