mod admin;
mod admin_ui;
mod anthropic;
mod common;
mod http_client;
mod kiro;
mod model;
mod stats;
pub mod token;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::stats::StatsStore;

use anyhow::Context;
use clap::Parser;
use kiro::model::credentials::{CredentialsConfig, KiroCredentials};
use kiro::provider::KiroProvider;
use kiro::token_manager::MultiTokenManager;
use model::arg::Args;
use model::config::Config;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use anthropic::{HistoryStoreConfig, init_global_store, start_cleanup_task};

type AppResult<T> = anyhow::Result<T>;

fn init_tracing() -> Option<WorkerGuard> {
    let filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive(tracing::Level::INFO.into());

    // 控制台 layer
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_ansi(true)
        .with_target(true);

    // 文件 layer：按天滚动（daily）+ 异步写入（non_blocking）
    let log_dir = resolve_writable_log_dir();
    let file_appender = tracing_appender::rolling::daily(&log_dir, "kiro-rs.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true)
        .with_file(true)
        .with_writer(file_writer);

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    tracing::info!("日志文件按天输出到目录: {}", log_dir.display());
    Some(guard)
}

fn resolve_writable_log_dir() -> PathBuf {
    // 目标：输出到“当前程序所在目录”。但如果该目录不可写，自动回退到当前工作目录。
    let program_dir = resolve_program_dir();
    if is_dir_writable(&program_dir) {
        return program_dir;
    }

    let current_dir = resolve_current_dir_fallback();
    if is_dir_writable(&current_dir) {
        return current_dir;
    }

    PathBuf::from(".")
}

fn resolve_writable_data_dir(preferred: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = preferred {
        if is_dir_writable(&dir) {
            return dir;
        }
    }

    let program_dir = resolve_program_dir();
    if is_dir_writable(&program_dir) {
        return program_dir;
    }

    let current_dir = resolve_current_dir_fallback();
    if is_dir_writable(&current_dir) {
        return current_dir;
    }

    PathBuf::from(".")
}

fn resolve_stats_path(credentials_path: &str) -> PathBuf {
    let cred_path = PathBuf::from(credentials_path);
    let preferred_dir = cred_path.parent().map(|p| p.to_path_buf());
    let dir = resolve_writable_data_dir(preferred_dir);
    dir.join("credential-stats.json")
}

fn resolve_program_dir() -> PathBuf {
    match std::env::current_exe() {
        Ok(p) => match p.parent() {
            Some(dir) => dir.to_path_buf(),
            None => resolve_current_dir_fallback(),
        },
        Err(_) => resolve_current_dir_fallback(),
    }
}

fn resolve_current_dir_fallback() -> PathBuf {
    match std::env::current_dir() {
        Ok(dir) => dir,
        Err(_) => PathBuf::from("."),
    }
}

fn is_dir_writable(dir: &Path) -> bool {
    let test_path = dir.join(".kiro-rs.write_test");
    let write_res = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&test_path);

    match write_res {
        Ok(_f) => {
            let _ = std::fs::remove_file(&test_path);
            true
        }
        Err(_) => false,
    }
}

#[tokio::main]
async fn main() {
    // 初始化日志（尽量早，避免启动阶段丢日志）
    // - 控制台：便于实时观察
    // - 文件：按天滚动 + 异步写入，便于长期诊断
    let _log_guard = init_tracing();

    // panic hook：尽量把 panic 也打进日志，便于定位“跑着跑着断掉”
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::capture();
        tracing::error!("panic: {}\nbacktrace: {}", info, bt);
    }));

    if let Err(e) = run().await {
        tracing::error!("服务启动/运行失败: {:#}", e);
        std::process::exit(1);
    }
}

