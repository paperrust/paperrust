<#
.SYNOPSIS
  PaperRust Desktop – build unsigned installers from a finished package directory
  (Tauri bundler, `cargo tauri bundle`): NSIS (`-setup.exe`) and/or MSI. Called by pack.ps1 -Installer / -Msi.

.DESCRIPTION
  - tauri-cli comes prebuilt and pinned from sources.json (tools.tauri_cli), no `cargo install`.
  - The bundler downloads NSIS + nsis_tauri_utils.dll itself (built-in checksums), no admin needed.
  - paperrust-app.exe must already be built (target\release); `tauri bundle` does not rebuild.
  - Installation type „nur für mich“ (currentUser): %LOCALAPPDATA%\PaperRust Desktop\ – deliberately
    separate from the data folder %LOCALAPPDATA%\PaperRust\ (uninstalling leaves the data in place).
  - MSI (WiX, also downloaded by the bundler): per-machine to %ProgramFiles%\PaperRust Desktop\, the
    format the ArtCraft Launcher installs (it runs msiexec and verifies SHA256SUMS.txt). Same data folder.
  - No signature, no updater artifact (both need keys/certificates of the user).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PackDir,
    [Parameter(Mandatory)][string]$Version,
    [string]$Cache,
    [string]$TargetDir,
    # Which installers: nsis, msi or both.
    [ValidateSet('nsis', 'msi')][string[]]$Bundles = @('nsis'),
    # LZMA dictionary in MB (Z3). Tauri only sets `SetCompressor /SOLID lzma` (NSIS default 8 MB);
    # 64 MB combine identical parts across the whole package (measured: 58.1 -> 54.6 MiB).
    # Cost: ~3 min more build time, ~64 MB RAM when installing. 0 = Tauri default.
    [int]$LzmaDictMB = 64
)

Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$Shell = Join-Path $Repo 'apps\desktop-shell'
if (-not $Cache) { $Cache = Join-Path $Repo 'results\pack\cache' }
if (-not $TargetDir) { $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' } }
$PackDir = (Resolve-Path $PackDir).Path
$Sources = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'sources.json') | ConvertFrom-Json

function Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

# ------------------------------------------------------------------ tauri-cli (gepinnt)
$t = $Sources.tools.tauri_cli
$cliDir = Join-Path $Cache "tauri-cli-$($t.version)"
$cli = Join-Path $cliDir 'cargo-tauri.exe'
if (-not (Test-Path $cli)) {
    Step "tauri-cli $($t.version)"
    New-Item -ItemType Directory -Force $Cache | Out-Null
    $zip = Join-Path $Cache ("{0}-{1}" -f $t.sha256.Substring(0, 16), ($t.url -split '/')[-1])
    if (-not (Test-Path $zip) -or (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant() -ne $t.sha256) {
        Invoke-WebRequest -UseBasicParsing -Uri $t.url -OutFile "$zip.part"
        $got = (Get-FileHash -Algorithm SHA256 "$zip.part").Hash.ToLowerInvariant()
        if ($got -ne $t.sha256) { Remove-Item -Force "$zip.part"; throw "tauri-cli SHA-256 falsch: $got" }
        Move-Item -Force "$zip.part" $zip
    }
    if (Test-Path $cliDir) { Remove-Item -Recurse -Force $cliDir }
    New-Item -ItemType Directory -Force $cliDir | Out-Null
    & tar.exe -xf $zip -C $cliDir
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $cli)) { throw 'tauri-cli entpacken fehlgeschlagen' }
}

$app = Join-Path $TargetDir 'release\paperrust-app.exe'
if (-not (Test-Path $app)) { throw "$app fehlt (cd apps\desktop-shell; cargo build --release -j 1)" }

# ------------------------------------------------------------------ bundle configuration
Step 'Bundle-Konfiguration (Ressourcen = Paketverzeichnis ohne paperrust-app.exe)'
$res = [ordered]@{}
foreach ($e in Get-ChildItem $PackDir) {
    if ($e.Name -eq 'paperrust-app.exe') { continue }        # The bundler places the main program itself
    $src = $e.FullName -replace '\\', '/'
    if ($e.PSIsContainer) { $res["$src/"] = "$($e.Name)/" } else { $res[$src] = $e.Name }
}
$conf = [ordered]@{
    productName    = 'PaperRust Desktop'
    mainBinaryName = 'paperrust-app'
    version        = $Version
    bundle         = [ordered]@{
        active      = $true
        targets     = $Bundles
        publisher   = 'PaperRust'
        copyright   = 'GPL-3.0-only; based on paperless-ngx'
        category    = 'Productivity'
        shortDescription = 'Dokumentenarchiv (paperless-ngx-kompatibel), lokal ohne Server'
        licenseFile = ((Join-Path $PackDir 'LICENSE') -replace '\\', '/')
        icon        = @('icons/icon.png', 'icons/icon.ico')
        resources   = $res
        windows     = [ordered]@{
            webviewInstallMode = [ordered]@{ type = 'downloadBootstrapper'; silent = $true }
            # One language only: with several, WiX builds one MSI per language.
            wix = [ordered]@{ language = 'en-US' }
            nsis = [ordered]@{
                installMode = 'currentUser'
                languages   = @('German', 'English')
                displayLanguageSelector = $false
                compression = 'lzma'
                # Remove old resources of the previous version before the update and after uninstalling
                # (finding from installer test 2026-11-19, see file)
                installerHooks = ((Join-Path $PSScriptRoot 'installer-hooks.nsh') -replace '\\', '/')
            }
        }
    }
}
$confFile = Join-Path $Cache 'tauri.bundle.json'
[IO.File]::WriteAllText($confFile, ($conf | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding $false))

