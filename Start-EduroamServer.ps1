#Requires -Version 7.0
# Eduroam NPS Dashboard — HTTP API Sunucusu
# Dinleme: http://127.0.0.1:8080  (yalnizca localhost)

$DBPath  = "C:\EduroamLogs\eduroam_logs.db"
$HTML    = "C:\EduroamLogs\eduroam_dashboard.html"
$Port    = 8080
$DaysBack = 7

if (-not (Get-Module -ListAvailable -Name PSSQLite)) {
    Install-Module PSSQLite -Force -Scope AllUsers -Repository PSGallery
}
Import-Module PSSQLite -ErrorAction Stop

# ── Yardimci fonksiyonlar ────────────────────────────────────────────────────
function Q([string]$sql, [hashtable]$p = @{}) {
    try {
        if ($p.Count -gt 0) {
            return @(Invoke-SqliteQuery -DataSource $DBPath -Query $sql -SqlParameters $p)
        }
        return @(Invoke-SqliteQuery -DataSource $DBPath -Query $sql)
    } catch {
        Write-Warning "SQL hatasi: $($_.Exception.Message)"
        return @()
    }
}

function Parse-QS([string]$qs) {
    $r = @{}
    if ($qs.StartsWith("?")) { $qs = $qs.Substring(1) }
    $qs.Split("&") | Where-Object { $_ } | ForEach-Object {
        $kv = $_.Split("=", 2)
        if ($kv.Length -eq 2) {
            $r[[Uri]::UnescapeDataString($kv[0])] = [Uri]::UnescapeDataString($kv[1])
        }
    }
    return $r
}

function Send-Json($resp, $obj, [int]$status = 200) {
    $json  = if ($obj -is [string]) { $obj } else { $obj | ConvertTo-Json -Depth 6 -Compress }
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $resp.StatusCode        = $status
    $resp.ContentType       = "application/json; charset=utf-8"
    $resp.ContentLength64   = $bytes.Length
    $resp.Headers.Add("Access-Control-Allow-Origin", "*")
    $resp.OutputStream.Write($bytes, 0, $bytes.Length)
    $resp.OutputStream.Close()
}

function Send-Html($resp, [string]$path) {
    if (-not (Test-Path $path)) { Send-Json $resp @{error="HTML bulunamadi"} 404; return }
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $resp.StatusCode      = 200
    $resp.ContentType     = "text/html; charset=utf-8"
    $resp.ContentLength64 = $bytes.Length
    $resp.OutputStream.Write($bytes, 0, $bytes.Length)
    $resp.OutputStream.Close()
}

function Send-File($resp, [string]$path, [string]$contentType) {
    if (-not (Test-Path $path)) { Send-Json $resp @{error="Dosya bulunamadi: $path"} 404; return }
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $resp.StatusCode      = 200
    $resp.ContentType     = $contentType
    $resp.ContentLength64 = $bytes.Length
    $resp.Headers.Add("Cache-Control", "max-age=300")
    $resp.OutputStream.Write($bytes, 0, $bytes.Length)
    $resp.OutputStream.Close()
}

