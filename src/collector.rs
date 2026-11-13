use crate::config::RadiusConfig;
use crate::db::{ensure_schema_and_migrate, open_db};
use crate::parser::{
    compute_event_hash, get_reason_text, parse_ias_event_line, parse_ias_timestamp, split_username,
};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CollectorState {
    pub file: String,
    pub timestamp: String,
    #[serde(default)]
    pub byte_offset: u64,
    #[serde(default)]
    pub line_no: usize,
}

#[derive(Debug, Clone)]
struct PendingAuthRequest {
    timestamp: NaiveDateTime,
    full_username: String,
    username: String,
    realm: String,
    nas_ip_address: String,
    nas_identifier: String,
    called_station_id: String,
    calling_station_id: String,
    client_ip: String,
    auth_type: String,
    policy_name: String,
    class_attr: String,
}

#[derive(Debug, Clone)]
struct CompletedAuthRecord {
    packet_type: i32,
    timestamp_str: String,
    full_username: String,
    username: String,
    realm: String,
    client_ip: String,
    nas_identifier: String,
    nas_ip_address: String,
    called_station_id: String,
    calling_station_id: String,
    auth_type: String,
    policy_name: String,
    reason_code: i32,
    reason: String,
    result: &'static str,
    source_file: String,
    source_line: usize,
    event_hash: String,
}

const INSERT_AUTH_SQL: &str = "
INSERT OR IGNORE INTO AuthLog
    (EventID, TimeCreated, FullUsername, Username, Realm, ClientIP, NASIdentifier,
     NASIPAddress, CalledStationID, CallingStationID, AuthType, PolicyName, ReasonCode, Reason, Result,
     SourceFile, SourceLine, EventHash)
VALUES
    (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
";

/// Chronologically-safe upsert for `Devices`:
/// - `FirstSeen` always retains the earliest timestamp.
/// - `LastSeen`, `LastUsername`, `LastResult` only update when the incoming record is at least as recent as `Devices.LastSeen`.
const UPSERT_DEVICE_SQL: &str = "
INSERT INTO Devices (MACAddress, FirstSeen, LastSeen, LastUsername, LastResult)
VALUES (?1, ?2, ?2, ?3, ?4)
ON CONFLICT(MACAddress) DO UPDATE SET
    FirstSeen = CASE WHEN excluded.FirstSeen < Devices.FirstSeen THEN excluded.FirstSeen ELSE Devices.FirstSeen END,
    LastSeen = CASE WHEN excluded.LastSeen >= Devices.LastSeen THEN excluded.LastSeen ELSE Devices.LastSeen END,
    LastUsername = CASE WHEN excluded.LastSeen >= Devices.LastSeen THEN excluded.LastUsername ELSE Devices.LastUsername END,
    LastResult = CASE WHEN excluded.LastSeen >= Devices.LastSeen THEN excluded.LastResult ELSE Devices.LastResult END
";

pub fn write_collector_log(cfg: &RadiusConfig, level: &str, msg: &str) {
    let log_path = Path::new(&cfg.paths.error_log);
    if let Some(dir) = log_path.parent() {
        if !dir.as_os_str().is_empty() && !dir.exists() {
            let _ = fs::create_dir_all(dir);
        }
    }
    let max_bytes = cfg.collector.max_log_mb * 1024 * 1024;
    if let Ok(meta) = fs::metadata(log_path) {
        if meta.len() > max_bytes {
            let old_path = log_path.with_extension("old.log");
            let _ = fs::remove_file(&old_path);
            let _ = fs::rename(log_path, &old_path);
        }
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(file, "[{}][{}] {}", now, level, msg);
    }
}

pub fn load_state(state_path: &Path) -> (String, NaiveDateTime, u64, usize) {
    if state_path.exists() {
        if let Ok(raw) = fs::read_to_string(state_path) {
            let trimmed = raw.trim_start_matches('\u{feff}').trim();
            if let Ok(st) = serde_json::from_str::<CollectorState>(trimmed) {
                let ts = parse_state_timestamp(&st.timestamp)
                    .unwrap_or_else(|| Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap());
                return (st.file, ts, st.byte_offset, st.line_no);
            }
        }
    }
    let today = Local::now().date_naive();
    let default_file = format!("IN{}.log", today.format("%y%m%d"));
    let default_ts = today.and_hms_opt(0, 0, 0).unwrap();
    (default_file, default_ts, 0, 0)
}

fn parse_state_timestamp(s: &str) -> Option<NaiveDateTime> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.naive_local());
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(ndt);
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(ndt);
    }
    None
}

