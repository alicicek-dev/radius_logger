use chrono::NaiveDateTime;
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Zero-copy representation of the fields extracted from a single IAS XML `<Event>` line.
#[derive(Debug, Default, Clone)]
pub struct IasEventFields<'a> {
    pub packet_type: &'a str,
    pub timestamp: &'a str,
    pub user_name: &'a str,
    pub client_ip: &'a str,
    pub nas_ip: &'a str,
    pub nas_identifier: &'a str,
    pub called_station_id: &'a str,
    pub calling_station_id: &'a str,
    pub provider_name: &'a str,
    pub proxy_policy_name: &'a str,
    pub reason_code: &'a str,
    pub class_attr: &'a str,
}

/// Ultra-fast single-pass byte scanner for IAS XML lines (`<Event><Tag ...>Value</Tag>...</Event>`).
/// Avoids regex allocations and heap HashMap overhead on every log line.
pub fn parse_ias_event_line(line: &str) -> IasEventFields<'_> {
    let mut out = IasEventFields::default();
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Find next '<'
        let Some(lt_rel) = bytes[i..].iter().position(|&b| b == b'<') else {
            break;
        };
        let lt = i + lt_rel;
        if lt + 1 >= len {
            break;
        }

        // Skip closing tags `</...>`
        if bytes[lt + 1] == b'/' {
            let Some(gt_rel) = bytes[lt + 2..].iter().position(|&b| b == b'>') else {
                break;
            };
            i = lt + 2 + gt_rel + 1;
            continue;
        }

        // Find end of opening tag `>`
        let Some(gt_rel) = bytes[lt + 1..].iter().position(|&b| b == b'>') else {
            break;
        };
        let gt = lt + 1 + gt_rel;
        let open_inner = &line[lt + 1..gt];

        // Self-closing tag `<Tag/>`
        if open_inner.ends_with('/') {
            i = gt + 1;
            continue;
        }

        // Extract tag name (up to first whitespace)
        let tag_name = open_inner
            .split_ascii_whitespace()
            .next()
            .unwrap_or(open_inner);

        if tag_name.eq_ignore_ascii_case("Event") {
            i = gt + 1;
            continue;
        }

        let val_start = gt + 1;
        // Find next '<' which starts `</Tag>`
        let Some(close_lt_rel) = bytes[val_start..].iter().position(|&b| b == b'<') else {
            break;
        };
        let close_lt = val_start + close_lt_rel;
        let val = line[val_start..close_lt].trim();

        if tag_name.eq_ignore_ascii_case("Packet-Type") {
            out.packet_type = val;
        } else if tag_name.eq_ignore_ascii_case("Timestamp") {
            out.timestamp = val;
        } else if tag_name.eq_ignore_ascii_case("User-Name") {
            out.user_name = val;
        } else if tag_name.eq_ignore_ascii_case("Client-IP-Address") {
            out.client_ip = val;
        } else if tag_name.eq_ignore_ascii_case("NAS-IP-Address") {
            out.nas_ip = val;
        } else if tag_name.eq_ignore_ascii_case("NAS-Identifier") {
            out.nas_identifier = val;
        } else if tag_name.eq_ignore_ascii_case("Called-Station-Id") {
            out.called_station_id = val;
        } else if tag_name.eq_ignore_ascii_case("Calling-Station-Id") {
            out.calling_station_id = val;
        } else if tag_name.eq_ignore_ascii_case("Provider-Name") {
            out.provider_name = val;
        } else if tag_name.eq_ignore_ascii_case("Proxy-Policy-Name") {
            out.proxy_policy_name = val;
        } else if tag_name.eq_ignore_ascii_case("Reason-Code") {
            out.reason_code = val;
        } else if tag_name.eq_ignore_ascii_case("Class") {
            out.class_attr = val;
        }

        // Advance past closing tag `</Tag>`
        if close_lt + 1 < len && bytes[close_lt + 1] == b'/' {
            if let Some(close_gt_rel) = bytes[close_lt + 2..].iter().position(|&b| b == b'>') {
                i = close_lt + 2 + close_gt_rel + 1;
                continue;
            }
        }
        i = close_lt;
    }

    out
}

/// Splits `FullUsername` into `(Username, Realm)`.
/// Matches PowerShell `Split-EduroamUsername` behavior (`^(.+)@(.+)$` -> greedy first part, so last `@` splits).
pub fn split_username(full_username: &str) -> (String, String) {
    let trimmed = full_username.trim();
    if trimmed.is_empty() {
        return (String::new(), String::new());
    }
    if let Some(at_idx) = trimmed.rfind('@') {
        if at_idx > 0 && at_idx + 1 < trimmed.len() {
            return (
                trimmed[..at_idx].to_string(),
                trimmed[at_idx + 1..].to_string(),
            );
        }
    }
    (trimmed.to_string(), String::new())
}

/// Computes the exact SHA-256 `EventHash` compatible with `Get-EduroamEventHash` in `Eduroam-Common.ps1`.
/// Input format MUST remain `"{source_file}\n{source_line}\n{raw_line}"`.
pub fn compute_event_hash(source_file: &str, source_line: usize, raw_line: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_file.as_bytes());
    hasher.update(b"\n");
    let mut line_buf = [0u8; 24];
    let line_str = format_usize(source_line, &mut line_buf);
    hasher.update(line_str.as_bytes());
    hasher.update(b"\n");
    hasher.update(raw_line.as_bytes());

    let digest = hasher.finalize();
    let mut hex_out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(&mut hex_out, "{:02x}", byte);
    }
    hex_out
}

