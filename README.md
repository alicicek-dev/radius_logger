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

### 2. Kurulum Adımları
`eduroam-nps.exe` harici hiçbir DLL veya SQLite kurulumu gerektirmez:

1. **NPS Log Konumunu Kontrol Edin:** Windows Server NPS varsayılan olarak günlük `IN{YYMMDD}.log` dosyalarını `C:\Windows\System32\LogFiles` klasörüne yazar. Logları farklı bir klasöre yönlendirdiyseniz (ör. `C:\NPSLogs`), tek komutla yapılandırma oluşturun:
   ```powershell
   .\eduroam-nps.exe setup --log-dir "C:\NPSLogs"
   ```
   *(Varsayılan olarak `C:\EduroamLogs\eduroam_config.json` dosyasını üretir; hedef dosya varsa `--force` olmadan üzerine yazmaz.)*
2. **Veritabanını Hazırlayın:**
   ```powershell
   .\eduroam-nps.exe setup --migrate
   ```
3. **Çalıştırın / Zamanlanmış Görevi Kaydedin:**
   ```powershell
   # İsteğe bağlı: Mevcut DB'yi silmeden son 90 günü tara (Append-Only Backfill)
   .\eduroam-nps.exe backfill --days 90

   # Arka plan log toplayıcı + HTTP Dashboard sunucusu (argümansız varsayılan mod)
   .\eduroam-nps.exe

   # Windows başlangıcında ExecutionTimeLimit=PT0S (sınırsız) ile otomatik çalışacak görevi kaydet (Yönetici olarak)
   .\eduroam-nps.exe register-tasks

   # Yalnızca tek seferlik log toplama çalıştırıp çıkmak için:
   .\eduroam-nps.exe collect
   ```

### 3. Yapılandırma, Arama Sırası ve Kısmi JSON
* **Arama Sırası:** Uygulama açılışta şu sırayla yapılandırma arar (ilk bulduğunu kullanır, hiçbiri yoksa kod içi varsayılanlarla çalışır):
  1. `--config <dosya>` parametresi (verilmişse)
  2. `C:\EduroamLogs\eduroam_config.json`
  3. `<exe_klasörü>\eduroam_config.json`
  4. `.\eduroam_config.json`
* **Kısmi JSON Desteği:** Dosyaya yalnızca değiştirmek istediğiniz alanları yazmanız yeterlidir; eksik bırakılan tüm alanlar kod içi varsayılan değerlerde kalır.
* **Alan Açıklamaları ([`eduroam_config.example.json`](./eduroam_config.example.json)):**
  * `paths.logDir`: NPS `IN{YYMMDD}.log` dosyalarının bulunduğu klasör (varsayılan: `C:\Windows\System32\LogFiles`).
  * `paths.dataDir` / `dbPath` / `stateFile` / `errorLog`: SQLite DB, `last_run.txt` ve `collector_errors.log` konumları (`C:\EduroamLogs\`).
  * `server.*`: Dinleme adresi/portu (`127.0.0.1:8080`), varsayılan gün sayısı, CORS beyaz listesi (`enableCors`, `allowedOrigins`), sayfalama ve CSV dışa aktarım limitleri.
  * `collector.retentionDays`: `AuthLog` kayıtlarının saklanma süresi (varsayılan: **365 gün**; Web Dashboard dönem seçicisi `7 / 14 / 30 / 90 / 180 / 365` gün aralıklarını destekler).
  * `collector.batchSize` / `pollIntervalSeconds` / `pendingTtlSeconds` / `maxLogMB`: Toplama periyodu, bellek kuyruğu TTL ve hata günlüğü rotasyon boyutu.

### 4. Güvenlik ve Gizlilik Notları
* **Güvenlik Notu:** Dashboard yalnızca `127.0.0.1:8080` adresine bağlıdır, kimlik doğrulaması yoktur ve tüm uç noktaları salt-okunur (`GET`) yapıdadır. `server.host` değerini `0.0.0.0`'a veya dış ağ arayüzüne açmak yetkisiz erişim riski doğurur; ağ üzerinden erişim gerekiyorsa mutlaka önüne TLS + kimlik doğrulamalı bir reverse proxy konumlandırılmalıdır.
* **Gizlilik Notu:** RADIUS logları kullanıcı adı, IP adresi ve MAC adresi gibi kişisel veriler içerir. KVKK/GDPR kapsamında saklama süresi (`collector.retentionDays`), veritabanı erişim izinleri ve disk güvenliği sorumluluğu sistem yöneticisine aittir.

### 5. Yükseltme Notu
* Bu sürümde kod içi varsayılan `paths.logDir` değeri jenerik NPS yolu olan `C:\Windows\System32\LogFiles` olarak güncellenmiştir.
* Önceki sürümlerden yükseltme yaparken özel bir log klasörü kullanıp henüz `eduroam_config.json` oluşturmamış sistemlerde, yeni `.exe` devreye alınmadan **önce** yapılandırma dosyası üretilmelidir:
  ```powershell
  .\eduroam-nps.exe setup --log-dir "<mevcut_log_klasorunuz>"
  ```

## Test ve Doğrulama

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Lisans

MIT License. Ayrıntılar için [`LICENSE`](./LICENSE) dosyasına bakın.
