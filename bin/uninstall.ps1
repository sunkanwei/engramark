# Engramark Windows uninstaller. It removes program files and managed host
# wiring only; the user's cards, state, backups, and configuration are retained.
[CmdletBinding()]
param(
    [Alias("Home")]
    [string]$UserHome = $env:USERPROFILE
)

$ErrorActionPreference = "Stop"
$InstallHome = (Resolve-Path $UserHome).Path
$AppRoot = Join-Path $env:LOCALAPPDATA "Engramark"
$DataHome = Join-Path $InstallHome "engramark"
$Binary = Join-Path $AppRoot "bin\engramark.exe"

if (-not (Test-Path $Binary -PathType Leaf)) {
    throw "找不到 Engramark 程序：$Binary"
}
if ((Get-Item $AppRoot).Attributes -band [IO.FileAttributes]::ReparsePoint) {
    throw "程序目录不能是重解析点。"
}

$Running = Get-Process | Where-Object {
    try { $_.Path -and $_.Path.StartsWith($AppRoot, [StringComparison]::OrdinalIgnoreCase) }
    catch { $false }
}
if ($Running) {
    throw "Engramark 程序仍被宿主占用。请关闭 Codex/OpenCode 后重试；记忆数据未受影响。"
}

& $Binary host-setup uninstall --home $InstallHome --app-root $AppRoot --data-home $DataHome
if ($LASTEXITCODE -ne 0) { throw "宿主接线拆除失败；程序目录未删除。" }

Remove-Item -Recurse -Force $AppRoot
Write-Host "Engramark 程序与宿主接线已移除。"
Write-Host "记忆数据仍保留在：$DataHome"
