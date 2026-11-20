<#
.SYNOPSIS
  PaperRust Desktop – build the Windows x64 package (portable directory + ZIP, optionally NSIS installer).
  No signature (needs a certificate of the user, docs/DESKTOP.md section 19).

.DESCRIPTION
  Builds the installation directory that apps/desktop/src/layout.rs expects:

    <Out>\
      paperrust-app.exe          Tauri shell (window, tray) – start program
      paperrust-desktop.exe      supervisor/console shell (run, backup, manage, stop, paths)
      paperrust.exe              server + worker (crates/pl-server)
      paperrust-migrate-db.exe   database migration (crates/pl-dbmigrate), first start „Datenbank übernehmen“
      paperrust-pdftool.exe      PDF tools redact/shrink/repair (crates/pl-pdftools, PdfCraft)
      routes.toml                repo routes.toml unchanged (at most switches routes off; no Django)
      static\                    server statics (scripts/build-static.sh → results/static, PAPERLESS_STATICDIR)
      light\                     UI (frontend-light/dist) – /rs/app/ (pl-server, / redirects there) and app UI (mode 2b, prapi)
      pgsql\{bin,lib,share}\     PostgreSQL 17 (theseus-rs), thinned out
      pdfium\pdfium.dll          PDFium (bblanchon/pdfium-binaries)
      tesseract\                 tesseract.exe + DLLs + tessdata\{eng,deu,osd}.traineddata
      fonts\LiberationSerif-Regular.ttf  font for text thumbnails (PAPERLESS_THUMBNAIL_FONT_NAME)
      PaperRust.cmd              start script (portable; data optionally next to the program, see -Portable)
      LICENSE, NOTICE            GPL-3.0 (PaperRust, paperless-ngx)
      THIRD-PARTY-LICENSES.txt   overview of all bundled third-party parts + licenses
      THIRD-PARTY-LICENSES\      license texts (PostgreSQL incl. ICU/OpenSSL/…, PDFium, Tesseract, Rust crates)
      PACKAGE.json               versions + SHA-256 of all third-party parts, size

  Third-party binaries come exclusively from sources.json (URL + SHA-256 pinned). Downloads land in the
  cache (file name = checksum) and are checked again before every use. An entry without
  sha256 aborts. Archives are only unpacked, never executed (Tesseract installer: 7-Zip; if
  7-Zip is missing, the pinned 7-Zip MSI is unpacked via `msiexec /a` – only unpacking files, without
  admin – into the cache).

  After thinning PostgreSQL a self-test runs (initdb with ICU, start on 127.0.0.1,
  SELECT 1, stop), at the end a Tesseract self-test (--version, --list-langs).

.EXAMPLE
  # Build the release programs beforehand (only with > 3.5 GB free RAM, -j 1):
  #   cargo build --release -p pl-server --bin paperrust -j 1
  #   cargo build --release -p paperrust-desktop -j 1
  #   cargo build --release -p pl-dbmigrate --bin paperrust-migrate-db -j 1
  #   cargo build --release -p pl-pdftools --features engine --bin paperrust-pdftool -j 1
  #   cd apps\desktop-shell; cargo build --release -j 1
  .\apps\desktop\pack\pack.ps1 -Zip

.EXAMPLE
  # additionally NSIS installer (Tauri bundler; downloads pinned tauri-cli + NSIS, no admin):
  .\apps\desktop\pack\pack.ps1 -Zip -Installer

.EXAMPLE
  # fetch and check only small parts:
  .\apps\desktop\pack\pack.ps1 -Only pdfium,tessdata -Out $env:TEMP\paperrust-pack-test
#>
[CmdletBinding()]
param(
    [string]$Out,
    [string]$Cache,
    [ValidateSet('release', 'debug')][string]$BuildProfile = 'release',
    [string]$TargetDir,
    [string]$StaticDir,
    # UI (default frontend-light\dist; required – the Angular UI no longer exists)
    [string]$LightDir,
    [string]$RoutesFile,
    [ValidateSet('app', 'pg', 'pdfium', 'tesseract', 'tessdata', 'fonts', 'licenses')][string[]]$Only = @('app', 'pg', 'pdfium', 'tesseract', 'tessdata', 'fonts', 'licenses'),
    # 0 = no limit. Larger downloads are skipped (the component is then missing from the package).
    [int]$MaxDownloadMB = 0,
    [switch]$SkipSelfCheck,
    # ship the .gz/.br variants of the UI (default: omit, see below)
    [switch]$KeepPrecompressed,
    # ZIP of the directory next to <Out> (PaperRust-<version>-windows-x64.zip)
    [switch]$Zip,
    # NSIS installer via the Tauri bundler (see section Installer below)
    [switch]$Installer
)

Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow with a progress bar in PS 5.1

$Target = 'x86_64-pc-windows-msvc'
$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$Version = ((Get-Content -Raw (Join-Path $Repo 'Cargo.toml')) -split "`n" | Where-Object { $_ -match '^version\s*=\s*"([^"]+)"' } | Select-Object -First 1) -replace '^version\s*=\s*"([^"]+)".*$', '$1'
$Version = $Version.Trim()
if (-not $Version) { throw 'Version aus Cargo.toml nicht lesbar' }
$PackName = "PaperRust-$Version-windows-x64"
if (-not $Out) { $Out = Join-Path $Repo "results\desktop\$PackName" }
if (-not $Cache) { $Cache = Join-Path $Repo 'results\pack\cache' }
if (-not $TargetDir) {
    $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' }
}
if (-not $RoutesFile) { $RoutesFile = Join-Path $Repo 'routes.toml' }
$Sources = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'sources.json') | ConvertFrom-Json
$Manifest = [ordered]@{ name = 'PaperRust'; version = $Version; target = $Target; created = (Get-Date).ToUniversalTime().ToString('o'); signed = $false; parts = [ordered]@{} }
# Overview for THIRD-PARTY-LICENSES.txt: part, version, license, source
$Licenses = New-Object System.Collections.Generic.List[object]

function Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

function New-CleanDir($path) {
    if (Test-Path $path) { Remove-Item -Recurse -Force $path }
    New-Item -ItemType Directory -Force $path | Out-Null
}

function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding $false))
}

