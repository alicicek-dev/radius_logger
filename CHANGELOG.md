# Sürüm Geçmişi (`CHANGELOG.md`)

Bu projedeki tüm önemli değişiklikler bu dosyada belgelenmektedir.

---

## [v5.2] - 2026-11 (Rust Edition & Mimari İyileştirmeler)

### Eklendi (Rust Yeniden Yazımı)
- **Tümleşik Rust Motoru (`eduroam-nps.exe`):** `Cargo.toml` ve `src/` altında tüm kolektör, veritabanı yönetimi, backfill ve HTTP API sunucusu Rust (`axum`, `tokio`, `rusqlite` bundled) ile yeniden yazıldı.
- **Sıfır-Kopya (Zero-Copy) IAS XML Ayrıştırıcı (`src/parser.rs`):** Regex maliyetini ortadan kaldıran tek geçişli byte tarayıcı ile saniyede 200.000+ satır ayrıştırma hızı sağlandı.
- **Byte-Offset Artımlı Okuma (`src/collector.rs`):** `last_run.txt` içine `ByteOffset` ve `LineNo` eklendi; yüzlerce megabaytlık günlük log dosyalarının her çalışmada baştan taranması önlendi (`SeekFrom::Start`).
- **Dosyalar Arası Kesintisiz Gece Yarısı Önbelleği ve 60sn TTL:** `PT=1` kuyruğuna zaman aşımı (TTL) ve `Class` / `Calling-Station-Id` (MAC) öncelikli eşleştirme eklendi; `23:59:59` -> `00:00:00` dosya geçişlerinde kuyruk korunarak veri kayması (cross-contamination) ve paket kaybı engellendi.
- **SQLite `mmap` + Bellek İçi `/api/charts` Önbelleği (`src/db.rs`, `src/server.rs`):** 512 MB Memory-Mapped I/O, 64 MB Page Cache ve atomik nesil (`db_generation`) takipli RAM önbelleği ile `/api/charts` yanıt süresi ~200 ms'den `<50 µs` seviyesine indirildi.
- **Gömülü Arayüz (`include_str!`):** `eduroam_dashboard.html` ve `eduroam_dashboard.css` doğrudan `.exe` içine gömüldü; diskte dosya bulunmasa bile sunucu çalışmaya devam eder.

### Düzeltildi (Mevcut Hatalar)
- **Kronolojik `Devices` Upsert Koruması:** Geçmişe dönük backfill işlemlerinde `Devices.LastSeen` ve `FirstSeen` alanlarının eski tarihlerle ezilmesi `CASE WHEN excluded.LastSeen >= Devices.LastSeen` koşuluyla düzeltildi.
- **CSV Formül Enjeksiyonu Koruması:** `=`, `+`, `-`, `@`, `\t`, `\r` ile başlayan hücrelerin başına `'` eklenerek Excel/LibreOffice formül enjeksiyonu kapatıldı.
- **Dashboard (`eduroam_dashboard.html`) Düzeltmeleri:**
  - `renderHourly` içinde `rgba(hsl(...), alpha)` nedeniyle oluşan geçersiz Canvas renk hatası `ctx.globalAlpha` ile düzeltildi.
  - Log tablosundaki `NASIdentifier`, `CallingStationID`, `Result` ve `ReasonCode` sütun başlıklarına eksik olan `id="th-..."` öznitelikleri eklendi.
  - `renderInsights` içindeki `Username`, `Realm` ve `NASIdentifier` çıktıları `esc()` fonksiyonundan geçirilerek DOM XSS riski kapatıldı.
  - Kullanılmayan eski `renderDeviceSummary` ve `zamanToISO` ölü fonksiyonları temizlendi.

### Değiştirildi
- **Varsayılan Saklama Süresi (365 Gün) ve UI Dönem Seçenekleri (`src/config.rs`, `eduroam_dashboard.html`):** Varsayılan `collector.retentionDays` değeri `180` günden `365` güne çıkarıldı; Web Dashboard üst dönem seçicisine `180` ve `365` gün seçenekleri eklendi.
- **Jenerik Varsayılan Log Yolu ve `setup --log-dir` (`src/config.rs`, `src/main.rs`):** Varsayılan `paths.logDir` standart `C:\Windows\System32\LogFiles` yoluna çekildi; `setup --log-dir` ile tek komutla kısmi `eduroam_config.json` üretimi ve eksik/boş log klasörü için açıklayıcı başlangıç doğrulaması eklendi.
- **Task Scheduler `PT0S` XML ve Kalite İyileştirmeleri (`src/main.rs`, `src/server.rs`, `src/collector.rs`, `.github/workflows/ci.yml`):** `register-tasks` komutu görev XML'i ile doğrudan `ExecutionTimeLimit=PT0S` ayarlayacak şekilde güncellendi; Axum HTTP ve log rotasyon testleri ile CI `fmt`/`clippy` kontrolleri eklendi.

---

## [v5.1] - 2026-06

### Eklendi
- **Ortak Çekirdek Modülü (`Eduroam-Common.ps1`):** Yapılandırma okuma, XML ayrıştırma, `EventHash` hesaplama, veritabanı migration, parametreli sorgu oluşturma ve CSV dönüştürme fonksiyonları tek modülde toplandı.
- **Yapılandırma Dosyası Desteği (`eduroam_config.json`):** `eduroam_config.example.json` şablonu eklendi.
- **EventHash Tabanlı Tekillik:** Kaynak dosya adı (`SourceFile`), satır numarası (`SourceLine`) ve ham XML satırının SHA-256 özeti (`EventHash`) ile kesin tekillik sağlandı (`idx_event_hash`).
- **Append-Only Backfill (`Backfill-LastDays.ps1`):** Veritabanını silmeden geçmiş $N$ günlük logların güvenle yeniden taranabilmesi sağlandı.
- **Sunucu Taraflı CSV Dışa Aktarım (`/api/export.csv`):** Aktif tablo filtreleriyle uyumlu, güvenli CSV export endpoint'i eklendi.
- **Neural Expressive Dashboard Tasarımı:** `eduroam_dashboard.html` ve `eduroam_dashboard.css` dosyaları Neural Expressive tasarım sistemiyle yenilendi.
- **Test ve CI Altyapısı:** `tests/Eduroam.Tests.ps1` ve `.github/workflows/ci.yml` eklendi.

---

## [v4.0] - 2026-05

### Eklendi
- Microsoft Access'ten **SQLite (WAL modu)** veritabanına geçiş yapıldı (`Setup-SQLite.ps1`).
- `Start-EduroamServer.ps1` ile `127.0.0.1:8080` üzerinde çalışan yerleşik HTTP API sunucusu eklendi.

---

## [v3.0] - 2026-04

### Eklendi
- `PT=1` → `PT=2/3` için `Client-IP` bazlı FIFO kuyruk eşleştirme mekanizması ve batch transaction yapısı eklendi.

---

## [v2.0] - 2026-04

### Eklendi
- NPS Windows Event ID (6272–6280) ve RADIUS Reason Code (0–113) referans kataloğu ve 4 sekmeli dashboard düzeni eklendi.

---

## [v1.0] - 2026-04

### Eklendi
- İlk sürüm: Microsoft Access veritabanı, JSON dışa aktarım betiği ve statik HTML dashboard.
