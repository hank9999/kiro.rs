//! SQLite 数据库模块
//!
//! Input: rusqlite, bcrypt, 凭据数据
//! Output: 数据库连接、用户认证、凭据 CRUD
//! Pos: 数据持久化层，管理用户和凭据存储

use parking_lot::Mutex;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Arc;

use crate::kiro::model::credentials::KiroCredentials;
use crate::model::runtime_config::RuntimeConfig;

/// 数据库初始化配置
pub struct DbInitConfig {
    pub admin_username: String,
    pub admin_password: String,
}

impl Default for DbInitConfig {
    fn default() -> Self {
        Self {
            admin_username: "admin".to_string(),
            admin_password: "admin123".to_string(),
        }
    }
}

/// 数据库管理器
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// 创建或打开数据库（使用默认配置）
    pub fn open<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        Self::open_with_config(path, DbInitConfig::default())
    }

    /// 创建或打开数据库（使用自定义配置）
    pub fn open_with_config<P: AsRef<Path>>(path: P, init_config: DbInitConfig) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init(init_config)?;
        Ok(db)
    }

    /// 初始化数据库（表结构 + 默认数据）
    fn init(&self, init_config: DbInitConfig) -> anyhow::Result<()> {
        let conn = self.conn.lock();

        // 创建表结构
        self.create_tables(&conn)?;

        // 初始化默认数据（admin 账号 + 系统设置）
        self.init_default_data(&conn, &init_config)?;

        Ok(())
    }

    /// 创建数据库表结构
    fn create_tables(&self, conn: &Connection) -> anyhow::Result<()> {

        // 用户表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
            [],
        )?;

        // 凭据表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS credentials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                access_token TEXT,
                refresh_token TEXT,
                profile_arn TEXT,
                expires_at TEXT,
                auth_method TEXT,
                client_id TEXT,
                client_secret TEXT,
                priority INTEGER NOT NULL DEFAULT 0,
                region TEXT NOT NULL DEFAULT 'us-east-1',
                machine_id TEXT,
                email TEXT,
                subscription_title TEXT,
                current_usage REAL DEFAULT 0,
                usage_limit REAL DEFAULT 0,
                disabled INTEGER NOT NULL DEFAULT 0,
                failure_count INTEGER NOT NULL DEFAULT 0,
                proxy_url TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
            [],
        )?;

        // 系统设置表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
            [],
        )?;

        Ok(())
    }

    /// 初始化默认数据（admin 账号 + 系统设置）
    fn init_default_data(&self, conn: &Connection, init_config: &DbInitConfig) -> anyhow::Result<()> {
        // 检查是否需要初始化（用户表为空表示首次启动）
        let user_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users",
            [],
            |row| row.get(0),
        )?;

        if user_count == 0 {
            tracing::info!("首次启动，初始化默认数据...");

            // 创建默认管理员账户
            let password_hash = bcrypt::hash(&init_config.admin_password, bcrypt::DEFAULT_COST)?;
            conn.execute(
                "INSERT INTO users (username, password_hash) VALUES (?1, ?2)",
                params![&init_config.admin_username, password_hash],
            )?;
            tracing::info!("已创建默认管理员账户: {}/{}", init_config.admin_username, init_config.admin_password);
        }

        // 初始化默认系统设置（使用 INSERT OR IGNORE 避免覆盖已有设置）
        let defaults = [
            ("kiro_version", "0.8.0"),
            ("system_version", "darwin#24.6.0"),
            ("node_version", "v22.12.0"),
            ("min_usage_threshold", "5"),
            ("count_tokens_auth_type", "x-api-key"),
        ];

        for (key, value) in defaults {
            conn.execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }

        Ok(())
    }

    /// 验证用户登录
    pub fn verify_user(&self, username: &str, password: &str) -> anyhow::Result<Option<i64>> {
        let conn = self.conn.lock();

        let result: Result<(i64, String), _> = conn.query_row(
            "SELECT id, password_hash FROM users WHERE username = ?1",
            params![username],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        match result {
            Ok((id, hash)) => {
                if bcrypt::verify(password, &hash)? {
                    Ok(Some(id))
                } else {
                    Ok(None)
                }
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 修改用户密码
    pub fn change_password(&self, user_id: i64, old_password: &str, new_password: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock();

        // 验证旧密码
        let hash: String = conn.query_row(
            "SELECT password_hash FROM users WHERE id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;

        if !bcrypt::verify(old_password, &hash)? {
            return Ok(false);
        }

        // 更新新密码
        let new_hash = bcrypt::hash(new_password, bcrypt::DEFAULT_COST)?;
        conn.execute(
            "UPDATE users SET password_hash = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![new_hash, user_id],
        )?;

        Ok(true)
    }

    /// 获取所有凭据
    pub fn get_all_credentials(&self) -> anyhow::Result<Vec<CredentialRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, access_token, refresh_token, profile_arn, expires_at,
                    auth_method, client_id, client_secret, priority, region,
                    machine_id, email, subscription_title, current_usage, usage_limit,
                    disabled, failure_count, proxy_url
             FROM credentials ORDER BY priority ASC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(CredentialRow {
                id: row.get(0)?,
                access_token: row.get(1)?,
                refresh_token: row.get(2)?,
                profile_arn: row.get(3)?,
                expires_at: row.get(4)?,
                auth_method: row.get(5)?,
                client_id: row.get(6)?,
                client_secret: row.get(7)?,
                priority: row.get(8)?,
                region: row.get(9)?,
                machine_id: row.get(10)?,
                email: row.get(11)?,
                subscription_title: row.get(12)?,
                current_usage: row.get::<_, Option<f64>>(13)?.unwrap_or(0.0),
                usage_limit: row.get::<_, Option<f64>>(14)?.unwrap_or(0.0),
                disabled: row.get(15)?,
                failure_count: row.get(16)?,
                proxy_url: row.get(17)?,
            })
        })?;

        let mut credentials = Vec::new();
        for row in rows {
            credentials.push(row?);
        }
        Ok(credentials)
    }

    /// 添加凭据
    pub fn add_credential(&self, cred: &KiroCredentials) -> anyhow::Result<i64> {
        let conn = self.conn.lock();
        // region 默认为 us-east-1（与数据库 DEFAULT 保持一致）
        let region = cred.region.as_deref().unwrap_or("us-east-1");
        conn.execute(
            "INSERT INTO credentials (access_token, refresh_token, profile_arn, expires_at,
                                      auth_method, client_id, client_secret, priority, region, machine_id, proxy_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                cred.access_token,
                cred.refresh_token,
                cred.profile_arn,
                cred.expires_at,
                cred.auth_method,
                cred.client_id,
                cred.client_secret,
                cred.priority,
                region,
                cred.machine_id,
                cred.proxy_url,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 更新凭据
    pub fn update_credential(&self, id: i64, cred: &KiroCredentials) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        // region 默认为 us-east-1（与数据库 DEFAULT 保持一致）
        let region = cred.region.as_deref().unwrap_or("us-east-1");
        let rows = conn.execute(
            "UPDATE credentials SET
                access_token = ?1, refresh_token = ?2, profile_arn = ?3, expires_at = ?4,
                auth_method = ?5, client_id = ?6, client_secret = ?7, priority = ?8,
                region = ?9, machine_id = ?10, proxy_url = ?11, updated_at = datetime('now')
             WHERE id = ?12",
            params![
                cred.access_token,
                cred.refresh_token,
                cred.profile_arn,
                cred.expires_at,
                cred.auth_method,
                cred.client_id,
                cred.client_secret,
                cred.priority,
                region,
                cred.machine_id,
                cred.proxy_url,
                id,
            ],
        )?;
        Ok(rows > 0)
    }

    /// 删除凭据
    pub fn delete_credential(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let rows = conn.execute(
            "DELETE FROM credentials WHERE id = ?1",
            params![id],
        )?;
        Ok(rows > 0)
    }

    /// 设置凭据禁用状态
    pub fn set_credential_disabled(&self, id: i64, disabled: bool) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let rows = conn.execute(
            "UPDATE credentials SET disabled = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![disabled as i32, id],
        )?;
        Ok(rows > 0)
    }

    /// 设置凭据优先级
    pub fn set_credential_priority(&self, id: i64, priority: u32) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let rows = conn.execute(
            "UPDATE credentials SET priority = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![priority, id],
        )?;
        Ok(rows > 0)
    }

    /// 重置失败计数
    pub fn reset_failure_count(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let rows = conn.execute(
            "UPDATE credentials SET failure_count = 0, disabled = 0, updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(rows > 0)
    }

    /// 更新凭据元数据（优先级、region、machineId、refreshToken、clientId、clientSecret、proxyUrl）
    pub fn update_credential_metadata(
        &self,
        id: i64,
        priority: Option<u32>,
        region: Option<String>,
        machine_id: Option<String>,
        refresh_token: Option<String>,
        client_id: Option<String>,
        client_secret: Option<String>,
        proxy_url: Option<String>,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.lock();

        let mut updates = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(p) = priority {
            updates.push("priority = ?");
            params_vec.push(Box::new(p));
        }
        if let Some(r) = region {
            updates.push("region = ?");
            params_vec.push(Box::new(r));
        }
        if let Some(m) = machine_id {
            updates.push("machine_id = ?");
            params_vec.push(Box::new(m));
        }
        if let Some(rt) = refresh_token {
            updates.push("refresh_token = ?");
            params_vec.push(Box::new(rt));
            // 清除 access_token 和 expires_at，强制下次刷新
            updates.push("access_token = NULL");
            updates.push("expires_at = NULL");
        }
        if let Some(cid) = client_id {
            updates.push("client_id = ?");
            params_vec.push(Box::new(cid));
        }
        if let Some(cs) = client_secret {
            updates.push("client_secret = ?");
            params_vec.push(Box::new(cs));
        }
        if let Some(pu) = proxy_url {
            updates.push("proxy_url = ?");
            // 空字符串转为 NULL
            if pu.is_empty() {
                params_vec.push(Box::new(None::<String>));
            } else {
                params_vec.push(Box::new(pu));
            }
        }

        if updates.is_empty() {
            return Ok(false);
        }

        updates.push("updated_at = datetime('now')");
        params_vec.push(Box::new(id));

        let sql = format!(
            "UPDATE credentials SET {} WHERE id = ?",
            updates.join(", ")
        );

        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = conn.execute(&sql, params_refs.as_slice())?;
        Ok(rows > 0)
    }

    /// 增加失败计数
    pub fn increment_failure_count(&self, id: i64) -> anyhow::Result<u32> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE credentials SET failure_count = failure_count + 1, updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        let count: u32 = conn.query_row(
            "SELECT failure_count FROM credentials WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 更新 access_token 和 expires_at
    pub fn update_token(&self, id: i64, access_token: &str, expires_at: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let rows = conn.execute(
            "UPDATE credentials SET access_token = ?1, expires_at = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![access_token, expires_at, id],
        )?;
        Ok(rows > 0)
    }

    /// 获取单个凭据
    pub fn get_credential(&self, id: i64) -> anyhow::Result<Option<CredentialRow>> {
        let conn = self.conn.lock();
        let result = conn.query_row(
            "SELECT id, access_token, refresh_token, profile_arn, expires_at,
                    auth_method, client_id, client_secret, priority, region,
                    machine_id, email, subscription_title, current_usage, usage_limit,
                    disabled, failure_count, proxy_url
             FROM credentials WHERE id = ?1",
            params![id],
            |row| {
                Ok(CredentialRow {
                    id: row.get(0)?,
                    access_token: row.get(1)?,
                    refresh_token: row.get(2)?,
                    profile_arn: row.get(3)?,
                    expires_at: row.get(4)?,
                    auth_method: row.get(5)?,
                    client_id: row.get(6)?,
                    client_secret: row.get(7)?,
                    priority: row.get(8)?,
                    region: row.get(9)?,
                    machine_id: row.get(10)?,
                    email: row.get(11)?,
                    subscription_title: row.get(12)?,
                    current_usage: row.get::<_, Option<f64>>(13)?.unwrap_or(0.0),
                    usage_limit: row.get::<_, Option<f64>>(14)?.unwrap_or(0.0),
                    disabled: row.get(15)?,
                    failure_count: row.get(16)?,
                    proxy_url: row.get(17)?,
                })
            },
        );

        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 更新凭据的余额信息
    pub fn update_credential_usage(
        &self,
        id: i64,
        email: Option<&str>,
        subscription_title: Option<&str>,
        current_usage: f64,
        usage_limit: f64,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let rows = conn.execute(
            "UPDATE credentials SET
                email = ?1, subscription_title = ?2, current_usage = ?3, usage_limit = ?4,
                updated_at = datetime('now')
             WHERE id = ?5",
            params![email, subscription_title, current_usage, usage_limit, id],
        )?;
        Ok(rows > 0)
    }

    /// 从数据库加载凭据列表转换为 KiroCredentials
    pub fn load_credentials_list(&self) -> anyhow::Result<Vec<KiroCredentials>> {
        let rows = self.get_all_credentials()?;
        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    // ============ 系统设置 ============

    /// 获取所有系统设置
    pub fn get_all_settings(&self) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut settings = Vec::new();
        for row in rows {
            settings.push(row?);
        }
        Ok(settings)
    }

    /// 获取单个设置
    pub fn get_setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock();
        let result = conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        );

        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 更新设置
    pub fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = datetime('now')",
            params![key, value],
        )?;
        Ok(())
    }

    /// 批量更新设置
    pub fn set_settings(&self, settings: &[(&str, &str)]) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        for (key, value) in settings {
            conn.execute(
                "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
                 ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = datetime('now')",
                params![key, value],
            )?;
        }
        Ok(())
    }

    /// 获取运行时配置
    pub fn get_runtime_config(&self) -> RuntimeConfig {
        RuntimeConfig {
            kiro_version: self.get_setting("kiro_version")
                .ok()
                .flatten()
                .unwrap_or_else(|| "0.8.0".to_string()),
            system_version: self.get_setting("system_version")
                .ok()
                .flatten()
                .unwrap_or_else(|| "darwin#24.6.0".to_string()),
            node_version: self.get_setting("node_version")
                .ok()
                .flatten()
                .unwrap_or_else(|| "v22.12.0".to_string()),
            count_tokens_api_url: self.get_setting("count_tokens_api_url")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
            count_tokens_api_key: self.get_setting("count_tokens_api_key")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
            count_tokens_auth_type: self.get_setting("count_tokens_auth_type")
                .ok()
                .flatten()
                .unwrap_or_else(|| "x-api-key".to_string()),
            min_usage_threshold: self.get_setting("min_usage_threshold")
                .ok()
                .flatten()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5.0),
        }
    }
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Self {
            conn: Arc::clone(&self.conn),
        }
    }
}

