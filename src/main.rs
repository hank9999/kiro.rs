mod admin;
mod admin_ui;
mod anthropic;
mod common;
mod http_client;
mod kiro;
mod model;
mod monitoring;
mod openai;
mod shared_state;
pub mod token;

use std::{
    collections::HashMap,
    ffi::OsStr,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Datelike, FixedOffset, TimeZone, Utc};
use clap::Parser;
use kiro::endpoint::{IdeEndpoint, KiroEndpoint};
use kiro::model::credentials::{CredentialsConfig, KiroCredentials};
use kiro::provider::KiroProvider;
use kiro::token_manager::MultiTokenManager;
use model::arg::Args;
use model::config::Config;
use shared_state::ModelsCacheHandle;

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

fn next_monthly_enable_time_utc(now_utc: DateTime<Utc>) -> DateTime<Utc> {
    let utc_plus_one = FixedOffset::east_opt(3600).expect("UTC+01:00 是有效固定时区");
    let now_local = now_utc.with_timezone(&utc_plus_one);

    let mut year = now_local.year();
    let mut month = now_local.month();

    let this_month = utc_plus_one
        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .expect("每月 1 号 00:00 在固定时区中唯一存在");

    let next_local = if now_local <= this_month {
        this_month
    } else {
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }

        utc_plus_one
            .with_ymd_and_hms(year, month, 1, 0, 0, 0)
            .single()
            .expect("每月 1 号 00:00 在固定时区中唯一存在")
    };

    next_local.with_timezone(&Utc)
}

