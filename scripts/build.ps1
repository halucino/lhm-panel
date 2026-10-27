# 构建 lhm-panel 并同步到 release/ 目录
# 用法：在仓库根目录执行  pwsh -File scripts/build.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host '=== 1/3  cargo build --release ===' -ForegroundColor Cyan
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build 失败' }

$src = Join-Path $root 'target\release\lhm-panel.exe'
if (-not (Test-Path $src)) {
    # cargo 可能按包名生成 lhm_panel.exe（下划线）
    $alt = Join-Path $root 'target\release\lhm_panel.exe'
    if (Test-Path $alt) { $src = $alt } else { throw "找不到编译产物: $src" }
}

Write-Host '=== 2/3  复制到 release/ ===' -ForegroundColor Cyan
$dstDir = Join-Path $root 'release'
New-Item -ItemType Directory -Force -Path $dstDir | Out-Null
$dst = Join-Path $dstDir 'lhm-panel.exe'
Copy-Item $src $dst -Force

Write-Host '=== 3/3  结果 ===' -ForegroundColor Cyan
$info = Get-Item $dst
'{0}  {1:N0} bytes  {2}' -f $info.Name, $info.Length, $info.LastWriteTime
Write-Host '完成。提交后 GitHub Release 可直接附上 release/lhm-panel.exe' -ForegroundColor Green