pub fn save_state_atomic(
    state_path: &Path,
    file: &str,
    ts: NaiveDateTime,
    byte_offset: u64,
    line_no: usize,
) -> std::io::Result<()> {
    if let Some(dir) = state_path.parent() {
        if !dir.as_os_str().is_empty() && !dir.exists() {
            fs::create_dir_all(dir)?;
        }
    }
    let tmp_path = state_path.with_extension("txt.tmp");
    let state = CollectorState {
        file: file.to_string(),
        timestamp: ts.format("%Y-%m-%dT%H:%M:%S").to_string(),
        byte_offset,
        line_no,
    };
    let json = serde_json::to_string_pretty(&state)?;
    fs::write(&tmp_path, json.as_bytes())?;
    fs::rename(&tmp_path, state_path)?;
    Ok(())
}

pub fn extract_file_date(file_name: &str) -> Option<NaiveDate> {
    let lower = file_name.to_ascii_uppercase();
    if lower.starts_with("IN") && lower.ends_with(".LOG") && lower.len() == 12 {
        let date_part = &lower[2..8];
        if date_part.bytes().all(|b| b.is_ascii_digit()) {
            return NaiveDate::parse_from_str(date_part, "%y%m%d").ok();
        }
    }
    None
}

fn get_log_files_after(log_dir: &Path, after_file: &str) -> Vec<PathBuf> {
    let after_date = extract_file_date(after_file)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2000, 1, 1).unwrap());

    let Ok(entries) = fs::read_dir(log_dir) else {
        return Vec::new();
    };

    let mut files: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let file_date = extract_file_date(&name)?;
            if file_date >= after_date {
                Some((name.to_ascii_uppercase(), e.path()))
            } else {
                None
            }
        })
        .collect();

    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.into_iter().map(|(_, p)| p).collect()
}

#[cfg(windows)]
fn open_shared_log_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ (1) | FILE_SHARE_WRITE (2) | FILE_SHARE_DELETE (4)
    OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
        .open(path)
}

#[cfg(not(windows))]
fn open_shared_log_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

/// Evicts expired `PT=1` entries (older than `ttl_secs`) and returns the best matching request:
/// 1. Matches by `Class` attribute if present on both `PT=1` and `PT=2/3`.
/// 2. Matches by `Calling-Station-Id` (MAC) if present on both.
/// 3. Falls back to FIFO oldest non-expired entry for that `Client-IP`.
fn match_pending_request(
    queue: &mut VecDeque<PendingAuthRequest>,
    current_ts: NaiveDateTime,
    ttl_secs: i64,
    class_attr: &str,
    calling_station_id: &str,
) -> Option<PendingAuthRequest> {
    // 1. Evict stale PT=1 requests that timed out without an Accept/Reject
    while let Some(front) = queue.front() {
        let age = (current_ts - front.timestamp).num_seconds();
        if age > ttl_secs {
            queue.pop_front();
        } else {
            break;
        }
    }

    if queue.is_empty() {
        return None;
    }

    // 2. Priority match by RADIUS Class attribute if available
    if !class_attr.is_empty() {
        if let Some(idx) = queue
            .iter()
            .position(|r| !r.class_attr.is_empty() && r.class_attr == class_attr)
        {
            return queue.remove(idx);
        }
    }

    // 3. Secondary match by Calling-Station-Id (MAC) if available in PT=2/3
    if !calling_station_id.is_empty() {
        if let Some(idx) = queue.iter().position(|r| {
            !r.calling_station_id.is_empty()
                && r.calling_station_id
                    .eq_ignore_ascii_case(calling_station_id)
        }) {
            return queue.remove(idx);
        }
    }

    // 4. Fallback to FIFO oldest live entry for this Client-IP
    queue.pop_front()
}

