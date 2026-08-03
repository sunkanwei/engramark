# Engramark Windows installer (PowerShell). Program files are replaceable; memories stay separate.
[CmdletBinding()]
param(
    [string]$Package,
    [string]$Checksum,
    [string]$Version,
    [string]$Repo = "sunkanwei/engramark",
    [Alias("Home")]
    [string]$UserHome = $env:USERPROFILE
)

$ErrorActionPreference = "Stop"
$Target = "windows-x86_64"
$InstallHome = (Resolve-Path $UserHome).Path
if ($InstallHome.TrimEnd("\") -eq [IO.Path]::GetPathRoot($InstallHome).TrimEnd("\")) {
    throw "用户目录不能是磁盘根目录。"
}
$AppRoot = Join-Path $env:LOCALAPPDATA "Engramark"
$DataHome = Join-Path $InstallHome "engramark"
if ((Test-Path $AppRoot) -and
    ((Get-Item $AppRoot).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw "程序目录不能是重解析点。"
}

$TempRoot = Join-Path $env:TEMP ("engramark-install-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $TempRoot | Out-Null
$InstallLockStream = $null
try {
    $PackagePath = $Package
    $Expected = $Checksum
    if (-not $PackagePath) {
        if ($Repo -notmatch "^[^/]+/[^/]+$") { throw "GitHub 仓库地址应为账号/仓库。" }
        $Base = if ($Version) {
            "https://github.com/$Repo/releases/download/v$($Version.TrimStart('v'))"
        } else {
            "https://github.com/$Repo/releases/latest/download"
        }
        $ChecksumsText = Join-Path $TempRoot "checksums.txt"
        Invoke-WebRequest -Uri "$Base/checksums.txt" -OutFile $ChecksumsText -UseBasicParsing
        $Asset = $null
        foreach ($line in Get-Content $ChecksumsText) {
            $parts = $line -split "\s+"
            if ($parts.Count -ge 2 -and $parts[1] -like "engramark-*-$Target.zip") { $Asset = $parts[1]; $Expected = $parts[0]; break }
        }
        if (-not $Asset) { throw "发布清单中没有 $Target 安装包。" }
        $PackagePath = Join-Path $TempRoot $Asset
        Invoke-WebRequest -Uri "$Base/$Asset" -OutFile $PackagePath -UseBasicParsing
    }
    if (-not (Test-Path $PackagePath)) { throw "找不到安装包。" }
    if ($Expected) {
        $Actual = (Get-FileHash -Algorithm SHA256 $PackagePath).Hash.ToLowerInvariant()
        if ($Actual -ne $Expected.ToLowerInvariant()) { throw "安装包校验失败。" }
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Archive = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path $PackagePath).Path)
    try {
        $Names = [System.Collections.Generic.HashSet[string]]::new(
            [System.StringComparer]::OrdinalIgnoreCase)
        [int64]$TotalSize = 0
        if ($Archive.Entries.Count -eq 0 -or $Archive.Entries.Count -gt 4096) {
            throw "安装包条目数量非法。"
        }
        foreach ($Entry in $Archive.Entries) {
            $Name = $Entry.FullName
            if ([string]::IsNullOrWhiteSpace($Name) -or $Name.Contains("\") -or
                -not ($Name -eq "engramark/" -or $Name.StartsWith("engramark/")) -or
                -not $Names.Add($Name.TrimEnd("/"))) {
                throw "安装包包含不安全或重复路径：$Name"
            }
            $Parts = $Name.TrimEnd("/").Split("/")
            if ($Parts.Count -eq 0) { throw "安装包路径为空。" }
            foreach ($Part in $Parts) {
                if (-not $Part -or $Part -eq "." -or $Part -eq ".." -or
                    $Part.EndsWith(".") -or $Part.EndsWith(" ") -or
                    $Part.Contains(":") -or $Part -match '[\x00-\x1f]' -or
                    $Part -match '^(?i:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\..*)?$') {
                    throw "安装包包含 Windows 不安全路径：$Name"
                }
            }
            $IsDirectory = $Name.EndsWith("/")
            $UnixType = (($Entry.ExternalAttributes -shr 16) -band 0xF000)
            if (($IsDirectory -and $UnixType -ne 0 -and $UnixType -ne 0x4000) -or
                (-not $IsDirectory -and $UnixType -ne 0 -and $UnixType -ne 0x8000)) {
                throw "安装包包含链接或特殊文件：$Name"
            }
            if ($Entry.Length -gt 268435456) { throw "安装包单个文件过大：$Name" }
            $TotalSize += $Entry.Length
            if ($TotalSize -gt 536870912) { throw "安装包展开大小超过限制。" }
            if ($Entry.Length -gt 1048576 -and
                ($Entry.CompressedLength -eq 0 -or $Entry.Length / $Entry.CompressedLength -gt 1000)) {
                throw "安装包压缩比异常：$Name"
            }
            $Destination = [IO.Path]::GetFullPath((Join-Path $TempRoot ($Name.Replace("/", [IO.Path]::DirectorySeparatorChar))))
            $Prefix = [IO.Path]::GetFullPath($TempRoot).TrimEnd("\") + "\"
            if (-not $Destination.StartsWith($Prefix, [StringComparison]::OrdinalIgnoreCase)) {
                throw "安装包路径越界：$Name"
            }
        }
    } finally {
        $Archive.Dispose()
    }
    $Stage = Join-Path $TempRoot "engramark"
    Expand-Archive -Path $PackagePath -DestinationPath $TempRoot
    $Stage = (Get-Item $Stage).FullName
    $Binary = Join-Path $Stage "bin\engramark.exe"
    if (-not (Test-Path $Binary)) { throw "安装包缺少原生二进制。" }
    $Manifest = Join-Path $Stage "MANIFEST.tsv"
    if (-not (Test-Path $Manifest -PathType Leaf)) { throw "安装包缺少逐文件清单。" }
    $ExpectedPaths = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase)
    foreach ($Line in Get-Content $Manifest) {
        $Fields = $Line.Split("`t")
        if ($Fields.Count -ne 4) { throw "逐文件清单格式非法。" }
        $Kind, $Size, $Digest, $Relative = $Fields
        if (($Kind -ne "d" -and $Kind -ne "f") -or $Size -notmatch '^\d+$' -or
            -not $Relative -or $Relative -eq "MANIFEST.tsv" -or
            -not $ExpectedPaths.Add($Relative)) {
            throw "逐文件清单条目非法：$Line"
        }
        foreach ($Part in $Relative.Split("/")) {
            if (-not $Part -or $Part -eq "." -or $Part -eq ".." -or
                $Part.EndsWith(".") -or $Part.EndsWith(" ") -or $Part.Contains(":") -or
                $Part -match '[\x00-\x1f]' -or
                $Part -match '^(?i:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\..*)?$') {
                throw "逐文件清单路径非法：$Relative"
            }
        }
        $Item = Join-Path $Stage ($Relative.Replace("/", [IO.Path]::DirectorySeparatorChar))
        if ($Kind -eq "d") {
            if (-not (Test-Path $Item -PathType Container)) { throw "清单目录缺失：$Relative" }
        } else {
            if (-not (Test-Path $Item -PathType Leaf)) { throw "清单文件缺失：$Relative" }
            $Info = Get-Item $Item
            if ($Info.Length -ne [int64]$Size) { throw "文件大小校验失败：$Relative" }
            if ((Get-FileHash -Algorithm SHA256 $Item).Hash.ToLowerInvariant() -ne $Digest) {
                throw "逐文件校验失败：$Relative"
            }
        }
    }
    $ExpectedPaths.Add("MANIFEST.tsv") | Out-Null
    $StagePrefix = [IO.Path]::GetFullPath($Stage).TrimEnd("\") + "\"
    $ActualPaths = Get-ChildItem $Stage -Force -Recurse | ForEach-Object {
        if ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "解包结果包含重解析点：$($_.FullName)"
        }
        $ItemPath = [IO.Path]::GetFullPath($_.FullName)
        if (-not $ItemPath.StartsWith($StagePrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "解包结果路径越界：$ItemPath"
        }
        $ItemPath.Substring($StagePrefix.Length).Replace("\", "/")
    }
    if ($ActualPaths.Count -ne $ExpectedPaths.Count -or
        @($ActualPaths | Where-Object { -not $ExpectedPaths.Contains($_) }).Count -ne 0) {
        throw "安装包存在清单外条目或缺少声明条目。"
    }
    $PackageVersion = (Get-Content (Join-Path $Stage "VERSION") -First 1).Trim()
    if ($Version -and ($Version.TrimStart('v') -ne $PackageVersion)) { throw "安装包版本与指定版本不一致。" }

    $env:ENGRAMARK_HOME = Join-Path $TempRoot "selfcheck"
    & $Binary rebuild | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "二进制自检失败（SQLite 能力探针未通过）。" }

    & $Binary host-setup check --home $InstallHome --app-root $AppRoot --data-home $DataHome
    if ($LASTEXITCODE -ne 0) { throw "宿主预检失败。" }

    New-Item -ItemType Directory -Path (Split-Path $AppRoot -Parent) -Force | Out-Null
    $InstallLockPath = "${AppRoot}.install.lock"
    try {
        $InstallLockStream = [IO.File]::Open(
            $InstallLockPath, [IO.FileMode]::OpenOrCreate,
            [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    } catch {
        throw "另一个安装进程正在运行，或安装锁无法获取：$InstallLockPath"
    }
    $Previous = $null
    if (Test-Path $AppRoot) {
        $previous = "${AppRoot}.previous-$PID"
        $running = Get-Process | Where-Object { $_.Path -and $_.Path.StartsWith($AppRoot) }
        if ($running) {
            throw "旧程序仍在运行（可能由宿主占用）。请先关闭 Codex/OpenCode 后重试，未做任何修改。"
        }
        Move-Item $AppRoot $Previous
    }
    $Restore = {
        if (Test-Path $AppRoot) { Remove-Item -Recurse -Force $AppRoot }
        if ($Previous -and (Test-Path $Previous)) {
            Move-Item $Previous $AppRoot
        }
    }
    try {
        Move-Item $Stage $AppRoot
        $env:ENGRAMARK_HOME = $DataHome
        $InstalledBinary = Join-Path $AppRoot "bin\engramark.exe"
        & $InstalledBinary migrate-v1 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "数据迁移失败。" }
        & $InstalledBinary rebuild | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "缓存重建失败。" }
        & $InstalledBinary diagnose --full | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "诊断失败。" }
        & $InstalledBinary search "" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "安装后冒烟失败。" }
        & $InstalledBinary host-setup install --home $InstallHome --app-root $AppRoot --data-home $DataHome | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "宿主接线失败。" }
    } catch {
        & $Restore
        throw "$_ 已恢复旧版本。"
    }
    if ($Previous) {
        try { Remove-Item -Recurse -Force $Previous }
        catch { Write-Warning "旧程序备份未能清理，请人工检查：$Previous" }
    }
    Write-Host "`n安装完成：Engramark $PackageVersion"
    Write-Host "程序目录：$AppRoot"
    Write-Host "记忆目录：$DataHome（重装不会覆盖）"
    Write-Host "命令入口：$AppRoot\bin\engramark.exe"
    Write-Host "`n如 Codex 或 OpenCode 正在运行，请重启宿主以加载新二进制；仍在运行的旧会话可能引用已被替换的旧程序路径。"
} finally {
    if ($InstallLockStream) {
        $InstallLockStream.Dispose()
        Remove-Item $InstallLockPath -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -Recurse -Force $TempRoot -ErrorAction SilentlyContinue
}
