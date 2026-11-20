<#
.SYNOPSIS
  PaperRust Desktop – update artifacts (Z4, docs/DESKTOP.md §23): sign the NSIS installer
  (minisign/Ed25519 via `cargo tauri signer`, signature bound to the version) and write the
  update manifest in Tauri format (`latest.json`) for a channel. Generate keys.

.DESCRIPTION
  Deliberately separate from pack.ps1/installer.ps1: the package build needs no key (unsigned
  builds remain possible, updates are then disabled in the app).

  Location (proposal, a constant in apps/desktop-shell/src/update.rs, DEFAULT_ENDPOINT):
    GitHub release with the tag `updater` in the repo paperrust/paperrust, assets `stable.json` and
    `beta.json`; the installers hang on the respective version release (`v0.2.0` …).
  The app reads `<address>/{{channel}}.json`; channel beta reads beta.json and stable.json and takes
  the highest version, stable never takes a pre-release.

  Keys:
    -NewKey      real key pair (once, by the owner): private to -KeyPath (with password from
                 $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD), public to <KeyPath>.pub. Enter the content of the
                 .pub file in apps/desktop-shell/tauri.conf.json → plugins.updater.pubkey.
                 NEVER put the private key in the repo; loss = no more updates for all
                 installed versions (then new key + reinstall).
    -NewTestKey  test key without password under results\pack\updater-test-key\ (git-ignored),
                 only for tests/E2E.

.EXAMPLE
  # once
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '<long password>'
  .\apps\desktop\pack\updater.ps1 -NewKey -KeyPath "$HOME\.paperrust\updater.key"

.EXAMPLE
  # per release (after pack.ps1 -Installer)
  .\apps\desktop\pack\updater.ps1 -KeyPath "$HOME\.paperrust\updater.key" -Channel stable `
    -Installer results\desktop\PaperRust-0.2.0-windows-x64-setup.exe -Version 0.2.0 `
    -Url https://github.com/paperrust/paperrust/releases/download/v0.2.0/PaperRust-0.2.0-windows-x64-setup.exe `
    -NotesFile docs\release-notes\0.2.0.md -OutDir results\desktop\updater
  # → results\desktop\updater\stable.json + <Installer>.sig; replace stable.json as an asset of the release `updater`.
#>
[CmdletBinding()]
param(
    [string]$Installer,
    [string]$Version,
    [string]$Url,
    [ValidateSet('stable', 'beta')][string]$Channel = 'stable',
    [string]$Notes = '',
    [string]$NotesFile,
    [string]$KeyPath = $env:TAURI_SIGNING_PRIVATE_KEY_PATH,
    [string]$OutDir,
    [string]$Cache,
    [switch]$NewKey,
    [switch]$NewTestKey,
    # Allow download address http://127.0.0.1/localhost (tests only).
    [switch]$AllowLocalHttp
)

Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
if (-not $Cache) { $Cache = Join-Path $Repo 'results\pack\cache' }
$Sources = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'sources.json') | ConvertFrom-Json
$TestKeyDir = Join-Path $Repo 'results\pack\updater-test-key'

function Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

# ------------------------------------------------------------------ tauri-cli (pinned, as installer.ps1)
function Get-TauriCli {
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
    return $cli
}

function Invoke-Cli([string[]]$CliArgs) {
    $cli = Get-TauriCli
    # tauri writes notes to stderr: in PS 5.1 with ErrorActionPreference=Stop that would be an abort.
    $ErrorActionPreference = 'Continue'
    $out = & $cli @CliArgs 2>&1 | ForEach-Object { "$_" }
    $rc = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($rc -ne 0) { $out | ForEach-Object { Write-Host "    $_" }; throw "cargo tauri $($CliArgs[0..1] -join ' ') fehlgeschlagen ($rc)" }
}

function New-KeyPair([string]$Path, [string]$Password) {
    if (Test-Path $Path) { throw "$Path existiert schon – nicht überschrieben." }
    New-Item -ItemType Directory -Force (Split-Path $Path -Parent) | Out-Null
    # `--password=<value>` instead of `-p <value>`: PS 5.1 drops empty arguments.
    Invoke-Cli @('signer', 'generate', '--ci', '-w', $Path, "--password=$Password")
    $pub = (Get-Content -Raw "$Path.pub").Trim()
    Write-Host "    privat:      $Path"
    Write-Host "    öffentlich:  $Path.pub"
    Write-Host "    pubkey:      $pub"
}

