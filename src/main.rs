pub mod collector;
pub mod config;
pub mod db;
pub mod parser;
pub mod server;

use crate::collector::{run_collector_pass, save_state_atomic};
use crate::config::EduroamConfig;
use crate::db::{ensure_schema_and_migrate, open_db, reset_tables};
use crate::server::{build_router, AppState};
use chrono::{Duration, Local};
use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicU64, Arc, RwLock};

#[derive(Parser, Debug)]
#[command(
    name = "eduroam-nps",
    version = "5.2.0",
    about = "Eduroam NPS RADIUS Loglama ve Dashboard Sistemi (Rust Edition)"
)]
struct Cli {
    /// Opsiyonel eduroam_config.json dosya yolu
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// HTTP sunucusunu ve arka plan otomatik log toplayiciyi birlikte calistirir (Varsayilan)
    Serve {
        /// Arka plan otomatik log toplama dongusunu devre disi birak (yalnizca HTTP sunucusu)
        #[arg(long)]
        no_collector: bool,
    },
    /// Tek seferlik log toplama calistirir ve cikar (Scheduled Task uyumlu)
    Collect,
    /// Yalnizca HTTP API ve Dashboard sunucusunu baslatir
    Server,
    /// SQLite veritabanini olusturur veya mevcut sema uzerinde migration calistirir
    Setup {
        /// Mevcut DB varsa sormadan sifirlar
        #[arg(long)]
        force: bool,
        /// Mevcut DB'yi silmeden yalnizca sema migration calistirir
        #[arg(long)]
        migrate: bool,
    },
    /// Mevcut veritabanini silmeden son N gunu yeniden tarar (Append-Only Backfill)
    Backfill {
        #[arg(short, long, default_value_t = 90)]
        days: i64,
    },
    /// Veritabanini sifirlar ve son N gunu bastan doldurur
    ResetBackfill {
        #[arg(short, long, default_value_t = 7)]
        days: i64,
        #[arg(long)]
        force: bool,
    },
    /// Windows Zamanlanmis Gorevlerini (Scheduled Tasks) kaydeder (Yonetici olarak calistirin)
    RegisterTasks,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let cfg = EduroamConfig::load(cli.config.as_deref());

    match cli.command.unwrap_or(Commands::Serve {
        no_collector: false,
    }) {
        Commands::Collect => {
            if let Err(e) = run_collector_pass(&cfg, None, true) {
                eprintln!("[HATA] {}", e);
                std::process::exit(1);
            }
        }
        Commands::Server => {
            run_http_server(cfg, true).await;
        }
        Commands::Serve { no_collector } => {
            run_http_server(cfg, no_collector).await;
        }
        Commands::Setup { force, migrate } => {
            run_setup(&cfg, force, migrate);
        }
        Commands::Backfill { days } => {
            run_backfill(&cfg, days);
        }
        Commands::ResetBackfill { days, force } => {
            run_reset_and_backfill(&cfg, days, force);
        }
        Commands::RegisterTasks => {
            run_register_tasks();
        }
    }
}

async fn run_http_server(cfg: EduroamConfig, no_collector: bool) {
    let db_path = Path::new(&cfg.paths.db_path);
    if let Ok(conn) = open_db(db_path) {
        let _ = ensure_schema_and_migrate(&conn);
    }

    let db_generation = Arc::new(AtomicU64::new(1));

    if !no_collector {
        let bg_cfg = cfg.clone();
        let bg_gen = Arc::clone(&db_generation);
        let poll_secs = cfg.collector.poll_interval_seconds.max(5);
        tokio::task::spawn_blocking(move || loop {
            let _ = run_collector_pass(&bg_cfg, Some(&bg_gen), false);
            std::thread::sleep(std::time::Duration::from_secs(poll_secs));
        });
    }

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Sunucu baslatilamadi: http://{}/ ({})", addr, e);
            eprintln!("Bu portta baska bir surec calisiyor olabilir.");
            std::process::exit(1);
        }
    };

    println!(
        "Eduroam Dashboard Sunucusu (Rust v5.2) baslatildi: http://{}/",
        addr
    );
    if !no_collector {
        println!(
            "Arka plan log toplayici aktif (her {} saniyede bir artimli tarama).",
            cfg.collector.poll_interval_seconds.max(5)
        );
    }
    println!("Durdurmak icin Ctrl+C");

    let app_state = Arc::new(AppState {
        config: cfg,
        db_generation,
        charts_cache: RwLock::new(None),
    });

    let app = build_router(app_state);
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("Sunucu hatasi: {}", e);
    }
}