fn format_usize(mut n: usize, buf: &mut [u8; 24]) -> &str {
    if n == 0 {
        return "0";
    }
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    std::str::from_utf8(&buf[i..]).unwrap_or("0")
}

/// Parses IAS timestamp (`MM/dd/yyyy HH:mm:ss[.fff]`) without slow format exception loops.
pub fn parse_ias_timestamp(ts_raw: &str) -> Option<NaiveDateTime> {
    let s = ts_raw.trim();
    if s.is_empty() {
        return None;
    }
    // Strip fractional seconds if present (`06/08/2026 10:11:12.123` -> `06/08/2026 10:11:12`)
    let base = match s.find('.') {
        Some(dot) => &s[..dot],
        None => s,
    };
    NaiveDateTime::parse_from_str(base, "%m/%d/%Y %H:%M:%S").ok()
}

pub fn get_reason_text(code: i32, raw_str: &str) -> String {
    match code {
        0 if raw_str.is_empty() || raw_str == "0" => "Basari".to_string(),
        1 => "Dahili hata".to_string(),
        2 => "Erisim reddedildi".to_string(),
        3 => "Hatali istek".to_string(),
        4 => "Global Katalog erisilemiyor".to_string(),
        5 => "Etki alani kullanilamiyor".to_string(),
        6 => "Sunucu kullanilamiyor".to_string(),
        7 => "Etki alani bulunamadi".to_string(),
        8 => "Kullanici bulunamadi".to_string(),
        16 => "Kimlik dogrulama basarisiz".to_string(),
        17 => "Sifre degistirme basarisiz".to_string(),
        18 => "Desteklenmeyen kimlik dogrulama turu".to_string(),
        22 => "EAP el sikisma hatasi (istemci baglantiyi kesti)".to_string(),
        23 => "Beklenmeyen EAP hatasi".to_string(),
        32 => "Yalnizca yerel kullanicilar".to_string(),
        33 => "Sifre degistirilmeli".to_string(),
        34 => "Hesap devre disi".to_string(),
        35 => "Hesap suresi dolmus".to_string(),
        36 => "Hesap kilitli".to_string(),
        37 => "Gecersiz giris saatleri".to_string(),
        38 => "Hesap kisitlamasi".to_string(),
        48 => "Esleyen ilke yok".to_string(),
        49 => "Baglanti istegi ilkesi eslesmedi".to_string(),
        64 => "Dial-in kilitli".to_string(),
        65 => "Dial-in devre disi".to_string(),
        66 => "Gecersiz kimlik dogrulama turu".to_string(),
        67 => "Gecersiz cagiran istasyon".to_string(),
        68 => "Gecersiz dial-in saatleri".to_string(),
        69 => "Gecersiz aranan istasyon".to_string(),
        70 => "Gecersiz port turu".to_string(),
        71 => "Gecersiz kisitlama".to_string(),
        80 => "Kayit bulunamadi".to_string(),
        96 => "Oturum zaman asimi".to_string(),
        97 => "Beklenmeyen istek".to_string(),
        100 => "Proxy reddetti".to_string(),
        101 => "Proxy baglanti hatasi".to_string(),
        102 => "Proxy iletme hatasi".to_string(),
        103 => "Proxy gecersiz yanit".to_string(),
        104 => "Proxy hatali yanit".to_string(),
        105 => "Proxy zaman asimi".to_string(),
        106 => "Proxy yanit yok".to_string(),
        107 => "Gecersiz oznitelik".to_string(),
        108 => "Proxy sessizce reddetti".to_string(),
        109 => "Proxy kimlik dogrulama adimini reddetti".to_string(),
        110 => "Kullanici adinda kurum (realm) eksik".to_string(),
        111 => "EAP kimlik dogrulamasi basarisiz".to_string(),
        112 => "Istek iletilmedi (realm yonlendirme)".to_string(),
        113 => "Baglanti istegi iptal edildi".to_string(),
        256 => "Sertifika suresi dolmus".to_string(),
        259 => "Sertifika iptal edilmis".to_string(),
        260 => "Sertifika iptal kontrolu yapilamadi".to_string(),
        262 => "Sertifika veren makama (CA) guvenilmiyor".to_string(),
        265 => "TLS sertifika guven / el sikisma hatasi".to_string(),
        266 => "EAP/TLS mesaj butunlugu hatasi".to_string(),
        269 => "Gecersiz TLS el sikisma mesaji".to_string(),
        299 => "EAP oturumu istemci tarafindan yarida kesildi".to_string(),
        300 => "EAP kimlik bilgisi veya ic tunel dogrulama hatasi".to_string(),
        _ => format!("Kod: {}", raw_str),
    }
}

/// Escapes SQL LIKE wildcards (`\`, `%`, `_`) and wraps in `%...%`.
pub fn to_sql_like_pattern(val: &str) -> String {
    let mut out = String::with_capacity(val.len() + 4);
    out.push('%');
    for ch in val.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            c => out.push(c),
        }
    }
    out.push('%');
    out
}

/// Formats a CSV field with double-quote escaping AND CSV formula injection protection.
pub fn format_csv_field(val: &str) -> String {
    let trimmed_start = val.trim_start_matches([' ', '\t']);
    let needs_formula_guard = trimmed_start
        .chars()
        .next()
        .map(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'))
        .unwrap_or(false);

    let mut out = String::with_capacity(val.len() + 4);
    out.push('"');
    if needs_formula_guard {
        out.push('\'');
    }
    for ch in val.chars() {
        if ch == '"' {
            out.push_str("\"\"");
        } else {
            out.push(ch);
        }
    }
    out.push('"');
    out
}
