param(
    [switch]$Debug,
    [ValidatePattern('^MusicPlayer-windows-x64(?:-[a-z0-9.-]+)?$')]
    [string]$OutputName = 'MusicPlayer-windows-x64'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Split-Path -Parent $PSScriptRoot)).ProviderPath
Set-Location -LiteralPath $root
$dist = Join-Path $root 'dist'
$build = Join-Path $root 'build'
$work = Join-Path $build 'windows-package'
$next = Join-Path $work 'next'
$previous = Join-Path $work 'previous'
$lockPath = Join-Path $work 'package.lock'

# Only move or delete directories inside this checkout. Do not follow links in old outputs.
foreach ($path in $build, $work, $lockPath, $dist, $next, $previous) {
    $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    if (![IO.Path]::GetFullPath($path).StartsWith(($root + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase) -or
        ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint))) {
        throw "Unsafe packaging path: $path"
    }
}
function Remove-PackageTree([IO.FileSystemInfo]$entry) {
    if (!($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        if ($entry -is [IO.DirectoryInfo]) {
            foreach ($child in $entry.GetFileSystemInfos()) { Remove-PackageTree $child }
        }
        if ($entry.Attributes -band [IO.FileAttributes]::ReadOnly) {
            $entry.Attributes = $entry.Attributes -band (-bnot [IO.FileAttributes]::ReadOnly)
        }
    }
    $entry.Delete()
}

[IO.Directory]::CreateDirectory($work) | Out-Null
$lock = [IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
try {
    # Recover if an interrupted run moved dist aside before installing its replacement.
    if ([IO.Directory]::Exists($previous)) {
        if ([IO.Directory]::Exists($dist)) { Remove-PackageTree ([IO.DirectoryInfo]$previous) }
        else { [IO.Directory]::Move($previous, $dist) }
    }
    if ([IO.Directory]::Exists($next)) { Remove-PackageTree ([IO.DirectoryInfo]$next) }

    $mpv = Join-Path $dist 'native/mpv-2.dll'
    if (!(Test-Path -LiteralPath $mpv -PathType Leaf)) { throw 'Run scripts/get-mpv.ps1 first' }
    $native = [IO.Directory]::CreateDirectory((Join-Path $next 'native'))
    Copy-Item -LiteralPath $mpv -Destination $native.FullName

    cargo xtask plugin $next
    if ($LASTEXITCODE -ne 0) { throw 'Plugin build failed' }
    $profile = if ($Debug) { 'debug' } else { 'release' }
    if ($Debug) { cargo build -p music-app --locked } else { cargo build -p music-app --release --locked }
    if ($LASTEXITCODE -ne 0) { throw 'Application build failed' }

    $package = Join-Path $next $OutputName
    [IO.Directory]::CreateDirectory($package) | Out-Null
    Copy-Item -LiteralPath "target/$profile/music-app.exe", (Join-Path $native.FullName 'mpv-2.dll'), 'README.md', 'THIRD_PARTY_NOTICES.md' -Destination $package
    Copy-Item -LiteralPath 'docs' -Destination $package -Recurse
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($package, "$package.zip")

    [IO.Directory]::Move($dist, $previous)
    try { [IO.Directory]::Move($next, $dist) }
    catch {
        [IO.Directory]::Move($previous, $dist)
        throw
    }
    try { Remove-PackageTree ([IO.DirectoryInfo]$previous) }
    catch { Write-Warning "Previous package cleanup will be retried on the next run: $_" }
    Write-Output "Player: dist/$OutputName.zip; optional sources: dist/*.ypm.zip; online index: dist/plugins.json"
} finally {
    try {
        if ([IO.Directory]::Exists($next)) { Remove-PackageTree ([IO.DirectoryInfo]$next) }
    } finally { $lock.Dispose() }
}