fn run_setup(cfg: &EduroamConfig, force: bool, migrate_only: bool) {
    let db_path = Path::new(&cfg.paths.db_path);
    if db_path.exists() {
        if migrate_only {
            let conn = open_db(db_path).expect("DB acilamadi");
            ensure_schema_and_migrate(&conn).expect("Migration basarisiz");
            println!("Migration tamamlandi: {}", db_path.display());
            return;
        }
        if !force {
            println!("UYARI: {} zaten mevcut.", db_path.display());
            print!("Uzerine yazmak icin 'EVET' yazin, migration icin 'MIGRATE' yazin: ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            let _ = io::stdin().read_line(&mut input);
            let choice = input.trim();
            if choice.eq_ignore_ascii_case("MIGRATE") {
                let conn = open_db(db_path).expect("DB acilamadi");
                ensure_schema_and_migrate(&conn).expect("Migration basarisiz");
                println!("Migration tamamlandi: {}", db_path.display());
                return;
            }
            if choice != "EVET" {
                println!("Iptal.");
                return;
            }
        }
        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = fs::remove_file(format!("{}-shm", db_path.display()));
        println!("Eski veritabani silindi.");
    }

    let conn = open_db(db_path).expect("DB olusturulamadi");
    ensure_schema_and_migrate(&conn).expect("Sema olusturulamadi");
    println!("Veritabani olusturuldu: {}", db_path.display());
}

fn run_backfill(cfg: &EduroamConfig, days_back: i64) {
    let db_path = Path::new(&cfg.paths.db_path);
    let state_path = Path::new(&cfg.paths.state_file);
    let log_dir = Path::new(&cfg.paths.log_dir);

    if !db_path.exists() {
        eprintln!(
            "DB bulunamadi: {}. Once `eduroam-nps setup` calistirin.",
            db_path.display()
        );
        std::process::exit(1);
    }
    if !log_dir.exists() {
        eprintln!("Log klasoru bulunamadi: {}", log_dir.display());
        std::process::exit(1);
    }

    let conn = open_db(db_path).expect("DB acilamadi");
    ensure_schema_and_migrate(&conn).expect("Migration hatasi");

    let oldest_date = (Local::now() - Duration::days(days_back)).date_naive();
    let oldest_ts = oldest_date.and_hms_opt(0, 0, 0).unwrap();
    let oldest_file = format!("IN{}.log", oldest_date.format("%y%m%d"));

    println!("===== APPEND-ONLY BACKFILL (RUST) =====");
    println!("DB        : {}", db_path.display());
    println!("Log klasor: {}", log_dir.display());
    println!(
        "Baslangic : {} ({})",
        oldest_date.format("%Y-%m-%d"),
        oldest_file
    );

    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM AuthLog", [], |r| r.get(0))
        .unwrap_or(0);
    println!("Backfill oncesi AuthLog: {}", before);

    save_state_atomic(state_path, &oldest_file, oldest_ts, 0, 0).expect("State yazilamadi");
    drop(conn);

    if let Err(e) = run_collector_pass(cfg, None, true) {
        eprintln!("[HATA] {}", e);
        std::process::exit(1);
    }

    let conn = open_db(db_path).expect("DB acilamadi");
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM AuthLog", [], |r| r.get(0))
        .unwrap_or(0);
    println!("Backfill sonrasi AuthLog: {}", after);
    println!("Yeni eklenen kayit     : {}", after - before);
}

fn run_reset_and_backfill(cfg: &EduroamConfig, days_back: i64, force: bool) {
    let db_path = Path::new(&cfg.paths.db_path);
    let state_path = Path::new(&cfg.paths.state_file);
    let log_dir = Path::new(&cfg.paths.log_dir);

    println!("===== DB SIFIRLA + BACKFILL (RUST) =====");
    println!("DB     : {}", db_path.display());
    println!("Log kl.: {}", log_dir.display());
    println!("Aralik : Son {} gun", days_back);

    let conn = open_db(db_path).expect("DB acilamadi");
    ensure_schema_and_migrate(&conn).expect("Migration hatasi");

    let auth_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM AuthLog", [], |r| r.get(0))
        .unwrap_or(0);
    let dev_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM Devices", [], |r| r.get(0))
        .unwrap_or(0);
    println!(
        "Mevcut: {} AuthLog kaydi, {} cihaz\n",
        auth_count, dev_count
    );

    if !force {
        println!("UYARI: Bu islem tum AuthLog ve Devices kayitlarini silecek!");
        print!("Devam etmek icin 'EVET' yazin: ");
        let _ = io::stdout().flush();
        let mut input = String::new();
        let _ = io::stdin().read_line(&mut input);
        if input.trim() != "EVET" {
            println!("Iptal edildi.");
            return;
        }
    }

    reset_tables(&conn).expect("Tablolar temizlenemedi");
    println!("DB temizlendi.");
    drop(conn);

    let oldest_date = (Local::now() - Duration::days(days_back)).date_naive();
    let oldest_ts = oldest_date.and_hms_opt(0, 0, 0).unwrap();
    let oldest_file = format!("IN{}.log", oldest_date.format("%y%m%d"));
    save_state_atomic(state_path, &oldest_file, oldest_ts, 0, 0).expect("State yazilamadi");

    if let Err(e) = run_collector_pass(cfg, None, true) {
        eprintln!("[HATA] {}", e);
        std::process::exit(1);
    }
}

fn run_register_tasks() {
    let exe_path = std::env::current_exe().expect("Calistirilabilir dosya yolu alinamadi");
    let exe_str = exe_path.to_string_lossy();

    let status = std::process::Command::new("schtasks.exe")
        .args([
            "/Create",
            "/TN",
            "EduroamDashboardServer",
            "/TR",
            &format!("\"{}\" serve", exe_str),
            "/SC",
            "ONSTART",
            "/RU",
            "SYSTEM",
            "/RL",
            "HIGHEST",
            "/F",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!(
                "Gorev kaydedildi: EduroamDashboardServer ({} serve - ONSTART / SYSTEM)",
                exe_str
            );
        }
        Ok(s) => {
            eprintln!(
                "Gorev kaydedilemedi (Cikis kodu: {:?}). Lutfen Yonetici (Administrator) olarak calistirin.",
                s.code()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("schtasks.exe calistirilamadi: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::load_state;
    use crate::parser::{
        compute_event_hash, format_csv_field, parse_ias_event_line, parse_ias_timestamp,
        split_username,
    };
    use crate::server::build_recent_query;
    use chrono::NaiveDate;
    use std::collections::HashMap;
    use std::fs::OpenOptions;

    #[test]
    fn test_default_config_backward_compatible() {
        let cfg = EduroamConfig::default();
        assert_eq!(cfg.paths.log_dir, r"C:\NPSLogs");
        assert_eq!(cfg.paths.db_path, r"C:\EduroamLogs\eduroam_logs.db");
        assert_eq!(cfg.server.host, "127.0.0.1");
        assert_eq!(cfg.server.max_page_limit, 1000);
        assert!(!cfg.collector.collect_challenges);
    }

    #[test]
    fn test_parses_ias_xml_fields() {
        let line = r#"<Event><Timestamp data_type="4">06/08/2026 10:11:12.123</Timestamp><Packet-Type data_type="0">3</Packet-Type><User-Name data_type="1">ali@kastamonu.edu.tr</User-Name><Client-IP-Address data_type="3">10.0.0.1</Client-IP-Address><Reason-Code data_type="0">16</Reason-Code></Event>"#;
        let fields = parse_ias_event_line(line);
        assert_eq!(fields.packet_type, "3");
        assert_eq!(fields.user_name, "ali@kastamonu.edu.tr");
        assert_eq!(fields.client_ip, "10.0.0.1");
        assert_eq!(fields.reason_code, "16");
        let ts = parse_ias_timestamp(fields.timestamp).expect("valid timestamp");
        assert_eq!(
            ts.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-06-08 10:11:12"
        );
    }

    #[test]
    fn test_splits_usernames() {
        let (u1, r1) = split_username("ali@kastamonu.edu.tr");
        assert_eq!(u1, "ali");
        assert_eq!(r1, "kastamonu.edu.tr");

        let (u2, r2) = split_username("localuser");
        assert_eq!(u2, "localuser");
        assert_eq!(r2, "");
    }

    #[test]
    fn test_event_hash_matches_powershell_sha256() {
        let a = compute_event_hash("IN260608.log", 10, "<Event>A</Event>");
        let b = compute_event_hash("IN260608.log", 10, "<Event>A</Event>");
        let c = compute_event_hash("IN260608.log", 11, "<Event>A</Event>");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn test_parameterized_recent_query_and_limits() {
        let mut q = HashMap::new();
        q.insert("username".to_string(), "a%' OR 1=1 --".to_string());
        q.insert("realm".to_string(), "kastamonu.edu.tr".to_string());
        q.insert("rc".to_string(), "16".to_string());
        q.insert("sort".to_string(), "Username".to_string());
        q.insert("dir".to_string(), "asc".to_string());
        q.insert("limit".to_string(), "999999".to_string());

        let rq = build_recent_query(&q, 7, 200, 1000);
        assert!(rq.where_clause.contains("Username LIKE ?"));
        assert!(rq.where_clause.contains("Realm LIKE ?"));
        assert!(rq.where_clause.contains("ReasonCode = ?"));
        assert_eq!(rq.sort_column, "Username");
        assert_eq!(rq.sort_direction, "ASC");
        assert_eq!(rq.limit, 1000);
    }

    #[test]
    fn test_csv_escaping_and_formula_injection_guard() {
        assert_eq!(format_csv_field(r#"a"b,c"#), r#""a""b,c""#);
        assert_eq!(
            format_csv_field("=cmd|'/C calc'!A0"),
            "\"'=cmd|'/C calc'!A0\""
        );
    }

    #[test]
    fn test_end_to_end_collector_and_byte_offset_resume() {
        let tmp_dir = std::env::temp_dir().join(format!("eduroam_rust_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let log_file = tmp_dir.join("IN260608.log");
        let db_file = tmp_dir.join("test.db");
        let state_file = tmp_dir.join("last_run.txt");
        let err_file = tmp_dir.join("errors.log");

        let mut cfg = EduroamConfig::default();
        cfg.paths.log_dir = tmp_dir.to_string_lossy().to_string();
        cfg.paths.data_dir = tmp_dir.to_string_lossy().to_string();
        cfg.paths.db_path = db_file.to_string_lossy().to_string();
        cfg.paths.state_file = state_file.to_string_lossy().to_string();
        cfg.paths.error_log = err_file.to_string_lossy().to_string();
        cfg.collector.retention_days = 36500;

        let initial_lines = concat!(
            "<Event><Timestamp>06/08/2026 10:00:01.000</Timestamp><Packet-Type>1</Packet-Type><User-Name>user1@kastamonu.edu.tr</User-Name><Client-IP-Address>10.1.1.1</Client-IP-Address><Calling-Station-Id>aa-bb-cc-dd-ee-01</Calling-Station-Id><NAS-Identifier>AP-1</NAS-Identifier></Event>\r\n",
            "<Event><Timestamp>06/08/2026 10:00:02.000</Timestamp><Packet-Type>2</Packet-Type><Client-IP-Address>10.1.1.1</Client-IP-Address><Calling-Station-Id>aa-bb-cc-dd-ee-01</Calling-Station-Id></Event>\r\n"
        );
        fs::write(&log_file, initial_lines).unwrap();

        // Save initial state pointing to IN260608.log
        let start_ts = NaiveDate::from_ymd_opt(2026, 6, 8)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        save_state_atomic(&state_file, "IN260608.log", start_ts, 0, 0).unwrap();

        let inserted1 = run_collector_pass(&cfg, None, false).unwrap();
        assert_eq!(inserted1, 1);

        // Running again without changes should skip via byte_offset and insert 0
        let inserted_dup = run_collector_pass(&cfg, None, false).unwrap();
        assert_eq!(inserted_dup, 0);

        // Append a new failure event to the same file
        let mut f = OpenOptions::new().append(true).open(&log_file).unwrap();
        writeln!(
            f,
            "<Event><Timestamp>06/08/2026 10:05:00.000</Timestamp><Packet-Type>3</Packet-Type><User-Name>user2@kastamonu.edu.tr</User-Name><Client-IP-Address>10.1.1.1</Client-IP-Address><Calling-Station-Id>aa-bb-cc-dd-ee-02</Calling-Station-Id><Reason-Code>16</Reason-Code></Event>"
        )
        .unwrap();
        drop(f);

        let inserted2 = run_collector_pass(&cfg, None, false).unwrap();
        assert_eq!(inserted2, 1);

        let conn = open_db(&db_file).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM AuthLog", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        let dev_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM Devices", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dev_count, 2);

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_like_pattern_escapes_wildcards() {
        assert_eq!(crate::parser::to_sql_like_pattern("a%b_c\\d"), "%a\\%b\\_c\\\\d%");
        assert_eq!(crate::parser::to_sql_like_pattern(""), "%%");
    }

    #[test]
    fn test_order_by_whitelist_rejects_unknown_column() {
        let mut q = HashMap::new();
        q.insert("sort".to_string(), "Username; DROP TABLE AuthLog".to_string());
        q.insert("dir".to_string(), "sideways".to_string());
        let rq = build_recent_query(&q, 7, 200, 1000);
        assert_eq!(rq.sort_column, "TimeCreated");
        assert_eq!(rq.sort_direction, "DESC");
        assert!(!rq.where_clause.contains("DROP"));
    }

    #[test]
    fn test_invalid_paging_and_dates_fall_back_safely() {
        let mut q = HashMap::new();
        q.insert("page".to_string(), "-5".to_string());
        q.insert("limit".to_string(), "0".to_string());
        q.insert("from".to_string(), "2026-01-01' OR '1'='1".to_string());
        let rq = build_recent_query(&q, 7, 200, 1000);
        assert_eq!(rq.page, 1);
        assert_eq!(rq.limit, 200);
        assert_eq!(rq.offset, 0);
        assert!(!rq.where_clause.contains("OR '1'"));
    }

    #[test]
    fn test_cors_origin_rules() {
        let mut cfg = EduroamConfig::default();
        assert!(!cfg.is_cors_origin_allowed(Some("http://a.example")));
        cfg.server.enable_cors = true;
        cfg.server.allowed_origins = vec!["http://a.example".to_string()];
        assert!(cfg.is_cors_origin_allowed(Some("http://a.example")));
        assert!(cfg.is_cors_origin_allowed(Some("  http://a.example ")));
        assert!(!cfg.is_cors_origin_allowed(Some("http://evil.example")));
        assert!(!cfg.is_cors_origin_allowed(Some("")));
        assert!(!cfg.is_cors_origin_allowed(None));
    }

    #[test]
    fn test_extract_file_date() {
        assert_eq!(
            crate::collector::extract_file_date("IN261106.log"),
            NaiveDate::from_ymd_opt(2026, 11, 6)
        );
        assert_eq!(
            crate::collector::extract_file_date("in261106.LOG"),
            NaiveDate::from_ymd_opt(2026, 11, 6)
        );
        assert_eq!(crate::collector::extract_file_date("IN261306.log"), None);
        assert_eq!(crate::collector::extract_file_date("IN26110.log"), None);
        assert_eq!(crate::collector::extract_file_date("notes.txt"), None);
    }

    #[test]
    fn test_parse_ias_timestamp_rejects_garbage() {
        assert!(parse_ias_timestamp("not a date").is_none());
        assert!(parse_ias_timestamp("").is_none());
    }

    #[test]
    fn test_reason_text_known_and_unknown() {
        use crate::parser::get_reason_text;
        assert_eq!(get_reason_text(0, ""), "Basari");
        assert_eq!(get_reason_text(112, "112"), "Istek iletilmedi (realm yonlendirme)");
        assert_eq!(get_reason_text(9999, "9999"), "Kod: 9999");
    }

    #[test]
    fn test_state_roundtrip_and_atomic_tmp_cleanup() {
        let dir = std::env::temp_dir().join(format!("eduroam_state_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let state = dir.join("last_run.txt");
        let ts = NaiveDate::from_ymd_opt(2026, 11, 7)
            .unwrap()
            .and_hms_opt(23, 15, 14)
            .unwrap();
        save_state_atomic(&state, "IN261107.log", ts, 4096, 17).unwrap();
        let (file, got_ts, off, line) = load_state(&state);
        assert_eq!((file.as_str(), got_ts, off, line), ("IN261107.log", ts, 4096, 17));
        assert!(!state.with_extension("txt.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_retention_prunes_old_records_only() {
        let dir = std::env::temp_dir().join(format!("eduroam_retention_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db_file = dir.join("test.db");
        let state_file = dir.join("last_run.txt");

        let mut cfg = EduroamConfig::default();
        cfg.paths.log_dir = dir.to_string_lossy().to_string();
        cfg.paths.data_dir = dir.to_string_lossy().to_string();
        cfg.paths.db_path = db_file.to_string_lossy().to_string();
        cfg.paths.state_file = state_file.to_string_lossy().to_string();
        cfg.paths.error_log = dir.join("errors.log").to_string_lossy().to_string();
        cfg.collector.retention_days = 180;

        fs::write(
            dir.join("IN260608.log"),
            "<Event><Timestamp>06/08/2026 10:00:01.000</Timestamp><Packet-Type>3</Packet-Type><User-Name>u@kastamonu.edu.tr</User-Name><Client-IP-Address>10.1.1.1</Client-IP-Address><Reason-Code>16</Reason-Code></Event>\r\n",
        )
        .unwrap();
        let start = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap().and_hms_opt(0, 0, 0).unwrap();
        save_state_atomic(&state_file, "IN260608.log", start, 0, 0).unwrap();
        run_collector_pass(&cfg, None, false).unwrap();

        let conn = open_db(&db_file).unwrap();
        conn.execute(
            "INSERT INTO AuthLog (TimeCreated, Result, EventHash) VALUES ('2000-01-01 00:00:00','FAILURE','old-row')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO AuthLog (TimeCreated, Result, EventHash) VALUES (datetime('now'),'FAILURE','new-row')",
            [],
        )
        .unwrap();
        drop(conn);

        // Yeni log verisi olmadan da dosya listesi doluyken retention calismali.
        run_collector_pass(&cfg, None, false).unwrap();

        let conn = open_db(&db_file).unwrap();
        let old: i64 = conn
            .query_row("SELECT COUNT(*) FROM AuthLog WHERE EventHash='old-row'", [], |r| r.get(0))
            .unwrap();
        let new: i64 = conn
            .query_row("SELECT COUNT(*) FROM AuthLog WHERE EventHash='new-row'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(old, 0, "180 gunden eski kayit silinmeli");
        assert_eq!(new, 1, "yeni kayit korunmali");
        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_split_username_edge_cases() {
        assert_eq!(split_username(""), (String::new(), String::new()));
        assert_eq!(split_username("   "), (String::new(), String::new()));
        assert_eq!(split_username("user@"), ("user@".to_string(), String::new()));
        assert_eq!(split_username("@realm"), ("@realm".to_string(), String::new()));
        assert_eq!(
            split_username("john.doe@sub.kastamonu.edu.tr"),
            ("john.doe".to_string(), "sub.kastamonu.edu.tr".to_string())
        );
        assert_eq!(
            split_username("user@inner@domain.com"),
            ("user@inner".to_string(), "domain.com".to_string())
        );
    }

    #[test]
    fn test_rows_to_csv_formatting_and_escaping() {
        use crate::server::{rows_to_csv, RecentLogRow};
        let rows = vec![
            RecentLogRow {
                log_id: 1,
                zaman: "2026-11-07 10:00:00".to_string(),
                username: "testuser".to_string(),
                realm: "kastamonu.edu.tr".to_string(),
                client_ip: "10.0.0.1".to_string(),
                nas_identifier: "AP-Floor-1".to_string(),
                calling_station_id: "AA-BB-CC-DD-EE-FF".to_string(),
                result: "SUCCESS".to_string(),
                reason_code: 0,
                reason: "Basari".to_string(),
            },
            RecentLogRow {
                log_id: 2,
                zaman: "2026-11-07 10:01:00".to_string(),
                username: "=cmd|' /C calc'!A0".to_string(),
                realm: "evil.org".to_string(),
                client_ip: "10.0.0.2".to_string(),
                nas_identifier: r#"AP,"Special""#.to_string(),
                calling_station_id: "11-22-33-44-55-66".to_string(),
                result: "FAILURE".to_string(),
                reason_code: 16,
                reason: "Hatali sifre".to_string(),
            },
        ];

        let csv = rows_to_csv(&rows);
        assert!(csv.starts_with("\"Zaman\",\"Username\",\"Realm\","));
        assert!(csv.contains("\"'=cmd|' /C calc'!A0\""));
        assert!(csv.contains(r#""AP,""Special""""#));
        assert!(csv.ends_with("\r\n"));
    }

    #[test]
    fn test_schema_migration_and_reset_idempotent() {
        let dir = std::env::temp_dir().join(format!("eduroam_db_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("schema_test.db");

        let conn = open_db(&db_path).unwrap();
        // First migration
        ensure_schema_and_migrate(&conn).unwrap();
        // Second migration (idempotent)
        ensure_schema_and_migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO AuthLog (EventID, TimeCreated, Result, EventHash) VALUES (1, '2026-11-07 12:00:00', 'SUCCESS', 'h1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO Devices (MACAddress, FirstSeen, LastSeen, LastUsername, LastResult) VALUES ('AA-BB', '2026-11-07', '2026-11-07', 'u', 'SUCCESS')",
            [],
        )
        .unwrap();

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM AuthLog", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);

        reset_tables(&conn).unwrap();
        let count_after: i64 = conn.query_row("SELECT COUNT(*) FROM AuthLog", [], |r| r.get(0)).unwrap();
        assert_eq!(count_after, 0);
        let dev_count_after: i64 = conn.query_row("SELECT COUNT(*) FROM Devices", [], |r| r.get(0)).unwrap();
        assert_eq!(dev_count_after, 0);

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }
}