/// Runs a collection pass across all IAS log files from `last_run.txt` forward.
/// Returns the total number of newly inserted records.
pub fn run_collector_pass(
    cfg: &RadiusConfig,
    db_generation: Option<&AtomicU64>,
    verbose: bool,
) -> Result<usize, String> {
    let db_path = Path::new(&cfg.paths.db_path);
    let state_path = Path::new(&cfg.paths.state_file);
    let log_dir = Path::new(&cfg.paths.log_dir);

    let (since_file, since_ts, saved_offset, saved_line_no) = load_state(state_path);
    if verbose {
        println!(
            "[Kolektor] Baslangic: {} | {} (offset: {} B, satir: {})",
            since_file,
            since_ts.format("%Y-%m-%d %H:%M:%S"),
            saved_offset,
            saved_line_no
        );
    }

    let files = get_log_files_after(log_dir, &since_file);
    if files.is_empty() {
        if verbose {
            println!("[Kolektor] Islenecek dosya yok.");
        }
        return Ok(0);
    }

    let mut conn = open_db(db_path).map_err(|e| format!("DB acilamadi: {}", e))?;
    ensure_schema_and_migrate(&conn).map_err(|e| format!("DB migration hatasi: {}", e))?;

    // Retention cleanup
    let cutoff_arg = format!("-{} days", cfg.collector.retention_days);
    if let Ok(pruned) = conn.execute(
        "DELETE FROM AuthLog WHERE TimeCreated < date('now', ?1)",
        params![cutoff_arg],
    ) {
        if pruned > 0 {
            write_collector_log(
                cfg,
                "INFO",
                &format!(
                    "{} gunden eski {} kayit silindi.",
                    cfg.collector.retention_days, pruned
                ),
            );
        }
    }

    // Keep PT=1 cache across files so midnight cross-file sessions (23:59:59 -> 00:00:00) match seamlessly!
    let mut pt1_cache: HashMap<String, VecDeque<PendingAuthRequest>> = HashMap::new();
    let max_queue = cfg.collector.pending_queue_per_client.max(1);
    let ttl_secs = cfg.collector.pending_ttl_seconds.max(5);
    let batch_size = cfg.collector.batch_size.max(100);
    let collect_challenges = cfg.collector.collect_challenges;

    let mut total_inserted = 0usize;
    let mut last_ts = since_ts;
    let mut last_file = since_file.clone();
    let mut last_offset = saved_offset;
    let mut last_line_no = saved_line_no;

    for file_path in files {
        let file_name = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let file_len = fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);

        let is_same_as_state_file = file_name.eq_ignore_ascii_case(&since_file);
        if is_same_as_state_file && saved_offset > 0 && file_len == saved_offset {
            // Nothing new appended to this file since last run
            last_file = file_name;
            continue;
        }

        let sw = Instant::now();
        if verbose {
            println!(
                "[Kolektor] {} ({:.1} MB)...",
                file_name,
                file_len as f64 / (1024.0 * 1024.0)
            );
        }

        let mut file = match open_shared_log_file(&file_path) {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("Dosya acilamadi {}: {}", file_name, e);
                write_collector_log(cfg, "ERROR", &msg);
                if verbose {
                    eprintln!("[HATA] {}", msg);
                }
                continue;
            }
        };

        let mut line_no = 0usize;
        let mut used_byte_seek = false;

        // If we have a valid byte offset for the current active file, seek directly to it!
        if is_same_as_state_file
            && saved_offset > 0
            && file_len > saved_offset
            && file.seek(SeekFrom::Start(saved_offset)).is_ok()
        {
            line_no = saved_line_no;
            used_byte_seek = true;
        }

        let mut reader = BufReader::with_capacity(256 * 1024, file);
        let mut raw_line = String::with_capacity(2048);
        let mut current_offset = if used_byte_seek { saved_offset } else { 0 };
        let mut pending_batch: Vec<CompletedAuthRecord> = Vec::with_capacity(batch_size);
        let mut file_inserted = 0usize;

        loop {
            raw_line.clear();
            let bytes_read = match reader.read_line(&mut raw_line) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    write_collector_log(
                        cfg,
                        "ERROR",
                        &format!("Okuma hatasi {}: {}", file_name, e),
                    );
                    break;
                }
            };

            // If NPS is mid-write and the last line doesn't end with '\n' yet, stop before this incomplete line
            // so the next pass reads the complete line from `current_offset`.
            if !raw_line.ends_with('\n') && file_path == *log_dir.join(&file_name) {
                // Only treat as partial if it doesn't contain closing `</Event>`
                let trimmed_check = raw_line.trim_end();
                if !trimmed_check.ends_with("</Event>") {
                    break;
                }
            }

            current_offset += bytes_read as u64;
            line_no += 1;

            let line = raw_line.trim_end_matches(['\r', '\n']);
            if !line.starts_with("<Event>") {
                continue;
            }

            let fields = parse_ias_event_line(line);
            let pt = fields.packet_type;
            let wanted = matches!(pt, "1" | "2" | "3") || (collect_challenges && pt == "11");
            if !wanted {
                continue;
            }

            let Some(ts) = parse_ias_timestamp(fields.timestamp) else {
                continue;
            };

            let cache_key = if !fields.client_ip.is_empty() {
                fields.client_ip
            } else if !fields.nas_ip.is_empty() {
                fields.nas_ip
            } else {
                continue;
            };

            let full_un = fields.user_name.trim();

            if pt == "1" {
                // Even if `ts < since_ts` when not using byte seek, still cache PT=1 within 60s of `since_ts`
                // so a PT=2 right after `since_ts` doesn't miss its PT=1!
                if !used_byte_seek
                    && is_same_as_state_file
                    && (since_ts - ts).num_seconds() > ttl_secs
                {
                    continue;
                }
                if full_un.is_empty() {
                    continue;
                }
                let (username, realm) = split_username(full_un);
                let nas_ip = if !fields.nas_ip.is_empty() {
                    fields.nas_ip
                } else {
                    fields.client_ip
                };

                let req = PendingAuthRequest {
                    timestamp: ts,
                    full_username: full_un.to_string(),
                    username,
                    realm,
                    nas_ip_address: nas_ip.to_string(),
                    nas_identifier: fields.nas_identifier.to_string(),
                    called_station_id: fields.called_station_id.to_string(),
                    calling_station_id: fields.calling_station_id.to_string(),
                    client_ip: fields.client_ip.to_string(),
                    auth_type: fields.provider_name.to_string(),
                    policy_name: fields.proxy_policy_name.to_string(),
                    class_attr: fields.class_attr.to_string(),
                };

                let q = pt1_cache
                    .entry(cache_key.to_string())
                    .or_insert_with(|| VecDeque::with_capacity(max_queue));
                if q.len() >= max_queue {
                    q.pop_front();
                }
                q.push_back(req);
                continue;
            }

            // For PT=2 / PT=3, skip if already processed before `since_ts` (when not seeking by byte offset)
            if !used_byte_seek && is_same_as_state_file && ts < since_ts {
                continue;
            }

            match pt {
                "2" => {
                    let Some(q) = pt1_cache.get_mut(cache_key) else {
                        continue;
                    };
                    let Some(matched) = match_pending_request(
                        q,
                        ts,
                        ttl_secs,
                        fields.class_attr,
                        fields.calling_station_id,
                    ) else {
                        continue;
                    };

                    if ts > last_ts {
                        last_ts = ts;
                    }
                    let event_hash = compute_event_hash(&file_name, line_no, line);
                    pending_batch.push(CompletedAuthRecord {
                        packet_type: 2,
                        timestamp_str: ts.format("%Y-%m-%d %H:%M:%S").to_string(),
                        full_username: matched.full_username,
                        username: matched.username,
                        realm: matched.realm,
                        client_ip: matched.client_ip,
                        nas_identifier: matched.nas_identifier,
                        nas_ip_address: matched.nas_ip_address,
                        called_station_id: matched.called_station_id,
                        calling_station_id: matched.calling_station_id,
                        auth_type: matched.auth_type,
                        policy_name: matched.policy_name,
                        reason_code: 0,
                        reason: get_reason_text(0, "0"),
                        result: "SUCCESS",
                        source_file: file_name.clone(),
                        source_line: line_no,
                        event_hash,
                    });
                }
                "3" => {
                    let rc_str = if !fields.reason_code.is_empty() {
                        fields.reason_code
                    } else {
                        "0"
                    };
                    let rc_num = rc_str.parse::<i32>().unwrap_or(0);
                    let reason_text = get_reason_text(rc_num, rc_str);

                    let record_opt = if !full_un.is_empty() {
                        let (username, realm) = split_username(full_un);
                        let nas_ip = if !fields.nas_ip.is_empty() {
                            fields.nas_ip
                        } else {
                            fields.client_ip
                        };
                        let event_hash = compute_event_hash(&file_name, line_no, line);
                        Some(CompletedAuthRecord {
                            packet_type: 3,
                            timestamp_str: ts.format("%Y-%m-%d %H:%M:%S").to_string(),
                            full_username: full_un.to_string(),
                            username,
                            realm,
                            client_ip: fields.client_ip.to_string(),
                            nas_identifier: fields.nas_identifier.to_string(),
                            nas_ip_address: nas_ip.to_string(),
                            called_station_id: fields.called_station_id.to_string(),
                            calling_station_id: fields.calling_station_id.to_string(),
                            auth_type: fields.provider_name.to_string(),
                            policy_name: fields.proxy_policy_name.to_string(),
                            reason_code: rc_num,
                            reason: reason_text,
                            result: "FAILURE",
                            source_file: file_name.clone(),
                            source_line: line_no,
                            event_hash,
                        })
                    } else if let Some(q) = pt1_cache.get_mut(cache_key) {
                        match_pending_request(
                            q,
                            ts,
                            ttl_secs,
                            fields.class_attr,
                            fields.calling_station_id,
                        )
                        .map(|matched| {
                            let event_hash = compute_event_hash(&file_name, line_no, line);
                            CompletedAuthRecord {
                                packet_type: 3,
                                timestamp_str: ts.format("%Y-%m-%d %H:%M:%S").to_string(),
                                full_username: matched.full_username,
                                username: matched.username,
                                realm: matched.realm,
                                client_ip: matched.client_ip,
                                nas_identifier: matched.nas_identifier,
                                nas_ip_address: matched.nas_ip_address,
                                called_station_id: matched.called_station_id,
                                calling_station_id: matched.calling_station_id,
                                auth_type: matched.auth_type,
                                policy_name: matched.policy_name,
                                reason_code: rc_num,
                                reason: reason_text,
                                result: "FAILURE",
                                source_file: file_name.clone(),
                                source_line: line_no,
                                event_hash,
                            }
                        })
                    } else {
                        None
                    };

                    if let Some(rec) = record_opt {
                        if ts > last_ts {
                            last_ts = ts;
                        }
                        pending_batch.push(rec);
                    }
                }
                _ => {}
            }

            if pending_batch.len() >= batch_size {
                let inserted = flush_batch(&mut conn, &pending_batch, cfg)?;
                file_inserted += inserted;
                total_inserted += inserted;
                pending_batch.clear();
            }
        }

        if !pending_batch.is_empty() {
            let inserted = flush_batch(&mut conn, &pending_batch, cfg)?;
            file_inserted += inserted;
            total_inserted += inserted;
            pending_batch.clear();
        }

        last_file = file_name.clone();
        last_offset = current_offset;
        last_line_no = line_no;

        let elapsed = sw.elapsed().as_secs_f64();
        let rps = if elapsed > 0.0 {
            (file_inserted as f64 / elapsed).round() as usize
        } else {
            file_inserted
        };
        if file_inserted > 0 || verbose {
            write_collector_log(
                cfg,
                "INFO",
                &format!(
                    "{}: {} yeni kayit ({:.2} sn, {}/sn).",
                    file_name, file_inserted, elapsed, rps
                ),
            );
        }
        if verbose {
            println!(
                "  Tamam: {} yeni kayit | {:.2} sn | {}/sn",
                file_inserted, elapsed, rps
            );
        }
    }

    let _ = save_state_atomic(state_path, &last_file, last_ts, last_offset, last_line_no);

    if total_inserted > 0 {
        if let Some(gen) = db_generation {
            gen.fetch_add(1, Ordering::Release);
        }
    }

    if verbose {
        println!(
            "[Kolektor] TAMAMLANDI. Toplam {} yeni kayit.",
            total_inserted
        );
    }

    Ok(total_inserted)
}

