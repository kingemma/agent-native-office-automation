# 发布前版本一致性校验
#
# 用法：pwsh -NoProfile -File .\tools\release-check.ps1 -Version v0.1.0
#
# 版本号分散在 Cargo.toml（workspace）、tauri.conf.json、package.json 三处，
# 漏改任何一处都会出现「安装包装的是旧版本号」或「更新器判断版本不升」的问题，
# 所以发版流水线的第一步就是拿 tag 把这三处对一遍。
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Root = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$expected = $Version.Trim().TrimStart('v')
if (-not $expected) { throw '版本号为空' }

function Get-WorkspaceVersion {
    param([string]$Path)
    $section = ''
    foreach ($line in Get-Content -LiteralPath $Path) {
        if ($line -match '^\s*\[(.+?)\]\s*$') { $section = $Matches[1]; continue }
        if ($section -eq 'workspace.package' -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    return $null
}

$cargo = Get-WorkspaceVersion (Join-Path $Root 'Cargo.toml')
$tauriConf = (Get-Content -LiteralPath (Join-Path $Root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
$pkg = (Get-Content -LiteralPath (Join-Path $Root 'frontend/package.json') -Raw | ConvertFrom-Json).version

$rows = @(
    [pscustomobject]@{ File = 'Cargo.toml [workspace.package]'; Version = $cargo }
    [pscustomobject]@{ File = 'src-tauri/tauri.conf.json'; Version = $tauriConf }
    [pscustomobject]@{ File = 'frontend/package.json'; Version = $pkg }
)

Write-Host "目标版本（tag）：$expected"
$rows | Format-Table -AutoSize | Out-String | Write-Host

$bad = $rows | Where-Object { $_.Version -ne $expected }
if ($bad) {
    $detail = ($bad | ForEach-Object { "$($_.File)=$($_.Version)" }) -join '，'
    Write-Host "版本不一致：$detail" -ForegroundColor Red
    exit 1
}

Write-Host '三处版本一致' -ForegroundColor Green
exit 0
