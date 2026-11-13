# AGENTS.md — AI Ajan ve Geliştirici Rehberi

Bu dosya, **Eduroam NPS Loglama Sistemi (v5.2 — Rust Edition)** reposunda çalışan yapay zeka kodlama ajanları ve geliştiriciler için mimari kuralları, komutları ve güvenlik kısıtlarını tanımlar.

---

## 1. Proje Özeti

Windows Server NPS (Network Policy Server) tarafından üretilen IAS XML formatındaki (`IN{YYMMDD}.log`) RADIUS kimlik doğrulama loglarını ayrıştıran, SQLite (WAL + `mmap` modu) veritabanına kaydeden ve `127.0.0.1:8080` üzerinde çalışan yerleşik HTTP API + Web Dashboard ile görselleştiren yüksek performanslı izleme sistemidir.

- **Çalışma Zamanı:** Rust 2021 (`Cargo.toml`, `axum` + `tokio` + `rusqlite` bundled -> `eduroam-nps.exe`)
- **Arayüz:** Vanilla JS + HTML5 + CSS3 (**Ember Studio Design System** — `DESIGN.md`, `eduroam_dashboard.html` & `eduroam_dashboard.css`)
- **Hedef Ortam:** Windows Server 2016+ (varsayılan çalışma dizini: `C:\EduroamLogs\`, varsayılan log dizini: `C:\Windows\System32\LogFiles\`)

---

## 2. Dosya Haritası ve Sorumluluklar

| Dosya | Rolü |
|---|---|
| `Cargo.toml` | Rust bağımlılıkları ve `release` profili (`lto = "thin"`, `opt-level = 3`, `strip = true`) |
| `src/main.rs` | CLI komut satırı arayüzü (`serve`, `collect`, `server`, `setup`, `backfill`, `reset-backfill`, `register-tasks`) ve testler |
| `src/config.rs` | `eduroam_config.json` okuma, geriye dönük varsayılanlar ve CORS origin doğrulaması (`is_cors_origin_allowed`) |
| `src/parser.rs` | Sıfır-kopya (zero-copy) `<Event>` ayrıştırıcı (`parse_ias_event_line`), `EventHash` (`compute_event_hash`), `LIKE` kaçışı ve CSV formül koruması |
| `src/db.rs` | SQLite WAL + 512 MB `mmap` bağlantı yönetimi (`open_db`) ve şema migration (`ensure_schema_and_migrate`) |
| `src/collector.rs` | `FILE_SHARE_READ \| FILE_SHARE_WRITE` ile log okuma, `ByteOffset` takibi, 60sn TTL + `Class`/MAC öncelikli `PT=1` kuyruğu ve batch transaction yazımı |
| `src/server.rs` | `axum` HTTP sunucusu (`/`, `/style.css`, `/api/charts`, `/api/recent`, `/api/export.csv`) ve atomik nesil takipli `/api/charts` bellek önbelleği |
| `eduroam_dashboard.html` | 4 sekmeli (Genel Bakış, Log Kayıtları, Analiz, NPS Referans) SPA arayüzü |
| `eduroam_dashboard.css` | Ember Studio tasarım sistemi (`DESIGN.md` — Terracotta & sıcak taş/krem renk paleti, açık/koyu tema) |
| `eduroam_config.example.json` | İsteğe bağlı `eduroam_config.json` için örnek yapılandırma şablonu |
| `.github/workflows/ci.yml` | Rust (`cargo test` + `cargo build --release`) CI iş akışı |

---

## 3. Doğrulama ve Test Komutları

Kod değişikliklerinden sonra aşağıdaki doğrulama adımlarını çalıştırın:

```powershell
# 1. Rust birim ve uçtan uca entegrasyon testleri
cargo test

# 2. Optimize edilmiş Windows .exe derlemesi
cargo build --release
```

---

## 4. Mimari ve Güvenlik Kuralları (Değiştirilemez İlkeler)

1. **Duplicate Önleme (`EventHash`):**
   - `AuthLog` tablosunda tekillik `idx_event_hash` (`UNIQUE INDEX ON AuthLog(EventHash) WHERE EventHash IS NOT NULL`) ile sağlanır.
   - `compute_event_hash` (`src/parser.rs`) girdisi `"{source_file}\n{source_line}\n{raw_line}"` formatındadır. Bu formatı değiştirmeyin; değiştirilmesi durumunda geçmiş backfill işlemlerinde mükerrer kayıtlar oluşur.
2. **SQL Güvenliği (Parametreli Sorgular):**
   - `build_recent_query` (`src/server.rs`) içindeki tüm filtreler parametreli (`?1`, `?2` vb.) çalışmalıdır.
   - `ORDER BY` sütun adları yalnızca beyaz listeden (`Zaman`, `Username`, `Realm`, `ClientIP`, `NASIdentifier`, `CallingStationID`, `Result`, `ReasonCode`) seçilebilir.
3. **XSS ve CSV Formül Enjeksiyonu Güvenliği:**
   - `eduroam_dashboard.html` içinde kullanıcıdan/logdan gelen tüm metinler `esc()` fonksiyonundan geçirilmelidir.
   - CSV dışa aktarımı `/api/export.csv` üzerinden sunucu tarafında yapılmalı ve `=`, `+`, `-`, `@` ile başlayan hücreler `'` ile zırhlanmalıdır (`format_csv_field`).
4. **Atomik Durum Yazımı ve Dosya Paylaşımı:**
   - NPS aktif olarak log dosyasına yazarken kolektör dosyayı `FILE_SHARE_READ | FILE_SHARE_WRITE` ile açmalıdır.
   - `last_run.txt` güncellemeleri önce `.tmp` dosyasına yazılıp atomik `rename` ile taşınmalıdır (`save_state_atomic`).