if ($NewTestKey) {
    Step 'Testschlüssel (nur für Tests, ohne Passwort, git-ignoriert)'
    New-KeyPair (Join-Path $TestKeyDir 'test.key') ''
    exit 0
}
if ($NewKey) {
    if (-not $KeyPath) { throw '-KeyPath fehlt (Ablage des privaten Schlüssels, außerhalb des Repos).' }
    $full = [IO.Path]::GetFullPath($KeyPath)
    if ($full.StartsWith($Repo, [StringComparison]::OrdinalIgnoreCase)) { throw 'Den privaten Schlüssel nicht im Repo ablegen.' }
    $pw = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    if (-not $pw) { throw 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD setzen (Passwort des neuen Schlüssels).' }
    Step 'Schlüsselpaar für Updates'
    New-KeyPair $full $pw
    Write-Host '    Nächster Schritt: pubkey in apps/desktop-shell/tauri.conf.json → plugins.updater.pubkey eintragen.'
    exit 0
}

# ------------------------------------------------------------------ Signing + manifest
foreach ($p in 'Installer', 'Version', 'Url', 'KeyPath', 'OutDir') {
    if (-not (Get-Variable $p -ValueOnly)) { throw "-$p fehlt" }
}
$Installer = (Resolve-Path $Installer).Path
$KeyPath = (Resolve-Path $KeyPath).Path
if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$') { throw "Version $Version ist kein SemVer" }
if ($Channel -eq 'stable' -and $Version -match '-') { throw "Vorab-Version $Version gehört in den Kanal beta" }
$u = [Uri]$Url
$localHttp = $u.Scheme -eq 'http' -and ($u.Host -eq '127.0.0.1' -or $u.Host -eq 'localhost')
if ($u.Scheme -ne 'https' -and -not ($AllowLocalHttp -and $localHttp)) { throw "Download-Adresse muss https sein: $Url" }
if ($NotesFile) { $Notes = Get-Content -Raw -Encoding UTF8 $NotesFile }

# Does the key match the app? (otherwise every installed version rejects the update)
$conf = Get-Content -Raw -Encoding UTF8 (Join-Path $Repo 'apps\desktop-shell\tauri.conf.json') | ConvertFrom-Json
$appKey = $conf.plugins.updater.pubkey
$myKey = if (Test-Path "$KeyPath.pub") { (Get-Content -Raw "$KeyPath.pub").Trim() } else { '' }
if (-not $appKey) {
    Write-Warning 'tauri.conf.json hat keinen pubkey: Builds aus diesem Stand suchen keine Updates.'
} elseif ($myKey -and $myKey -ne $appKey) {
    Write-Warning 'Schlüssel passt nicht zu plugins.updater.pubkey in tauri.conf.json – Apps aus diesem Stand lehnen das Update ab.'
}

Step "Signieren ($Version, Kanal $Channel)"
$pw = if ($env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD } else { '' }
Invoke-Cli @('signer', 'sign', '-f', $KeyPath, "--password=$pw", '--app-version', $Version, $Installer)
$sig = (Get-Content -Raw "$Installer.sig").Trim()

New-Item -ItemType Directory -Force $OutDir | Out-Null
$entry = [ordered]@{ signature = $sig; url = $Url }
$manifest = [ordered]@{
    version   = $Version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'")
    platforms = [ordered]@{
        'windows-x86_64-nsis' = $entry
        'windows-x86_64'      = $entry
    }
}
$out = Join-Path $OutDir "$Channel.json"
[IO.File]::WriteAllText($out, ($manifest | ConvertTo-Json -Depth 6), (New-Object Text.UTF8Encoding $false))
Write-Host ("    {0}`n    {1}.sig`n    Installer SHA-256 {2}" -f $out, $Installer, (Get-FileHash -Algorithm SHA256 $Installer).Hash.ToLowerInvariant())
