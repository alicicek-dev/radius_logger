pub mod collector;
pub mod config;
pub mod db;
pub mod parser;
pub mod server;

use crate::collector::{extract_file_date, run_collector_pass, save_state_atomic};
use crate::config::{write_setup_config, EduroamConfig};
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
    /// Yapilandirma dosyasi (eduroam_config.json) uretir veya SQLite veritabanini kurar/migrate eder
    Setup {
        /// NPS log klasoru yolu (verilirse yalnizca eduroam_config.json olusturur)
        #[arg(long)]
        log_dir: Option<String>,
        /// Yapilandirma dosyasinin yazilacagi yol (varsayilan: C:\EduroamLogs\eduroam_config.json)
        #[arg(long)]
        config_path: Option<PathBuf>,
        /// Mevcut config/DB varsa uzerine yazar
        #[arg(long)]
        force: bool,
        /// --log-dir klasoru henuz yoksa da config dosyasini olusturur
        #[arg(long)]
        allow_missing: bool,
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
    let (cfg, config_source) = EduroamConfig::load_with_source(cli.config.as_deref());

    match cli.command.unwrap_or(Commands::Serve {
        no_collector: false,
    }) {
        Commands::Collect => {
            ensure_log_dir_ready(&cfg.paths.log_dir, config_source.as_deref());
            if let Err(e) = run_collector_pass(&cfg, None, true) {
                eprintln!("[HATA] {}", e);
                std::process::exit(1);
            }
        }
        Commands::Server => {
            run_http_server(cfg, true).await;
        }
        Commands::Serve { no_collector } => {
            if !no_collector {
                ensure_log_dir_ready(&cfg.paths.log_dir, config_source.as_deref());
            }
            run_http_server(cfg, no_collector).await;
        }
        Commands::Setup {
            log_dir,
            config_path,
            force,
            allow_missing,
            migrate,
        } => {
            if let Some(dir) = log_dir {
                run_setup_config(&dir, config_path.as_deref(), force, allow_missing);
            } else {
                run_setup(&cfg, force, migrate);
            }
        }
        Commands::Backfill { days } => {
            ensure_log_dir_ready(&cfg.paths.log_dir, config_source.as_deref());
            run_backfill(&cfg, days);
        }
        Commands::ResetBackfill { days, force } => {
            ensure_log_dir_ready(&cfg.paths.log_dir, config_source.as_deref());
            run_reset_and_backfill(&cfg, days, force);
        }
        Commands::RegisterTasks => {
            run_register_tasks();
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LogDirCheck {
    Ready,
    EmptyWarning(String),
}

fn check_log_dir(log_dir: &str, config_source: Option<&Path>) -> Result<LogDirCheck, String> {
    let source_desc = config_source
        .map(|p| format!("config dosyasi: {}", p.display()))
        .unwrap_or_else(|| "kod ici varsayilan".to_string());
    let path = Path::new(log_dir);

    if !path.exists() || !path.is_dir() {
        return Err(format!(
            "Log klasoru bulunamadi: '{}' (kaynak: {}).\n\
             Cozum: `eduroam-nps.exe setup --log-dir \"<log_klasorunuz>\"` komutunu calistirin veya `eduroam_config.example.json` dosyasini referans alarak `eduroam_config.json` olusturun.",
            log_dir, source_desc
        ));
    }

    let has_logs = fs::read_dir(path)
        .ok()
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                extract_file_date(&name).is_some()
            })
        })
        .unwrap_or(false);

    if !has_logs {
        return Ok(LogDirCheck::EmptyWarning(format!(
            "Log klasorunde henuz eslesen `IN{{YYMMDD}}.log` (IN*.log) dosyasi bulunamadi: '{}' (kaynak: {}).\n\
             Yeni kurulumda NPS henuz log yazmamis olabilir; farkli bir klasor kullaniyorsaniz `eduroam-nps.exe setup --log-dir \"<log_klasorunuz>\"` komutunu calistirin veya `eduroam_config.example.json` dosyasina bakin.",
            log_dir, source_desc
        )));
    }

    Ok(LogDirCheck::Ready)
}

fn ensure_log_dir_ready(log_dir: &str, config_source: Option<&Path>) {
    match check_log_dir(log_dir, config_source) {
        Ok(LogDirCheck::Ready) => {}
        Ok(LogDirCheck::EmptyWarning(msg)) => eprintln!("[UYARI] {}", msg),
        Err(msg) => {
            eprintln!("[HATA] {}", msg);
            std::process::exit(1);
        }
    }
}

