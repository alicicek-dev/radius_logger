use crate::config::EduroamConfig;
use crate::db::open_db;
use crate::parser::{format_csv_field, to_sql_like_pattern};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use chrono::{Duration, Local};
use rusqlite::{params_from_iter, types::Value as SqlValue};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};
use std::time::{Instant, SystemTime};

const EMBEDDED_HTML: &str = include_str!("../eduroam_dashboard.html");
const EMBEDDED_CSS: &str = include_str!("../eduroam_dashboard.css");

pub struct AppState {
    pub config: EduroamConfig,
    pub db_generation: Arc<AtomicU64>,
    pub charts_cache: RwLock<Option<CachedCharts>>,
}

pub struct CachedCharts {
    pub generation: u64,
    pub days_back: i64,
    pub today_str: String,
    pub db_mtime: Option<SystemTime>,
    pub created_at: Instant,
    pub json_bytes: Vec<u8>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub supports_challenge: bool,
    pub max_page_limit: usize,
    pub max_export_rows: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SummaryRow {
    result: String,
    cnt: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct TrendRow {
    gun: String,
    result: String,
    cnt: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct UserRealmCountRow {
    username: String,
    realm: String,
    cnt: i64,
}

#[derive(Serialize)]
struct NasRow {
    #[serde(rename = "NASIdentifier")]
    nas_identifier: String,
    #[serde(rename = "Total")]
    total: i64,
    #[serde(rename = "Suc")]
    suc: i64,
    #[serde(rename = "Fail")]
    fail: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct HourRow {
    saat: i64,
    cnt: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct RealmRow {
    realm: String,
    total: i64,
    succ_cnt: i64,
    fail_cnt: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ReasonRow {
    reason_code: i64,
    cnt: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChartsResponse {
    generated: String,
    days_back: i64,
    last_timestamp: String,
    capabilities: Capabilities,
    unique_users: i64,
    active_devices: i64,
    unique_devices: i64,
    summary: Vec<SummaryRow>,
    trend: Vec<TrendRow>,
    top_fail: Vec<UserRealmCountRow>,
    top_users: Vec<UserRealmCountRow>,
    #[serde(rename = "byNAS")]
    by_nas: Vec<NasRow>,
    by_hour: Vec<HourRow>,
    by_realm: Vec<RealmRow>,
    by_reason: Vec<ReasonRow>,
}

#[derive(Serialize, Clone)]
pub struct RecentLogRow {
    #[serde(rename = "LogID")]
    pub log_id: i64,
    #[serde(rename = "Zaman")]
    pub zaman: String,
    #[serde(rename = "Username")]
    pub username: String,
    #[serde(rename = "Realm")]
    pub realm: String,
    #[serde(rename = "ClientIP")]
    pub client_ip: String,
    #[serde(rename = "NASIdentifier")]
    pub nas_identifier: String,
    #[serde(rename = "CallingStationID")]
    pub calling_station_id: String,
    #[serde(rename = "Result")]
    pub result: String,
    #[serde(rename = "ReasonCode")]
    pub reason_code: i64,
    #[serde(rename = "Reason")]
    pub reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecentResponse {
    page: usize,
    limit: usize,
    total: usize,
    pages: usize,
    capabilities: Capabilities,
    data: Vec<RecentLogRow>,
}

#[derive(Debug)]
pub struct BuiltRecentQuery {
    pub page: usize,
    pub limit: usize,
    pub offset: usize,
    pub where_clause: String,
    pub params: Vec<SqlValue>,
    pub sort_column: &'static str,
    pub sort_direction: &'static str,
}

fn is_yyyy_mm_dd(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[7] == b'-'
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
}

pub fn build_recent_query(
    qs: &HashMap<String, String>,
    days_back: i64,
    default_limit: usize,
    max_limit: usize,
) -> BuiltRecentQuery {
    let page = qs
        .get("page")
        .and_then(|v| v.parse::<isize>().ok())
        .map(|p| if p < 1 { 1 } else { p as usize })
        .unwrap_or(1);

    let limit = qs
        .get("limit")
        .and_then(|v| v.parse::<isize>().ok())
        .map(|l| {
            if l < 1 {
                default_limit
            } else if (l as usize) > max_limit {
                max_limit
            } else {
                l as usize
            }
        })
        .unwrap_or(default_limit);

    let offset = (page - 1) * limit;

    let mut where_parts: Vec<String> = Vec::with_capacity(8);
    let mut params: Vec<SqlValue> = Vec::with_capacity(8);

    where_parts.push("Result IN ('SUCCESS','FAILURE')".to_string());

    let eff_days = qs
        .get("days")
        .and_then(|v| v.parse::<i64>().ok())
        .map(|d| d.clamp(1, 365))
        .unwrap_or(days_back);
    let default_since = (Local::now() - Duration::days(eff_days))
        .format("%Y-%m-%d")
        .to_string();
    let from_val = match qs.get("from") {
        Some(f) if is_yyyy_mm_dd(f) => f.clone(),
        _ => default_since,
    };
    params.push(SqlValue::Text(from_val));
    where_parts.push(format!("TimeCreated >= ?{}", params.len()));

    if let Some(to_val) = qs.get("to") {
        if is_yyyy_mm_dd(to_val) {
            params.push(SqlValue::Text(format!("{} 23:59:59", to_val)));
            where_parts.push(format!("TimeCreated <= ?{}", params.len()));
        }
    }

    if let Some(res) = qs.get("result") {
        if res == "SUCCESS" || res == "FAILURE" {
            params.push(SqlValue::Text(res.clone()));
            where_parts.push(format!("Result = ?{}", params.len()));
        }
    }

    for (key, col) in [
        ("username", "Username"),
        ("realm", "Realm"),
        ("nas", "NASIdentifier"),
        ("mac", "CallingStationID"),
        ("ip", "ClientIP"),
    ] {
        if let Some(val) = qs.get(key) {
            if !val.is_empty() {
                params.push(SqlValue::Text(to_sql_like_pattern(val)));
                where_parts.push(format!("{} LIKE ?{} ESCAPE '\\'", col, params.len()));
            }
        }
    }

    if let Some(rc) = qs.get("rc") {
        if !rc.is_empty() && rc.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(rc_int) = rc.parse::<i64>() {
                params.push(SqlValue::Integer(rc_int));
                where_parts.push(format!("ReasonCode = ?{}", params.len()));
            }
        }
    }

    let sort_column = match qs.get("sort").map(|s| s.as_str()) {
        Some("Zaman") => "TimeCreated",
        Some("Username") => "Username",
        Some("Realm") => "Realm",
        Some("ClientIP") => "ClientIP",
        Some("NASIdentifier") => "NASIdentifier",
        Some("CallingStationID") => "CallingStationID",
        Some("Result") => "Result",
        Some("ReasonCode") => "ReasonCode",
        _ => "TimeCreated",
    };

    let sort_direction = match qs.get("dir").map(|s| s.as_str()) {
        Some("asc") => "ASC",
        _ => "DESC",
    };

    BuiltRecentQuery {
        page,
        limit,
        offset,
        where_clause: where_parts.join(" AND "),
        params,
        sort_column,
        sort_direction,
    }
}

pub fn rows_to_csv(rows: &[RecentLogRow]) -> String {
    let headers = [
        "Zaman",
        "Username",
        "Realm",
        "ClientIP",
        "NASIdentifier",
        "CallingStationID",
        "Result",
        "ReasonCode",
        "Reason",
    ];
    let mut out = String::with_capacity((rows.len() + 1) * 120);
    let header_line: Vec<String> = headers.iter().map(|h| format_csv_field(h)).collect();
    out.push_str(&header_line.join(","));
    out.push_str("\r\n");

    for r in rows {
        let rc_str = r.reason_code.to_string();
        let fields = [
            format_csv_field(&r.zaman),
            format_csv_field(&r.username),
            format_csv_field(&r.realm),
            format_csv_field(&r.client_ip),
            format_csv_field(&r.nas_identifier),
            format_csv_field(&r.calling_station_id),
            format_csv_field(&r.result),
            format_csv_field(&rc_str),
            format_csv_field(&r.reason),
        ];
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(handle_index))
        .route("/style.css", get(handle_css))
        .route("/api/charts", get(handle_charts))
        .route("/api/recent", get(handle_recent))
        .route("/api/export.csv", get(handle_export_csv))
        .fallback(handle_not_found)
        .with_state(state)
}

fn build_response(
    state: &AppState,
    req_headers: &HeaderMap,
    status: StatusCode,
    content_type: &'static str,
    cache_control: Option<&'static str>,
    body: Vec<u8>,
) -> Response {
    let mut resp = Response::new(axum::body::Body::from(body));
    *resp.status_mut() = status;
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Some(cc) = cache_control {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
    }
    let origin = req_headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if state.config.is_cors_origin_allowed(origin) {
        if let Some(orig) = origin {
            if let Ok(hv) = HeaderValue::from_str(orig) {
                headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, hv);
                headers.insert(header::VARY, HeaderValue::from_static("Origin"));
            }
        }
    }
    resp
}

fn resolve_asset_path(data_dir: &str, filename: &str) -> Option<PathBuf> {
    let p1 = Path::new(data_dir).join(filename);
    if p1.exists() {
        return Some(p1);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p2 = dir.join(filename);
            if p2.exists() {
                return Some(p2);
            }
        }
    }
    let p3 = PathBuf::from(filename);
    if p3.exists() {
        return Some(p3);
    }
    None
}

async fn handle_index(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let bytes = if let Some(path) =
        resolve_asset_path(&state.config.paths.data_dir, "eduroam_dashboard.html")
    {
        fs::read(path).unwrap_or_else(|_| EMBEDDED_HTML.as_bytes().to_vec())
    } else {
        EMBEDDED_HTML.as_bytes().to_vec()
    };
    build_response(
        &state,
        &headers,
        StatusCode::OK,
        "text/html; charset=utf-8",
        None,
        bytes,
    )
}

async fn handle_css(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let bytes = if let Some(path) =
        resolve_asset_path(&state.config.paths.data_dir, "eduroam_dashboard.css")
    {
        fs::read(path).unwrap_or_else(|_| EMBEDDED_CSS.as_bytes().to_vec())
    } else {
        EMBEDDED_CSS.as_bytes().to_vec()
    };
    build_response(
        &state,
        &headers,
        StatusCode::OK,
        "text/css; charset=utf-8",
        Some("max-age=300"),
        bytes,
    )
}

fn get_db_mtime(db_path: &str) -> Option<SystemTime> {
    let wal_path = format!("{}-wal", db_path);
    if let Ok(meta) = fs::metadata(&wal_path) {
        if meta.len() > 0 {
            if let Ok(m) = meta.modified() {
                return Some(m);
            }
        }
    }
    fs::metadata(db_path).and_then(|m| m.modified()).ok()
}

async fn handle_charts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(qs): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let now = Local::now();
    let today_str = now.format("%Y-%m-%d").to_string();
    let cur_gen = state.db_generation.load(Ordering::Acquire);
    let cur_mtime = get_db_mtime(&state.config.paths.db_path);
    let req_days = qs
        .get("days")
        .and_then(|v| v.parse::<i64>().ok())
        .map(|d| d.clamp(1, 365))
        .unwrap_or_else(|| state.config.server.days_back.max(1));

    // Fast path: check in-memory cache (< 50 microseconds)
    if let Ok(guard) = state.charts_cache.read() {
        if let Some(cached) = guard.as_ref() {
            if cached.generation == cur_gen
                && cached.days_back == req_days
                && cached.today_str == today_str
                && cached.db_mtime == cur_mtime
                && cached.created_at.elapsed().as_secs() < 15
            {
                return build_response(
                    &state,
                    &headers,
                    StatusCode::OK,
                    "application/json; charset=utf-8",
                    None,
                    cached.json_bytes.clone(),
                );
            }
        }
    }

    let since_str = (now - Duration::days(req_days))
        .format("%Y-%m-%d")
        .to_string();
    let generated_str = now.format("%Y-%m-%d %H:%M:%S").to_string();

    let result_filter = if state.config.collector.collect_challenges {
        "Result IN ('SUCCESS','FAILURE','CHALLENGE')"
    } else {
        "Result IN ('SUCCESS','FAILURE')"
    };

    let db_path = Path::new(&state.config.paths.db_path);
    let mut summary = Vec::new();
    let mut trend = Vec::new();
    let mut top_fail = Vec::new();
    let mut top_users = Vec::new();
    let mut by_nas = Vec::new();
    let mut by_hour = Vec::new();
    let mut by_realm = Vec::new();
    let mut by_reason = Vec::new();
    let mut unique_users = 0i64;
    let mut active_devices = 0i64;
    let mut unique_devices = 0i64;
    let mut last_timestamp = String::new();

    if let Ok(conn) = open_db(db_path) {
        if let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT Result, COUNT(*) AS Cnt FROM AuthLog WHERE TimeCreated >= ?1 AND {} GROUP BY Result",
            result_filter
        )) {
            if let Ok(rows) = stmt.query_map([&since_str], |r| {
                Ok(SummaryRow {
                    result: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    cnt: r.get(1)?,
                })
            }) {
                summary = rows.filter_map(|r| r.ok()).collect();
            }
        }

        if let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT DATE(TimeCreated) AS Gun, Result, COUNT(*) AS Cnt FROM AuthLog WHERE TimeCreated >= ?1 AND {} GROUP BY DATE(TimeCreated), Result ORDER BY DATE(TimeCreated)",
            result_filter
        )) {
            if let Ok(rows) = stmt.query_map([&since_str], |r| {
                Ok(TrendRow {
                    gun: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    result: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    cnt: r.get(2)?,
                })
            }) {
                trend = rows.filter_map(|r| r.ok()).collect();
            }
        }

        if let Ok(mut stmt) = conn.prepare(
            "SELECT Username, Realm, COUNT(*) AS Cnt FROM AuthLog WHERE Result='FAILURE' AND TimeCreated >= ?1 AND Username != '' GROUP BY Username, Realm ORDER BY COUNT(*) DESC LIMIT 20",
        ) {
            if let Ok(rows) = stmt.query_map([&since_str], |r| {
                Ok(UserRealmCountRow {
                    username: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    realm: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    cnt: r.get(2)?,
                })
            }) {
                top_fail = rows.filter_map(|r| r.ok()).collect();
            }
        }

        if let Ok(mut stmt) = conn.prepare(
            "SELECT Username, Realm, COUNT(*) AS Cnt FROM AuthLog WHERE Result='SUCCESS' AND TimeCreated >= ?1 AND Username != '' GROUP BY Username, Realm ORDER BY COUNT(*) DESC LIMIT 20",
        ) {
            if let Ok(rows) = stmt.query_map([&since_str], |r| {
                Ok(UserRealmCountRow {
                    username: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    realm: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    cnt: r.get(2)?,
                })
            }) {
                top_users = rows.filter_map(|r| r.ok()).collect();
            }
        }

        if let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT NASIdentifier, COUNT(*) AS Total, SUM(CASE WHEN Result='SUCCESS' THEN 1 ELSE 0 END) AS Suc, SUM(CASE WHEN Result='FAILURE' THEN 1 ELSE 0 END) AS Fail FROM AuthLog WHERE TimeCreated >= ?1 AND NASIdentifier != '' AND {} GROUP BY NASIdentifier ORDER BY COUNT(*) DESC LIMIT 30",
            result_filter
        )) {
            if let Ok(rows) = stmt.query_map([&since_str], |r| {
                Ok(NasRow {
                    nas_identifier: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    total: r.get(1)?,
                    suc: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    fail: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                })
            }) {
                by_nas = rows.filter_map(|r| r.ok()).collect();
            }
        }

        if let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT CAST(strftime('%H', TimeCreated) AS INTEGER) AS Saat, COUNT(*) AS Cnt FROM AuthLog WHERE TimeCreated >= ?1 AND {} GROUP BY Saat ORDER BY Saat",
            result_filter
        )) {
            if let Ok(rows) = stmt.query_map([&since_str], |r| {
                Ok(HourRow {
                    saat: r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    cnt: r.get(1)?,
                })
            }) {
                by_hour = rows.filter_map(|r| r.ok()).collect();
            }
        }

        if let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT Realm, COUNT(*) AS Total, SUM(CASE WHEN Result='SUCCESS' THEN 1 ELSE 0 END) AS SuccCnt, SUM(CASE WHEN Result='FAILURE' THEN 1 ELSE 0 END) AS FailCnt FROM AuthLog WHERE TimeCreated >= ?1 AND Realm != '' AND {} GROUP BY Realm ORDER BY COUNT(*) DESC LIMIT 30",
            result_filter
        )) {
            if let Ok(rows) = stmt.query_map([&since_str], |r| {
                Ok(RealmRow {
                    realm: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    total: r.get(1)?,
                    succ_cnt: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    fail_cnt: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                })
            }) {
                by_realm = rows.filter_map(|r| r.ok()).collect();
            }
        }

        if let Ok(mut stmt) = conn.prepare(
            "SELECT ReasonCode, COUNT(*) AS Cnt FROM AuthLog WHERE Result='FAILURE' AND TimeCreated >= ?1 GROUP BY ReasonCode ORDER BY COUNT(*) DESC LIMIT 30",
        ) {
            if let Ok(rows) = stmt.query_map([&since_str], |r| {
                Ok(ReasonRow {
                    reason_code: r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    cnt: r.get(1)?,
                })
            }) {
                by_reason = rows.filter_map(|r| r.ok()).collect();
            }
        }

        unique_users = conn
            .query_row(
                "SELECT COUNT(DISTINCT Username) FROM AuthLog WHERE TimeCreated >= ?1 AND Result='SUCCESS'",
                [&since_str],
                |r| r.get(0),
            )
            .unwrap_or(0);

        active_devices = conn
            .query_row(
                "SELECT COUNT(DISTINCT CallingStationID) FROM AuthLog WHERE TimeCreated >= ?1 AND CallingStationID != ''",
                [&since_str],
                |r| r.get(0),
            )
            .unwrap_or(0);

        unique_devices = conn
            .query_row("SELECT COUNT(*) FROM Devices", [], |r| r.get(0))
            .unwrap_or(0);

        last_timestamp = conn
            .query_row(
                "SELECT COALESCE(MAX(TimeCreated), '') FROM AuthLog",
                [],
                |r| r.get(0),
            )
            .unwrap_or_default();
    }

    let payload = ChartsResponse {
        generated: generated_str,
        days_back: req_days,
        last_timestamp,
        capabilities: Capabilities {
            supports_challenge: state.config.collector.collect_challenges,
            max_page_limit: state.config.server.max_page_limit,
            max_export_rows: state.config.server.max_export_rows,
        },
        unique_users,
        active_devices,
        unique_devices,
        summary,
        trend,
        top_fail,
        top_users,
        by_nas,
        by_hour,
        by_realm,
        by_reason,
    };

    let json_bytes = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());

    if let Ok(mut guard) = state.charts_cache.write() {
        *guard = Some(CachedCharts {
            generation: cur_gen,
            days_back: req_days,
            today_str,
            db_mtime: cur_mtime,
            created_at: Instant::now(),
            json_bytes: json_bytes.clone(),
        });
    }

    build_response(
        &state,
        &headers,
        StatusCode::OK,
        "application/json; charset=utf-8",
        None,
        json_bytes,
    )
}

