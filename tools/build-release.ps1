#Requires -Version 7.0

<#
.SYNOPSIS
    Builds ggg in release mode and packages it as ggg-<version>-<hash>.zip.

.DESCRIPTION
    Builds the release binaries, then stages the binaries, docs, examples,
    locales and the Chrome extension into a single top-level folder and zips it.

    The archive never contains a config/ directory: ggg treats <exe_dir>/config
    as its live config directory, so shipping one would turn every extracted
    archive into an accidental portable install.

    On non-Windows hosts only the ggg binary is built and packaged; the Windows
    helpers (ggg-dnd, ggg-bridge) and the extension are skipped.

.PARAMETER OutputDir
    Directory that receives the archive. Relative paths are resolved against the
    repository root. Defaults to "dist".

.PARAMETER SkipBuild
    Package the binaries already present in the cargo target directory.

.EXAMPLE
    ./tools/build-release.ps1

.EXAMPLE
    ./tools/build-release.ps1 -SkipBuild
#>
[CmdletBinding()]
param(
    [string]$OutputDir = "dist",
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ExeSuffix = if ($IsWindows) { ".exe" } else { "" }

function Get-CargoMetadata {
    $json = cargo metadata --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed (exit code $LASTEXITCODE)" }
    return ($json -join "`n") | ConvertFrom-Json
}

function Get-PackageVersion {
    param($Metadata)
    $pkgs = @($Metadata.packages | Where-Object name -eq 'ggg')
    if ($pkgs.Count -ne 1) { throw "Expected exactly one package named 'ggg', found $($pkgs.Count)" }
    return $pkgs[0].version
}

function Get-GitHash {
    # Falls back to "unknown" like build.rs does.
    $hash = git rev-parse --short HEAD 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $hash) {
        Write-Host "[warn] git rev-parse failed; using 'unknown' as the hash" -ForegroundColor Yellow
        return "unknown"
    }
    return "$hash".Trim()
}

function Invoke-Build {
    if ($IsWindows) {
        Write-Host "[info] cargo build --release" -ForegroundColor Cyan
        cargo build --release
    } else {
        # ggg-bridge does not compile off Windows, so build the main crate only.
        Write-Host "[info] cargo build --release -p ggg" -ForegroundColor Cyan
        cargo build --release -p ggg
    }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit code $LASTEXITCODE)" }
}

function New-Package {
    param(
        [string]$ReleaseDir,
        [string]$OutputPath,
        [string]$Name
    )

    $stagingDir = Join-Path $OutputPath $Name
    $archivePath = Join-Path $OutputPath "$Name.zip"

    New-Item -ItemType Directory -Force -Path $OutputPath | Out-Null
    if (Test-Path $stagingDir) { Remove-Item -Recurse -Force $stagingDir }
    if (Test-Path $archivePath) { Remove-Item -Force $archivePath }

    try {
        New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null

        # Helpers must sit next to ggg: `ggg bridge install` and ggg-dnd
        # auto-launch both look in the directory of the ggg executable.
        $binaries = @("ggg$ExeSuffix")
        if ($IsWindows) { $binaries += @("ggg-dnd$ExeSuffix", "ggg-bridge$ExeSuffix") }
        foreach ($bin in $binaries) {
            $src = Join-Path $ReleaseDir $bin
            if (-not (Test-Path $src -PathType Leaf)) { throw "Missing build output: $src" }
            Copy-Item $src $stagingDir
        }

        Copy-Item (Join-Path $ProjectRoot "README.md") $stagingDir
        Copy-Item (Join-Path $ProjectRoot "LICENSE") $stagingDir

        $docsDir = Join-Path $stagingDir "docs"
        New-Item -ItemType Directory -Force -Path $docsDir | Out-Null
        foreach ($doc in @("i18n.md", "Keybindings.md", "Script.md")) {
            Copy-Item (Join-Path $ProjectRoot "docs/$doc") $docsDir
        }

        $examplesDir = Join-Path $stagingDir "examples"
        $scriptsDir = Join-Path $examplesDir "scripts"
        New-Item -ItemType Directory -Force -Path $scriptsDir | Out-Null
        Copy-Item (Join-Path $ProjectRoot "examples/settings.toml.example") $examplesDir
        Copy-Item (Join-Path $ProjectRoot "examples/scripts/*.js") $scriptsDir

        # Shipped for users to copy into the external override directory;
        # ggg does not read it from beside the executable.
        foreach ($localeDir in Get-ChildItem (Join-Path $ProjectRoot "locales") -Directory) {
            $dest = Join-Path $stagingDir "locales/$($localeDir.Name)"
            New-Item -ItemType Directory -Force -Path $dest | Out-Null
            Copy-Item (Join-Path $localeDir.FullName "*.ftl") $dest
        }

        if ($IsWindows) {
            $extDest = Join-Path $stagingDir "windows/ggg-extension"
            New-Item -ItemType Directory -Force -Path $extDest | Out-Null
            foreach ($file in @("manifest.json", "background.js", "README.md")) {
                Copy-Item (Join-Path $ProjectRoot "windows/ggg-extension/$file") $extDest
            }
        } else {
            Write-Host "[warn] Windows helpers (ggg-dnd, ggg-bridge) and windows/ggg-extension were skipped on this host" -ForegroundColor Yellow
        }

        Add-Type -AssemblyName System.IO.Compression.FileSystem
        # $true includes the base directory, giving the ggg-<version>-<hash>/ top folder.
        [System.IO.Compression.ZipFile]::CreateFromDirectory(
            $stagingDir, $archivePath,
            [System.IO.Compression.CompressionLevel]::Optimal, $true)

        $zip = [System.IO.Compression.ZipFile]::OpenRead($archivePath)
        try {
            $bad = @($zip.Entries | Where-Object { $_.FullName -match '(^|/)config/' })
        } finally {
            $zip.Dispose()
        }
        if ($bad.Count -gt 0) {
            Remove-Item -Force $archivePath
            throw "Archive contains a config/ entry ($($bad[0].FullName)); refusing to ship it"
        }
    } finally {
        if (Test-Path $stagingDir) { Remove-Item -Recurse -Force $stagingDir }
    }

    return $archivePath
}

Push-Location $ProjectRoot
try {
    $metadata = Get-CargoMetadata
    $version = Get-PackageVersion $metadata
    $hash = Get-GitHash
    $name = "ggg-$version-$hash"
    $releaseDir = Join-Path $metadata.target_directory "release"

    $dirty = git status --porcelain
    if ($LASTEXITCODE -ne 0) { throw "git status failed (exit code $LASTEXITCODE)" }
    if ($dirty) {
        Write-Host "[warn] working tree is dirty; the hash '$hash' does not describe the packaged code" -ForegroundColor Yellow
    }

    if ($SkipBuild) {
        Write-Host "[info] skipping build" -ForegroundColor Cyan
    } else {
        Invoke-Build
    }

    $outputPath = if ([IO.Path]::IsPathRooted($OutputDir)) { $OutputDir } else { Join-Path $ProjectRoot $OutputDir }

    Write-Host "[info] packaging $name" -ForegroundColor Cyan
    $archive = New-Package -ReleaseDir $releaseDir -OutputPath $outputPath -Name $name

    Write-Host "[done] $archive" -ForegroundColor Green
} finally {
    Pop-Location
}
