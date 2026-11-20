# Starts a program, waits and outputs the maximum memory size (PeakWorkingSet).
# Invocation: powershell -File peakmem.ps1 <exe> <stdout> <stderr> <args...>
param([string]$Exe, [string]$Out, [string]$Err, [Parameter(ValueFromRemainingArguments = $true)][string[]]$Rest)
$p = Start-Process -FilePath $Exe -ArgumentList $Rest -RedirectStandardOutput $Out -RedirectStandardError $Err -PassThru -NoNewWindow
$null = $p.Handle
$peak = 0
while (-not $p.HasExited) {
  try { $p.Refresh(); if ($p.PeakWorkingSet64 -gt $peak) { $peak = $p.PeakWorkingSet64 } } catch {}
  Start-Sleep -Milliseconds 200
}
$p.WaitForExit()
Write-Output ("PEAK_MB=" + [math]::Round($peak / 1MB, 1))
exit $p.ExitCode
