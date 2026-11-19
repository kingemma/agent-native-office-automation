# 生成 Tauri v2 更新器用的静态清单 latest.json
#
# 用法：pwsh -NoProfile -File .\tools\make-updater-json.ps1 -Version v0.1.0
#
# Tauri CLI 只产出 .sig 签名文件，不生成 latest.json；客户端的 updater 插件
# 需要一个固定 URL 的 JSON 清单来决定「有没有新版本、包在哪、签名是多少」。
# 这里按 v2 的格式（platforms.<target-triple>）拼出来，供 tauri build 之后调用。
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [string]$BundleDir = 'target/release/bundle',
    [string]$Repository = 'kingemma/agent-native-office-automation',
    [string]$Notes = '',
    [string]$OutFile = ''
)

$ErrorActionPreference = 'Stop'
$ver = $Version.Trim().TrimStart('v')
$bundle = Join-Path $Root $BundleDir
if (-not (Test-Path $bundle)) { throw "打包目录不存在：$bundle（先执行 tauri build）" }
if (-not $OutFile) { $OutFile = Join-Path $bundle 'latest.json' }

# Windows 上 updater 走 NSIS 安装包；没有 NSIS 时退回 MSI
$installer = Get-ChildItem -Path $bundle -Recurse -File -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
    Sort-Object Length -Descending | Select-Object -First 1
if (-not $installer) {
    $installer = Get-ChildItem -Path $bundle -Recurse -File -Filter '*.msi' -ErrorAction SilentlyContinue |
        Sort-Object Length -Descending | Select-Object -First 1
}
if (-not $installer) { throw "在 $bundle 下没找到 *-setup.exe 或 *.msi，确认 bundle.active=true 且打包成功" }

$sigPath = "$($installer.FullName).sig"
if (-not (Test-Path $sigPath)) {
    throw "缺少签名文件 $sigPath：说明构建时没有设置 TAURI_SIGNING_PRIVATE_KEY，或未开启 createUpdaterArtifacts"
}
$signature = (Get-Content -LiteralPath $sigPath -Raw).Trim()

# 更新包必须能从公网直接下载，因此指向同 tag 的 Release 附件
$assetName = [uri]::EscapeDataString($installer.Name)
$url = "https://github.com/$Repository/releases/download/v$ver/$assetName"

$manifest = [ordered]@{
    version   = $ver
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = [ordered]@{
        'windows-x86_64' = [ordered]@{
            signature = $signature
            url       = $url
        }
    }
}

$json = $manifest | ConvertTo-Json -Depth 6
Set-Content -LiteralPath $OutFile -Value $json -Encoding utf8NoBOM

Write-Host "已生成 $OutFile"
Write-Host "  version : $ver"
Write-Host "  payload : $($installer.Name)"
Write-Host "  url     : $url"
