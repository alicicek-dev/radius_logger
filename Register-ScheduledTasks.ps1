#Requires -Version 7.0
# Eduroam Zamanlanmis Gorev — Yonetici olarak calistirin
# Kolektoer her 10 dk calisir, bittikten sonra export'u kendisi tetikler.
# Ayri bir export gorevi GEREKMEZ.

$pwsh = (Get-Command pwsh -ErrorAction SilentlyContinue)?.Source
if (-not $pwsh) { Write-Host "pwsh bulunamadi. winget install Microsoft.PowerShell" -ForegroundColor Red; exit 1 }
Write-Host "PS7 yolu: $pwsh" -ForegroundColor DarkGray

$action = New-ScheduledTaskAction `
    -Execute $pwsh `
    -Argument "-NonInteractive -ExecutionPolicy Bypass -File `"C:\EduroamLogs\Eduroam-NPS-LogCollector.ps1`""

$trigger = New-ScheduledTaskTrigger `
    -RepetitionInterval (New-TimeSpan -Minutes 10) -Once -At (Get-Date)

$settings = New-ScheduledTaskSettingsSet `
    -MultipleInstances IgnoreNew `
    -StartWhenAvailable `
    -ExecutionTimeLimit (New-TimeSpan -Minutes 9) `
    -RunOnlyIfNetworkAvailable

Register-ScheduledTask `
    -TaskName "EduroamNPSLogCollector" `
    -Action   $action `
    -Trigger  $trigger `
    -Settings $settings `
    -RunLevel Highest `
    -User     "NT AUTHORITY\SYSTEM" `
    -Force
Write-Host "Gorev kayit edildi: EduroamNPSLogCollector" -ForegroundColor Green

# ── 2. HTTP Dashboard Sunucusu (sistem baslarken) ────────────────────────────
$srvAction = New-ScheduledTaskAction `
    -Execute $pwsh `
    -Argument "-NonInteractive -ExecutionPolicy Bypass -File `"C:\EduroamLogs\Start-EduroamServer.ps1`""

$srvTrigger  = New-ScheduledTaskTrigger -AtStartup
$srvSettings = New-ScheduledTaskSettingsSet `
    -MultipleInstances IgnoreNew `
    -StartWhenAvailable `
    -ExecutionTimeLimit (New-TimeSpan -Hours 0)  # Suresiz calisir

Register-ScheduledTask `
    -TaskName "EduroamDashboardServer" `
    -Action   $srvAction `
    -Trigger  $srvTrigger `
    -Settings $srvSettings `
    -RunLevel Highest `
    -User     "NT AUTHORITY\SYSTEM" `
    -Force
Write-Host "Gorev kayit edildi: EduroamDashboardServer (sistem baslarken)" -ForegroundColor Green

Write-Host ""
Get-ScheduledTask -TaskName "EduroamNPSLogCollector","EduroamDashboardServer" |
    Select-Object TaskName, State | Format-Table -AutoSize
