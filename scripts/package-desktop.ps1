$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$target = 'x86_64-pc-windows-msvc'
$binary = Join-Path $root "target/$target/release/easypanel-desktop.exe"
if (!(Test-Path -LiteralPath $binary -PathType Leaf) -or (Get-Item -LiteralPath $binary).Length -eq 0) {
    throw "Build $binary first"
}
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$archive = Join-Path $dist "easypanel-desktop-$target.zip"
Compress-Archive -LiteralPath $binary -DestinationPath $archive -Force

Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($archive)
try {
    if ($zip.Entries.Count -ne 1 -or $zip.Entries[0].FullName -ne 'easypanel-desktop.exe' -or $zip.Entries[0].Length -eq 0) {
        throw 'Desktop archive must contain exactly one nonempty easypanel-desktop.exe'
    }
} finally {
    $zip.Dispose()
}
Write-Output "Packaged $archive"