async fn run() -> AppResult<()> {
    // 解析命令行参数
    let args = Args::parse();

    // 加载配置
    let config_path = args
        .config
        .unwrap_or_else(|| Config::default_config_path().to_string());
    let config = Config::load(&config_path)
        .with_context(|| format!("加载配置失败: {}", config_path))?;

    // 加载凭证（支持单对象或数组格式）
    let credentials_path = args
        .credentials
        .unwrap_or_else(|| KiroCredentials::default_credentials_path().to_string());
    let credentials_config = CredentialsConfig::load(&credentials_path)
        .with_context(|| format!("加载凭证失败: {}", credentials_path))?;

    // 判断是否为多凭据格式（用于刷新后回写）
    let is_multiple_format = credentials_config.is_multiple();

    // 转换为按优先级排序的凭据列表
    let credentials_list = credentials_config.into_sorted_credentials();
    tracing::info!("已加载 {} 个凭据配置", credentials_list.len());

    // 加载/初始化统计存储（按凭据 ID）
    let stats_path = resolve_stats_path(&credentials_path);
    let stats_store = StatsStore::load_or_new(stats_path.clone())
        .with_context(|| format!("加载统计失败: {:?}", stats_path))?;
    tracing::info!("统计文件: {}", stats_path.display());

    // 初始化历史存储（与统计文件同目录）
    let history_dir = stats_path.parent()
        .map(|p| p.join("history"))
        .unwrap_or_else(|| PathBuf::from("history"));
    init_global_store(HistoryStoreConfig {
        storage_dir: history_dir.clone(),
        expire_secs: 24 * 60 * 60, // 24 小时过期
        enabled: true,
    });
    tracing::info!("历史存储目录: {}", history_dir.display());

    // 启动定期清理任务（每小时清理一次过期历史）
    start_cleanup_task(Duration::from_secs(60 * 60));

    let first_profile_arn = credentials_list
        .first()
        .and_then(|c| c.profile_arn.clone());

    // 获取 API Key
    let api_key = config
        .api_key
        .clone()
        .ok_or_else(|| anyhow::anyhow!("配置文件中未设置 apiKey"))?;

    // 构建代理配置
    let proxy_config = config.proxy_url.as_ref().map(|url| {
        let mut proxy = http_client::ProxyConfig::new(url);
        if let (Some(username), Some(password)) = (&config.proxy_username, &config.proxy_password) {
            proxy = proxy.with_auth(username, password);
        }
        proxy
    });

    if let Some(url) = config.proxy_url.as_ref() {
        tracing::info!("已配置 HTTP 代理: {}", url);
    }

    // 创建 MultiTokenManager 和 KiroProvider
    let token_manager = MultiTokenManager::new(
        config.clone(),
        credentials_list,
        proxy_config.clone(),
        Some(credentials_path.into()),
        is_multiple_format,
    )
    .context("创建 Token 管理器失败")?;

    // 启动时为缺少 profileArn 的 IdC 凭据尝试获取
    token_manager.fetch_missing_profile_arns().await;

    let token_manager = Arc::new(token_manager);

    let kiro_provider = KiroProvider::with_proxy(token_manager.clone(), proxy_config.clone())
        .context("创建 KiroProvider 失败")?
        .with_stats(stats_store.clone());

    // 初始化 count_tokens 配置
    token::init_config(token::CountTokensConfig {
        api_url: config.count_tokens_api_url.clone(),
        api_key: config.count_tokens_api_key.clone(),
        auth_type: config.count_tokens_auth_type.clone(),
        proxy: proxy_config,
        tls_backend: config.tls_backend,
    });

    // 构建 Anthropic API 路由
    let anthropic_app = anthropic::create_router_with_provider(
        &api_key,
        Some(kiro_provider),
        first_profile_arn,
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
            let admin_service = admin::AdminService::new(token_manager.clone(), Some(stats_store.clone()));
            let admin_state = admin::AdminState::new(admin_key, admin_service);
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

    // 启动服务器（带自动重试 / 重启）
    let addr = format!("{}:{}", config.host, config.port);
    log_routes(&addr, &api_key, admin_key_valid);

    serve_with_restart(addr, app).await
}

fn log_routes(addr: &str, api_key: &str, admin_key_valid: bool) {
    tracing::info!("启动 Anthropic API 端点: {}", addr);

    let mask_len = api_key.len() / 2;
    if mask_len > 0 {
        tracing::info!("API Key: {}***", &api_key[..mask_len]);
    } else {
        tracing::info!("API Key: <empty>");
    }

    tracing::info!("可用 API:");
    tracing::info!("  GET  /v1/models");
    tracing::info!("  POST /v1/messages");
    tracing::info!("  POST /v1/messages/count_tokens");
    if admin_key_valid {
        tracing::info!("Admin API:");
        tracing::info!("  GET  /api/admin/credentials");
        tracing::info!("  POST /api/admin/credentials/:index/disabled");
        tracing::info!("  POST /api/admin/credentials/:index/priority");
        tracing::info!("  POST /api/admin/credentials/:index/reset");
        tracing::info!("  GET  /api/admin/credentials/:index/balance");
        tracing::info!("Admin UI:");
        tracing::info!("  GET  /admin");
    }
}

async fn serve_with_restart(addr: String, app: axum::Router) -> AppResult<()> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // 捕获 Ctrl-C，触发优雅退出
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                tracing::info!("收到 Ctrl-C，开始优雅退出...");
                let _ = shutdown_tx.send(true);
            }
            Err(e) => {
                tracing::error!("注册 Ctrl-C 信号失败: {}", e);
                let _ = shutdown_tx.send(true);
            }
        }
    });

    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        if *shutdown_rx.borrow() {
            tracing::info!("退出：shutdown 信号已触发");
            return Ok(());
        }

        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => {
                tracing::info!("监听成功: {}", addr);
                backoff = Duration::from_secs(1);
                l
            }
            Err(e) => {
                tracing::error!("绑定端口失败（{}），{} 秒后重试: {}", addr, backoff.as_secs(), e);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        let mut shutdown_rx2 = shutdown_rx.clone();
        let shutdown_fut = async move {
            while !*shutdown_rx2.borrow() {
                if shutdown_rx2.changed().await.is_err() {
                    break;
                }
            }
        };

        let serve_res = axum::serve(listener, app.clone())
            .with_graceful_shutdown(shutdown_fut)
            .await;

        if *shutdown_rx.borrow() {
            tracing::info!("服务已停止（收到 shutdown 信号）");
            return Ok(());
        }

        match serve_res {
            Ok(()) => {
                tracing::error!("服务异常停止（serve 返回 Ok，但未收到 shutdown），{} 秒后重启", backoff.as_secs());
            }
            Err(e) => {
                tracing::error!("服务运行时错误，{} 秒后重启: {}", backoff.as_secs(), e);
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}