fn query_recent_rows(
    db_path: &Path,
    rq: &BuiltRecentQuery,
    include_offset: bool,
) -> (usize, Vec<RecentLogRow>) {
    let Ok(conn) = open_db(db_path) else {
        return (0, Vec::new());
    };

    let count_sql = format!("SELECT COUNT(*) FROM AuthLog WHERE {}", rq.where_clause);
    let total: usize = conn
        .query_row(&count_sql, params_from_iter(rq.params.iter()), |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n.max(0) as usize)
        .unwrap_or(0);

    let offset_val = if include_offset { rq.offset } else { 0 };
    let data_sql = format!(
        "SELECT LogID, TimeCreated AS Zaman, Username, Realm, ClientIP, NASIdentifier, CallingStationID, Result, ReasonCode, Reason \
         FROM AuthLog WHERE {} ORDER BY {} {} LIMIT {} OFFSET {}",
        rq.where_clause, rq.sort_column, rq.sort_direction, rq.limit, offset_val
    );

    let mut rows_out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(&data_sql) {
        if let Ok(mapped) = stmt.query_map(params_from_iter(rq.params.iter()), |r| {
            Ok(RecentLogRow {
                log_id: r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                zaman: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                username: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                realm: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                client_ip: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                nas_identifier: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                calling_station_id: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                result: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                reason_code: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
                reason: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
            })
        }) {
            rows_out = mapped.filter_map(|r| r.ok()).collect();
        }
    }

    (total, rows_out)
}

