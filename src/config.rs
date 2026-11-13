use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Default Windows Server NPS IAS log directory.
pub const DEFAULT_LOG_DIR: &str = r"C:\Windows\System32\LogFiles";
/// Default runtime configuration override file path.
pub const DEFAULT_CONFIG_PATH: &str = r"C:\EduroamLogs\eduroam_config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathsConfig {
    pub log_dir: String,
    pub data_dir: String,
    pub db_path: String,
    pub state_file: String,
    pub error_log: String,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            log_dir: DEFAULT_LOG_DIR.to_string(),
            data_dir: r"C:\EduroamLogs".to_string(),
            db_path: r"C:\EduroamLogs\eduroam_logs.db".to_string(),
            state_file: r"C:\EduroamLogs\last_run.txt".to_string(),
            error_log: r"C:\EduroamLogs\collector_errors.log".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub days_back: i64,
    pub enable_cors: bool,
    pub allowed_origins: Vec<String>,
    pub default_page_limit: usize,
    pub max_page_limit: usize,
    pub max_export_rows: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            days_back: 7,
            enable_cors: false,
            allowed_origins: Vec::new(),
            default_page_limit: 200,
            max_page_limit: 1000,
            max_export_rows: 10000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectorConfig {
    pub batch_size: usize,
    pub retention_days: i64,
    pub pending_queue_per_client: usize,
    pub collect_challenges: bool,
    pub max_log_mb: u64,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_seconds: u64,
    #[serde(default = "default_pending_ttl_secs")]
    pub pending_ttl_seconds: i64,
}

fn default_poll_interval_secs() -> u64 {
    30
}

fn default_pending_ttl_secs() -> i64 {
    60
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            batch_size: 2000,
            retention_days: 365,
            pending_queue_per_client: 50,
            collect_challenges: false,
            max_log_mb: 5,
            poll_interval_seconds: default_poll_interval_secs(),
            pending_ttl_seconds: default_pending_ttl_secs(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EduroamConfig {
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub collector: CollectorConfig,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PartialPaths {
    log_dir: Option<String>,
    data_dir: Option<String>,
    db_path: Option<String>,
    state_file: Option<String>,
    error_log: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PartialServer {
    host: Option<String>,
    port: Option<u16>,
    days_back: Option<i64>,
    enable_cors: Option<bool>,
    allowed_origins: Option<Vec<String>>,
    default_page_limit: Option<usize>,
    max_page_limit: Option<usize>,
    max_export_rows: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PartialCollector {
    batch_size: Option<usize>,
    retention_days: Option<i64>,
    pending_queue_per_client: Option<usize>,
    collect_challenges: Option<bool>,
    #[serde(alias = "maxLogMB")]
    max_log_mb: Option<u64>,
    poll_interval_seconds: Option<u64>,
    pending_ttl_seconds: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PartialConfig {
    paths: Option<PartialPaths>,
    server: Option<PartialServer>,
    collector: Option<PartialCollector>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupConfigPayload<'a> {
    paths: SetupPathsPayload<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupPathsPayload<'a> {
    log_dir: &'a str,
}

/// Writes a partial `eduroam_config.json` containing only `paths.logDir`.
pub fn write_setup_config(
    log_dir: &str,
    config_path: Option<&Path>,
    force: bool,
    allow_missing: bool,
) -> Result<PathBuf, String> {
    let clean_log_dir = log_dir.trim();
    if clean_log_dir.is_empty() {
        return Err("--log-dir degeri bos olamaz.".to_string());
    }

    let log_path = Path::new(clean_log_dir);
    if !allow_missing && !log_path.is_dir() {
        return Err(format!(
            "Belirtilen log klasoru bulunamadi veya bir klasor degil: '{}'.\n\
             Once klasoru olusturun veya klasor sonradan olusacaksa --allow-missing bayragini ekleyin.",
            clean_log_dir
        ));
    }

    let target = config_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));

    if target.exists() && !force {
        return Err(format!(
            "Yapilandirma dosyasi zaten mevcut: '{}'.\n\
             Uzerine yazmak istemiyorsaniz mevcut dosyayi duzenleyin veya --force bayragi ile yeniden calistirin.",
            target.display()
        ));
    }

    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Hedef config klasoru olusturulamadi ({}): {}",
                    parent.display(),
                    e
                )
            })?;
        }
    }

    let payload = SetupConfigPayload {
        paths: SetupPathsPayload {
            log_dir: clean_log_dir,
        },
    };
    let mut json = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("JSON olusturulamadi: {}", e))?;
    json.push('\n');

    fs::write(&target, json.as_bytes()).map_err(|e| {
        format!(
            "Yapilandirma dosyasi yazilamadi ({}): {}",
            target.display(),
            e
        )
    })?;

    Ok(target)
}