fn flush_batch(
    conn: &mut rusqlite::Connection,
    batch: &[CompletedAuthRecord],
    cfg: &RadiusConfig,
) -> Result<usize, String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("Transaction baslatilamadi: {}", e))?;
    let mut inserted_count = 0usize;

    {
        let mut auth_stmt = tx
            .prepare_cached(INSERT_AUTH_SQL)
            .map_err(|e| format!("Auth SQL hazirlanamadi: {}", e))?;
        let mut dev_stmt = tx
            .prepare_cached(UPSERT_DEVICE_SQL)
            .map_err(|e| format!("Device SQL hazirlanamadi: {}", e))?;

        for rec in batch {
            match auth_stmt.execute(params![
                rec.packet_type,
                rec.timestamp_str,
                rec.full_username,
                rec.username,
                rec.realm,
                rec.client_ip,
                rec.nas_identifier,
                rec.nas_ip_address,
                rec.called_station_id,
                rec.calling_station_id,
                rec.auth_type,
                rec.policy_name,
                rec.reason_code,
                rec.reason,
                rec.result,
                rec.source_file,
                rec.source_line as i64,
                rec.event_hash,
            ]) {
                Ok(affected) if affected > 0 => {
                    inserted_count += 1;
                    let mac = rec.calling_station_id.trim().to_ascii_uppercase();
                    if !mac.is_empty() {
                        let _ = dev_stmt.execute(params![
                            mac,
                            rec.timestamp_str,
                            rec.username,
                            rec.result,
                        ]);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    write_collector_log(cfg, "ERROR", &format!("INSERT: {}", e));
                }
            }
        }
    }

    tx.commit()
        .map_err(|e| format!("Transaction commit hatasi: {}", e))?;
    Ok(inserted_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(sec: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 7)
            .unwrap()
            .and_hms_opt(23, 52, sec)
            .unwrap()
    }

    fn req(sec: u32, user: &str, mac: &str, class: &str) -> PendingAuthRequest {
        PendingAuthRequest {
            timestamp: ts(sec),
            full_username: user.to_string(),
            username: user.to_string(),
            realm: String::new(),
            nas_ip_address: String::new(),
            nas_identifier: String::new(),
            called_station_id: String::new(),
            calling_station_id: mac.to_string(),
            client_ip: "10.1.1.113".to_string(),
            auth_type: String::new(),
            policy_name: String::new(),
            class_attr: class.to_string(),
        }
    }

    #[test]
    fn class_attribute_wins_over_fifo_order() {
        let mut q: VecDeque<_> = vec![req(1, "a", "m1", "C1"), req(2, "b", "m2", "C2")].into();
        let got = match_pending_request(&mut q, ts(3), 60, "C2", "").unwrap();
        assert_eq!(got.username, "b");
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn mac_match_is_case_insensitive_fallback() {
        let mut q: VecDeque<_> = vec![req(1, "a", "AA-BB", ""), req(2, "b", "CC-DD", "")].into();
        let got = match_pending_request(&mut q, ts(3), 60, "", "cc-dd").unwrap();
        assert_eq!(got.username, "b");
    }

    #[test]
    fn falls_back_to_oldest_when_no_keys_match() {
        let mut q: VecDeque<_> = vec![req(1, "a", "", ""), req(2, "b", "", "")].into();
        let got = match_pending_request(&mut q, ts(3), 60, "X", "zz").unwrap();
        assert_eq!(got.username, "a");
    }

    #[test]
    fn expired_requests_are_evicted_by_ttl() {
        let mut q: VecDeque<_> = vec![req(1, "a", "", "C1")].into();
        let late = ts(1) + chrono::Duration::seconds(120);
        assert!(match_pending_request(&mut q, late, 60, "C1", "").is_none());
        assert!(q.is_empty());
    }

    #[test]
    fn empty_class_never_matches_empty_class() {
        let mut q: VecDeque<_> = vec![req(1, "a", "m1", ""), req(2, "b", "m2", "")].into();
        let got = match_pending_request(&mut q, ts(3), 60, "", "m2").unwrap();
        assert_eq!(got.username, "b");
    }

    #[test]
    fn log_rotation_moves_oversized_file_to_old_log() {
        let dir = std::env::temp_dir().join(format!("radius_log_rot_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("collector_errors.log");
        let old_path = dir.join("collector_errors.old.log");

        let mut cfg = RadiusConfig::default();
        cfg.paths.error_log = log_path.to_string_lossy().to_string();
        cfg.collector.max_log_mb = 1;

        fs::write(&log_path, vec![b'x'; 1024 * 1024 + 64]).unwrap();
        write_collector_log(&cfg, "INFO", "rotated ok");

        assert!(old_path.exists(), ".old.log olusmali");
        let new_content = fs::read_to_string(&log_path).unwrap();
        assert!(new_content.contains("rotated ok"));
        assert!(new_content.len() < 1024);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_end_to_end_collector_and_byte_offset_resume() {
        let tmp_dir = std::env::temp_dir().join(format!("radius_rust_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let log_file = tmp_dir.join("IN260608.log");
        let db_file = tmp_dir.join("test.db");
        let state_file = tmp_dir.join("last_run.txt");
        let err_file = tmp_dir.join("errors.log");

        let mut cfg = RadiusConfig::default();
        cfg.paths.log_dir = tmp_dir.to_string_lossy().to_string();
        cfg.paths.data_dir = tmp_dir.to_string_lossy().to_string();
        cfg.paths.db_path = db_file.to_string_lossy().to_string();
        cfg.paths.state_file = state_file.to_string_lossy().to_string();
        cfg.paths.error_log = err_file.to_string_lossy().to_string();
        cfg.collector.retention_days = 36500;

        let initial_lines = concat!(
            "<Event><Timestamp>06/08/2026 10:00:01.000</Timestamp><Packet-Type>1</Packet-Type><User-Name>user1@kurum.edu.tr</User-Name><Client-IP-Address>10.1.1.1</Client-IP-Address><Calling-Station-Id>aa-bb-cc-dd-ee-01</Calling-Station-Id><NAS-Identifier>AP-1</NAS-Identifier></Event>\r\n",
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
            "<Event><Timestamp>06/08/2026 10:05:00.000</Timestamp><Packet-Type>3</Packet-Type><User-Name>user2@kurum.edu.tr</User-Name><Client-IP-Address>10.1.1.1</Client-IP-Address><Calling-Station-Id>aa-bb-cc-dd-ee-02</Calling-Station-Id><Reason-Code>16</Reason-Code></Event>"
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
    fn test_extract_file_date() {
        assert_eq!(
            extract_file_date("IN261106.log"),
            NaiveDate::from_ymd_opt(2026, 11, 6)
        );
        assert_eq!(
            extract_file_date("in261106.LOG"),
            NaiveDate::from_ymd_opt(2026, 11, 6)
        );
        assert_eq!(extract_file_date("IN261306.log"), None);
        assert_eq!(extract_file_date("IN26110.log"), None);
        assert_eq!(extract_file_date("notes.txt"), None);
    }

    #[test]
    fn test_state_roundtrip_and_atomic_tmp_cleanup() {
        let dir = std::env::temp_dir().join(format!("radius_state_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let state = dir.join("last_run.txt");
        let ts = NaiveDate::from_ymd_opt(2026, 11, 7)
            .unwrap()
            .and_hms_opt(23, 15, 14)
            .unwrap();
        save_state_atomic(&state, "IN261107.log", ts, 4096, 17).unwrap();
        let (file, got_ts, off, line) = load_state(&state);
        assert_eq!(
            (file.as_str(), got_ts, off, line),
            ("IN261107.log", ts, 4096, 17)
        );
        assert!(!state.with_extension("txt.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_retention_prunes_old_records_only() {
        let dir =
            std::env::temp_dir().join(format!("radius_retention_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db_file = dir.join("test.db");
        let state_file = dir.join("last_run.txt");

        let mut cfg = RadiusConfig::default();
        assert_eq!(cfg.collector.retention_days, 365);
        cfg.paths.log_dir = dir.to_string_lossy().to_string();
        cfg.paths.data_dir = dir.to_string_lossy().to_string();
        cfg.paths.db_path = db_file.to_string_lossy().to_string();
        cfg.paths.state_file = state_file.to_string_lossy().to_string();
        cfg.paths.error_log = dir.join("errors.log").to_string_lossy().to_string();
        cfg.collector.retention_days = 180;

        fs::write(
            dir.join("IN260608.log"),
            "<Event><Timestamp>06/08/2026 10:00:01.000</Timestamp><Packet-Type>3</Packet-Type><User-Name>u@kurum.edu.tr</User-Name><Client-IP-Address>10.1.1.1</Client-IP-Address><Reason-Code>16</Reason-Code></Event>\r\n",
        )
        .unwrap();
        let start = NaiveDate::from_ymd_opt(2026, 6, 8)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
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
            .query_row(
                "SELECT COUNT(*) FROM AuthLog WHERE EventHash='old-row'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let new: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM AuthLog WHERE EventHash='new-row'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old, 0, "180 gunden eski kayit silinmeli");
        assert_eq!(new, 1, "yeni kayit korunmali");
        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }
}