fn display_full_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    }
}

fn run_setup_config(log_dir: &str, config_path: Option<&Path>, force: bool, allow_missing: bool) {
    match write_setup_config(log_dir, config_path, force, allow_missing) {
        Ok(written) => {
            let full = display_full_path(&written);
            println!("Yapilandirma dosyasi yazildi: {}", full.display());
            println!("Sonraki adimlar:");
            println!("  1. Veritabanini hazirlayin : eduroam-nps.exe setup --migrate");
            println!("  2. Sistemi baslatin        : eduroam-nps.exe");
        }
        Err(e) => {
            eprintln!("[HATA] {}", e);
            std::process::exit(1);
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

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Builds a Task Scheduler 2.0 XML definition with `ExecutionTimeLimit=PT0S` (unlimited)
/// so the `serve` daemon is never stopped after Windows' default 72-hour limit.
fn build_scheduled_task_xml(exe_path: &str) -> String {
    let escaped_exe = escape_xml(exe_path);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers>
    <BootTrigger>
      <Enabled>true</Enabled>
    </BootTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>S-1-5-18</UserId>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>serve</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        escaped_exe
    )
}

fn run_register_tasks() {
    let exe_path = std::env::current_exe().expect("Calistirilabilir dosya yolu alinamadi");
    let exe_str = exe_path.to_string_lossy();
    let xml = build_scheduled_task_xml(&exe_str);
    let xml_path = std::env::temp_dir().join(format!("eduroam_task_{}.xml", std::process::id()));

    if let Err(e) = fs::write(&xml_path, xml.as_bytes()) {
        eprintln!("Gecici gorev XML dosyasi yazilamadi: {}", e);
        std::process::exit(1);
    }

    let status = std::process::Command::new("schtasks.exe")
        .args([
            "/Create",
            "/TN",
            "EduroamDashboardServer",
            "/XML",
            &xml_path.to_string_lossy(),
            "/F",
        ])
        .status();

    let _ = fs::remove_file(&xml_path);

    match status {
        Ok(s) if s.success() => {
            println!(
                "Gorev kaydedildi: EduroamDashboardServer ({} serve - ONSTART / SYSTEM / PT0S)",
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

    #[test]
    fn test_scheduled_task_xml_uses_pt0s_and_escapes_path() {
        let xml = build_scheduled_task_xml(r#"C:\Eduroam & Logs\eduroam-nps.exe"#);
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<BootTrigger>"));
        assert!(xml.contains("<UserId>S-1-5-18</UserId>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<Arguments>serve</Arguments>"));
        assert!(xml.contains(r#"<Command>C:\Eduroam &amp; Logs\eduroam-nps.exe</Command>"#));
    }

    #[test]
    fn test_check_log_dir_reports_missing_empty_and_ready() {
        let dir = std::env::temp_dir().join(format!("eduroam_chk_dir_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        // 1. Missing directory -> Err with searched path, source, setup command, example json
        let missing = dir.join("nope");
        let err = check_log_dir(&missing.to_string_lossy(), None).unwrap_err();
        assert!(err.contains(&missing.to_string_lossy().to_string()));
        assert!(err.contains("kod ici varsayilan"));
        assert!(err.contains("setup --log-dir"));
        assert!(err.contains("eduroam_config.example.json"));

        // 2. Existing empty directory -> EmptyWarning citing config source
        fs::create_dir_all(&dir).unwrap();
        let src_cfg = PathBuf::from(r"C:\EduroamLogs\eduroam_config.json");
        match check_log_dir(&dir.to_string_lossy(), Some(&src_cfg)).unwrap() {
            LogDirCheck::EmptyWarning(msg) => {
                assert!(msg.contains(&dir.to_string_lossy().to_string()));
                assert!(msg.contains("eduroam_config.json"));
                assert!(msg.contains("setup --log-dir"));
                assert!(msg.contains("eduroam_config.example.json"));
            }
            LogDirCheck::Ready => panic!("Bos klasorde uyari donmeliydi"),
        }

        // 3. Matching IN{YYMMDD}.log present -> Ready
        fs::write(dir.join("IN261113.log"), "").unwrap();
        assert_eq!(
            check_log_dir(&dir.to_string_lossy(), Some(&src_cfg)).unwrap(),
            LogDirCheck::Ready
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
