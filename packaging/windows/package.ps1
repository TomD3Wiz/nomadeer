<#
.SYNOPSIS
Builds the Nomadeer Windows installer and portable zip from release binaries.

.DESCRIPTION
Runs Inno Setup on packaging\windows\nomadeer.iss to produce
Nomadeer-<version>-Setup.exe, builds Nomadeer-<version>-Windows-Portable.zip
and writes a .sha256 file (sha256sum format) next to each. The release
workflow runs it after building both editions with:

  RUSTFLAGS=-C target-cpu=x86-64-v3 cargo build --release --target-dir target/optimized
  cargo build --release --target-dir target/compatible

.EXAMPLE
./packaging/windows/package.ps1 -Version 0.1.0
#>
param(
    [Parameter(Mandatory = $true)] [string] $Version,
    [string] $OptimizedExe = 'target/optimized/release/nomadeer.exe',
    [string] $CompatibleExe = 'target/compatible/release/nomadeer.exe',
    [string] $OutDir = 'dist/windows',
    # Path to ISCC.exe; found on PATH or in Program Files when not given.
    [string] $Iscc = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path

function Find-Iscc {
    $command = Get-Command 'iscc.exe' -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    foreach ($base in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if (-not $base) { continue }
        $candidate = Join-Path $base 'Inno Setup 6\ISCC.exe'
        if (Test-Path -LiteralPath $candidate) { return $candidate }
    }
    throw 'Inno Setup 6 (ISCC.exe) was not found. Install it with: choco install innosetup'
}

function Write-Checksum([string] $Path) {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    $name = Split-Path -Leaf $Path
    [IO.File]::WriteAllText("$Path.sha256", "$hash  $name`n")
}

# Text files in the zip get Windows line endings so Notepad shows them as-is.
function Write-CrlfText([string] $Source, [string] $Destination) {
    $text = [IO.File]::ReadAllText($Source).Replace("`r`n", "`n").Replace("`n", "`r`n")
    [IO.File]::WriteAllText($Destination, $text.Replace('@VERSION@', $Version))
}

$OptimizedExe = (Resolve-Path -LiteralPath $OptimizedExe).Path
$CompatibleExe = (Resolve-Path -LiteralPath $CompatibleExe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
if (-not $Iscc) { $Iscc = Find-Iscc }

Write-Host "== installer ($Iscc)"
& $Iscc /Q "/DAppVersion=$Version" "/DOptimizedExe=$OptimizedExe" `
    "/DCompatibleExe=$CompatibleExe" "/O$OutDir" `
    (Join-Path $root 'packaging/windows/nomadeer.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
$setup = Join-Path $OutDir "Nomadeer-$Version-Setup.exe"
Write-Checksum $setup
Write-Host "wrote $setup"

Write-Host '== portable zip'
$name = "Nomadeer-$Version-Windows-Portable"
$staging = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
$folder = Join-Path $staging $name
New-Item -ItemType Directory -Force -Path $folder | Out-Null
try {
    Copy-Item -LiteralPath $OptimizedExe -Destination (Join-Path $folder 'Nomadeer.exe')
    Copy-Item -LiteralPath $CompatibleExe -Destination (Join-Path $folder 'Nomadeer-compatible.exe')
    # This marker makes Nomadeer keep its data in a "data" folder beside the exe.
    New-Item -ItemType File -Path (Join-Path $folder 'portable') | Out-Null
    Write-CrlfText (Join-Path $PSScriptRoot 'portable-README.txt') (Join-Path $folder 'README.txt')
    Write-CrlfText (Join-Path $root 'LICENSE') (Join-Path $folder 'LICENSE.txt')

    $zip = Join-Path $OutDir "$name.zip"
    Remove-Item -LiteralPath $zip -ErrorAction SilentlyContinue
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory(
        $folder, $zip, [IO.Compression.CompressionLevel]::Optimal, $true)
    Write-Checksum $zip
    Write-Host "wrote $zip"
}
finally {
    Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
}
