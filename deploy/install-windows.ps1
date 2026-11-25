# 驭工 OA 服务端 · Windows 部署脚本
#
# 用法（管理员 PowerShell 7）：
#   pwsh -File .\deploy\install-windows.ps1 -Source .\target\release\oa-server.exe
#
# 做的事：
#   1. 把 oa-server.exe 复制到安装目录（默认 C:\oa），附件留在同目录的 oa-files；
#   2. 把 OA_DB / OA_FILES 写入机器级环境变量（计划任务会继承）；
#   3. 注册开机自启的计划任务（SYSTEM 账户、崩溃后自动重启）；
#   4. 放行入站端口，让局域网客户端能直连；
#   5. 启动并轮询 /api/health 确认可用。
# 数据库需事先准备好 Postgres 实例。密码含 @ : / ? # 等字符时不要用 -Database 连接串，
# 改为预先设置机器级 OA_DB_HOST / OA_DB_PORT / OA_DB_NAME / OA_DB_USER / OA_DB_PASSWORD，
# 并传 -Database '' 跳过写入 OA_DB（空值按未设置处理，OA_DB 非空时优先级更高）。
#
# 为什么用计划任务而不是系统服务：oa-server 是普通控制台程序，没有实现 SCM 接口，
# 直接 sc create 会因「服务未及时响应启动请求」失败；计划任务零依赖（不需要 NSSM）。
param(
    [Parameter(Mandatory = $true)][string]$Source,
    [string]$BinDir = 'C:\oa',
    [int]$Port = 8787,
    [string]$TaskName = 'YugongOA',
    [string]$Database = 'postgresql://oa:oa@127.0.0.1:5432/oa'
)

$ErrorActionPreference = 'Stop'

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator
)
if (-not $isAdmin) {
    throw '需要管理员权限：注册计划任务与防火墙规则都要提权运行'
}
if (-not (Test-Path $Source)) {
    throw "未找到服务端二进制：$Source（先执行 cargo build -p oa-server --release）"
}

New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
$exe = Join-Path $BinDir 'oa-server.exe'
Copy-Item -LiteralPath $Source -Destination $exe -Force
Write-Host "→ 已复制到 $exe"

[Environment]::SetEnvironmentVariable('OA_DB', $Database, 'Machine')
[Environment]::SetEnvironmentVariable('OA_FILES', (Join-Path $BinDir 'oa-files'), 'Machine')
Write-Host "→ 已写入机器级 OA_DB / OA_FILES"

# 停掉正在运行的旧实例，避免端口占用导致新任务启动失败
Get-CimInstance Win32_Process -Filter "Name='oa-server.exe'" -ErrorAction SilentlyContinue |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

$action = New-ScheduledTaskAction -Execute $exe -WorkingDirectory $BinDir
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero)
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
    -Settings $settings -Principal $principal -Force | Out-Null
Write-Host "→ 已注册计划任务 $TaskName（开机自启，工作目录 $BinDir）"

$ruleName = "Yugong OA $Port"
if (-not (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Protocol TCP `
        -LocalPort $Port -Action Allow -Profile Private, Domain | Out-Null
    Write-Host "→ 已放行入站 TCP $Port（Private/Domain 网络）"
}
else {
    Write-Host "→ 防火墙规则已存在：$ruleName"
}

Start-ScheduledTask -TaskName $TaskName
$deadline = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $deadline) {
    try {
        $health = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2
        if ($health.ok) {
            Write-Host "→ 服务已就绪：http://127.0.0.1:$Port（局域网用 http://<本机IP>:$Port）" -ForegroundColor Green
            Write-Host '  首次启动会建表并写入演示数据，上线前请改掉 evan / oa123456'
            return
        }
    }
    catch {
        Start-Sleep -Milliseconds 500
    }
}
throw "启动后 30s 内 /api/health 未就绪，检查 OA_DB 指向的 Postgres 是否可达、账号密码是否正确、端口 $Port 是否被占用"
