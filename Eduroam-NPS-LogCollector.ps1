#Requires -Version 7.0
# Eduroam NPS Log Kolektoru v4 — SQLite
# PT=1 -> Queue cache (ClientIP bazli FIFO) -> PT=2 SUCCESS / PT=3 FAILURE

$LogDir    = "C:\Users\radius1\Desktop\nps"
$DBPath    = "C:\EduroamLogs\eduroam_logs.db"
$StateFile = "C:\EduroamLogs\last_run.txt"
$ErrorLog  = "C:\EduroamLogs\collector_errors.log"
$WantedPacketTypes = @("1","2","3")
$BatchSize = 500

if (-not (Get-Module -ListAvailable -Name PSSQLite)) {
    Write-Host "PSSQLite bulunamadi, kuruluyor..." -ForegroundColor Yellow
    Install-Module PSSQLite -Force -Scope AllUsers -Repository PSGallery
}
Import-Module PSSQLite -ErrorAction Stop

$MaxLogMB = 5   # Log dosyasi bu boyutu asinca rotate edilir

function Write-Log { param([string]$M,[string]$L="INFO")
    try {
        # Rotation: dosya MaxLogMB'i astiysa .old yap, temiz baslat
        if ((Test-Path $ErrorLog) -and (Get-Item $ErrorLog).Length -gt ($MaxLogMB * 1MB)) {
            $oldLog = $ErrorLog -replace '\.log$', '.old.log'
            if (Test-Path $oldLog) { Remove-Item $oldLog -Force }
            Rename-Item $ErrorLog $oldLog -Force
        }
        $fs = [System.IO.FileStream]::new($ErrorLog,[System.IO.FileMode]::Append,[System.IO.FileAccess]::Write,[System.IO.FileShare]::ReadWrite)
        $sw = [System.IO.StreamWriter]::new($fs)
        $sw.WriteLine("[$(Get-Date -f 'yyyy-MM-dd HH:mm:ss')][$L] $M")
        $sw.Close(); $fs.Close()
    } catch { Write-Host "[LOG-HATA] $($_.Exception.Message)" -ForegroundColor DarkRed }
}

function Get-LastState {
    if (Test-Path $StateFile) {
        $r = Get-Content $StateFile -Raw | ConvertFrom-Json
        return @{ File = $r.File; Timestamp = [DateTime]::Parse($r.Timestamp) }
    }
    return @{ File = "IN$(Get-Date -f 'yyMMdd').log"; Timestamp = (Get-Date).Date }
}

function Set-LastState { param([string]$File,[DateTime]$Timestamp)
    # Temp dosyaya yaz, sonra rename (atomic) — yari yazili state riskini ortadan kaldirir
    $tmp = $StateFile + ".tmp"
    @{ File=$File; Timestamp=$Timestamp.ToString("o") } | ConvertTo-Json | Set-Content $tmp -Encoding UTF8
    Move-Item $tmp $StateFile -Force
}

function Get-LogFilesAfter { param([string]$AfterFile)
    $afterDate = [DateTime]::ParseExact(($AfterFile -replace "IN(\d{6})\.log",'$1'), "yyMMdd", $null)
    Get-ChildItem $LogDir -Filter "IN??????.log" | Where-Object {
        [DateTime]::ParseExact(($_.Name -replace "IN(\d{6})\.log",'$1'), "yyMMdd", $null) -ge $afterDate
    } | Sort-Object Name
}

function Get-F([string]$data,[string]$n) {
    if ($data -match "<$n[^>]*>([^<]*)</$n>") { return $Matches[1].Trim() }
    return ""
}