async fn handle_recent(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(qs): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let rq = build_recent_query(
        &qs,
        state.config.server.days_back,
        state.config.server.default_page_limit,
        state.config.server.max_page_limit,
    );

    let db_path = Path::new(&state.config.paths.db_path);
    let (total, data) = query_recent_rows(db_path, &rq, true);
    let pages = if total == 0 {
        0
    } else {
        (total + rq.limit - 1) / rq.limit
    };

    let payload = RecentResponse {
        page: rq.page,
        limit: rq.limit,
        total,
        pages,
        capabilities: Capabilities {
            supports_challenge: state.config.collector.collect_challenges,
            max_page_limit: state.config.server.max_page_limit,
            max_export_rows: state.config.server.max_export_rows,
        },
        data,
    };

    let json_bytes = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
    build_response(
        &state,
        &headers,
        StatusCode::OK,
        "application/json; charset=utf-8",
        None,
        json_bytes,
    )
}

async fn handle_export_csv(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(mut qs): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    qs.insert("page".to_string(), "1".to_string());
    if !qs.contains_key("limit") {
        qs.insert(
            "limit".to_string(),
            state.config.server.max_export_rows.to_string(),
        );
    }

    let rq = build_recent_query(
        &qs,
        state.config.server.days_back,
        state.config.server.max_export_rows,
        state.config.server.max_export_rows,
    );

    let db_path = Path::new(&state.config.paths.db_path);
    let (_total, rows) = query_recent_rows(db_path, &rq, false);
    let csv_text = rows_to_csv(&rows);

    build_response(
        &state,
        &headers,
        StatusCode::OK,
        "text/csv; charset=utf-8",
        None,
        csv_text.into_bytes(),
    )
}

async fn handle_not_found(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    let body = serde_json::json!({
        "error": format!("Endpoint bulunamadi: {}", uri.path())
    });
    build_response(
        &state,
        &headers,
        StatusCode::NOT_FOUND,
        "application/json; charset=utf-8",
        None,
        serde_json::to_vec(&body).unwrap_or_default(),
    )
}
