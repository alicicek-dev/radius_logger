# 📡 Eduroam NPS Loglama Sistemi

> Windows NPS üzerindeki eduroam RADIUS kimlik doğrulama loglarını otomatik toplayan, SQLite veritabanında saklayan ve HTTP API ile gerçek zamanlı izleyen sistem.

**Kastamonu Üniversitesi — Bilgi İşlem Daire Başkanlığı**

---

## 🖥️ Dashboard Önizleme

Material Design 3 tabanlı, koyu/açık tema destekli web dashboard:

- **Genel Bakış** — İstatistikler, trend grafikleri, en aktif AP'ler, realm dağılımı
- **Log Kayıtları** — Filtreli/sayfalı tablo, CSV export, kolon sıralama
- **Analiz** — Başarı oranı, hata nedenleri, anomali tespiti
- **NPS Referans** — Reason Code kataloğu, hızlı sorun giderme rehberi

---

## ⚙️ Mimari

```
NPS Log Dosyaları (.log)
        │  her 10 dk
        ▼
Kolektör (PowerShell 7)
  PT=1 → önbellek (FIFO)
  PT=2 → SUCCESS yaz
  PT=3 → FAILURE yaz
        │
        ▼
  SQLite Veritabanı
  (eduroam_logs.db)
        │
        ▼
HTTP API Sunucusu (:8080)
  /api/charts
  /api/recent
        │
        ▼
  Web Dashboard
  http://127.0.0.1:8080
```

---

## 📁 Dosya Yapısı

| Dosya | Açıklama |
|-------|----------|
| `Eduroam-NPS-LogCollector.ps1` | Ana kolektör — IAS XML parse, SQLite'a yazar |
| `Start-EduroamServer.ps1` | HTTP API sunucusu — dashboard + REST endpoint'ler |
| `Reset-And-Backfill.ps1` | DB sıfırlama ve geçmiş veri doldurma aracı |
| `Register-ScheduledTasks.ps1` | Zamanlanmış görev kayıt scripti |
| `eduroam_dashboard.html` | Dashboard şablonu (JS + API çağrıları) |
| `eduroam_dashboard.css` | Material Design 3 stiller (koyu + açık tema) |
| `generate_report.py` | Teknik dokümantasyon PDF üretici |

---

## 🚀 Kurulum

### Gereksinimler

- Windows Server 2016+
- PowerShell 7+ (`winget install Microsoft.PowerShell`)
- NPS rolü kurulu, IAS text logging aktif
- NPS log klasörü: `C:\Users\radius1\Desktop\nps\`

### Adımlar

```powershell
# 1. Dosyaları kopyala
# Tüm dosyaları C:\EduroamLogs\ klasörüne kopyalayın

# 2. SQLite veritabanını oluştur
pwsh -ExecutionPolicy Bypass -File C:\EduroamLogs\Setup-SQLite.ps1

# 3. Geçmiş veriyi doldur (son 7 gün)
pwsh -ExecutionPolicy Bypass -File C:\EduroamLogs\Reset-And-Backfill.ps1

# 4. Zamanlanmış görevleri kaydet (yönetici olarak)
pwsh -ExecutionPolicy Bypass -File C:\EduroamLogs\Register-ScheduledTasks.ps1

# 5. Dashboard'u aç
Start-Process "http://127.0.0.1:8080"
```

---

## 🔌 HTTP API

| Endpoint | Açıklama |
|----------|----------|
| `GET /` | Dashboard HTML |
| `GET /style.css` | CSS stilleri |
| `GET /api/charts` | Grafik verileri (7 günlük özet) |
| `GET /api/recent` | Filtreli log kayıtları |

### /api/recent Parametreleri

```
?page=1&limit=200&result=FAILURE&username=ali&realm=kastamonu.edu.tr
&from=2026-04-28&to=2026-04-30&nas=ruckus&mac=EC-58&rc=16&sort=Zaman&dir=desc
```

---

## 🗄️ Veritabanı

**AuthLog** tablosu: `TimeCreated`, `Username`, `Realm`, `ClientIP`, `NASIdentifier`, `CallingStationID`, `Result`, `ReasonCode`, `Reason`

**Devices** tablosu: `MACAddress`, `FirstSeen`, `LastSeen`, `LastUsername`, `LastResult`

> WAL modu aktif — kolektör yazarken API eşzamanlı okuma yapabilir.

---

## ⚡ Performans

| Metrik | Değer |
|--------|-------|
| Kolektör hızı | ~260 kayıt/sn |
| Batch boyutu | 500 kayıt/commit |
| Güncelleme aralığı | Her 10 dakika |
| İşlenen toplam kayıt | 880.000+ |
| Veritabanı boyutu | ~280 MB |

---

## 🔒 Güvenlik

- HTTP sunucu yalnızca `127.0.0.1` dinler — dışarı açık değil
- SQL injection koruması: single-quote escape + integer cast
- HTML injection koruması: `</script>` ve `<!--` escape edilir
- Unique index ile duplikat kayıt önlenir

---

## 📊 RADIUS Paket Mantığı

| Packet-Type | Mesaj | Davranış |
|-------------|-------|----------|
| PT=1 | Access-Request | FIFO önbelleğe al (kullanıcı bilgisi burada) |
| PT=2 | Access-Accept | Önbellekten eşleştir → **SUCCESS** yaz |
| PT=3 | Access-Reject | Doğrudan **FAILURE** yaz (RC burada) |
| PT=11 | Access-Challenge | Atla (EAP gürültüsü) |

---

## 📝 Lisans

MIT License — Kastamonu Üniversitesi Bilgi İşlem Daire Başkanlığı
