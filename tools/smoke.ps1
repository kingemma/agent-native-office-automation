# 驭工 OA · REST 端点冒烟测试
#
# 用法：.\tools\smoke.ps1
# 前置：已执行 cargo build -p oa-server（或 .\tools\smoke.ps1 -Build）；Docker 正在运行（未设 OA_DB 时会起一次性 Postgres）
# 覆盖：未授权 401、登录、会话、指标、工具清单、计划预览、Agent 本地执行、
#       多步编排的确认门禁、RBAC 拒绝、审计、重置权限、合同两级审批、
#       附件上传/读取/删除与越权拦截、费用标准超标拦截、补卡审批后写考勤、
#       模型配置的权限与掩码、健康检查。
param([switch]$Build)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$OnWindows = if (Test-Path Variable:IsWindows) { $IsWindows } else { $true }
$Exe = if ($OnWindows) {
    Join-Path $Root 'target/debug/oa-server.exe'
}
else {
    Join-Path $Root 'target/debug/oa-server'
}
$Base = 'http://127.0.0.1:18787'
$OwnedPg = $false
$SmokePgName = "oa-smoke-pg-$([guid]::NewGuid().ToString('N').Substring(0, 8))"

if ($Build -or -not (Test-Path $Exe)) {
    Push-Location $Root
    if ($OnWindows) {
        # 与 tools/check.ps1 同一套约定：MSVC 不可用，走 GNU 工具链 + 自带 MinGW
        $Mingw = Join-Path $Root 'tools/mingw-bin/mingw64/bin'
        $Toolchain = 'stable-x86_64-pc-windows-gnu'
        $Sysroot = (& rustup run $Toolchain rustc --print sysroot).Trim()
        $env:PATH = "$Mingw;$Sysroot/lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained;$env:PATH"
        & cargo "+$Toolchain" build -p oa-server
    }
    else {
        & cargo build -p oa-server
    }
    $buildExit = $LASTEXITCODE
    Pop-Location
    if ($buildExit -ne 0) { throw 'cargo build -p oa-server failed' }
}

# 附件落盘目录隔离到 target 下，跑完即删
$Files = Join-Path $Root 'target/smoke-files'
if (Test-Path $Files) { Remove-Item $Files -Recurse -Force }

# 未指定 OA_DB 时起一个一次性 Postgres；指定时由调用方负责库生命周期
if ($env:OA_DB) {
    Write-Host "→ 使用外部 Postgres：$($env:OA_DB -replace ':[^:@/]+@', ':***@')"
}
else {
    $null = docker run -d --rm --name $SmokePgName `
        -e POSTGRES_USER=oa -e POSTGRES_PASSWORD=oa -e POSTGRES_DB=oa `
        -p 127.0.0.1::5432 postgres:16-alpine
    if ($LASTEXITCODE -ne 0) { throw '启动临时 Postgres 失败：请确认 Docker Desktop 正在运行' }
    $OwnedPg = $true
    $portLine = (& docker port $SmokePgName '5432/tcp' | Select-Object -First 1).Trim()
    $pgPort = ($portLine -split ':')[-1]
    $env:OA_DB = "postgresql://oa:oa@127.0.0.1:$pgPort/oa"
    Write-Host "→ 已启动临时 Postgres（$SmokePgName，端口 $pgPort）"
    $ready = $false
    for ($i = 0; $i -lt 60; $i++) {
        $null = & docker exec $SmokePgName pg_isready -U oa -d oa 2>$null
        if ($LASTEXITCODE -eq 0) { $ready = $true; break }
        Start-Sleep -Milliseconds 500
    }
    if (-not $ready) {
        $null = & docker stop $SmokePgName 2>$null
        throw '临时 Postgres 30s 内未就绪'
    }
}
$env:OA_FILES = $Files
$env:OA_ADDR = '127.0.0.1:18787'
$env:OA_AGENT_MODE = 'local'
$spArgs = @{ FilePath = $Exe; PassThru = $true }
if ($OnWindows) { $spArgs.WindowStyle = 'Hidden' }
$proc = Start-Process @spArgs
Start-Sleep -Seconds 2