/// 数据库凭据行
#[derive(Debug, Clone)]
pub struct CredentialRow {
    pub id: i64,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub profile_arn: Option<String>,
    pub expires_at: Option<String>,
    pub auth_method: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub priority: u32,
    pub region: Option<String>,
    pub machine_id: Option<String>,
    pub email: Option<String>,
    pub subscription_title: Option<String>,
    pub current_usage: f64,
    pub usage_limit: f64,
    pub disabled: i32,
    pub failure_count: u32,
    pub proxy_url: Option<String>,
}

impl From<CredentialRow> for KiroCredentials {
    fn from(row: CredentialRow) -> Self {
        KiroCredentials {
            id: Some(row.id as u64),
            access_token: row.access_token,
            refresh_token: row.refresh_token,
            profile_arn: row.profile_arn,
            expires_at: row.expires_at,
            auth_method: row.auth_method,
            client_id: row.client_id,
            client_secret: row.client_secret,
            priority: row.priority,
            region: row.region,
            machine_id: row.machine_id,
            proxy_url: row.proxy_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_database_init() {
        let db_path = "/tmp/test_kiro_db.sqlite";
        let _ = fs::remove_file(db_path);

        let db = Database::open(db_path).unwrap();

        // 验证默认管理员
        let user_id = db.verify_user("admin", "admin123").unwrap();
        assert!(user_id.is_some());

        // 验证错误密码
        let user_id = db.verify_user("admin", "wrong").unwrap();
        assert!(user_id.is_none());

        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn test_credential_crud() {
        let db_path = "/tmp/test_kiro_cred.sqlite";
        let _ = fs::remove_file(db_path);

        let db = Database::open(db_path).unwrap();

        // 添加凭据
        let cred = KiroCredentials {
            id: None,
            access_token: Some("test_token".to_string()),
            refresh_token: Some("test_refresh".to_string()),
            profile_arn: None,
            expires_at: Some("2025-12-31T00:00:00Z".to_string()),
            auth_method: Some("social".to_string()),
            client_id: None,
            client_secret: None,
            priority: 0,
            region: None,
            machine_id: None,
        };

        let id = db.add_credential(&cred).unwrap();
        assert!(id > 0);

        // 获取凭据
        let row = db.get_credential(id).unwrap().unwrap();
        assert_eq!(row.access_token, Some("test_token".to_string()));

        // 删除凭据
        let deleted = db.delete_credential(id).unwrap();
        assert!(deleted);

        let _ = fs::remove_file(db_path);
    }
}