# ------------------------------------------------------------------ bundling
Step ("Installer bauen: {0} (cargo tauri bundle)" -f ($Bundles -join ', '))
$env:CARGO_TARGET_DIR = $TargetDir
Push-Location $Shell
try {
    # tauri writes progress to stderr: in PS 5.1 with ErrorActionPreference=Stop that would be an abort.
    $ErrorActionPreference = 'Continue'
    & $cli bundle --bundles ($Bundles -join ',') --config $confFile 2>&1 | ForEach-Object { Write-Host "    $_" }
    $rc = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($rc -ne 0) { throw "tauri bundle fehlgeschlagen ($rc)" }
} finally { Pop-Location }

# ------------------------------------------------------------------ MSI
if ($Bundles -contains 'msi') {
    $msi = Get-ChildItem (Join-Path $TargetDir 'release\bundle\msi') -Filter '*.msi' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $msi) { throw 'MSI nicht gefunden (target\release\bundle\msi)' }
    # Asset name as the ArtCraft Launcher reads it: <stem>-<version>-windows-x64.msi
    $msiDest = Join-Path (Split-Path $PackDir -Parent) ("PaperRust-$Version-windows-x64.msi")
    Copy-Item -Force $msi.FullName $msiDest
    Write-Host ("    {0} ({1:N1} MB, unsigniert), SHA-256 {2}" -f $msiDest, ($msi.Length / 1MB), (Get-FileHash -Algorithm SHA256 $msiDest).Hash.ToLowerInvariant())
}
if ($Bundles -notcontains 'nsis') { return }

$setup = Get-ChildItem (Join-Path $TargetDir 'release\bundle\nsis') -Filter '*-setup.exe' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) { throw 'Installer nicht gefunden (target\release\bundle\nsis)' }

# ------------------------------------------------------------------ larger LZMA dictionary (Z3)
# Compile the script generated by the bundler (target\release\nsis\x64\installer.nsi) once more with one extra line
# using the same makensis; the bundler offers no setting for this.
if ($LzmaDictMB -gt 0) {
    $nsiDir = Join-Path $TargetDir 'release\nsis\x64'
    $nsi = Get-Content -Raw -Encoding UTF8 (Join-Path $nsiDir 'installer.nsi')
    $mark = '  SetCompressor /SOLID "lzma"'
    if (-not $nsi.Contains($mark)) { throw "installer.nsi: '$mark' nicht gefunden (Tauri-Vorlage geändert?)" }
    $nsi = $nsi.Replace($mark, "$mark`r`n  SetCompressorDictSize $LzmaDictMB")
    $nsi = $nsi -replace '!define OUTFILE "[^"]*"', '!define OUTFILE "paperrust-dict.exe"'
    $nsiFile = Join-Path $nsiDir 'installer-dict.nsi'
    [IO.File]::WriteAllText($nsiFile, $nsi, (New-Object Text.UTF8Encoding $true))
    $makensis = Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'
    if (-not (Test-Path $makensis)) { throw "$makensis fehlt (lädt der Tauri-Bundler beim ersten Lauf)" }
    Step "NSIS neu übersetzen mit SetCompressorDictSize $LzmaDictMB"
    Push-Location $nsiDir
    try {
        $ErrorActionPreference = 'Continue'
        & $makensis -V2 -INPUTCHARSET UTF8 $nsiFile 2>&1 | ForEach-Object { Write-Host "    $_" }
        $rc = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        if ($rc -ne 0) { throw "makensis fehlgeschlagen ($rc)" }
    } finally { Pop-Location }
    $dict = Get-Item (Join-Path $nsiDir 'paperrust-dict.exe')
    Write-Host ("    {0:N1} MB statt {1:N1} MB (Tauri-Standard)" -f ($dict.Length / 1MB), ($setup.Length / 1MB))
    Move-Item -Force $dict.FullName $setup.FullName
    $setup = Get-Item $setup.FullName
}
$dest = Join-Path (Split-Path $PackDir -Parent) ("PaperRust-$Version-windows-x64-setup.exe")
Copy-Item -Force $setup.FullName $dest
Write-Host ("    {0} ({1:N1} MB, unsigniert), SHA-256 {2}" -f $dest, ($setup.Length / 1MB), (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLowerInvariant())