# ── /api/charts — tüm grafik verileri ───────────────────────────────────────
function Handle-Charts {
    $since = (Get-Date).AddDays(-$DaysBack).ToString("yyyy-MM-dd")
    $today = (Get-Date).ToString("yyyy-MM-dd")
    $clean = "Result IN ('SUCCESS','FAILURE')"

    $summary  = Q "SELECT Result, COUNT(*) AS Cnt FROM AuthLog WHERE TimeCreated >= '$since' AND $clean GROUP BY Result"
    $trend    = Q "SELECT DATE(TimeCreated) AS Gun, Result, COUNT(*) AS Cnt FROM AuthLog WHERE TimeCreated >= '$since' AND $clean GROUP BY DATE(TimeCreated), Result ORDER BY DATE(TimeCreated)"
    $topFail  = Q "SELECT Username, Realm, COUNT(*) AS Cnt FROM AuthLog WHERE Result='FAILURE' AND TimeCreated >= '$today' AND Username != '' GROUP BY Username, Realm ORDER BY COUNT(*) DESC LIMIT 15"
    $topUsers = Q "SELECT Username, Realm, COUNT(*) AS Cnt FROM AuthLog WHERE Result='SUCCESS' AND TimeCreated >= '$since' AND Username != '' GROUP BY Username, Realm ORDER BY COUNT(*) DESC LIMIT 20"
    $byNAS    = Q "SELECT NASIdentifier, COUNT(*) AS Total, SUM(CASE WHEN Result='SUCCESS' THEN 1 ELSE 0 END) AS Suc, SUM(CASE WHEN Result='FAILURE' THEN 1 ELSE 0 END) AS Fail FROM AuthLog WHERE TimeCreated >= '$since' AND NASIdentifier != '' AND $clean GROUP BY NASIdentifier ORDER BY COUNT(*) DESC LIMIT 15"
    $byHour   = Q "SELECT CAST(strftime('%H', TimeCreated) AS INTEGER) AS Saat, COUNT(*) AS Cnt FROM AuthLog WHERE TimeCreated >= '$since' AND $clean GROUP BY Saat ORDER BY Saat"
    $byRealm  = Q "SELECT Realm, COUNT(*) AS Total, SUM(CASE WHEN Result='SUCCESS' THEN 1 ELSE 0 END) AS SuccCnt, SUM(CASE WHEN Result='FAILURE' THEN 1 ELSE 0 END) AS FailCnt FROM AuthLog WHERE TimeCreated >= '$since' AND Realm != '' AND $clean GROUP BY Realm ORDER BY COUNT(*) DESC LIMIT 20"
    $byReason = Q "SELECT ReasonCode, COUNT(*) AS Cnt FROM AuthLog WHERE Result='FAILURE' AND TimeCreated >= '$since' GROUP BY ReasonCode ORDER BY COUNT(*) DESC LIMIT 20"
    $uUsers   = (Q "SELECT COUNT(DISTINCT Username) AS N FROM AuthLog WHERE TimeCreated >= '$since' AND Result='SUCCESS'" | Select-Object -First 1).N ?? 0
    $uDevices = (Q "SELECT COUNT(*) AS N FROM Devices" | Select-Object -First 1).N ?? 0

    return @{
        generated     = (Get-Date -Format "yyyy-MM-dd HH:mm:ss")
        daysBack      = $DaysBack
        uniqueUsers   = $uUsers
        uniqueDevices = $uDevices
        summary       = $summary
        trend         = $trend
        topFail       = $topFail
        topUsers      = $topUsers
        byNAS         = $byNAS
        byHour        = $byHour
        byRealm       = $byRealm
        byReason      = $byReason
    }
}

