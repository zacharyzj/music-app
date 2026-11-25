$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
$url = 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-dev-x86_64-20261111-git-bcde6b93cc.7z'
New-Item -ItemType Directory -Force dist/native | Out-Null
Invoke-WebRequest $url -OutFile dist/native/mpv.7z
if ((Get-FileHash dist/native/mpv.7z -Algorithm SHA256).Hash -ne '273C079B11EA5575822F972756F4C9A71149A5C10130353ABEB8586738938452') { throw 'libmpv checksum mismatch' }
tar -xf dist/native/mpv.7z -C dist/native
if ($LASTEXITCODE -ne 0) { throw 'Failed to extract libmpv' }
Copy-Item dist/native/libmpv-2.dll dist/native/mpv-2.dll -Force
Get-FileHash dist/native/mpv.7z -Algorithm SHA256 | Format-List
