param(
    [switch]$Debug,
    [ValidatePattern('^MusicPlayer-windows-x64(?:-[a-z0-9.-]+)?$')]
    [string]$OutputName = 'MusicPlayer-windows-x64'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Split-Path -Parent $PSScriptRoot)).ProviderPath
Set-Location -LiteralPath $root
$dist = Join-Path $root 'dist'
$native = Join-Path $dist 'native'
$build = Join-Path $root 'build'
$work = Join-Path $build 'windows-package'
$next = Join-Path $work 'next'
$pending = Join-Path $work 'pending'
$retired = Join-Path $work 'retired'
$cargoTarget = Join-Path $work 'cargo'
$previous = Join-Path $work 'previous'
$lockPath = Join-Path $work 'package.lock'

function Assert-PackagePath([string]$path) {
    $full = [IO.Path]::GetFullPath($path)
    if (!$full.StartsWith(($root + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) {
        throw "Unsafe packaging path: $path"
    }
}
foreach ($path in $build, $work, $next, $pending, $retired, $cargoTarget, $lockPath, $previous, (Join-Path $previous 'native'), $dist, $native) {
    Assert-PackagePath $path
    $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Unsafe packaging path: $path" }
}
function Remove-PackageTree([IO.FileSystemInfo]$entry) {
    Assert-PackagePath $entry.FullName
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
# A directory rename can succeed even when one of its descendants is in use. Check before
# moving anything so a running portable player keeps its executable, DLL and documentation.
function Assert-PackageIdle([IO.FileSystemInfo]$entry) {
    if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { return }
    if ($entry -is [IO.DirectoryInfo]) {
        foreach ($child in $entry.GetFileSystemInfos()) { Assert-PackageIdle $child }
    } else {
        [IO.File]::Open($entry.FullName, 'Open', 'Read', 'None').Dispose()
    }
}
function Move-ToRetired([IO.FileSystemInfo]$entry) {
    Assert-PackagePath $entry.FullName
    $destination = Join-Path $retired ([guid]::NewGuid().ToString())
    Assert-PackagePath $destination
    try {
        if ($entry -is [IO.DirectoryInfo]) {
            $prefix = $entry.FullName + [IO.Path]::DirectorySeparatorChar
            foreach ($process in Get-Process -Name music-app -ErrorAction SilentlyContinue) {
                if ($process.Path -and $process.Path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
                    Write-Warning "Keeping running player directory: $($entry.FullName)"
                    return $false
                }
            }
        }
        Assert-PackageIdle $entry
        $entry.MoveTo($destination)
        return $true
    } catch [IO.IOException], [UnauthorizedAccessException] {
        Write-Warning "Keeping occupied output for the next package run: $($entry.FullName)"
        return $false
    }
}
function Install-PendingPackage {
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $pending 'outputs.json') | ConvertFrom-Json
    $player = $manifest.player
    if ($player -notmatch '^MusicPlayer-windows-x64(?:-[a-z0-9.-]+)?$') { throw 'Invalid portable directory name' }
    $package = Join-Path $pending $player
    $record = Join-Path $pending 'installed-as.txt'
    $deferred = @()
    if ([IO.Directory]::Exists($package)) {
        $name = $player
        $suffix = 2
        while ($true) {
            $destination = Join-Path $dist $name
            Assert-PackagePath $destination
            $old = Get-Item -LiteralPath $destination -Force -ErrorAction SilentlyContinue
            if (!$old -or (Move-ToRetired $old)) { break }
            $deferred += $name
            $name = "$player-$suffix"
            $suffix++
        }
        [IO.File]::WriteAllText($record, $name)
        [IO.Directory]::Move($package, $destination)
    }
    $installed = [IO.File]::ReadAllText($record)
    # Install complete files atomically, then publish the index that refers to them.
    foreach ($name in $manifest.files) {
        if ($name -notmatch '^[a-z0-9.-]+\.zip$|^plugins\.json$') { throw 'Invalid package filename' }
        $source = Join-Path $pending $name
        if (![IO.File]::Exists($source)) { continue } # Already installed before an interruption.
        $destination = Join-Path $dist $name
        if ([IO.File]::Exists($destination)) {
            $backup = Join-Path $retired ([guid]::NewGuid().ToString())
            [IO.File]::Replace($source, $destination, $backup)
        }
        else { [IO.File]::Move($source, $destination) }
    }
    $done = Join-Path $retired ([guid]::NewGuid().ToString())
    Assert-PackagePath $done
    [IO.Directory]::Move($pending, $done)

    $keep = @($installed, 'native', 'android') + $manifest.files + $deferred
    foreach ($entry in ([IO.DirectoryInfo]$dist).GetFileSystemInfos()) {
        if ($keep -notcontains $entry.Name) { $null = Move-ToRetired $entry }
    }
    foreach ($entry in ([IO.DirectoryInfo]$native).GetFileSystemInfos()) {
        if ($entry.Name -ne 'mpv-2.dll') { $null = Move-ToRetired $entry }
    }
    foreach ($entry in ([IO.DirectoryInfo]$retired).GetFileSystemInfos()) {
        try { Remove-PackageTree $entry }
        catch { Write-Warning "Cleanup will be retried on the next run: $_" }
    }
    Write-Output "Portable: dist/$installed; ZIP: dist/$player.zip; optional sources: dist/*.ypm.zip; online index: dist/plugins.json"
}

[IO.Directory]::CreateDirectory($work) | Out-Null
$lock = [IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
try {
    [IO.Directory]::CreateDirectory($retired) | Out-Null
    # Recover staging left by the previous version of this script.
    if ([IO.Directory]::Exists($previous)) {
        if ([IO.Directory]::Exists($dist)) { $null = Move-ToRetired ([IO.DirectoryInfo]$previous) }
        else { [IO.Directory]::Move($previous, $dist) }
    }
    if ([IO.Directory]::Exists($pending)) { Install-PendingPackage }
    if ([IO.Directory]::Exists($next)) { Remove-PackageTree ([IO.DirectoryInfo]$next) }
    $mpv = Join-Path $native 'mpv-2.dll'
    if (!(Test-Path -LiteralPath $mpv -PathType Leaf)) { throw 'Run scripts/get-mpv.ps1 first' }
    [IO.Directory]::CreateDirectory($next) | Out-Null
    cargo xtask plugin $next
    if ($LASTEXITCODE -ne 0) { throw 'Plugin build failed' }

    # Cargo can hard-link the executable to its build cache. Keep both outside the dev target.
    $originalBuildDir = $env:CARGO_BUILD_BUILD_DIR
    try {
        $env:CARGO_BUILD_BUILD_DIR = $cargoTarget
        $argsBuild = @('build', '-p', 'music-app', '--locked', '--target-dir', $cargoTarget)
        $profile = if ($Debug) { 'debug' } else { 'release' }
        if (!$Debug) { $argsBuild += '--release' }
        cargo @argsBuild
        if ($LASTEXITCODE -ne 0) { throw 'Application build failed' }
    } finally { $env:CARGO_BUILD_BUILD_DIR = $originalBuildDir }

    $package = Join-Path $next $OutputName
    [IO.Directory]::CreateDirectory($package) | Out-Null
    Copy-Item -LiteralPath (Join-Path $cargoTarget "$profile/music-app.exe"), $mpv, 'README.md', 'THIRD_PARTY_NOTICES.md' -Destination $package
    Copy-Item -LiteralPath 'docs' -Destination $package -Recurse
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($package, "$package.zip")
    $files = @((Get-ChildItem -LiteralPath $next -Filter '*.zip' -File).Name) + 'plugins.json'
    if (![IO.File]::Exists((Join-Path $next 'plugins.json'))) { throw 'Missing plugin index' }
    [IO.File]::WriteAllText((Join-Path $next 'outputs.json'), (@{player = $OutputName; files = $files} | ConvertTo-Json))
    # Once staged, an interrupted installation can finish on the next invocation.
    [IO.Directory]::Move($next, $pending)
    Install-PendingPackage
} finally {
    $lock.Dispose()
}