# ── /api/recent — filtreli + sayfalı log kayıtları ───────────────────────────
function Handle-Recent([hashtable]$qs) {
    $page   = [int]($qs["page"]   ?? 1);  if ($page  -lt 1) { $page  = 1 }
    $limit  = [int]($qs["limit"]  ?? 200); if ($limit -lt 1 -or $limit -gt 1000) { $limit = 200 }
    $offset = ($page - 1) * $limit

    $where  = "Result IN ('SUCCESS','FAILURE')"
    $params = @{}

    $defaultSince = (Get-Date).AddDays(-$DaysBack).ToString("yyyy-MM-dd")

    if ($qs["from"] -and $qs["from"] -match '^\d{4}-\d{2}-\d{2}$') {
        $where += " AND TimeCreated >= @from"
        $params["@from"] = $qs["from"]
    } else {
        $where += " AND TimeCreated >= '$defaultSince'"
    }
    if ($qs["to"] -and $qs["to"] -match '^\d{4}-\d{2}-\d{2}$') {
        $where += " AND TimeCreated <= @to"
        $params["@to"] = $qs["to"] + " 23:59:59"
    }
    if ($qs["result"] -and $qs["result"] -in @("SUCCESS","FAILURE")) {
        $where += " AND Result = @result"
        $params["@result"] = $qs["result"]
    }
    if ($qs["username"] -and $qs["username"].Length -gt 0) {
        $where += " AND Username LIKE @uname"
        $params["@uname"] = "%$($qs['username'])%"
    }
    if ($qs["realm"] -and $qs["realm"].Length -gt 0) {
        $where += " AND Realm LIKE @realm"
        $params["@realm"] = "%$($qs['realm'])%"
    }
    if ($qs["nas"] -and $qs["nas"].Length -gt 0) {
        $where += " AND NASIdentifier LIKE @nas"
        $params["@nas"] = "%$($qs['nas'])%"
    }
    if ($qs["mac"] -and $qs["mac"].Length -gt 0) {
        $where += " AND CallingStationID LIKE @mac"
        $params["@mac"] = "%$($qs['mac'])%"
    }
    if ($qs["ip"] -and $qs["ip"].Length -gt 0) {
        $where += " AND ClientIP LIKE @ip"
        $params["@ip"] = "%$($qs['ip'])%"
    }
    if ($qs["rc"] -and $qs["rc"] -match '^\d+$') {
        $where += " AND ReasonCode = $([int]$qs['rc'])"
    }

    $countParams = $params.Clone()
    $total = (Q "SELECT COUNT(*) AS N FROM AuthLog WHERE $where" $countParams | Select-Object -First 1).N ?? 0
    $pages = [math]::Ceiling($total / $limit)

    # Sıralama — sadece izin verilen sütunlar (injection koruması)
    $allowed = @{Zaman="TimeCreated";Username="Username";Realm="Realm";
                 ClientIP="ClientIP";NASIdentifier="NASIdentifier";
                 CallingStationID="CallingStationID";Result="Result";ReasonCode="ReasonCode"}
    $sortCol = if($qs["sort"] -and $allowed[$qs["sort"]]) { $allowed[$qs["sort"]] } else { "TimeCreated" }
    $sortDir = if($qs["dir"] -eq "asc") { "ASC" } else { "DESC" }

    $data = Q "SELECT LogID, TimeCreated AS Zaman, Username, Realm, ClientIP, NASIdentifier, CallingStationID, Result, ReasonCode, Reason FROM AuthLog WHERE $where ORDER BY $sortCol $sortDir LIMIT $limit OFFSET $offset" $params

    $dataJson = if ($data -and @($data | Where-Object { $_ -ne $null }).Count -gt 0) {
        ConvertTo-Json @($data | Where-Object { $_ -ne $null }) -Depth 4 -Compress
    } else { "[]" }
    return "{`"page`":$page,`"limit`":$limit,`"total`":$total,`"pages`":$([math]::Round($pages,0)),`"data`":$dataJson}"
}

# ── HTTP sunucusu ─────────────────────────────────────────────────────────────
$url = "http://127.0.0.1:$Port/"
$listener = [System.Net.HttpListener]::new()
$listener.Prefixes.Add($url)
$listener.Start()
Write-Host "Eduroam Dashboard Sunucusu baslatildi: $url" -ForegroundColor Green
Write-Host "Durdurmak icin Ctrl+C" -ForegroundColor DarkGray

try {
    while ($listener.IsListening) {
        $ctx  = $listener.GetContext()
        $req  = $ctx.Request
        $resp = $ctx.Response
        $path = $req.Url.LocalPath
        $qs   = Parse-QS $req.Url.Query

        Write-Host "$(Get-Date -f 'HH:mm:ss')  $($req.HttpMethod) $path" -ForegroundColor DarkGray

        try {
            switch -Regex ($path) {
                "^/$"              { Send-Html  $resp $HTML }
                "^/style\.css$"   { Send-File  $resp "C:\EduroamLogs\eduroam_dashboard.css" "text/css; charset=utf-8" }
                "^/api/charts$"   { Send-Json  $resp (Handle-Charts) }
                "^/api/recent$"   { Send-Json  $resp (Handle-Recent $qs) }
                default           { Send-Json  $resp @{error="Endpoint bulunamadi: $path"} 404 }
            }
        } catch {
            try { Send-Json $resp @{error=$_.Exception.Message} 500 } catch {}
        }
    }
} finally {
    $listener.Stop()
    Write-Host "Sunucu durduruldu." -ForegroundColor Yellow
}





