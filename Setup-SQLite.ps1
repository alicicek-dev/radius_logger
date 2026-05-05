#Requires -Version 7.0
# Eduroam NPS — SQLite veritabani kurulumu
# Ilk kurulumda bir kez calistirin (yonetici gerektirmez).

$DBPath = "C:\EduroamLogs\eduroam_logs.db"
$DBDir  = Split-Path $DBPath

if (-not (Get-Module -ListAvailable -Name PSSQLite)) {
    Write-Host "PSSQLite kuruluyor..." -ForegroundColor Yellow
    Install-Module PSSQLite -Force -Scope AllUsers -Repository PSGallery
}
Import-Module PSSQLite -ErrorAction Stop

if (-not (Test-Path $DBDir)) {
    New-Item -ItemType Directory -Path $DBDir -Force | Out-Null
    Write-Host "Klasor olusturuldu: $DBDir" -ForegroundColor DarkGray
}

if (Test-Path $DBPath) {
    Write-Host "UYARI: $DBPath zaten mevcut." -ForegroundColor Yellow
    $c = Read-Host "Uzerine yazmak icin 'EVET' yazin, yoksa cikis yapilir"
    if ($c -ne 'EVET') { Write-Host "Iptal." -ForegroundColor Yellow; exit 0 }
    Remove-Item $DBPath -Force
    Write-Host "Eski veritabani silindi." -ForegroundColor DarkGray
}

$schema = @"
CREATE TABLE AuthLog (
    LogID            INTEGER PRIMARY KEY AUTOINCREMENT,
    EventID          INTEGER,
    TimeCreated      TEXT,
    FullUsername     TEXT,
    Username         TEXT,
    Realm            TEXT,
    ClientIP         TEXT,
    NASIdentifier    TEXT,
    NASIPAddress     TEXT,
    CalledStationID  TEXT,
    CallingStationID TEXT,
    AuthType         TEXT,
    PolicyName       TEXT,
    ReasonCode       INTEGER DEFAULT 0,
    Reason           TEXT,
    Result           TEXT
);

CREATE TABLE Devices (
    DeviceID     INTEGER PRIMARY KEY AUTOINCREMENT,
    MACAddress   TEXT UNIQUE,
    FirstSeen    TEXT,
    LastSeen     TEXT,
    LastUsername TEXT,
    LastResult   TEXT
);

CREATE UNIQUE INDEX idx_no_dup ON AuthLog(TimeCreated, Username, CallingStationID, Result, ClientIP);
CREATE INDEX idx_time   ON AuthLog(TimeCreated);
CREATE INDEX idx_user   ON AuthLog(Username);
CREATE INDEX idx_result ON AuthLog(Result);
CREATE INDEX idx_realm  ON AuthLog(Realm);
CREATE INDEX idx_nas    ON AuthLog(NASIdentifier);
CREATE INDEX idx_mac    ON Devices(MACAddress);

PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
"@

$schema -split ";\s*`n" | Where-Object { $_.Trim() } | ForEach-Object {
    Invoke-SqliteQuery -DataSource $DBPath -Query $_.Trim()
}

Write-Host "Veritabani olusturuldu: $DBPath" -ForegroundColor Green
Write-Host ""
Write-Host "Sonraki adimlar:" -ForegroundColor Cyan
Write-Host "  1. Gecmis veriyi doldur : pwsh -ExecutionPolicy Bypass -File C:\EduroamLogs\Reset-And-Backfill.ps1"
Write-Host "  2. Gorevleri kaydet     : pwsh -ExecutionPolicy Bypass -File C:\EduroamLogs\Register-ScheduledTasks.ps1"
Write-Host "  3. Dashboard'u ac       : Start-Process 'http://127.0.0.1:8080'"
