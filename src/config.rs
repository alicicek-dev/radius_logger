use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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
            log_dir: r"C:\NPSLogs".to_string(),
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

impl EduroamConfig {
    pub fn load(custom_path: Option<&Path>) -> Self {
        let mut cfg = EduroamConfig::default();

        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = custom_path {
            candidates.push(p.to_path_buf());
        }
        candidates.push(PathBuf::from(r"C:\EduroamLogs\eduroam_config.json"));
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("eduroam_config.json"));
            }
        }
        candidates.push(PathBuf::from("eduroam_config.json"));

        for candidate in candidates {
            if candidate.exists() {
                if let Ok(raw) = fs::read_to_string(&candidate) {
                    let trimmed = raw.trim_start_matches('\u{feff}').trim();
                    if !trimmed.is_empty() {
                        if let Ok(partial) = serde_json::from_str::<PartialConfig>(trimmed) {
                            cfg.apply_partial(partial);
                        }
                    }
                }
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

        cfg
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