# Tum alanlari tek bir pass ile cikartir — her alan icin ayri regex yerine
# bir kez tum tag'leri sozluge donusturur (~8x daha az regex calistirir)
function Get-AllFields([string]$line) {
    $d = [System.Collections.Generic.Dictionary[string,string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    $m = [regex]::Matches($line, '<([A-Za-z0-9_-]+)[^>]*>([^<]*)</\1>')
    foreach ($match in $m) {
        $d[$match.Groups[1].Value] = $match.Groups[2].Value.Trim()
    }
    return $d
}

$ReasonMap = @{
    0="Basari"; 1="Dahili hata"; 2="Erisim reddedildi"; 3="Hatali istek"
    4="Global Katalog erisilemiyor"; 5="Etki alani kullanilamiyor"
    6="Sunucu kullanilamiyor"; 7="Etki alani bulunamadi"; 8="Kullanici bulunamadi"
    16="Kimlik dogrulama basarisiz"; 17="Sifre degistirme basarisiz"
    18="Desteklenmeyen kimlik dogrulama turu"; 32="Yalnizca yerel kullanicilar"
    33="Sifre degistirilmeli"; 34="Hesap devre disi"; 35="Hesap suresi dolmus"
    36="Hesap kilitli"; 37="Gecersiz giris saatleri"; 38="Hesap kisitlamasi"
    48="Esleyen ilke yok"; 64="Dial-in kilitli"; 65="Dial-in devre disi"
    66="Gecersiz kimlik dogrulama turu"; 67="Gecersiz cagiran istasyon"
    68="Gecersiz dial-in saatleri"; 69="Gecersiz aranan istasyon"
    70="Gecersiz port turu"; 71="Gecersiz kisitlama"; 80="Kayit bulunamadi"
    96="Oturum zaman asimi"; 97="Beklenmeyen istek"; 100="Proxy reddetti"
    101="Proxy baglanti hatasi"; 102="Proxy iletme hatasi"
    103="Proxy gecersiz yanit"; 104="Proxy hatali yanit"
    105="Proxy zaman asimi"; 106="Proxy yanit yok"; 107="Gecersiz oznitelik"
    112="Istek iletilmedi"
}
function Get-ReasonText([string]$c) {
    if ($c -match '^\d+$' -and $ReasonMap.ContainsKey([int]$c)) { return $ReasonMap[[int]$c] }
    return "Kod: $c"
}

# ── SQLite yardimcilari ───────────────────────────────────────────────────────
$insertSQL = @"
INSERT OR IGNORE INTO AuthLog
    (EventID,TimeCreated,FullUsername,Username,Realm,ClientIP,NASIdentifier,
     NASIPAddress,CalledStationID,CallingStationID,AuthType,PolicyName,ReasonCode,Reason,Result)
VALUES
    (@EventID,@TimeCreated,@FullUsername,@Username,@Realm,@ClientIP,@NASIdentifier,
     @NASIPAddress,@CalledStationID,@CallingStationID,@AuthType,@PolicyName,@ReasonCode,@Reason,@Result)
"@

$upsertDevSQL = @"
INSERT INTO Devices (MACAddress,FirstSeen,LastSeen,LastUsername,LastResult)
VALUES (@mac,@ts,@ts,@user,@result)
ON CONFLICT(MACAddress) DO UPDATE SET
    LastSeen=@ts, LastUsername=@user, LastResult=@result
"@

function Write-AuthRecord {
    param($cmd, $trans, [string]$PacketType, [hashtable]$D)
    $rc  = if ($D.ReasonCode -match '^\d+$') { [int]$D.ReasonCode } else { 0 }
    $cmd.Parameters.Clear()
    $cmd.Transaction = $trans
    @{
        "@EventID"          = [int]$PacketType
        "@TimeCreated"      = if ($D.Timestamp) { $D.Timestamp.ToString("yyyy-MM-dd HH:mm:ss") } else { "1900-01-01 00:00:00" }
        "@FullUsername"     = $D.FullUsername ?? ""
        "@Username"         = $D.Username ?? ""
        "@Realm"            = $D.Realm ?? ""
        "@ClientIP"         = $D.ClientIP ?? ""
        "@NASIdentifier"    = $D.NASIdentifier ?? ""
        "@NASIPAddress"     = $D.NASIPAddress ?? ""
        "@CalledStationID"  = $D.CalledStationID ?? ""
        "@CallingStationID" = $D.CallingStationID ?? ""
        "@AuthType"         = $D.AuthType ?? ""
        "@PolicyName"       = $D.PolicyName ?? ""
        "@ReasonCode"       = $rc
        "@Reason"           = Get-ReasonText $D.ReasonCode
        "@Result"           = $D.Result
    }.GetEnumerator() | ForEach-Object { $cmd.Parameters.AddWithValue($_.Key,$_.Value) | Out-Null }
    try { $cmd.ExecuteNonQuery() | Out-Null; return $true }
    catch { Write-Log "INSERT: $($_.Exception.Message)" "ERROR"; return $false }
}

function Upsert-Device {
    param($devCmd, $trans, [hashtable]$D)
    $mac = ($D.CallingStationID ?? "").ToUpper().Trim()
    if (-not $mac) { return }
    $devCmd.Parameters.Clear()
    $devCmd.Transaction = $trans
    $devCmd.Parameters.AddWithValue("@mac",    $mac)                                                    | Out-Null
    $devCmd.Parameters.AddWithValue("@ts",     $D.Timestamp.ToString("yyyy-MM-dd HH:mm:ss"))           | Out-Null
    $devCmd.Parameters.AddWithValue("@user",   $D.Username ?? "")                                       | Out-Null
    $devCmd.Parameters.AddWithValue("@result", $D.Result)                                               | Out-Null
    try { $devCmd.ExecuteNonQuery() | Out-Null } catch { Write-Log "Upsert-Device: $($_.Exception.Message)" "WARN" }
}

# ── ANA AKIS ─────────────────────────────────────────────────────────────────
try { [System.Diagnostics.Process]::GetCurrentProcess().PriorityClass = "High" } catch {}

Write-Log "Kolektoer basladi (v4 SQLite)."
$state     = Get-LastState
$sinceTs   = $state.Timestamp
$sinceFile = $state.File
Write-Host "[Kolektoer] Baslangic: $sinceFile | $sinceTs" -ForegroundColor Cyan

$files = Get-LogFilesAfter -AfterFile $sinceFile
if (-not $files) { Write-Host "[Kolektoer] Islenecek dosya yok." -ForegroundColor Yellow; Write-Log "Dosya yok."; exit 0 }
Write-Host "[Kolektoer] $($files.Count) dosya." -ForegroundColor Cyan

$dbConn  = New-SQLiteConnection -DataSource $DBPath
# WAL modu: okuma ve yazma esz zamanli calisabilir, kilit sorunu olmaz
$pragmaCmd = $dbConn.CreateCommand()
$pragmaCmd.CommandText = "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;"
$pragmaCmd.ExecuteNonQuery() | Out-Null
$authCmd = $dbConn.CreateCommand(); $authCmd.CommandText = $insertSQL
$devCmd  = $dbConn.CreateCommand(); $devCmd.CommandText  = $upsertDevSQL

$pruneCmd = $dbConn.CreateCommand()
$pruneCmd.CommandText = "DELETE FROM AuthLog WHERE TimeCreated < date('now','-180 days')"
$pruned = $pruneCmd.ExecuteNonQuery()
if ($pruned -gt 0) { Write-Log "180 gunden eski $pruned kayit silindi." "INFO" }

$totalInserted = 0
$lastTs        = $sinceTs
$lastFile      = $sinceFile
$sw            = [System.Diagnostics.Stopwatch]::StartNew()

foreach ($file in $files) {
    $mb = [math]::Round($file.Length/1MB,1)
    Write-Host "[Kolektoer] $($file.Name) ($mb MB)..." -ForegroundColor DarkCyan
    Write-Log "Isleniyor: $($file.Name) ($mb MB)"

    # Cache: ClientIP -> Queue<hashtable> (FIFO, birden fazla eszamanli auth icin dogru)
    $pt1Cache   = [System.Collections.Generic.Dictionary[string, System.Collections.Generic.Queue[hashtable]]]::new()
    $inserted   = 0
    $batchCount = 0
    $inTrans    = $false
    $lineNo     = 0

    try {
        $fs     = [System.IO.FileStream]::new($file.FullName,[System.IO.FileMode]::Open,[System.IO.FileAccess]::Read,[System.IO.FileShare]::ReadWrite)
        $reader = [System.IO.StreamReader]::new($fs)
        $trans  = $dbConn.BeginTransaction(); $inTrans = $true

        while (-not $reader.EndOfStream) {
            $line = $reader.ReadLine(); $lineNo++

            if ($lineNo % 50000 -eq 0) {
                $rps = if ($sw.Elapsed.TotalSeconds -gt 0) { [math]::Round($inserted/$sw.Elapsed.TotalSeconds) } else { 0 }
                Write-Host "  $lineNo satir | $inserted kayit | $rps/sn" -ForegroundColor DarkGray
            }

            if ($line -notmatch "^<Event>") { continue }

            # Tum alanlari tek seferde parse et
            $f  = Get-AllFields $line
            $pt = $f['Packet-Type']
            if ($pt -notin $WantedPacketTypes) { continue }

            # Timestamp parse
            $tsRaw = $f['Timestamp']; $ts = $null
            if ($tsRaw) {
                $ic = [System.Globalization.CultureInfo]::InvariantCulture
                foreach ($fmt in @("MM/dd/yyyy HH:mm:ss.fff","MM/dd/yyyy HH:mm:ss","MM/dd/yyyy HH:mm:ss.ff","MM/dd/yyyy HH:mm:ss.f")) {
                    try { $ts = [DateTime]::ParseExact($tsRaw,$fmt,$ic); break } catch {}
                }
            }
            if ($null -eq $ts) { continue }
            if ($ts -lt $sinceTs -and $file.Name -eq $sinceFile) { continue }

            $clientIP = $f['Client-IP-Address']
            $nasIP    = $f['NAS-IP-Address']
            $cacheKey = if ($clientIP) { $clientIP } elseif ($nasIP) { $nasIP } else { $null }
            if (-not $cacheKey) { continue }
            $un    = ($f['User-Name'] ?? '').Trim()
            $uname = $un; $realm = ""
            if ($un -match "^(.+)@(.+)$") { $uname = $Matches[1]; $realm = $Matches[2] }

            switch ($pt) {
                "1" {
                    if ([string]::IsNullOrWhiteSpace($un)) { continue }
                    $rec = @{
                        Timestamp        = $ts
                        FullUsername     = $un
                        Username         = $uname
                        Realm            = $realm
                        NASIPAddress     = if ($nasIP) { $nasIP } else { $clientIP }
                        NASIdentifier    = $f['NAS-Identifier']    ?? ""
                        CalledStationID  = $f['Called-Station-Id'] ?? ""
                        CallingStationID = $f['Calling-Station-Id']?? ""
                        ClientIP         = $clientIP
                        AuthType         = $f['Provider-Name']     ?? ""
                        PolicyName       = $f['Proxy-Policy-Name'] ?? ""
                        ReasonCode       = "0"
                        Result           = "SUCCESS"
                    }
                    # FIFO queue: ayni AP'den eszamanli auth'lari dogru siraya koyar
                    if (-not $pt1Cache.ContainsKey($cacheKey)) {
                        $pt1Cache[$cacheKey] = [System.Collections.Generic.Queue[hashtable]]::new()
                    }
                    # Bir AP'den en fazla 50 bekleyen istek — asiri bellegi onler
                    if ($pt1Cache[$cacheKey].Count -ge 50) { $pt1Cache[$cacheKey].Dequeue() | Out-Null }
                    $pt1Cache[$cacheKey].Enqueue($rec)
                }

                "2" {
                    if (-not $pt1Cache.ContainsKey($cacheKey) -or $pt1Cache[$cacheKey].Count -eq 0) { continue }
                    $data = $pt1Cache[$cacheKey].Dequeue()
                    $data.Timestamp = $ts; $data.Result = "SUCCESS"
                    if (Write-AuthRecord $authCmd $trans "2" $data) {
                        Upsert-Device $devCmd $trans $data
                        $inserted++; $totalInserted++; $batchCount++
                        if ($ts -gt $lastTs) { $lastTs = $ts }
                    }
                }

                "3" {
                    $rc = $f['Reason-Code'] ?? "0"
                    $data = if (-not [string]::IsNullOrWhiteSpace($un)) {
                        @{
                            Timestamp        = $ts
                            FullUsername     = $un; Username = $uname; Realm = $realm
                            NASIPAddress     = if ($nasIP) { $nasIP } else { $clientIP }
                            NASIdentifier    = $f['NAS-Identifier']    ?? ""
                            CalledStationID  = $f['Called-Station-Id'] ?? ""
                            CallingStationID = $f['Calling-Station-Id']?? ""
                            ClientIP         = $clientIP
                            AuthType         = $f['Provider-Name']     ?? ""
                            PolicyName       = $f['Proxy-Policy-Name'] ?? ""
                            ReasonCode       = $rc; Result = "FAILURE"
                        }
                    } elseif ($pt1Cache.ContainsKey($cacheKey) -and $pt1Cache[$cacheKey].Count -gt 0) {
                        $c = $pt1Cache[$cacheKey].Dequeue()
                        $c.Timestamp = $ts; $c.ReasonCode = $rc; $c.Result = "FAILURE"; $c
                    } else { $null }

                    if ($null -eq $data) { continue }
                    if (Write-AuthRecord $authCmd $trans "3" $data) {
                        Upsert-Device $devCmd $trans $data
                        $inserted++; $totalInserted++; $batchCount++
                        if ($ts -gt $lastTs) { $lastTs = $ts }
                    }
                }
            }

            if ($batchCount -ge $BatchSize) {
                $trans.Commit(); $trans = $dbConn.BeginTransaction(); $batchCount = 0
            }
        }

        if ($inTrans) { $trans.Commit(); $inTrans = $false }
        $reader.Close(); $fs.Close()

    } catch {
        Write-Log "Dosya hatasi $($file.Name): $_" "ERROR"
        Write-Host "[HATA] $($file.Name): $_" -ForegroundColor Red
        if ($inTrans) { try { $trans.Rollback() } catch {} }
        if ($reader)  { try { $reader.Close() } catch {} }
        if ($fs)      { try { $fs.Close() } catch {} }
        continue
    }

    $lastFile = $file.Name
    $elapsed  = [math]::Round($sw.Elapsed.TotalSeconds,1)
    $rps      = if ($elapsed -gt 0) { [math]::Round($inserted/$elapsed) } else { 0 }
    Write-Log "$($file.Name): $inserted kayit."
    Write-Host "  Tamam: $inserted kayit | $elapsed sn | $rps/sn" -ForegroundColor Green
    $sw.Restart()
}

$dbConn.Close()
Set-LastState -File $lastFile -Timestamp $lastTs
Write-Log "Bitti. Toplam: $totalInserted kayit."
Write-Host "[Kolektoer] TAMAMLANDI. Toplam $totalInserted kayit." -ForegroundColor Green

# HTTP sunucu SQLite'dan canlı okuyor — JSON export artık gerekmiyor

