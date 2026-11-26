param(
    [switch]$Debug,
    [ValidatePattern('^MusicPlayer-windows-x64(?:-[a-z0-9.-]+)?$')]
    [string]$OutputName = 'MusicPlayer-windows-x64'
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
cargo xtask plugin
if ($LASTEXITCODE -ne 0) { throw 'Plugin build failed' }
$profile = if ($Debug) { 'debug' } else { 'release' }
if ($Debug) { cargo build -p music-app --locked } else { cargo build -p music-app --release --locked }
if ($LASTEXITCODE -ne 0) { throw 'Application build failed' }
$package = Join-Path (Get-Location) "dist/$OutputName"
New-Item -ItemType Directory -Force $package | Out-Null
Copy-Item -LiteralPath "target/$profile/music-app.exe" -Destination $package
$mpv = Join-Path (Get-Location) 'dist/native/mpv-2.dll'
if (!(Test-Path -LiteralPath $mpv)) { throw 'Run scripts/get-mpv.ps1 first' }
Copy-Item -LiteralPath $mpv -Destination $package
Copy-Item README.md,THIRD_PARTY_NOTICES.md -Destination $package
Copy-Item -Path docs -Destination $package -Recurse -Force
Compress-Archive -Path "$package/*" -DestinationPath "dist/$OutputName.zip" -Force
Write-Output "Player: dist/$OutputName.zip; optional sources: dist/netease.ypm.zip, dist/incompetech.ypm.zip, dist/audius.ypm.zip, dist/bilibili.ypm.zip, dist/24bit.ypm.zip; online index: dist/plugins.json"
