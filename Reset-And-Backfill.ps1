#Requires -Version 7.0
# Eduroam — DB Sifirla ve Son 7 Gunu Yeniden Doldur (SQLite)

$DBPath    = "C:\EduroamLogs\eduroam_logs.db"
$StateFile = "C:\EduroamLogs\last_run.txt"
$LogDir    = "C:\Users\radius1\Desktop\nps"
$Collector = "C:\EduroamLogs\Eduroam-NPS-LogCollector.ps1"
$DaysBack  = 7

Import-Module PSSQLite

Write-Host "===== DB SIFIRLA + BACKFILL =====" -ForegroundColor Cyan
Write-Host "DB     : $DBPath"
Write-Host "Log kl.: $LogDir"
Write-Host "Aralik : Son $DaysBack gun"
Write-Host ""

# ── Mevcut kayit sayisini goster ─────────────────────────────────────────────
$authCount = (Invoke-SqliteQuery -DataSource $DBPath -Query "SELECT COUNT(*) AS N FROM AuthLog")[0].N
$devCount  = (Invoke-SqliteQuery -DataSource $DBPath -Query "SELECT COUNT(*) AS N FROM Devices")[0].N
Write-Host "Mevcut: $authCount AuthLog kaydi, $devCount cihaz" -ForegroundColor Yellow
Write-Host ""

# ── Onay ─────────────────────────────────────────────────────────────────────
Write-Host "UYARI: Bu islem tum AuthLog ve Devices kayitlarini silecek!" -ForegroundColor Red
$confirm = Read-Host "Devam etmek icin 'EVET' yazin"
if ($confirm -ne 'EVET') {
    Write-Host "Iptal edildi." -ForegroundColor Yellow
    exit 0
}

# ── DB temizle ────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "Temizleniyor..." -ForegroundColor Yellow
Invoke-SqliteQuery -DataSource $DBPath -Query "DELETE FROM AuthLog"
Invoke-SqliteQuery -DataSource $DBPath -Query "DELETE FROM Devices"
Invoke-SqliteQuery -DataSource $DBPath -Query "DELETE FROM sqlite_sequence WHERE name IN ('AuthLog','Devices')"
Write-Host "DB temizlendi." -ForegroundColor Green

# ── Log dosyasi kontrolu ──────────────────────────────────────────────────────
Write-Host ""
Write-Host "Log dosyalari:" -ForegroundColor Yellow
$found = 0
for ($i = $DaysBack; $i -ge 0; $i--) {
    $day  = (Get-Date).AddDays(-$i).Date
    $file = Join-Path $LogDir ("IN" + $day.ToString("yyMMdd") + ".log")
    if (Test-Path $file) {
        $mb = [math]::Round((Get-Item $file).Length/1MB,1)
        Write-Host "  $($day.ToString('yyyy-MM-dd')): $mb MB" -ForegroundColor DarkCyan
        $found++
    } else {
        Write-Host "  $($day.ToString('yyyy-MM-dd')): yok" -ForegroundColor DarkGray
    }
}

if ($found -eq 0) { Write-Host "Log dosyasi bulunamadi." -ForegroundColor Red; exit 1 }

# ── State dosyasini geri al ───────────────────────────────────────────────────
$oldest = (Get-Date).AddDays(-$DaysBack).Date
$tmp = $StateFile + ".tmp"
@{ File="IN$($oldest.ToString('yyMMdd')).log"; Timestamp=$oldest.ToString("o") } |
    ConvertTo-Json | Set-Content $tmp -Encoding UTF8
Move-Item $tmp $StateFile -Force
Write-Host ""
Write-Host "State: $($oldest.ToString('yyyy-MM-dd'))" -ForegroundColor Cyan

# ── Kolektoru calistir ────────────────────────────────────────────────────────
Write-Host "Kolektoer baslatiliyor..." -ForegroundColor Cyan
& $Collector

# ── Sonuc ─────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== SONUC ===" -ForegroundColor Yellow
for ($i = $DaysBack; $i -ge 0; $i--) {
    $day     = (Get-Date).AddDays(-$i).Date
    $dayStr  = $day.ToString("yyyy-MM-dd")
    $nextDay = $day.AddDays(1).ToString("yyyy-MM-dd")
    $cnt     = (Invoke-SqliteQuery -DataSource $DBPath -Query "SELECT COUNT(*) AS N FROM AuthLog WHERE TimeCreated >= '$dayStr' AND TimeCreated < '$nextDay'")[0].N
    Write-Host "  $dayStr : $cnt kayit" -ForegroundColor $(if($cnt -gt 0){"Green"}else{"DarkGray"})
}

Write-Host ""
Write-Host "Bitti. Dashboard'u acmak icin sunucunun calistiginden emin olun:" -ForegroundColor Green
Write-Host "  Start-Process 'http://127.0.0.1:8080'" -ForegroundColor Yellow
