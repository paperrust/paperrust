<#
.SYNOPSIS
  E2E automatic updates (Z4, docs/DESKTOP.md §23): build two test installers (0.1.0 and
  0.1.1-test, both with the TEST key), sign the update and run updater-e2e.mjs.

.DESCRIPTION
  - Source: the portable ZIP from pack.ps1 (results\desktop\PaperRust-<v>-windows-x64.zip), unpacked
    to <Work>\pack (without paperrust-app.exe).
  - Build the shell as a debug build with TAURI_CONFIG (version + test pubkey), once per version; afterwards
    the normal debug build is restored (without TAURI_CONFIG).
  - Own product name „PaperRust Updatetest“ (own registry keys, never the real
    installation „PaperRust Desktop“), installation only under <Work>.
  - Test key: results\pack\updater-test-key\test.key (git-ignored; if it is missing:
    apps\desktop\pack\updater.ps1 -NewTestKey).

.EXAMPLE
  .\apps\desktop-shell\e2e\updater-e2e.ps1 -Work <scratch>\z4\e2e -Out results\desktop\updater-e2e
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Work,
    [string]$Out,
    [string]$Zip,
    [string]$UpdateVersion = '0.1.1-test',
    [string]$BaseVersion = '0.1.0',
    [switch]$SkipBuild,
    [switch]$SkipRun
)

Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$Shell = Join-Path $Repo 'apps\desktop-shell'
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' }
if (-not $Out) { $Out = Join-Path $Repo 'results\desktop\updater-e2e' }
if (-not $Zip) { $Zip = Get-ChildItem (Join-Path $Repo 'results\desktop') -Filter 'PaperRust-*-windows-x64.zip' | Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName }
$Key = Join-Path $Repo 'results\pack\updater-test-key\test.key'
$Cli = Get-ChildItem (Join-Path $Repo 'results\pack\cache') -Filter 'cargo-tauri.exe' -Recurse | Select-Object -First 1 -ExpandProperty FullName
New-Item -ItemType Directory -Force $Work, $Out | Out-Null
$Work = (Resolve-Path $Work).Path
$Inst = Join-Path $Work 'installers'

function Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Native([scriptblock]$b, [string]$what) {
    $ErrorActionPreference = 'Continue'
    & $b 2>&1 | ForEach-Object { Write-Host "    $_" }
    $rc = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($rc -ne 0) { throw "$what fehlgeschlagen ($rc)" }
}

if (-not $SkipBuild) {
    if (-not (Test-Path $Key)) { & (Join-Path $Repo 'apps\desktop\pack\updater.ps1') -NewTestKey }
    if (-not $Cli) { throw 'cargo-tauri.exe fehlt (einmal pack.ps1 -Installer bzw. updater.ps1 ausführen)' }
    if (-not $Zip) { throw 'Paket-ZIP fehlt (pack.ps1 -Zip)' }
    $pub = (Get-Content -Raw "$Key.pub").Trim()

    Step "Paket entpacken: $Zip"
    $pack = Join-Path $Work 'pack'
    $unz = Join-Path $Work 'unzip'
    foreach ($d in $pack, $unz) { if (Test-Path $d) { Remove-Item -Recurse -Force $d } }
    New-Item -ItemType Directory -Force $unz | Out-Null
    Native { tar.exe -xf $Zip -C $unz } 'ZIP entpacken'
    Move-Item (Get-ChildItem $unz -Directory | Select-Object -First 1).FullName $pack
    Remove-Item -Recurse -Force $unz
    Remove-Item -Force (Join-Path $pack 'paperrust-app.exe')

    New-Item -ItemType Directory -Force $Inst | Out-Null
    $env:CARGO_TARGET_DIR = $TargetDir
    foreach ($v in @($UpdateVersion, $BaseVersion)) {
        Step "Hülle (Debug) als $v mit Test-pubkey"
        $env:TAURI_CONFIG = (@{ version = $v; plugins = @{ updater = @{ pubkey = $pub; requireSignedVersion = $true } } } | ConvertTo-Json -Compress -Depth 5)
        Push-Location $Shell
        try { Native { cargo build -j 2 } 'cargo build' } finally { Pop-Location }
        Remove-Item Env:TAURI_CONFIG

        Step "NSIS-Installer $v (Produkt „PaperRust Updatetest“)"
        $res = [ordered]@{}
        foreach ($e in Get-ChildItem $pack) {
            $src = $e.FullName -replace '\\', '/'
            if ($e.PSIsContainer) { $res["$src/"] = "$($e.Name)/" } else { $res[$src] = $e.Name }
        }
        $conf = [ordered]@{
            productName    = 'PaperRust Updatetest'
            identifier     = 'ch.paperrust.desktop.updatetest'
            mainBinaryName = 'paperrust-app'
            version        = $v
            bundle         = [ordered]@{
                active    = $true
                targets   = @('nsis')
                publisher = 'PaperRust'
                icon      = @('icons/icon.png', 'icons/icon.ico')
                resources = $res
                windows   = [ordered]@{
                    webviewInstallMode = [ordered]@{ type = 'skip' }
                    nsis = [ordered]@{ installMode = 'currentUser'; languages = @('German'); compression = 'lzma' }
                }
            }
        }
        $confFile = Join-Path $Work "bundle-$v.json"
        [IO.File]::WriteAllText($confFile, ($conf | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding $false))
        $nsisOut = Join-Path $TargetDir 'debug\bundle\nsis'
        Get-ChildItem $nsisOut -Filter 'PaperRust Updatetest_*' -ErrorAction SilentlyContinue | Remove-Item -Force
        Push-Location $Shell
        try { Native { & $Cli bundle --debug --bundles nsis --config $confFile } 'tauri bundle' } finally { Pop-Location }
        $setup = Get-ChildItem $nsisOut -Filter 'PaperRust Updatetest_*-setup.exe' | Select-Object -First 1
        if (-not $setup) { throw "Installer $v nicht gefunden" }
        $name = if ($v -eq $BaseVersion) { 'base-setup.exe' } else { 'update-setup.exe' }
        Move-Item -Force $setup.FullName (Join-Path $Inst $name)
        Write-Host ("    {0} ({1:N1} MB)" -f $name, ((Get-Item (Join-Path $Inst $name)).Length / 1MB))
    }

    Step 'Normalen Debug-Build wiederherstellen (ohne Test-Schlüssel)'
    Push-Location $Shell
    try { Native { cargo build -j 2 } 'cargo build' } finally { Pop-Location }

    Step "Update signieren ($UpdateVersion, Testschlüssel)"
    & (Join-Path $Repo 'apps\desktop\pack\updater.ps1') -Installer (Join-Path $Inst 'update-setup.exe') `
        -Version $UpdateVersion -Url 'http://127.0.0.1/files/update-setup.exe' -AllowLocalHttp `
        -Channel beta -Notes "Testversion $UpdateVersion (E2E)" -KeyPath $Key -OutDir $Inst
}

if (-not $SkipRun) {
    Step 'E2E'
    Native { node (Join-Path $PSScriptRoot 'updater-e2e.mjs') $Inst (Join-Path $Work 'run') $Out } 'updater-e2e.mjs'
}
