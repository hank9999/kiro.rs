//! 应用配置模块
//!
//! Input: .env 配置文件或环境变量
//! Output: Config 结构体
//! Pos: 配置加载和管理（仅启动时配置，运行时配置存储在数据库）

use std::env;
use std::path::Path;

/// 应用启动配置（运行时配置存储在数据库 settings 表）
#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
    /// 数据库文件路径（可选，默认 data.db）
    pub db_path: String,
    /// JWT 密钥（可选，默认自动生成）
    pub jwt_secret: Option<String>,
    /// JWT 过期时间（小时，默认 24）
    pub jwt_expiry_hours: i64,
    /// 默认管理员用户名（仅首次初始化时使用，默认 admin）
    pub admin_username: String,
    /// 默认管理员密码（仅首次初始化时使用，默认 admin123）
    pub admin_password: String,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_db_path() -> String {
    "data.db".to_string()
}

fn default_jwt_expiry_hours() -> i64 {
    24
}

fn default_admin_username() -> String {
    "admin".to_string()
}

fn default_admin_password() -> String {
    "admin123".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            api_key: None,
            db_path: default_db_path(),
            jwt_secret: None,
            jwt_expiry_hours: default_jwt_expiry_hours(),
            admin_username: default_admin_username(),
            admin_password: default_admin_password(),
        }
    }
}

impl Config {
    /// 获取默认配置文件路径
    pub fn default_config_path() -> &'static str {
        ".env"
    }

    /// 从 .env 文件加载配置
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();

        // 尝试加载 .env 文件
        if path.exists() {
            dotenvy::from_path(path).ok();
        } else {
            // 尝试加载默认 .env 文件
            dotenvy::dotenv().ok();
        }

        // 从环境变量构建配置
        Ok(Self {
            host: env::var("HOST").unwrap_or_else(|_| default_host()),
            port: env::var("PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(default_port),
            api_key: env::var("API_KEY").ok(),
            db_path: env::var("DB_PATH").unwrap_or_else(|_| default_db_path()),
            jwt_secret: env::var("JWT_SECRET").ok(),
            jwt_expiry_hours: env::var("JWT_EXPIRY_HOURS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(default_jwt_expiry_hours),
            admin_username: env::var("ADMIN_USERNAME").unwrap_or_else(|_| default_admin_username()),
            admin_password: env::var("ADMIN_PASSWORD").unwrap_or_else(|_| default_admin_password()),
        })
    }

    /// 获取 JWT 密钥（如果未配置则生成随机密钥）
    pub fn get_jwt_secret(&self) -> String {
        self.jwt_secret.clone().unwrap_or_else(|| {
            // 生成随机密钥
            use sha2::{Sha256, Digest};
            let random_bytes: [u8; 32] = std::array::from_fn(|_| fastrand::u8(..));
            let mut hasher = Sha256::new();
            hasher.update(&random_bytes);
            hex::encode(hasher.finalize())
        })
    }
}