fn duration_until(target_utc: DateTime<Utc>) -> std::time::Duration {
    let delta = target_utc - Utc::now();
    delta.to_std().unwrap_or_else(|_| std::time::Duration::ZERO)
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

#[cfg(test)]
mod tests {
    use super::next_monthly_enable_time_utc;
    use chrono::{TimeZone, Utc};

    #[test]
    fn next_monthly_enable_time_uses_next_month_when_after_first_utc_plus_one() {
        let now = Utc.with_ymd_and_hms(2026, 5, 25, 8, 0, 0).unwrap();
        let next = next_monthly_enable_time_utc(now);
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 31, 23, 0, 0).unwrap());
    }

    #[test]
    fn next_monthly_enable_time_keeps_current_month_before_first_utc_plus_one() {
        let now = Utc.with_ymd_and_hms(2026, 4, 30, 22, 30, 0).unwrap();
        let next = next_monthly_enable_time_utc(now);
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 4, 30, 23, 0, 0).unwrap());
    }

    #[test]
    fn next_monthly_enable_time_runs_immediately_at_exact_first_utc_plus_one() {
        let now = Utc.with_ymd_and_hms(2026, 4, 30, 23, 0, 0).unwrap();
        let next = next_monthly_enable_time_utc(now);
        assert_eq!(next, now);
    }
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
    let mut credentials_list = credentials_config.into_sorted_credentials();

    // 检查 KIRO_API_KEY 环境变量，自动创建 API Key 凭据
    if let Ok(kiro_api_key) = std::env::var("KIRO_API_KEY") {
        if kiro_api_key.is_empty() {
            tracing::warn!("KIRO_API_KEY 环境变量已设置但为空，视为未配置");
        } else {
            tracing::info!("检测到 KIRO_API_KEY 环境变量，添加 API Key 凭据（最高优先级）");
            let api_key_cred = KiroCredentials {
                kiro_api_key: Some(kiro_api_key),
                auth_method: Some("api_key".to_string()),
                priority: 0,
                ..Default::default()
            };
            credentials_list.insert(0, api_key_cred);
        }
    }

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
        let mut proxy = http_client::ProxyConfig::from_user_input(url, "http");
        if let (Some(username), Some(password)) = (&config.proxy_username, &config.proxy_password) {
            if proxy.username.is_none() {
                proxy = proxy.with_auth(username, password);
            }
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

    // 构建端点注册表（上游新增：支持按凭据选择 ide/cli 端点）
    let mut endpoints: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
    {
        let ide = IdeEndpoint::new();
        endpoints.insert(ide.name().to_string(), Arc::new(ide));
    }

    // 校验默认端点存在
    if !endpoints.contains_key(&config.default_endpoint) {
        tracing::error!("默认端点 \"{}\" 未注册", config.default_endpoint);
        std::process::exit(1);
    }

    // 校验所有凭据声明的端点都已注册
    for cred in &credentials_list {
        let name = cred.endpoint.as_deref().unwrap_or(&config.default_endpoint);
        if !endpoints.contains_key(name) {
            tracing::error!(
                "凭据 id={:?} 指定了未知端点 \"{}\"（已注册: {:?}）",
                cred.id,
                name,
                endpoints.keys().collect::<Vec<_>>()
            );
            std::process::exit(1);
        }
    }

    let endpoint_names: Vec<String> = endpoints.keys().cloned().collect();

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
        endpoints,
        config.default_endpoint.clone(),
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

    // 创建动态模型列表缓存（首次启动用 fallback 列表填充）
    // anthropic AppState 与 AdminService 共享同一个 handle
    let models_cache = ModelsCacheHandle::new_with_fallback(anthropic::fallback_models());

    // 创建共享的 AppState
    let app_state = anthropic::middleware::AppState::new(
        api_keys.clone(),
        config_path.clone().into(),
        request_monitor.clone(),
        config.extract_thinking,
        models_cache.clone(),
    );

    // 构建 Anthropic API 路由（profile_arn 由 provider 层根据实际凭据动态注入）
    let anthropic_app =
        anthropic::create_router_with_provider(app_state.clone(), Some(kiro_provider));

    // 总是构造 AdminService（用于后台周期刷新模型列表，即便 admin API 路由未启用）
    let admin_service = Arc::new(admin::AdminService::new(
        token_manager.clone(),
        request_monitor.clone(),
        log_path.clone(),
        config_path.clone().into(),
        endpoint_names.clone(),
        models_cache.clone(),
    ));

    // 启动动态模型列表刷新后台任务
    if config.dynamic_models.enabled {
        tracing::info!(
            interval_secs = config.dynamic_models.refresh_interval_secs,
            initial_delay_secs = config.dynamic_models.initial_delay_secs,
            "动态模型列表刷新已启用",
        );
        let svc = admin_service.clone();
        let initial_delay = config.dynamic_models.initial_delay_secs;
        let interval_secs = config.dynamic_models.refresh_interval_secs;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(initial_delay)).await;
            let mut backoff_secs: u64 = 30;
            loop {
                match svc.refresh_models().await {
                    Ok(_) => {
                        backoff_secs = 30;
                        tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            backoff_secs,
                            "动态模型列表刷新失败，稍后重试",
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                        backoff_secs = (backoff_secs.saturating_mul(2)).min(300);
                    }
                }
            }
        });
    } else {
        tracing::info!("动态模型列表刷新已禁用，使用静态 fallback 列表");
    }

    // 每月 1 号 UTC+01:00 自动启用全部可恢复凭据。
    // 这里仅恢复禁用状态与失败计数，配置无效的凭据仍保持禁用，避免月初反复触发无效账号。
    {
        let svc = admin_service.clone();
        tokio::spawn(async move {
            loop {
                let next_run = next_monthly_enable_time_utc(Utc::now());
                tracing::info!(
                    next_run_utc = %next_run.to_rfc3339(),
                    "凭据月初自动启用任务已计划"
                );

                tokio::time::sleep(duration_until(next_run)).await;

                match svc.enable_all_credentials() {
                    Ok(count) => tracing::info!(enabled = count, "月初自动启用凭据任务完成"),
                    Err(error) => tracing::warn!(
                        error = %error,
                        "月初自动启用凭据任务失败"
                    ),
                }

                // 防止在调度点附近因时钟精度立刻重复命中同一月份。
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    }

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
            let admin_state = admin::AdminState::new(admin_key, admin_service.clone(), app_state);
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
        tracing::info!("  POST /api/admin/credentials/query-balances-enable");
        tracing::info!("  POST /api/admin/credentials/enable-all");
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
