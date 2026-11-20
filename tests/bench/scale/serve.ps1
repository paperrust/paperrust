# Starts a PaperRust measurement instance against the scale DB, detached (Start-Process), writes PID + log.
# Invocation: powershell -NoProfile -File tests/bench/scale/serve.ps1 -Exe <paperrust.exe> -Work <dir> [-Database pl_scale_1m]
#         [-Port 58181] [-Worker] [-Log "warn,paperrust::access=info"] [-Tag name]
# Stop: Stop-Process -Id (Get-Content <Work>\serve-<Tag>.pid)   (only your own PID!)
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Work,
    [string]$Database = "pl_scale_1m",
    [int]$Port = 58181,
    [switch]$Worker,
    [string]$Log = "warn,paperrust::access=info",
    [string]$Tag = "main"
)
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path "$PSScriptRoot\..\..\..").Path
foreach ($d in "data", "media", "consume", "logs") { New-Item -ItemType Directory -Force (Join-Path $Work $d) | Out-Null }
$envs = @{
    PAPERLESS_DBHOST = "127.0.0.1"; PAPERLESS_DBPORT = "54329"; PAPERLESS_DBNAME = $Database
    PAPERLESS_DBUSER = "paperless"; PAPERLESS_DBPASS = "paperless"
    PAPERLESS_SECRET_KEY = "scale-local-only-secret-key-0123456789abcdef"
    PAPERLESS_TIME_ZONE = "Europe/Zurich"
    PAPERLESS_DATA_DIR = (Join-Path $Work "data"); PAPERLESS_MEDIA_ROOT = (Join-Path $Work "media")
    PAPERLESS_CONSUMPTION_DIR = (Join-Path $Work "consume"); PAPERLESS_LOGGING_DIR = (Join-Path $Work "logs")
    PAPERLESS_EMAIL_TASK_CRON = "disable"; PAPERLESS_TRAIN_TASK_CRON = "disable"
    PAPERLESS_INDEX_TASK_CRON = "disable"; PAPERLESS_SANITY_TASK_CRON = "disable"
    PAPERLESS_WORKFLOW_SCHEDULED_TASK_CRON = "disable"; PAPERLESS_EMPTY_TRASH_TASK_CRON = "disable"
    PAPERLESS_OCR_MODE = "skip"
    PAPERRUST_BIND = "127.0.0.1:$Port"
    PAPERRUST_ROUTES = (Join-Path $repo "results\parity\routes-all.toml")
    PAPERRUST_BRANDING = "off"; PAPERRUST_LOG = $Log
    PAPERRUST_UPDATE_CHECK = "off"; NO_COLOR = "1"
}
foreach ($k in $envs.Keys) { [Environment]::SetEnvironmentVariable($k, $envs[$k], "Process") }
$argv = @("serve"); if ($Worker) { $argv += "--worker" }
$out = Join-Path $Work "serve-$Tag.log"; $err = Join-Path $Work "serve-$Tag.err.log"
$p = Start-Process -FilePath $Exe -ArgumentList $argv -WorkingDirectory $Work -WindowStyle Hidden `
    -RedirectStandardOutput $out -RedirectStandardError $err -PassThru
$p.Id | Out-File -Encoding ascii (Join-Path $Work "serve-$Tag.pid")
Write-Output $p.Id