# Downloads a pinned file (or takes it from the cache) and checks SHA-256. Returns: path or $null
# (skipped because of -MaxDownloadMB).
function Get-Pinned([string]$Name, [string]$Url, [string]$Sha256, [long]$Size) {
    if (-not $Sha256 -or $Sha256.Length -ne 64) {
        throw "$Name hat keine gepinnte SHA-256 in sources.json – Abbruch (nie ungeprüft herunterladen)."
    }
    $Sha256 = $Sha256.ToLowerInvariant()
    New-Item -ItemType Directory -Force $Cache | Out-Null
    $file = Join-Path $Cache ("{0}-{1}" -f $Sha256.Substring(0, 16), ($Url -split '/')[-1])
    if (Test-Path $file) {
        if ((Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant() -eq $Sha256) { return $file }
        Write-Warning "$Name im Cache beschädigt – neu laden"
        Remove-Item -Force $file
    }
    if ($MaxDownloadMB -gt 0 -and $Size -gt $MaxDownloadMB * 1MB) {
        Write-Warning ("{0}: {1:N0} MB > -MaxDownloadMB {2} – übersprungen" -f $Name, ($Size / 1MB), $MaxDownloadMB)
        return $null
    }
    Write-Host ("    lade {0} ({1:N1} MB)" -f $Url, ($Size / 1MB))
    $part = "$file.part"
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $part
    $got = (Get-FileHash -Algorithm SHA256 $part).Hash.ToLowerInvariant()
    if ($got -ne $Sha256) {
        Remove-Item -Force $part
        throw "$Name SHA-256 falsch: erwartet $Sha256, erhalten $got – Abbruch."
    }
    Move-Item -Force $part $file
    return $file
}

function Expand-Archive2([string]$Archive, [string]$Dest) {
    New-CleanDir $Dest
    # tar.exe (bsdtar, Windows 10+) can handle .zip and .tgz/.tar.gz
    & tar.exe -xf $Archive -C $Dest
    if ($LASTEXITCODE -ne 0) { throw "Entpacken fehlgeschlagen: $Archive" }
}

# 7-Zip for unpacking the NSIS installer of Tesseract: PATH, program folders, otherwise a pinned MSI.
function Get-SevenZip {
    $c = Get-Command 7z.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    foreach ($p in @("$env:ProgramFiles\7-Zip\7z.exe", "${env:ProgramFiles(x86)}\7-Zip\7z.exe")) {
        if ($p -and (Test-Path $p)) { return $p }
    }
    $t = $Sources.tools.'7zip'
    $dir = Join-Path $Cache "7zip-$($t.version)"
    $exe = Get-ChildItem -Recurse -Filter 7z.exe $dir -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($exe) { return $exe.FullName }
    $msi = Get-Pinned '7-Zip' $t.url $t.sha256 $t.size
    if (-not $msi) { throw '7-Zip fehlt (Download übersprungen)' }
    New-CleanDir $dir
    # administrative install = unpack files from the MSI; no registration, no admin
    $p = Start-Process -FilePath msiexec.exe -ArgumentList @('/a', "`"$msi`"", '/qn', "TARGETDIR=`"$dir`"") -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "7-Zip-MSI auspacken fehlgeschlagen (msiexec $($p.ExitCode))" }
    $exe = Get-ChildItem -Recurse -Filter 7z.exe $dir | Select-Object -First 1
    if (-not $exe) { throw '7z.exe nach msiexec /a nicht gefunden' }
    return $exe.FullName
}

function Copy-Notice([string]$From, [string]$ToDir) {
    if (Test-Path $From) {
        New-Item -ItemType Directory -Force $ToDir | Out-Null
        Copy-Item -Recurse -Force $From $ToDir
    }
}

# Removes the .debug_* sections at the end of a PE file (MinGW builds of Tesseract carry DWARF
# in the image: libtesseract 97 MB, of which 94 MB debug). Only if all debug sections are at the end and
# the others have short names (no COFF string table needed); otherwise the file stays as it is.
# Returns: saved bytes.
function Remove-PeDebugSections([string]$Path) {
    $d = [IO.File]::ReadAllBytes($Path)
    $u16 = { param($o) [BitConverter]::ToUInt16($d, $o) }
    $u32 = { param($o) [BitConverter]::ToUInt32($d, $o) }
    $pe = & $u32 0x3c
    if ((& $u32 $pe) -ne 0x4550) { return 0 }
    $nsec = & $u16 ($pe + 6); $symptr = & $u32 ($pe + 12); $nsym = & $u32 ($pe + 16); $optsz = & $u16 ($pe + 20)
    $opt = $pe + 24; $salign = & $u32 ($opt + 32); $sh = $opt + $optsz
    $strtab = [long]$symptr + [long]$nsym * 18
    $secs = for ($i = 0; $i -lt $nsec; $i++) {
        $o = $sh + $i * 40
        $name = [Text.Encoding]::ASCII.GetString($d, $o, 8).TrimEnd([char]0)
        if ($name.StartsWith('/') -and $symptr) {
            $so = $strtab + [int]$name.Substring(1); $e = [Array]::IndexOf($d, [byte]0, [int]$so)
            $name = [Text.Encoding]::ASCII.GetString($d, $so, $e - $so)
        }
        [pscustomobject]@{ name = $name; vsize = (& $u32 ($o + 8)); va = (& $u32 ($o + 12)); rsize = (& $u32 ($o + 16)); rptr = (& $u32 ($o + 20)) }
    }
    $keep = $nsec
    while ($keep -gt 0 -and $secs[$keep - 1].name.StartsWith('.debug')) { $keep-- }
    if ($keep -eq $nsec -or $keep -eq 0) { return 0 }
    if (@($secs[0..($keep - 1)] | Where-Object { $_.name.StartsWith('/') }).Count) { return 0 }
    $end = ($secs[0..($keep - 1)] | ForEach-Object { [long]$_.rptr + $_.rsize } | Measure-Object -Maximum).Maximum
    $last = $secs[$keep - 1]
    $img = [uint32]([math]::Ceiling(([double]$last.va + $last.vsize) / $salign) * $salign)
    [BitConverter]::GetBytes([uint16]$keep).CopyTo($d, $pe + 6)
    [BitConverter]::GetBytes([uint32]0).CopyTo($d, $pe + 12)     # PointerToSymbolTable
    [BitConverter]::GetBytes([uint32]0).CopyTo($d, $pe + 16)     # NumberOfSymbols
    [BitConverter]::GetBytes($img).CopyTo($d, $opt + 56)         # SizeOfImage
    [BitConverter]::GetBytes([uint32]0).CopyTo($d, $opt + 64)    # CheckSum (for DLLs/EXEs without a driver left unchecked)
    for ($i = $keep; $i -lt $nsec; $i++) { [Array]::Clear($d, $sh + $i * 40, 40) }
    $out = New-Object byte[] $end
    [Array]::Copy($d, $out, $end)
    [IO.File]::WriteAllBytes($Path, $out)
    return ($d.Length - $end)
}

function Get-LlvmReadobj {
    Get-ChildItem "$env:USERPROFILE\.rustup\toolchains\*\lib\rustlib\*\bin\llvm-readobj.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
}

# DLLs from $DllDir that the programs/modules $Roots import directly or indirectly (static
# imports, recursive). Returns: hashtable of lower-case DLL names.
function Get-ImportClosure([string]$Llvm, [string[]]$Roots, [string]$DllDir) {
    $have = @{}; Get-ChildItem $DllDir -Filter '*.dll' | ForEach-Object { $have[$_.Name.ToLowerInvariant()] = $_.FullName }
    $need = @{}; $seen = @{}; $todo = New-Object System.Collections.Stack
    foreach ($r in $Roots) { $todo.Push($r) }
    while ($todo.Count) {
        $f = $todo.Pop()
        if ($seen.ContainsKey($f)) { continue }
        $seen[$f] = $true
        foreach ($m in [regex]::Matches((& $Llvm --coff-imports $f | Out-String), 'Name: (\S+\.dll)', 'IgnoreCase')) {
            $n = $m.Groups[1].Value.ToLowerInvariant()
            if ($have.ContainsKey($n) -and -not $need.ContainsKey($n)) { $need[$n] = $true; $todo.Push($have[$n]) }
        }
    }
    return $need
}

# PostgreSQL tools that the supervisor calls: pg::REQUIRED_TOOLS (apps/desktop/src/pg.rs) +
# pg_restore (restoring a backup by hand, docs/DESKTOP.md section 7/8). All
# other programs in bin\ (pg_upgrade, pg_basebackup, vacuumdb, …) are never called by PaperRust;
# pg_upgrade would come from the **new** package on a major version change anyway.
function Get-PgKeepTools {
    $src = Get-Content -Raw -Encoding UTF8 (Join-Path $Repo 'apps\desktop\src\pg.rs')
    $m = [regex]::Match($src, 'REQUIRED_TOOLS: &\[&str\] = &\[(?<l>[^\]]*)\]')
    if (-not $m.Success) { throw 'pg.rs: REQUIRED_TOOLS nicht gefunden – Keep-Liste für pgsql\bin unklar' }
    $tools = @([regex]::Matches($m.Groups['l'].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
    if ($tools.Count -lt 5) { throw "pg.rs: REQUIRED_TOOLS unplausibel ($($tools -join ','))" }
    return @($tools + 'pg_restore' | Select-Object -Unique)
}

function Add-License([string]$Part, [string]$Ver, [string]$License, [string]$Source, [string]$Texts) {
    $Licenses.Add([pscustomobject]@{ part = $Part; version = $Ver; license = $License; source = $Source; texts = $Texts })
}

$Notices = Join-Path $Out 'THIRD-PARTY-LICENSES'
$Work = Join-Path $Cache "work-$PID"
New-Item -ItemType Directory -Force $Out, $Notices | Out-Null
# Never version the output (results/desktop is not covered by the repo .gitignore)
foreach ($d in @((Split-Path $Out -Parent), (Split-Path $Cache -Parent))) {
    $gi = Join-Path $d '.gitignore'
    if ((Test-Path $d) -and -not (Test-Path $gi) -and $d -like "$Repo\results\*") { Write-Utf8 $gi "# Paketbau-Ausgabe (apps/desktop/pack/pack.ps1) – nicht versionieren`n*`n" }
}

try {
    # ------------------------------------------------------------------ own programs + UI
    if ($Only -contains 'app') {
        Step "PaperRust-Programme ($BuildProfile) aus $TargetDir\$BuildProfile"
        $bin = Join-Path $TargetDir $BuildProfile
        $build = if ($BuildProfile -eq 'release') { '--release ' } else { '' }
        $required = [ordered]@{
            'paperrust.exe'         = "cargo build $build-p pl-server --bin paperrust -j 1"
            'paperrust-desktop.exe' = "cargo build $build-p paperrust-desktop -j 1"
            'paperrust-app.exe'     = "cd apps\desktop-shell; cargo build $build-j 1"
            'paperrust-pdftool.exe' = "cargo build $build-p pl-pdftools --features engine --bin paperrust-pdftool -j 1"
        }
        foreach ($exe in $required.Keys) {
            $src = Join-Path $bin $exe
            if (-not (Test-Path $src)) { throw "$src fehlt. Bauen: $($required[$exe])" }
            Copy-Item -Force $src $Out
        }
        $mig = Join-Path $bin 'paperrust-migrate-db.exe'
        if (Test-Path $mig) { Copy-Item -Force $mig $Out } else { Write-Warning "paperrust-migrate-db fehlt ($mig) – Erst-Start ohne „Datenbank übernehmen“" }
        # Record the state of the programs (build time, size, SHA-256)
        $Manifest.parts['programs'] = [ordered]@{}
        foreach ($f in Get-ChildItem $Out -Filter '*.exe') {
            $Manifest.parts['programs'][$f.Name] = [ordered]@{ built = $f.LastWriteTimeUtc.ToString('o'); size = $f.Length; sha256 = (Get-FileHash -Algorithm SHA256 $f.FullName).Hash.ToLowerInvariant() }
        }

        # Since 2026-11-14 routes.toml only switches routes off ("off"); ids not mentioned are on.
        # The file is copied unchanged; an old value "django" would only be a warning → abort.
        Step "routes.toml aus $RoutesFile"
        $routes = Get-Content -Raw -Encoding UTF8 $RoutesFile
        if ($routes -match '(?m)^"[^"]+"\s*=\s*"django"') { throw 'routes.toml enthält noch django-Einträge' }
        $count = [regex]::Matches($routes, '(?m)^"([^"]+)"\s*=').Count
        Write-Utf8 (Join-Path $Out 'routes.toml') $routes
        $Manifest.parts['routes'] = @{ source = $RoutesFile; count = $count }

        Step 'Oberfläche (static)'
        if (-not $StaticDir) { $StaticDir = Join-Path $Repo 'results\static' }
        if (-not (Test-Path (Join-Path $StaticDir 'base.css'))) {
            throw "$StaticDir sieht nicht nach STATIC_ROOT aus (base.css fehlt) – bash scripts/build-static.sh"
        }
        New-CleanDir (Join-Path $Out 'static')
        # Pre-compressed .gz/.br variants (whitenoise) bring nothing over 127.0.0.1 and almost
        # double the size; pl-server serves the uncompressed file without them (statics.rs).
        $xf = if ($KeepPrecompressed) { @() } else { @('/XF', '*.gz', '*.br') }
        & robocopy.exe $StaticDir (Join-Path $Out 'static') /E /NFL /NDL /NJH /NJS /NP @xf | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "robocopy static fehlgeschlagen ($LASTEXITCODE)" }
        $Manifest.parts['static'] = @{ source = $StaticDir }

        Step 'Oberfläche (light)'
        if (-not $LightDir) { $LightDir = Join-Path $Repo 'frontend-light\dist' }
        if (Test-Path (Join-Path $LightDir 'index.html')) {
            New-CleanDir (Join-Path $Out 'light')
            & robocopy.exe $LightDir (Join-Path $Out 'light') /E /NFL /NDL /NJH /NJS /NP @xf | Out-Null
            if ($LASTEXITCODE -ge 8) { throw "robocopy light fehlgeschlagen ($LASTEXITCODE)" }
            $Manifest.parts['light'] = @{ source = $LightDir }
        } else {
            throw "$LightDir enthält keinen Build der Oberfläche (index.html fehlt; cd frontend-light; pnpm build)"
        }

        Step 'Startskripte, LICENSE'
        Copy-Item -Force (Join-Path $Repo 'LICENSE') (Join-Path $Out 'LICENSE')
        if (Test-Path (Join-Path $Repo 'NOTICE')) { Copy-Item -Force (Join-Path $Repo 'NOTICE') (Join-Path $Out 'NOTICE') }
        Copy-Notice (Join-Path $Repo 'upstream\LICENSE') (Join-Path $Notices 'paperless-ngx')
        $cmd = @'
@echo off
rem PaperRust starten (Fenster + Tray). Daten: %LOCALAPPDATA%\PaperRust
rem Portabel (Daten neben dem Programm):  PaperRust.cmd --portable
rem Konsole statt Fenster:               paperrust-desktop.exe run
setlocal
if /I "%~1"=="--portable" set "PAPERRUST_DESKTOP_HOME=%~dp0data"
start "" "%~dp0paperrust-app.exe"
'@
        Write-Utf8 (Join-Path $Out 'PaperRust.cmd') ($cmd -replace "`r?`n", "`r`n")
        $readme = @"
PaperRust $Version für Windows x64 (portabel, unsigniert)
=====================================================

Start:      PaperRust.cmd  (oder paperrust-app.exe)
Daten:      %LOCALAPPDATA%\PaperRust  (Datenbank, Dokumente, Logs, Sicherungen)
Portabel:   PaperRust.cmd --portable  -> Daten in .\data (nicht in OneDrive/Dropbox ablegen!)
Konsole:    paperrust-desktop.exe run | backup | stop | paths | manage <befehl>
Sicherung:  Tray-Symbol -> "Sicherung jetzt" (Ordner backups\<Zeit>: paperrust.pgdump + export\)

Erster Start: Formular "Willkommen" - neu beginnen, paperless-ngx-Export importieren oder
bestehende paperless-ngx-Datenbank übernehmen.

Windows SmartScreen warnt beim ersten Start, weil das Paket nicht signiert ist
("Weitere Informationen" -> "Trotzdem ausführen").

Benötigt: Windows 10/11 x64 mit WebView2 (Windows 11: vorinstalliert).
Lizenz: GPL-3.0 (LICENSE). Fremdteile: THIRD-PARTY-LICENSES.txt
"@
        Write-Utf8 (Join-Path $Out 'README.txt') ($readme -replace "`r?`n", "`r`n")
        Add-License 'PaperRust (paperrust, paperrust-desktop, paperrust-app, paperrust-migrate-db, paperrust-pdftool)' $Version 'GPL-3.0-only' 'https://github.com/paperrust/paperrust' 'LICENSE, NOTICE'
        Add-License 'paperless-ngx (Oberfläche static\, Schema, Verhalten)' '3.4.2' 'GPL-3.0' 'https://github.com/paperless-ngx/paperless-ngx' 'THIRD-PARTY-LICENSES\paperless-ngx\LICENSE'
    }

    # ------------------------------------------------------------------ PostgreSQL
    if ($Only -contains 'pg') {
        $s = $Sources.postgresql.targets.$Target
        Step "PostgreSQL $($Sources.postgresql.version)"
        $arc = Get-Pinned 'PostgreSQL' $s.url $s.sha256 $s.size
        if ($arc) {
            $tmp = Join-Path $Work 'pg'
            Expand-Archive2 $arc $tmp
            $root = Join-Path $tmp $s.strip_prefix
            $pg = Join-Path $Out 'pgsql'
            New-CleanDir $pg
            foreach ($d in 'bin', 'lib', 'share') { Move-Item (Join-Path $root $d) $pg }
            # Thinning (Z3): keep list instead of drop list, so new files in the archive do not
            # silently travel along. Justification per part in docs/DESKTOP.md section 17 ("PostgreSQL ausgedünnt").
            #  bin\   only Get-PgKeepTools (*.exe) + the DLLs that they or the retained server modules
            #         import statically (ICU, OpenSSL, libxml2, zstd, lz4, zlib, libpq …).
            #  lib\   plpgsql (initdb creates the extension; pl-sync triggers are plpgsql),
            #         dict_snowball (initdb creates the Snowball text search configurations),
            #         character set conversions *_and_*.dll (initdb registers them in pg_conversion;
            #         loaded with client_encoding ≠ UTF8, e.g. psql in the Windows console).
            #         Removed: contrib extensions (PaperRust/paperless-ngx create none – no CREATE
            #         EXTENSION in the code), replication (libpqwalreceiver, pgoutput), pgevent, pgxs, *.lib.
            #  share\ everything except doc\ and the contrib extension files in extension\ (only plpgsql).
            #         ICU data (icudt*.dll, 32 MB) stay: desktop.toml locale_provider = "icu" is
            #         the default (config.rs default_locale) and existing clusters need ICU.
            $keepTools = Get-PgKeepTools
            $binDir = Join-Path $pg 'bin'; $libDir = Join-Path $pg 'lib'
            foreach ($t in $keepTools) { if (-not (Test-Path (Join-Path $binDir "$t.exe"))) { throw "PostgreSQL-Archiv: $t.exe fehlt" } }
            Get-ChildItem $binDir -Directory | Remove-Item -Recurse -Force          # Lib\, DLLs\ (Python)
            Get-ChildItem $binDir -File | Where-Object { $_.Extension -ne '.dll' -and $keepTools -notcontains $_.BaseName } | Remove-Item -Force
            Get-ChildItem $libDir -Directory | Remove-Item -Recurse -Force          # pgxs\, pkgconfig\
            Get-ChildItem $libDir -File | Where-Object { -not ($_.Extension -eq '.dll' -and ($_.BaseName -in 'plpgsql', 'dict_snowball' -or $_.BaseName -match '^[a-z0-9_]+_and_[a-z0-9_]+$')) } | Remove-Item -Force
            Get-Item (Join-Path $pg 'share\doc') -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
            Get-ChildItem (Join-Path $pg 'share\extension') -File | Where-Object { $_.Name -notlike 'plpgsql*' } | Remove-Item -Force
            $llvm = Get-LlvmReadobj
            if ($llvm) {
                $roots = @(Get-ChildItem $binDir -Filter '*.exe' | ForEach-Object FullName) + @(Get-ChildItem $libDir -Filter '*.dll' | ForEach-Object FullName)
                $need = Get-ImportClosure $llvm.FullName $roots $binDir
                $dropped = @(Get-ChildItem $binDir -Filter '*.dll' | Where-Object { -not $need.ContainsKey($_.Name.ToLowerInvariant()) })
                $dropped | Remove-Item -Force
                Write-Host ("    bin: {0} Programme ({1}), {2} DLLs behalten, {3} entfernt ({4})" -f $keepTools.Count, ($keepTools -join ', '), $need.Count, $dropped.Count, (($dropped | ForEach-Object Name) -join ', '))
            } else { Write-Warning 'llvm-readobj (rustup component add llvm-tools) fehlt – PostgreSQL-DLLs nicht ausgedünnt' }
            Copy-Notice (Join-Path $root 'LICENSE') (Join-Path $Notices 'postgresql')
            Copy-Notice (Join-Path $root 'dependency-notices') (Join-Path $Notices 'postgresql')
            # Python (plpython), libxslt (contrib xml2 only) and pkgconf are thinned out – their notices do not belong in the package; neither do vcpkg build tools.
            Get-Item (Join-Path $Notices 'postgresql\dependency-notices\python'), (Join-Path $Notices 'postgresql\dependency-notices\vcpkg\vcpkg-*'), (Join-Path $Notices 'postgresql\dependency-notices\vcpkg\pkgconf'), (Join-Path $Notices 'postgresql\dependency-notices\vcpkg\libxslt') -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
            $mb = (Get-ChildItem -Recurse -File $pg | Measure-Object Length -Sum).Sum / 1MB
            Write-Host ("    pgsql ausgedünnt: {0:N1} MB" -f $mb)
            $Manifest.parts['postgresql'] = @{ version = $Sources.postgresql.version; sha256 = $s.sha256; url = $s.url; size_mb = [math]::Round($mb, 1) }
            $deps = (Get-ChildItem (Join-Path $Notices 'postgresql\dependency-notices\vcpkg') -ErrorAction SilentlyContinue | ForEach-Object { $_.Name }) -join ', '
            Add-License 'PostgreSQL (pgsql\)' $Sources.postgresql.version 'PostgreSQL License' $s.url 'THIRD-PARTY-LICENSES\postgresql\LICENSE'
            Add-License "  gebündelte Bibliotheken von PostgreSQL ($deps)" '' 'ICU (Unicode-3.0), OpenSSL (Apache-2.0), libxml2 (MIT), libiconv (LGPL-2.1+), zlib (zlib), lz4 (BSD-2), zstd (BSD-3)' $s.url 'THIRD-PARTY-LICENSES\postgresql\dependency-notices\'

            if (-not $SkipSelfCheck) {
                Step 'Selbsttest PostgreSQL (initdb ICU, Start, SELECT 1, Stopp)'
                $data = Join-Path $Work 'pgtest'
                $port = Get-Random -Minimum 55100 -Maximum 55900
                & (Join-Path $pg 'bin\initdb.exe') -D $data -U selftest --auth=trust --encoding=UTF8 --locale-provider=icu --icu-locale=und --locale=C | Out-Null
                if ($LASTEXITCODE -ne 0) { throw 'Selbsttest: initdb fehlgeschlagen' }
                # Not via a pipe (| Out-Null) and not Start-Process -Wait: the detached
                # postgres otherwise inherits the pipe or belongs to the process tree, and the call never returns.
                $ctl = Start-Process -FilePath (Join-Path $pg 'bin\pg_ctl.exe') -NoNewWindow -PassThru -ArgumentList @(
                    'start', '-D', "`"$data`"", '-w', '-t', '60', '-s', '-l', "`"$(Join-Path $Work 'pgtest.log')`"",
                    '-o', "`"-p $port -c listen_addresses=127.0.0.1 -c jit=off`"")
                $null = $ctl.Handle   # PS 5.1: without access to the handle ExitCode stays empty
                $ctl.WaitForExit()
                if ($ctl.ExitCode -ne 0) { throw "Selbsttest: Start fehlgeschlagen, siehe $Work\pgtest.log" }
                try {
                    $r = & (Join-Path $pg 'bin\psql.exe') -h 127.0.0.1 -p $port -U selftest -d postgres -tAc "select 1, current_setting('server_version')"
                    if ($LASTEXITCODE -ne 0 -or -not ($r -match '^1\|17\.')) { throw "Selbsttest: Abfrage fehlgeschlagen ($r)" }
                    Write-Host "    ok: $r"
                    # Z3: use everything that is still needed after thinning once.
                    $bin = Join-Path $pg 'bin'
                    & (Join-Path $bin 'pg_isready.exe') -h 127.0.0.1 -p $port -q
                    if ($LASTEXITCODE -ne 0) { throw 'Selbsttest: pg_isready' }
                    & (Join-Path $bin 'createdb.exe') -h 127.0.0.1 -p $port -U selftest selftestdb
                    if ($LASTEXITCODE -ne 0) { throw 'Selbsttest: createdb' }
                    $sql = @'
CREATE TABLE t (s text);
INSERT INTO t VALUES ('Zebra'), ('apfel'), (U&'\00C4pfel');
CREATE FUNCTION trg() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.s := upper(NEW.s); RETURN NEW; END $$;
CREATE TRIGGER t_trg BEFORE INSERT ON t FOR EACH ROW EXECUTE FUNCTION trg();
INSERT INTO t VALUES ('zz');
SELECT string_agg(CASE WHEN s = U&'\00C4pfel' THEN 'AEpfel' ELSE s END, ',' ORDER BY s) FROM t;
SELECT to_tsvector('german', 'Rechnungen der Zahnaerzte')::text;
SET client_encoding = 'WIN1252';
SELECT convert_to(U&'\00E4', 'LATIN1') = '\xe4'::bytea;
'@
                    # ASCII only over the pipe (PS 5.1 encodes pipes to programs as ASCII)
                    $q = $sql | & (Join-Path $bin 'psql.exe') -h 127.0.0.1 -p $port -U selftest -d selftestdb -X -q -tA -v ON_ERROR_STOP=1 2>&1
                    if ($LASTEXITCODE -ne 0) { throw "Selbsttest: SQL (ICU/plpgsql/Snowball/Konvertierung) fehlgeschlagen: $q" }
                    $q = @($q | Where-Object { $_ })
                    # ICU (und): language-correct apfel < Äpfel < Zebra < ZZ (C sorting would put Z before a);
                    # ZZ = trigger (plpgsql) took effect
                    if ($q[0] -cne 'apfel,AEpfel,Zebra,ZZ' -or $q[1] -notmatch "'rechnung'" -or $q[2] -ne 't') { throw "Selbsttest: unerwartete Ergebnisse: $($q -join ' | ')" }
                    $dump = Join-Path $Work 'pgtest.pgdump'
                    & (Join-Path $bin 'pg_dump.exe') -h 127.0.0.1 -p $port -U selftest -Fc -f $dump selftestdb
                    if ($LASTEXITCODE -ne 0) { throw 'Selbsttest: pg_dump' }
                    $list = & (Join-Path $bin 'pg_restore.exe') --list $dump
                    if ($LASTEXITCODE -ne 0 -or -not ($list -match 'TABLE DATA public t ')) { throw 'Selbsttest: pg_restore --list' }
                    Write-Host '    ok: pg_isready, createdb, ICU-Sortierung, plpgsql-Trigger, Snowball (german), WIN1252-Konvertierung, pg_dump/pg_restore'
                } finally {
                    & (Join-Path $pg 'bin\pg_ctl.exe') stop -D $data -m fast -w -s
                }
            }
        }
    }

    # ------------------------------------------------------------------ PDFium
    if ($Only -contains 'pdfium') {
        $s = $Sources.pdfium.targets.$Target
        Step "PDFium $($Sources.pdfium.version)"
        $arc = Get-Pinned 'PDFium' $s.url $s.sha256 $s.size
        if ($arc) {
            $tmp = Join-Path $Work 'pdfium'
            Expand-Archive2 $arc $tmp
            $dst = Join-Path $Out 'pdfium'
            New-CleanDir $dst
            Copy-Item -Force (Join-Path $tmp $s.file) $dst
            Copy-Notice (Join-Path $tmp 'LICENSE') (Join-Path $Notices 'pdfium')
            Get-ChildItem $tmp -Filter 'LICENSE*' | ForEach-Object { Copy-Notice $_.FullName (Join-Path $Notices 'pdfium') }
            $Manifest.parts['pdfium'] = @{ version = $Sources.pdfium.version; sha256 = $s.sha256; url = $s.url }
            Add-License 'PDFium (pdfium\pdfium.dll)' $Sources.pdfium.version 'BSD-3-Clause; enthält FreeType (FTL), ICU (Unicode-3.0), libjpeg-turbo, libpng, zlib, OpenJPEG (BSD-2), lcms2 (MIT), abseil (Apache-2.0) …' $s.url 'THIRD-PARTY-LICENSES\pdfium\'
        }
    }

    # ------------------------------------------------------------------ Tesseract
    if ($Only -contains 'tesseract') {
        $s = $Sources.tesseract.targets.$Target
        Step "Tesseract $($Sources.tesseract.version)"
        $arc = Get-Pinned 'Tesseract' $s.url $s.sha256 $s.size
        if ($arc) {
            $sevenZip = Get-SevenZip
            $dst = Join-Path $Out 'tesseract'
            $keepData = Join-Path $Work 'tessdata-keep'
            if (Test-Path (Join-Path $dst 'tessdata')) { New-Item -ItemType Directory -Force $Work | Out-Null; Move-Item (Join-Path $dst 'tessdata') $keepData }
            New-CleanDir $dst
            & $sevenZip x -y "-o$dst" $arc | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Tesseract: Entpacken fehlgeschlagen' }
            Get-Item (Join-Path $dst '$PLUGINSDIR'), (Join-Path $dst 'tesseract-uninstall.exe'), (Join-Path $dst 'winpath.exe') -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
            # Language data comes pinned from tessdata_fast (below), not from the installer; docs/training tools removed.
            Get-Item (Join-Path $dst 'tessdata') -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
            foreach ($p in 'doc', 'share', '*.html', 'ambiguous_words.exe', 'classifier_tester.exe', 'cntraining.exe', 'combine_lang_model.exe', 'combine_tessdata.exe', 'dawg2wordlist.exe', 'lstmeval.exe', 'lstmtraining.exe', 'merge_unicharsets.exe', 'mftraining.exe', 'set_unicharset_properties.exe', 'shapeclustering.exe', 'text2image.exe', 'unicharset_extractor.exe', 'wordlist2dawg.exe') {
                Get-Item (Join-Path $dst $p) -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
            }
            if (Test-Path $keepData) { Move-Item $keepData (Join-Path $dst 'tessdata') }
            # Thinning (D2, measured: 176 MB -> ~25 MB): only DLLs that tesseract.exe really loads
            # (imports recursively; pango/cairo/glib/ICU are needed only by text2image), debug info from
            # libtesseract (97 MB). Dependencies via llvm-readobj (rustup llvm-tools); if missing, all
            # DLLs stay in.
            $llvm = Get-ChildItem "$env:USERPROFILE\.rustup\toolchains\*\lib\rustlib\*\bin\llvm-readobj.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
            if ($llvm) {
                $have = @{}; Get-ChildItem $dst -Filter '*.dll' | ForEach-Object { $have[$_.Name.ToLowerInvariant()] = $_.FullName }
                $need = @{}; $todo = New-Object System.Collections.Stack; $todo.Push((Join-Path $dst 'tesseract.exe'))
                while ($todo.Count) {
                    $f = $todo.Pop()
                    foreach ($m in [regex]::Matches((& $llvm.FullName --coff-imports $f | Out-String), 'Name: (\S+\.dll)', 'IgnoreCase')) {
                        $n = $m.Groups[1].Value.ToLowerInvariant()
                        if ($have.ContainsKey($n) -and -not $need.ContainsKey($n)) { $need[$n] = $true; $todo.Push($have[$n]) }
                    }
                }
                $dropped = @($have.Keys | Where-Object { -not $need.ContainsKey($_) } | Sort-Object)
                foreach ($n in $dropped) { Remove-Item -Force $have[$n] }
                Write-Host ("    DLLs: {0} behalten, {1} entfernt (nur für Trainingswerkzeuge)" -f $need.Count, $dropped.Count)
                # llvm-strip/objcopy fail on the MinGW symbol tables ("invalid SymbolTableIndex") →
                # own function: cut off trailing .debug_* sections (like GNU strip --strip-debug).
                $saved = 0
                foreach ($f in Get-ChildItem $dst -File | Where-Object { $_.Extension -in '.dll', '.exe' }) { $saved += Remove-PeDebugSections $f.FullName }
                Write-Host ("    Debug-Abschnitte entfernt: {0:N1} MB" -f ($saved / 1MB))
            } else { Write-Warning 'llvm-readobj (rustup component add llvm-tools) fehlt – Tesseract nicht ausgedünnt' }
            $tmb = (Get-ChildItem -Recurse -File $dst | Measure-Object Length -Sum).Sum / 1MB
            Write-Host ("    tesseract: {0:N1} MB" -f $tmb)
            if (-not (Test-Path (Join-Path $dst 'tesseract.exe'))) { throw 'Tesseract: tesseract.exe fehlt nach dem Entpacken' }
            Get-ChildItem $dst -File | Where-Object Name -match '^(LICENSE|COPYING|AUTHORS|NOTICE)' | ForEach-Object { Copy-Notice $_.FullName (Join-Path $Notices 'tesseract') }
            # The UB-Mannheim build ships no license texts: Apache-2.0 text from the repo stock
            if (-not (Test-Path (Join-Path $Notices 'tesseract'))) { New-Item -ItemType Directory -Force (Join-Path $Notices 'tesseract') | Out-Null }
            $dlls = (Get-ChildItem $dst -Filter '*.dll' | ForEach-Object { $_.BaseName }) -join ', '
            $Manifest.parts['tesseract'] = @{ version = $Sources.tesseract.version; sha256 = $s.sha256; url = $s.url; dlls = $dlls }
            Add-License 'Tesseract OCR (tesseract\)' $Sources.tesseract.version 'Apache-2.0' 'https://github.com/UB-Mannheim/tesseract (Build), https://github.com/tesseract-ocr/tesseract' 'THIRD-PARTY-LICENSES\common\Apache-2.0.txt'
            Add-License "  mitgelieferte DLLs von Tesseract ($dlls)" '' 'Leptonica BSD-2, libpng + zlib (zlib/libpng), libjpeg-turbo (IJG/BSD-3), libtiff (libtiff), libwebp/libsharpyuv (BSD-3), giflib (MIT), OpenJPEG (BSD-2), libarchive (BSD-2), OpenSSL libcrypto (Apache-2.0), lz4 (BSD-2), zstd (BSD-3), xz/liblzma (0BSD), bzip2 (bzip2), libdeflate (MIT), expat (MIT), libiconv (LGPL-2.1+), LERC (Apache-2.0), jbigkit (GPL-2.0+), libb2 (CC0), GCC-Laufzeit libstdc++/libgcc (GPL-3.0 mit Runtime-Ausnahme), winpthreads (MIT/ZPL) – jeweils die MSYS2-Pakete des UB-Mannheim-Builds' 'https://github.com/UB-Mannheim/tesseract/wiki, https://packages.msys2.org/' 'THIRD-PARTY-LICENSES\tesseract\README.txt'
            Write-Utf8 (Join-Path $Notices 'tesseract\README.txt') ("Tesseract $($Sources.tesseract.version), Windows-Build der UB Mannheim (MSYS2/MinGW-w64).`r`nTesseract: Apache-2.0 (..\common\Apache-2.0.txt).`r`nMitgelieferte DLLs: $dlls`r`nDie Lizenzen der DLLs sind die der jeweiligen MSYS2-Pakete (https://packages.msys2.org/). Quellcode: https://github.com/tesseract-ocr/tesseract, Build-Skripte: https://github.com/UB-Mannheim/tesseract.`r`n")
            if (-not $SkipSelfCheck) {
                $v = & (Join-Path $dst 'tesseract.exe') --version 2>&1 | Select-Object -First 1
                Write-Host "    ok: $v"
            }
        }
    }

    if ($Only -contains 'tessdata') {
        Step "Sprachdaten $($Sources.tessdata.version)"
        $dst = Join-Path $Out 'tesseract\tessdata'
        New-Item -ItemType Directory -Force $dst | Out-Null
        $done = @()
        foreach ($p in $Sources.tessdata.files.PSObject.Properties) {
            $f = Get-Pinned $p.Name $p.Value.url $p.Value.sha256 $p.Value.size
            if ($f) { Copy-Item -Force $f (Join-Path $dst $p.Name); $done += $p.Name }
        }
        $Manifest.parts['tessdata'] = @{ version = $Sources.tessdata.version; files = $done }
        Add-License "Tesseract-Sprachdaten ($($done -join ', '))" $Sources.tessdata.version 'Apache-2.0' 'https://github.com/tesseract-ocr/tessdata_fast' 'THIRD-PARTY-LICENSES\common\Apache-2.0.txt'
        $tess = Join-Path $Out 'tesseract\tesseract.exe'
        if ((Test-Path $tess) -and -not $SkipSelfCheck) {
            $langs = (& $tess --tessdata-dir $dst --list-langs 2>&1 | Select-Object -Skip 1) -join ','
            Write-Host "    Sprachen: $langs"
            if ($langs -notmatch 'deu' -or $langs -notmatch 'eng') { throw "Tesseract-Selbsttest: deu/eng fehlen ($langs)" }
        }
    }

    # ------------------------------------------------------------------ Schrift (Text-Vorschaubilder)
    if ($Only -contains 'fonts') {
        $f = $Sources.fonts
        Step "Schrift $($f.version)"
        $arc = Get-Pinned 'Liberation-Schriften' $f.url $f.sha256 $f.size
        if ($arc) {
            $tmp = Join-Path $Work 'fonts'
            Expand-Archive2 $arc $tmp
            $root = Join-Path $tmp $f.strip_prefix
            $dst = Join-Path $Out 'fonts'
            New-CleanDir $dst
            foreach ($n in $f.files) { Copy-Item -Force (Join-Path $root $n) $dst }
            Copy-Notice (Join-Path $root 'LICENSE') (Join-Path $Notices 'liberation-fonts')
            if (-not (Test-Path (Join-Path $Notices 'liberation-fonts\LICENSE'))) { throw 'Liberation: LICENSE (SIL OFL 1.1) fehlt im Archiv' }
            $Manifest.parts['fonts'] = @{ version = $f.version; sha256 = $f.sha256; url = $f.url; files = $f.files }
            Add-License "Liberation-Schriften ($($f.files -join ', '); fonts\)" $f.version 'SIL Open Font License 1.1' 'https://github.com/liberationfonts/liberation-fonts' 'THIRD-PARTY-LICENSES\liberation-fonts\LICENSE'
        }
    }

    # ------------------------------------------------------------------ Rust crates + overview
    if ($Only -contains 'licenses') {
        Step 'Lizenzen: Rust-Abhängigkeiten (cargo metadata), Standardtexte, Übersicht'
        $common = Join-Path $Notices 'common'
        New-Item -ItemType Directory -Force $common | Out-Null
        foreach ($t in Get-ChildItem (Join-Path $PSScriptRoot 'licenses') -Filter '*.txt' -ErrorAction SilentlyContinue) { Copy-Item -Force $t.FullName $common }
        $crates = @{}
        foreach ($ws in @($Repo, (Join-Path $Repo 'apps\desktop-shell'))) {
            Push-Location $ws
            try {
                $meta = & cargo metadata --format-version 1 --locked --offline --filter-platform $Target 2>$null | ConvertFrom-Json
            } finally { Pop-Location }
            if (-not $meta) { Write-Warning "cargo metadata in $ws fehlgeschlagen – Crate-Liste unvollständig"; continue }
            foreach ($p in $meta.packages) {
                if ($null -eq $p.source) { continue }   # eigene Crates (GPL-3.0, siehe LICENSE)
                $crates["$($p.name) $($p.version)"] = "{0,-45} {1,-12} {2}" -f $p.name, $p.version, $(if ($p.license) { $p.license } else { "siehe $($p.license_file)" })
            }
        }
        $lines = @("Rust-Abhängigkeiten von paperrust, paperrust-desktop, paperrust-app und paperrust-migrate-db",
                   "(aus Cargo.lock beider Workspaces, Ziel $Target; Quelle: crates.io). Lizenztexte der",
                   "häufigsten Lizenzen: common\. Vollständige Copyright-Vermerke je Crate: siehe docs/DESKTOP.md",
                   "Abschnitt 19 (cargo about – offener Punkt).", "") + ($crates.Keys | Sort-Object | ForEach-Object { $crates[$_] })
        Write-Utf8 (Join-Path $Notices 'rust-crates.txt') (($lines -join "`r`n") + "`r`n")
        Add-License "Rust-Bibliotheken ($($crates.Count) Crates)" '' 'überwiegend MIT/Apache-2.0, außerdem BSD-2/BSD-3, ISC, Zlib, Unicode-3.0, MPL-2.0, CDLA-Permissive-2.0 (Liste je Crate)' 'crates.io' 'THIRD-PARTY-LICENSES\rust-crates.txt, THIRD-PARTY-LICENSES\common\'
        Add-License 'Tauri 2, wry, tao, WebView2-Loader-Bindings' '' 'MIT/Apache-2.0 (webview2-com-sys: Microsoft-Header, BSD-3)' 'https://tauri.app' 'THIRD-PARTY-LICENSES\rust-crates.txt'
        Add-License 'Microsoft Edge WebView2 Runtime' '' 'nicht mitgeliefert (Bestandteil von Windows 10/11 bzw. vom Installer nachgeladen)' 'https://developer.microsoft.com/microsoft-edge/webview2/' '-'
    }

    if ($Licenses.Count -gt 0) {
        $txt = New-Object System.Text.StringBuilder
        [void]$txt.AppendLine("PaperRust $Version – mitgelieferte Teile und ihre Lizenzen")
        [void]$txt.AppendLine("=" * 60)
        [void]$txt.AppendLine("PaperRust selbst steht unter GPL-3.0-only (LICENSE). Quellcode: siehe NOTICE.")
        [void]$txt.AppendLine("Alle unten genannten Lizenzen sind mit GPL-3.0 verträglich. Texte im Ordner THIRD-PARTY-LICENSES\.")
        [void]$txt.AppendLine("")
        foreach ($l in $Licenses) {
            [void]$txt.AppendLine($l.part + $(if ($l.version) { "  [$($l.version)]" } else { '' }))
            [void]$txt.AppendLine("    Lizenz: $($l.license)")
            [void]$txt.AppendLine("    Quelle: $($l.source)")
            [void]$txt.AppendLine("    Texte:  $($l.texts)")
            [void]$txt.AppendLine("")
        }
        Write-Utf8 (Join-Path $Out 'THIRD-PARTY-LICENSES.txt') ($txt.ToString() -replace "`r?`n", "`r`n")
    }

    $total = (Get-ChildItem -Recurse -File $Out | Measure-Object Length -Sum).Sum
    $Manifest['size_bytes'] = $total
    $Manifest | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 (Join-Path $Out 'PACKAGE.json')
    Step ("Verzeichnis fertig: {0} ({1:N1} MB)" -f $Out, ($total / 1MB))

    if ($Zip) {
        $zipFile = Join-Path (Split-Path $Out -Parent) "$PackName.zip"
        Step "ZIP $zipFile"
        if (Test-Path $zipFile) { Remove-Item -Force $zipFile }
        # bsdtar: -a chooses the format by extension; root in the ZIP = $PackName\
        & tar.exe -a -c -f $zipFile -C (Split-Path $Out -Parent) $PackName
        if ($LASTEXITCODE -ne 0) { throw 'ZIP fehlgeschlagen' }
        $z = Get-Item $zipFile
        Write-Host ("    {0:N1} MB, SHA-256 {1}" -f ($z.Length / 1MB), (Get-FileHash -Algorithm SHA256 $zipFile).Hash.ToLowerInvariant())
    }

    if ($Installer) {
        & (Join-Path $PSScriptRoot 'installer.ps1') -PackDir $Out -Version $Version -Cache $Cache -TargetDir $TargetDir
        if ($LASTEXITCODE -ne 0) { throw "installer.ps1 fehlgeschlagen ($LASTEXITCODE)" }
    }
} finally {
    if (Test-Path $Work) { Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue }
}
