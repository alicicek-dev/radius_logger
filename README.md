# Eduroam NPS Loglama Sistemi (v5.2 — Rust Edition)

Windows NPS (Network Policy Server) üzerindeki eduroam RADIUS kimlik doğrulama loglarını (`IN{YYMMDD}.log`) ayrıştırarak SQLite (WAL + `mmap`) veritabanına aktaran ve `127.0.0.1:8080` üzerinde çalışan yüksek performanslı HTTP API + Web Dashboard ile gerçek zamanlı izlenebilir hale getiren sistem.

**Kastamonu Üniversitesi — Bilgi İşlem Daire Başkanlığı**

## Dokümantasyon Haritası

| Doküman | İçerik |
|---|---|
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | Sistem mimarisi, `PT=1/2/3` TTL + MAC/Class eşleştirme mantığı, byte-offset okuma, veritabanı şeması ve API referansı |
| [`SECURITY.md`](./SECURITY.md) | Güvenlik kontrolleri (parametreli SQL, CORS, XSS ve CSV formül enjeksiyonu koruması), KVKK/veri saklama politikası |
| [`CHANGELOG.md`](./CHANGELOG.md) | v1.0'dan v5.2 (Rust Edition) sürümüne kadar tüm değişiklik geçmişi |
| [`AGENTS.md`](./AGENTS.md) | Geliştiriciler ve AI kodlama ajanları için proje kuralları, derleme/test komutları ve mimari kısıtlar |
| [`DESIGN.md`](./DESIGN.md) | Dashboard arayüzünde kullanılan **Ember Studio** tasarım sistemi spesifikasyonu |

## Mimari

```text
NPS IAS logları (IN{YYMMDD}.log)
        | FILE_SHARE_READ | FILE_SHARE_WRITE + ByteOffset Seek
        v
eduroam-nps.exe (Rust - Tokio + Axum + Bundled SQLite)
  ├─ [Collector Thread]
  │    ├─ Zero-copy IAS XML ayrıştırıcı
  │    ├─ PT=1 -> Client-IP + Class/MAC öncelikli + 60sn TTL kuyruk
  │    ├─ PT=2 -> SUCCESS / PT=3 -> FAILURE
  │    └─ 2000'lik Batch Transaction + EventHash (SHA-256)
  │
  ├─ [SQLite Veritabanı: eduroam_logs.db]
  │    └─ WAL Modu + 512 MB mmap + 64 MB Page Cache
  │
  └─ [Axum Async HTTP Sunucusu: 127.0.0.1:8080]
       ├─ GET /               (eduroam_dashboard.html - disk veya gömülü)
       ├─ GET /style.css      (eduroam_dashboard.css - disk veya gömülü)
       ├─ GET /api/charts     (Atomik nesil takipli RAM önbelleği <50 µs)
       ├─ GET /api/recent     (Parametreli, indeksli sayfalama)
       └─ GET /api/export.csv (Formül enjeksiyonu korumalı CSV export)
```

## Dosya Yapısı

| Dosya | Açıklama |
|---|---|
| `Cargo.toml` | Rust paket ve bağımlılık yapılandırması (`axum`, `tokio`, `rusqlite` bundled, `clap`, `sha2`, `chrono`) |
| `src/main.rs` | CLI giriş noktası (`serve`, `collect`, `server`, `setup`, `backfill`, `reset-backfill`, `register-tasks`) ve testler |
| `src/config.rs` | `eduroam_config.json` yükleyici ve CORS doğrulayıcı |
| `src/parser.rs` | Sıfır-kopya (zero-copy) IAS XML ayrıştırıcı, `EventHash` (SHA-256), `LIKE` ve CSV formül kaçış yardımcıları |
| `src/db.rs` | SQLite WAL + `mmap` bağlantı yöneticisi ve otomatik şema migration (`ensure_schema_and_migrate`) |
| `src/collector.rs` | Byte-offset takipli, TTL + MAC/Class eşleştirmeli yüksek hızlı log kolektörü |
| `src/server.rs` | Bellek içi grafik önbellekli Axum HTTP API sunucusu ve gömülü (embedded) arayüz sunumu |
| `eduroam_dashboard.html` / `.css` | **Ember Studio** (`DESIGN.md`) tasarım sistemine sahip 4 sekmeli web arayüzü |
| `eduroam_config.example.json` | İsteğe bağlı yapılandırma dosyası örneği (`eduroam_config.json`) |

## Kurulum ve Kullanım

### 1. Derleme
```powershell
cargo build --release
# Çıktı: target\release\eduroam-nps.exe
```

### 2. Komutlar
`eduroam-nps.exe` harici hiçbir DLL veya SQLite kurulumu gerektirmez:

```powershell
# 1. Veritabanını oluştur veya mevcut DB'yi migrate et
.\target\release\eduroam-nps.exe setup --migrate

# 2. Mevcut DB'yi silmeden son 90 günü hızla tara (Append-Only Backfill)
.\target\release\eduroam-nps.exe backfill --days 90

# 3. Hem arka plan log toplayıcıyı hem de HTTP Dashboard sunucusunu başlat (Varsayılan mod)
.\target\release\eduroam-nps.exe serve

# 4. Windows başlangıcında otomatik çalışacak görevi kaydet (Yönetici olarak)
.\target\release\eduroam-nps.exe register-tasks

# İsteğe bağlı: Windows Task Scheduler varsayılan 72 saatlik limitini sınırsız (PT0S) yapma
$t = Get-ScheduledTask -TaskName "EduroamDashboardServer"
$t.Settings.ExecutionTimeLimit = "PT0S"
Set-ScheduledTask -InputObject $t

# Yalnızca tek seferlik log toplama çalıştırıp çıkmak için:
.\target\release\eduroam-nps.exe collect
```

### 3. Yapılandırma ve Saklama Süresi
* **Varsayılan Saklama Süresi:** `AuthLog` kayıtları varsayılan olarak **365 gün** saklanır (`collector.retentionDays`); Web Dashboard dönem seçicisi `7 / 14 / 30 / 90 / 180 / 365` gün aralıklarını destekler.
* **Özelleştirme:** Tüm yol, port, CORS ve saklama ayarlarını kod değiştirmeden ezmek için [`eduroam_config.example.json`](./eduroam_config.example.json) şablonunu `C:\EduroamLogs\eduroam_config.json` (veya `.exe` yanına) kopyalayıp düzenleyebilirsiniz.

## Test ve Doğrulama

```powershell
cargo test
```

## Lisans

MIT License. Ayrıntılar için [`LICENSE`](./LICENSE) dosyasına bakın.
