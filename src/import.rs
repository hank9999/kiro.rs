//! 从 kiro-cli 的 SQLite 数据库导入凭据

use std::path::{Path, PathBuf};

use crate::kiro::model::credentials::KiroCredentials;
use crate::model::config::ClientMode;

/// 获取 kiro-cli 数据库默认路径
pub fn default_db_path() -> PathBuf {
    dirs_next().join("data.sqlite3")
}

fn dirs_next() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/share/kiro-cli")
    } else {
        PathBuf::from(".local/share/kiro-cli")
    }
}

/// 从 kiro-cli 数据库导入凭据
pub fn import_credentials(db_path: &Path) -> anyhow::Result<KiroCredentials> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;

    // 读取 token
    let token_json: String = conn.query_row(
        "SELECT value FROM auth_kv WHERE key = 'kirocli:social:token'",
        [],
        |row| row.get(0),
    )?;

    let token: serde_json::Value = serde_json::from_str(&token_json)?;

    let mut cred = KiroCredentials {
        access_token: token.get("access_token").and_then(|v| v.as_str()).map(String::from),
        refresh_token: token.get("refresh_token").and_then(|v| v.as_str()).map(String::from),
        profile_arn: token.get("profile_arn").and_then(|v| v.as_str()).map(String::from),
        expires_at: token.get("expires_at").and_then(|v| v.as_str()).map(String::from),
        auth_method: Some("social".to_string()),
        client_mode: Some(ClientMode::KiroCli),
        ..Default::default()
    };

    // 尝试读取 device registration（IdC 场景）
    if let Ok(device_json) = conn.query_row::<String, _, _>(
        "SELECT value FROM auth_kv WHERE key = 'kirocli:odic:device-registration'",
        [],
        |row| row.get(0),
    ) {
        if let Ok(device) = serde_json::from_str::<serde_json::Value>(&device_json) {
            // 如果 token 里没有 provider 字段，说明可能是 IdC 认证
            if token.get("provider").is_none() {
                cred.auth_method = Some("idc".to_string());
                cred.client_id = device.get("client_id").and_then(|v| v.as_str()).map(String::from);
                cred.client_secret = device.get("client_secret").and_then(|v| v.as_str()).map(String::from);
            }
            // 读取 region
            if let Some(region) = device.get("region").and_then(|v| v.as_str()) {
                cred.region = Some(region.to_string());
            }
        }
    }

    Ok(cred)
}