function PostJson([string]$Path, $Body, [string]$Token) {
    $headers = @{}
    if ($Token) { $headers.Authorization = "Bearer $Token" }
    Invoke-RestMethod -Uri "$Base$Path" -Method Post -ContentType 'application/json' `
        -Headers $headers -Body ($Body | ConvertTo-Json -Depth 8)
}
function GetJson([string]$Path, [string]$Token) {
    $headers = @{}
    if ($Token) { $headers.Authorization = "Bearer $Token" }
    Invoke-RestMethod -Uri "$Base$Path" -Headers $headers
}
function Expect-Code([int]$Code, [scriptblock]$Block) {
    try { & $Block | Out-Null; throw "expected HTTP $Code but request succeeded" }
    catch {
        $actual = $_.Exception.Response.StatusCode.value__
        if ($actual -ne $Code) { throw "expected HTTP $Code, got $actual" }
    }
}

$failed = 0
function Check([string]$Name, [scriptblock]$Assert) {
    try {
        $msg = & $Assert
        Write-Host "PASS  $Name  $msg" -ForegroundColor Green
    } catch {
        $script:failed++
        Write-Host "FAIL  $Name  $($_.Exception.Message)" -ForegroundColor Red
    }
}

try {
    Check 'health' {
        $h = GetJson '/api/health' ''
        if (-not $h.ok) { throw 'health not ok' }
        "service=$($h.service)"
    }
    Check 'unauthorized is 401' {
        Expect-Code 401 { GetJson '/api/metrics' '' }
        ''
    }
    $login = $null
    Check 'admin login' {
        $script:login = PostJson '/api/auth/login' @{ username = 'evan'; password = 'oa123456' } ''
        if (-not $script:login.token) { throw 'no token' }
        "$($script:login.user.display_name)/$($script:login.user.role) perms=$($script:login.user.permissions.Count)"
    }
    Check 'session me' {
        $me = GetJson '/api/auth/me' $login.token
        if ($me.employee_id -ne 'E01') { throw "unexpected employee $($me.employee_id)" }
        $me.employee_id
    }
    Check 'metrics shape' {
        $m = GetJson '/api/metrics' $login.token
        if ($m.active_employees -lt 1 -or $m.trend.Count -ne 6) { throw 'bad metrics' }
        "active=$($m.active_employees) pending=$($m.pending_claims) trend=$($m.trend.Count)"
    }
    Check 'tools list is 19 for admin' {
        $t = GetJson '/api/tools' $login.token
        $allowed = @($t.tools | Where-Object { $_.allowed }).Count
        if ($t.tools.Count -ne 19 -or $allowed -ne 19) { throw "tools=$($t.tools.Count) allowed=$allowed" }
        "19/19"
    }
    Check 'plan preview' {
        $p = PostJson '/api/agent/plan' @{ message = '为许知远办理入职，研发中心 / 前端工程师，月薪 18000' } $login.token
        if ($p.steps.Count -lt 2) { throw "steps=$($p.steps.Count)" }
        "$($p.steps.Count) steps"
    }
    Check 'agent local chat' {
        $c = PostJson '/api/agent/chat' @{ message = '查询研发中心在职员工花名册' } $login.token
        if ($c.mode -ne 'local' -or $c.trace.Count -lt 1 -or -not $c.trace[0].ok) { throw 'local chat failed' }
        "mode=$($c.mode) trace=$($c.trace.Count)"
    }
    Check 'onboarding stops at confirmation' {
        $c = PostJson '/api/agent/chat' @{ message = '为许知远办理入职，研发中心 / 前端工程师，月薪 18000' } $login.token
        if (-not $c.needs_confirmation -or $c.pending.tool -ne 'contract.create') {
            throw "needs_confirmation=$($c.needs_confirmation) pending=$($c.pending.tool)"
        }
        "trace=$($c.trace.Count) pending=$($c.pending.tool)"
    }
    $emp = $null
    Check 'employee login' {
        $script:emp = PostJson '/api/auth/login' @{ username = 'lina'; password = 'oa123456' } ''
        "$($script:emp.user.display_name)/$($script:emp.user.role) perms=$($script:emp.user.permissions.Count)"
    }
    Check 'RBAC blocks asset.register' {
        $r = PostJson '/api/tools/call' @{ name = 'asset.register'; args = @{ model = 'X1'; serial_no = 'SMOKE-1' } } $emp.token
        if ($r.ok -or -not $r.denied) { throw 'employee was not blocked' }
        $r.msg
    }
    Check 'employee self-scope enforced' {
        $r = PostJson '/api/tools/call' @{ name = 'attendance.checkin'; args = @{ employee_id = 'E02'; type = 'in'; time = '09:30' } } $emp.token
        if ($r.ok -or -not $r.denied) { throw 'employee punched for someone else' }
        $r.msg
    }
    Check 'table rows are role-scoped' {
        $own = GetJson '/api/tables/claims' $emp.token
        $foreign = @($own.rows | Where-Object { $_.applicant_id -ne 'E03' }).Count
        if ($foreign -gt 0) { throw "employee saw $foreign foreign claims" }
        $contracts = GetJson '/api/tables/contracts' $emp.token
        if ($contracts.rows.Count -ne 0) { throw 'employee can read contracts' }
        $audit = GetJson '/api/tables/audit_events' $emp.token
        if ($audit.rows.Count -ne 0) { throw 'employee can read audit events' }
        "own claims=$($own.rows.Count), contracts=0, audit=0"
    }
    Check 'audit records denied call' {
        $a = GetJson '/api/audit?limit=20' $login.token
        $hit = @($a.events | Where-Object { $_.actor -eq 'E03' -and $_.tool -eq 'asset.register' }).Count
        if ($hit -lt 1) { throw 'no denied audit row' }
        "events=$($a.events.Count)"
    }
    Check 'employee cannot reset seed (403)' {
        Expect-Code 403 { PostJson '/api/seed/reset' @{} $emp.token }
        ''
    }
    Check 'admin reset seed' {
        $r = PostJson '/api/seed/reset' @{} $login.token
        if (-not $r.ok) { throw 'reset failed' }
        'ok'
    }
    Check 'contract approval walks legal then seal' {
        $fin = PostJson '/api/auth/login' @{ username = 'wuhao'; password = 'oa123456' } ''
        # 演示合同 HT-2026-013 处于两级审批：财务经理 E10 法务节点 → 总经理 E01 用印节点
        $r1 = PostJson '/api/tools/call' @{ name = 'contract.approve'; args = @{ contract_id = 'HT-2026-013'; decision = 'approve'; comment = 'smoke 法务'; confirmed = $true } } $fin.token
        if (-not $r1.ok -or $r1.next_approver -ne 'E01') { throw "step1 failed: $($r1.msg)" }
        $r2 = PostJson '/api/tools/call' @{ name = 'contract.approve'; args = @{ contract_id = 'HT-2026-013'; decision = 'approve'; comment = 'smoke 用印'; confirmed = $true } } $login.token
        if (-not $r2.ok -or $r2.status -ne 'effective') { throw "step2 failed: $($r2.msg)" }
        "E10 -> E01, status=$($r2.status)"
    }
    Check 'attachment upload / list / read / delete' {
        # 1x1 PNG：验证「统一上传接口」的完整闭环
        $png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='
        $up = PostJson '/api/files' @{
            entity_type = 'claim'; entity_id = 'BX-2026-0048'
            file_name = '发票 1.png'; mime = 'image/png'; data_base64 = $png
        } $login.token
        if (-not $up.ok -or -not $up.id) { throw "upload failed: $($up.msg)" }
        $rows = @((GetJson '/api/files?entity_type=claim&entity_id=BX-2026-0048' $login.token).rows)
        if ($rows.Count -lt 1) { throw 'attachment not listed' }
        $got = GetJson "/api/files/$($up.id)" $login.token
        if (-not $got.data_url.StartsWith('data:image/png;base64,')) { throw 'bad data url' }
        $del = Invoke-RestMethod -Uri "$Base/api/files/$($up.id)" -Method Delete -Headers @{ Authorization = "Bearer $($login.token)" }
        if (-not $del.ok) { throw 'delete failed' }
        "$($rows.Count) listed, deleted $($up.id)"
    }
    Check 'employee cannot attach files to contracts' {
        $png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='
        Expect-Code 400 {
            PostJson '/api/files' @{
                entity_type = 'contract'; entity_id = 'HT-2026-013'
                file_name = 'x.png'; mime = 'image/png'; data_base64 = $png
            } $emp.token
        }
        'denied'
    }
    Check 'over-standard item requires a note' {
        # 住宿标准 5000 元/次：6000 元无备注必须被拒；补上说明才允许提交
        $bad = PostJson '/api/tools/call' @{ name = 'finance.claim.create'; args = @{
            type = 'daily'; applicant_id = 'E03'; dept_id = 'D02'; purpose = '客户拜访'
            items = @(@{ category = 'hotel'; amount = 6000 })
        } } $emp.token
        if ($bad.ok) { throw 'over-standard item without note was accepted' }
        $good = PostJson '/api/tools/call' @{ name = 'finance.claim.create'; args = @{
            type = 'daily'; applicant_id = 'E03'; dept_id = 'D02'; purpose = '客户拜访'
            items = @(@{ category = 'hotel'; amount = 6000; note = '客户指定酒店，超标部分自付' })
        } } $emp.token
        if (-not $good.ok -or $good.over_standard_count -ne 1) { throw "with note failed: $($good.msg)" }
        "rejected without note; accepted with note ($($good.over_standard_count) over)"
    }
    Check 'makeup approval writes attendance only after approval' {
        $day = (Get-Date).AddDays(-6).ToString('yyyy-MM-dd')
        $mk = PostJson '/api/tools/call' @{ name = 'attendance.makeup'; args = @{
            employee_id = 'E03'; date = $day; clock_in = '09:10'; clock_out = '18:20'; reason = 'smoke 补卡'
        } } $emp.token
        if (-not $mk.ok -or $mk.approver -ne 'E02') { throw "makeup failed: $($mk.msg)" }
        $before = @((GetJson '/api/tables/attendance' $emp.token).rows | Where-Object { $_.work_date -eq $day }).Count
        if ($before -ne 0) { throw 'attendance written before approval' }
        $mg = PostJson '/api/auth/login' @{ username = 'chenli'; password = 'oa123456' } ''
        $ok = PostJson '/api/tools/call' @{ name = 'attendance.approve'; args = @{
            request_id = $mk.request_id; decision = 'approve'; comment = 'smoke 核实'; confirmed = $true
        } } $mg.token
        if (-not $ok.ok -or $ok.status -ne 'approved') { throw "approve failed: $($ok.msg)" }
        $after = @((GetJson '/api/tables/attendance' $emp.token).rows | Where-Object { $_.work_date -eq $day })
        if ($after.Count -ne 1 -or $after[0].source -ne 'makeup') { throw 'attendance not written after approval' }
        "pending -> approved, attendance $($after[0].status) $($after[0].hours)h"
    }
    Check 'llm settings are admin-only' {
        Expect-Code 403 { GetJson '/api/settings/llm' $emp.token }
        $v = GetJson '/api/settings/llm' $login.token
        $codes = @($v.providers | ForEach-Object { $_.code })
        foreach ($need in @('deepseek', 'qwen', 'vllm', 'ollama')) {
            if ($codes -notcontains $need) { throw "missing provider $need" }
        }
        "providers=$($codes -join ',')"
    }
    Check 'llm settings mask the key' {
        $saved = PostJson '/api/settings/llm' @{
            provider = 'ollama'; base_url = 'http://127.0.0.1:11434/v1'
            model = 'qwen2.5:7b'; api_key = 'sk-smoke-1234567890'
        } $login.token
        if ($saved.source -ne 'db' -or -not $saved.has_key) { throw "source=$($saved.source) has_key=$($saved.has_key)" }
        if ($saved.key_hint -match '1234567890') { throw 'key leaked in response' }
        PostJson '/api/settings/llm' @{ clear = $true } $login.token | Out-Null
        $back = GetJson '/api/settings/llm' $login.token
        if ($back.source -eq 'db') { throw 'clear did not take effect' }
        "hint=$($saved.key_hint), cleared back to $($back.source)"
    }
    Check 'logout invalidates token' {
        PostJson '/api/auth/logout' @{} $login.token | Out-Null
        Expect-Code 401 { GetJson '/api/auth/me' $login.token }
        ''
    }
    Check 'change password revokes sessions' {
        $tmp = PostJson '/api/auth/login' @{ username = 'lina'; password = 'oa123456' } ''
        PostJson '/api/auth/change-password' @{ old_password = 'oa123456'; new_password = 'newpass123' } $tmp.token | Out-Null
        Expect-Code 401 { GetJson '/api/auth/me' $tmp.token }
        $new = PostJson '/api/auth/login' @{ username = 'lina'; password = 'newpass123' } ''
        if (-not $new.token) { throw 'login with new password failed' }
        'old token revoked, new password works'
    }
}
finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    $null = $proc.WaitForExit(5000)
    if ($OwnedPg) { $null = & docker stop $SmokePgName 2>$null }
    if (Test-Path $Files) { Remove-Item $Files -Recurse -Force -ErrorAction SilentlyContinue }
}

if ($failed -gt 0) {
    Write-Host "$failed check(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'all smoke checks passed' -ForegroundColor Green
