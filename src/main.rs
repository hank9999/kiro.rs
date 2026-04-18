mod admin;
mod admin_ui;
mod anthropic;
mod common;
mod http_client;
mod kiro;
mod model;
mod monitoring;
mod openai;
pub mod token;

use std::{
    ffi::OsStr,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::Parser;
use kiro::model::credentials::{CredentialsConfig, KiroCredentials};
use kiro::provider::KiroProvider;
use kiro::token_manager::MultiTokenManager;
use model::arg::Args;
use model::config::Config;

fn absolutize_path(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn resolve_log_path(config_path: &str) -> PathBuf {
    if let Some(path) = std::env::var_os("KIRO_LOG_PATH") {
        let path = path.to_string_lossy().trim().to_string();
        if !path.is_empty() {
            return absolutize_path(&path);
        }
    }

    let config_path = absolutize_path(config_path);
    if let Some(config_dir) = config_path.parent() {
        if config_dir.file_name() == Some(OsStr::new("config")) {
            if let Some(app_dir) = config_dir.parent() {
                return app_dir.join("kiro.log");
            }
        }

        return config_dir.join("kiro.log");
    }

    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("kiro.log")
}

fn init_file_logger(log_path: &Path) -> tracing_appender::non_blocking::WorkerGuard {
    if let Some(log_dir) = log_path.parent() {
        if let Err(error) = std::fs::create_dir_all(log_dir) {
            eprintln!("创建日志目录失败 ({}): {}", log_dir.display(), error);
        }
    }

    let log_dir = log_path.parent().unwrap_or_else(|| Path::new("."));
    let log_name = log_path
        .file_name()
        .unwrap_or_else(|| OsStr::new("kiro.log"));
    let file_appender = tracing_appender::rolling::never(log_dir, log_name);
    let (non_blocking, log_guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let writer = std::io::stdout.and(non_blocking);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(writer)
        .init();

    log_guard
}

#[tokio::main]
async fn main() {
    // 解析命令行参数
    let args = Args::parse();

    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| Config::default_config_path().to_string());
    let log_path = resolve_log_path(&config_path);
    let _log_guard = init_file_logger(&log_path);

    // 加载配置
    let config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::error!("加载配置失败: {}", e);
        std::process::exit(1);
    });

    // 加载凭证（支持单对象或数组格式）
    let credentials_path = args
        .credentials
        .unwrap_or_else(|| KiroCredentials::default_credentials_path().to_string());
    let credentials_config = CredentialsConfig::load(&credentials_path).unwrap_or_else(|e| {
        tracing::error!("加载凭证失败: {}", e);
        std::process::exit(1);
    });

    // 判断是否为多凭据格式（用于刷新后回写）
    let is_multiple_format = credentials_config.is_multiple();

    // 转换为按优先级排序的凭据列表
    let credentials_list = credentials_config.into_sorted_credentials();
    tracing::info!("已加载 {} 个凭据配置", credentials_list.len());

    // 获取第一个凭据用于日志显示
    let first_credentials = credentials_list.first().cloned().unwrap_or_default();
    tracing::debug!("主凭证: {:?}", first_credentials);

    // 收集所有有效的 API Keys
    let api_keys = config.collect_valid_keys();
    if api_keys.is_empty() {
        tracing::error!("配置文件中未设置有效的 API Key");
        std::process::exit(1);
    }
    tracing::info!("已加载 {} 个 API Key", api_keys.len());

    // 构建代理配置
    let proxy_config = config.proxy_url.as_ref().map(|url| {
        let mut proxy = http_client::ProxyConfig::new(url);
        if let (Some(username), Some(password)) = (&config.proxy_username, &config.proxy_password) {
            proxy = proxy.with_auth(username, password);
        }
        proxy
    });

    if proxy_config.is_some() {
        tracing::info!("已配置 HTTP 代理: {}", config.proxy_url.as_ref().unwrap());
    }

    // 构建代理池（用于 IP 轮询）
    let proxy_pool = config.build_proxy_pool().map(Arc::new);
    if let Some(pool) = proxy_pool.as_ref() {
        tracing::info!(
            "已启用代理池：共 {} 个代理，策略 {}",
            pool.len(),
            pool.strategy().as_str()
        );
    }

    // 创建 MultiTokenManager 和 KiroProvider
    let token_manager = MultiTokenManager::new(
        config.clone(),
        credentials_list,
        proxy_config.clone(),
        proxy_pool.clone(),
        Some(credentials_path.into()),
        is_multiple_format,
    )
    .unwrap_or_else(|e| {
        tracing::error!("创建 Token 管理器失败: {}", e);
        std::process::exit(1);
    });
    let token_manager = Arc::new(token_manager);
    let kiro_provider = KiroProvider::with_proxy_and_pool(
        token_manager.clone(),
        proxy_config.clone(),
        proxy_pool.clone(),
    );
    let request_monitor = monitoring::RequestMonitor::new(500);

    // 初始化 count_tokens 配置
    token::init_config(token::CountTokensConfig {
        api_url: config.count_tokens_api_url.clone(),
        api_key: config.count_tokens_api_key.clone(),
        auth_type: config.count_tokens_auth_type.clone(),
        proxy: proxy_config,
        tls_backend: config.tls_backend,
    });

    // 创建共享的 AppState
    let app_state = anthropic::middleware::AppState::new(
        api_keys.clone(),
        config_path.clone().into(),
        request_monitor.clone(),
        config.extract_thinking,
    );

    // 构建 Anthropic API 路由（profile_arn 由 provider 层根据实际凭据动态注入）
    let anthropic_app = anthropic::create_router_with_provider(
        app_state.clone(),
        Some(kiro_provider),
    );

    // 构建 Admin API 路由（如果配置了非空的 admin_api_key）
    // 安全检查：空字符串被视为未配置，防止空 key 绕过认证
    let admin_key_valid = config
        .admin_api_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);

    let app = if let Some(admin_key) = &config.admin_api_key {
        if admin_key.trim().is_empty() {
            tracing::warn!("admin_api_key 配置为空，Admin API 未启用");
            anthropic_app
        } else {
            let admin_service = admin::AdminService::new(
                token_manager.clone(),
                request_monitor.clone(),
                log_path,
                config_path.into(),
            );
            let admin_state = admin::AdminState::new(admin_key, admin_service, app_state);
            let admin_app = admin::create_admin_router(admin_state);

            // 创建 Admin UI 路由
            let admin_ui_app = admin_ui::create_admin_ui_router();

            tracing::info!("Admin API 已启用");
            tracing::info!("Admin UI 已启用: /admin");
            anthropic_app
                .nest("/api/admin", admin_app)
                .nest("/admin", admin_ui_app)
        }
    } else {
        anthropic_app
    };

    // 启动服务器
    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("启动 Anthropic API 端点: {}", addr);
    tracing::info!("已加载 {} 个 API Key", api_keys.len());
    tracing::info!("可用 API:");
    tracing::info!("  GET  /v1/models");
    tracing::info!("  POST /v1/messages");
    tracing::info!("  POST /v1/chat/completions");
    tracing::info!("  POST /v1/responses");
    tracing::info!("  POST /v1/messages/count_tokens");
    if admin_key_valid {
        tracing::info!("Admin API:");
        tracing::info!("  GET  /api/admin/credentials");
        tracing::info!("  GET  /api/admin/models");
        tracing::info!("  POST /api/admin/credentials/:index/disabled");
        tracing::info!("  POST /api/admin/credentials/:index/priority");
        tracing::info!("  POST /api/admin/credentials/:index/reset");
        tracing::info!("  GET  /api/admin/credentials/:index/balance");
        tracing::info!("Admin UI:");
        tracing::info!("  GET  /admin");
    }

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
