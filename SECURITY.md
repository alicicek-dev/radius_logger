# Güvenlik Politikası ve Mimari Kontroller (`SECURITY.md`)

Bu belge, **Eduroam NPS Loglama Sistemi (v5.2 — Rust Edition)** içindeki güvenlik kontrollerini, veri gizliliği gereksinimlerini ve güvenlik açığı bildirim sürecini açıklar.

---

## 1. Uygulanan Güvenlik Kontrolleri

### 1.1 Ağ ve Erişim İzolasyonu
- **Localhost Bağlaması:** `src/main.rs` ve `src/server.rs` varsayılan olarak yalnızca `127.0.0.1:8080` arayüzünü dinler; dış ağa doğrudan açık değildir.
- **Kısıtlı CORS Politikası:** `server.enableCors` varsayılan olarak `false` değerindedir. Aktif edilse dahi `is_cors_origin_allowed` (`src/config.rs`) yalnızca `server.allowedOrigins` beyaz listesindeki kaynaklara `Access-Control-Allow-Origin` başlığı döner (`*` joker karakteri kullanılmaz).

### 1.2 SQL Enjeksiyonu Koruması
- **Parametreli Sorgular:** `/api/recent`, `/api/export.csv` ve `/api/charts` üzerindeki tüm kullanıcı girdileri `rusqlite` parametreli sorguları (`?1`, `?2`) ile çalıştırılır.
- **LIKE Joker Karakter Kaçışı:** `to_sql_like_pattern` (`src/parser.rs`) fonksiyonu `\`, `%` ve `_` karakterlerini kaçışlayarak (`ESCAPE '\'`) istenmeyen joker aramalarını engeller.
- **Sıralama Beyaz Listesi:** `ORDER BY` ifadelerinde doğrudan kullanıcı girdisi kullanılmaz; `build_recent_query` (`src/server.rs`) içindeki sabit sütun eşlemesi ve yön kontrolü (`ASC`/`DESC`) uygulanır.
- **Üst Sınır Koruması (DoS Önleme):** Sayfa başına kayıt sayısı `maxPageLimit` (`1000`) ve CSV dışa aktarım satır sayısı `maxExportRows` (`10000`) ile sınırlandırılmıştır.

### 1.3 XSS ve CSV Formül Enjeksiyonu Güvenliği
- **HTML Escape (`esc`):** `eduroam_dashboard.html` içinde API'den dönen tüm kullanıcı adları, realm değerleri, MAC adresleri, NAS kimlikleri ve grafik etiketleri DOM'a yazılmadan önce `esc()` fonksiyonundan geçirilir.
- **Sunucu Taraflı CSV ve Formül Koruması:** CSV dışa aktarımı `format_csv_field` (`src/parser.rs`) ile çift tırnak kaçışı uygulanarak ve `=`, `+`, `-`, `@`, `\t`, `\r` ile başlayan hücrelerin önüne `'` eklenerek sunucu tarafında üretilir.

### 1.4 Derleme ve Çalışma Zamanı Güvenliği
- **Bellek Güvenliği ve Tek Binary:** Sistem tamamen güvenli Rust ile derlenmiştir ve SQLite motoru `bundled` olarak statik bağlanmıştır; çalışma zamanında harici betik veya modül indirmez.

---

## 2. Veri Gizliliği (KVKK / GDPR) ve Saklama Süresi

- **Kişisel Veriler:** `AuthLog` ve `Devices` tabloları kullanıcı kimliği (`Username`, `FullUsername`), kurum bilgisi (`Realm`), erişim noktası IP adresi (`ClientIP`) ve cihaz MAC adresi (`CallingStationID`) içerir.
- **Otomatik Veri Temizleme (Retention):** Kolektör her çalışmada `collector.retentionDays` (varsayılan `180` gün) süresinden eski `AuthLog` kayıtlarını otomatik olarak siler.
- **Depolama Güvenliği:**
  - `.gitignore` dosyası `*.db`, `*.db-wal`, `*.db-shm`, `*.log`, `last_run.txt` ve `eduroam_config.json` dosyalarının git deposuna eklenmesini engeller.
  - Sunucu üzerinde `C:\EduroamLogs\` dizin izinlerinin yalnızca `NT AUTHORITY\SYSTEM` ve yetkili sistem yöneticilerine açık olması ve disk şifrelemesinin (BitLocker) aktif tutulması önerilir.

---

## 3. Güvenlik Açığı Bildirimi

Sistemde bir güvenlik zaafiyeti tespit ederseniz lütfen herkese açık bir Issue açmak yerine doğrudan **Kastamonu Üniversitesi Bilgi İşlem Daire Başkanlığı** ağ/sistem yönetimi ekibiyle iletişime geçin.