impl EduroamConfig {
    pub fn default_search_candidates(custom_path: Option<&Path>) -> Vec<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = custom_path {
            candidates.push(p.to_path_buf());
        }
        candidates.push(PathBuf::from(DEFAULT_CONFIG_PATH));
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("eduroam_config.json"));
            }
        }
        candidates.push(PathBuf::from("eduroam_config.json"));
        candidates
    }

    pub fn load(custom_path: Option<&Path>) -> Self {
        Self::load_with_source(custom_path).0
    }

    pub fn load_with_source(custom_path: Option<&Path>) -> (Self, Option<PathBuf>) {
        let candidates = Self::default_search_candidates(custom_path);
        Self::load_from_candidates(&candidates)
    }

    pub fn load_from_candidates(candidates: &[PathBuf]) -> (Self, Option<PathBuf>) {
        let mut cfg = EduroamConfig::default();
        let mut source: Option<PathBuf> = None;

        for candidate in candidates {
            if candidate.exists() {
                match fs::read_to_string(candidate) {
                    Ok(raw) => {
                        let trimmed = raw.trim_start_matches('\u{feff}').trim();
                        if !trimmed.is_empty() {
                            match serde_json::from_str::<PartialConfig>(trimmed) {
                                Ok(partial) => cfg.apply_partial(partial),
                                Err(e) => {
                                    eprintln!(
                                        "[UYARI] Config dosyasi gecersiz JSON ({}): {}",
                                        candidate.display(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[UYARI] Config dosyasi okunamadi ({}): {}",
                            candidate.display(),
                            e
                        );
                    }
                }
                source = Some(candidate.clone());
                break;
            }
        }

        if cfg.paths.state_file.trim().is_empty() {
            cfg.paths.state_file = Path::new(&cfg.paths.data_dir)
                .join("last_run.txt")
                .to_string_lossy()
                .to_string();
        }
        if cfg.paths.error_log.trim().is_empty() {
            cfg.paths.error_log = Path::new(&cfg.paths.data_dir)
                .join("collector_errors.log")
                .to_string_lossy()
                .to_string();
        }

        (cfg, source)
    }

    fn apply_partial(&mut self, partial: PartialConfig) {
        if let Some(p) = partial.paths {
            if let Some(v) = p.log_dir {
                self.paths.log_dir = v;
            }
            if let Some(v) = p.data_dir {
                self.paths.data_dir = v;
            }
            if let Some(v) = p.db_path {
                self.paths.db_path = v;
            }
            if let Some(v) = p.state_file {
                self.paths.state_file = v;
            }
            if let Some(v) = p.error_log {
                self.paths.error_log = v;
            }
        }
        if let Some(s) = partial.server {
            if let Some(v) = s.host {
                self.server.host = v;
            }
            if let Some(v) = s.port {
                self.server.port = v;
            }
            if let Some(v) = s.days_back {
                self.server.days_back = v;
            }
            if let Some(v) = s.enable_cors {
                self.server.enable_cors = v;
            }
            if let Some(v) = s.allowed_origins {
                self.server.allowed_origins = v;
            }
            if let Some(v) = s.default_page_limit {
                self.server.default_page_limit = v;
            }
            if let Some(v) = s.max_page_limit {
                self.server.max_page_limit = v;
            }
            if let Some(v) = s.max_export_rows {
                self.server.max_export_rows = v;
            }
        }
        if let Some(c) = partial.collector {
            if let Some(v) = c.batch_size {
                self.collector.batch_size = v;
            }
            if let Some(v) = c.retention_days {
                self.collector.retention_days = v;
            }
            if let Some(v) = c.pending_queue_per_client {
                self.collector.pending_queue_per_client = v;
            }
            if let Some(v) = c.collect_challenges {
                self.collector.collect_challenges = v;
            }
            if let Some(v) = c.max_log_mb {
                self.collector.max_log_mb = v;
            }
            if let Some(v) = c.poll_interval_seconds {
                self.collector.poll_interval_seconds = v;
            }
            if let Some(v) = c.pending_ttl_seconds {
                self.collector.pending_ttl_seconds = v;
            }
        }
    }

    pub fn is_cors_origin_allowed(&self, origin: Option<&str>) -> bool {
        if !self.server.enable_cors {
            return false;
        }
        let Some(orig) = origin else {
            return false;
        };
        let orig = orig.trim();
        if orig.is_empty() {
            return false;
        }
        self.server.allowed_origins.iter().any(|a| a == orig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_backward_compatible() {
        let cfg = EduroamConfig::default();
        assert_eq!(cfg.paths.log_dir, DEFAULT_LOG_DIR);
        assert_eq!(cfg.paths.db_path, r"C:\EduroamLogs\eduroam_logs.db");
        assert_eq!(cfg.server.host, "127.0.0.1");
        assert_eq!(cfg.server.max_page_limit, 1000);
        assert_eq!(cfg.collector.retention_days, 365);
        assert!(!cfg.collector.collect_challenges);
    }

    #[test]
    fn test_load_uses_generic_default_when_no_config_exists() {
        let dir = std::env::temp_dir().join(format!("eduroam_cfg_none_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let candidates = vec![
            dir.join("missing1.json"),
            dir.join("missing2.json"),
            dir.join("missing3.json"),
        ];
        let (cfg, src) = EduroamConfig::load_from_candidates(&candidates);
        assert!(src.is_none());
        assert_eq!(cfg.paths.log_dir, DEFAULT_LOG_DIR);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_partial_json_overrides_log_dir_only() {
        let dir = std::env::temp_dir().join(format!("eduroam_cfg_part_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg_file = dir.join("eduroam_config.json");
        fs::write(&cfg_file, r#"{ "paths": { "logDir": "D:\\CustomNPS" } }"#).unwrap();

        let (cfg, src) = EduroamConfig::load_from_candidates(std::slice::from_ref(&cfg_file));
        assert_eq!(src.as_deref(), Some(cfg_file.as_path()));
        assert_eq!(cfg.paths.log_dir, r"D:\CustomNPS");
        assert_eq!(cfg.server.host, "127.0.0.1");
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.collector.retention_days, 365);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_respects_candidate_search_order() {
        let dir = std::env::temp_dir().join(format!("eduroam_cfg_order_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.json");
        let second = dir.join("second.json");
        let third = dir.join("third.json");

        fs::write(&second, r#"{ "paths": { "logDir": "D:\\Second" } }"#).unwrap();
        fs::write(&third, r#"{ "paths": { "logDir": "D:\\Third" } }"#).unwrap();

        // first does not exist -> second wins over third
        let candidates = vec![first.clone(), second.clone(), third.clone()];
        let (cfg1, src1) = EduroamConfig::load_from_candidates(&candidates);
        assert_eq!(src1.as_deref(), Some(second.as_path()));
        assert_eq!(cfg1.paths.log_dir, r"D:\Second");

        // once first exists -> first wins over second
        fs::write(&first, r#"{ "paths": { "logDir": "D:\\First" } }"#).unwrap();
        let (cfg2, src2) = EduroamConfig::load_from_candidates(&candidates);
        assert_eq!(src2.as_deref(), Some(first.as_path()));
        assert_eq!(cfg2.paths.log_dir, r"D:\First");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_setup_config_generation_escaping_and_overwrite_guards() {
        let dir = std::env::temp_dir().join(format!("eduroam_setup_cfg_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let logs_dir = dir.join("NPSLogs");
        let missing_dir = dir.join("MissingLogs");
        let nested_cfg = dir.join("nested").join("eduroam_config.json");

        // Fails if --log-dir does not exist
        let err_missing = write_setup_config(
            &missing_dir.to_string_lossy(),
            Some(&nested_cfg),
            false,
            false,
        )
        .unwrap_err();
        assert!(err_missing.contains("bulunamadi"));
        assert!(!nested_cfg.exists());

        // Succeeds when log dir exists, creates parent dirs, escapes Windows backslashes
        fs::create_dir_all(&logs_dir).unwrap();
        let written =
            write_setup_config(&logs_dir.to_string_lossy(), Some(&nested_cfg), false, false)
                .unwrap();
        assert_eq!(written, nested_cfg);
        let raw_json = fs::read_to_string(&nested_cfg).unwrap();
        assert!(raw_json.contains(r"\\NPSLogs"));
        let (loaded, _) = EduroamConfig::load_from_candidates(std::slice::from_ref(&nested_cfg));
        assert_eq!(loaded.paths.log_dir, logs_dir.to_string_lossy());

        // Refuses to overwrite without --force
        let err_exists =
            write_setup_config(&logs_dir.to_string_lossy(), Some(&nested_cfg), false, false)
                .unwrap_err();
        assert!(err_exists.contains("--force"));

        // Overwrites when --force is passed (and --allow-missing works for missing dir)
        let alt_path = r"E:\Archive\NPS\Logs";
        write_setup_config(alt_path, Some(&nested_cfg), true, true).unwrap();
        let raw_updated = fs::read_to_string(&nested_cfg).unwrap();
        assert!(raw_updated.contains(r"E:\\Archive\\NPS\\Logs"));
        let (reloaded, _) = EduroamConfig::load_from_candidates(std::slice::from_ref(&nested_cfg));
        assert_eq!(reloaded.paths.log_dir, alt_path);

        let _ = fs::remove_dir_all(&dir);
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
}
