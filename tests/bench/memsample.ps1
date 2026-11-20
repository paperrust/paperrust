# Sum of the working sets of ingest_bench + tesseract every 100 ms; maximum (MB) to $Out until $Stop exists.
param([string]$Out, [string]$Stop)
$max = 0
while (-not (Test-Path $Stop)) {
  $s = (Get-Process -Name ingest_bench, tesseract -ErrorAction SilentlyContinue | Measure-Object WorkingSet64 -Sum).Sum
  if ($s -gt $max) { $max = $s }
  Start-Sleep -Milliseconds 100
}
[math]::Round($max / 1MB) | Out-File -Encoding ascii $Out
