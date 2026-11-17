# 驭工 OA · Tauri 桌面/移动端启动器
#
# 用法：
#   .\tools\tauri.ps1 dev            # 开发模式（REST 8787 + 前端 5173 + 桌面窗口）
#   .\tools\tauri.ps1 build          # 打包当前平台安装包
#   .\tools\tauri.ps1 info           # 环境自检
#
# 说明：本机默认 MSVC 工具链缺 Windows SDK/CRT，脚本会自动切到 GNU 工具链并注入
# tools/mingw-bin 的 MinGW-w64 GCC（与 tools/check.ps1 同一套约定）。
#
# dev 会顺带拉起 oa-server（cargo run -p oa-server）：桌面窗口走 invoke 不依赖它，
# 但同一个 Vite 端口也能被浏览器直接打开（浏览器模式需要这个常驻 REST 服务）。
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CliArgs)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Mingw = Join-Path $Root 'tools\mingw-bin\mingw64\bin'
$Toolchain = 'stable-x86_64-pc-windows-gnu'
$Cli = Join-Path $Root 'frontend\node_modules\.bin\tauri.cmd'

if (-not (Test-Path $Cli)) {
    throw "未找到 Tauri CLI：$Cli（请先在 frontend 目录执行 npm install）"
}

if (Test-Path (Join-Path $Mingw 'gcc.exe')) {
    $sysroot = (& rustup run $Toolchain rustc --print sysroot).Trim()
    $env:PATH = "$Mingw;$sysroot\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained;$env:PATH"
    $env:RUSTUP_TOOLCHAIN = $Toolchain
}
else {
    Write-Warning "未找到 MinGW-w64 GCC（$Mingw），将使用默认 Rust 工具链"
}

$OaPort = 8787
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
        # cargo 只是父进程，真正的监听者是它拉起的 oa-server.exe，必须按进程树结束
        & taskkill.exe /T /F /PID $Proc.Id *> $null
    }
}

function Start-OaServer {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw '未找到 cargo，请确认 Rust 工具链已加入 PATH'
    }
    Write-Host "→ 启动 oa-server：cargo run -p oa-server（端口 $OaPort）"
    $proc = Start-Process -FilePath 'cargo' -ArgumentList @('run', '-p', 'oa-server') `
        -WorkingDirectory $Root -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput $ServerOut -RedirectStandardError $ServerErr
    # 首次编译要几分钟，给足超时；编译失败时 cargo 先退出，立即报错而不是干等
    $deadline = (Get-Date).AddSeconds(300)
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
    throw "等待 oa-server 就绪超时（300s），详见 $ServerErr"
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
