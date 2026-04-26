mod config;
mod domain;
mod infra;
mod interface;
mod service;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

use crate::config::Config;
use crate::domain::credential::Credential;
use crate::domain::endpoint::KiroEndpoint;
use crate::domain::retry::RetryPolicy;
use crate::infra::endpoint::{EndpointRegistry, IdeEndpoint};
use crate::infra::http::client::ProxyConfig;
use crate::infra::http::executor::RequestExecutor;
use crate::infra::http::retry::DefaultRetryPolicy;
use crate::infra::machine_id::MachineIdResolver;
use crate::infra::storage::{BalanceCacheStore, CredentialsFileStore, StatsFileStore};
use crate::interface::cli::Args;
use crate::interface::http::admin as http_admin;
use crate::interface::http::anthropic as http_anthropic;
use crate::interface::http::ui as http_ui;
use crate::service::admin::AdminService;
use crate::service::credential_pool::{
    CredentialPool, CredentialState, CredentialStats, CredentialStore, EntryStats,
};
use crate::service::KiroClient;

const DEFAULT_CREDENTIALS_PATH: &str = "credentials.json";

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // ====== 加载配置 ======
    let config_path: PathBuf = args
        .config
        .unwrap_or_else(|| Config::default_config_path().to_string())
        .into();
    let config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::error!("加载配置失败: {}", e);
        std::process::exit(1);
    });
    let config = Arc::new(config);

    // ====== 加载凭据 ======
    let credentials_path: PathBuf = args
        .credentials
        .unwrap_or_else(|| DEFAULT_CREDENTIALS_PATH.to_string())
        .into();
    let file_store = Arc::new(CredentialsFileStore::new(Some(credentials_path.clone())));
    let resolver = Arc::new(MachineIdResolver::new());

    let (cred_store, validation_issues) =
        CredentialStore::load(file_store, config.clone(), resolver.clone()).unwrap_or_else(|e| {
            tracing::error!("加载凭证失败: {}", e);
            std::process::exit(1);
        });
    let cred_store = Arc::new(cred_store);

    // 校验问题日志输出
    for issue in &validation_issues {
        tracing::warn!("凭据装载校验问题: {}", issue.message);
    }

    // ====== 加载 stats（与 credentials.json 同目录的 kiro_stats.json）======
    let stats_path = credentials_path
        .parent()
        .map(|dir| dir.join("kiro_stats.json"));
    let stats_file = Arc::new(StatsFileStore::new(stats_path));
    let initial_stats: HashMap<u64, EntryStats> = stats_file
        .load()
        .into_iter()
        .map(|(id, e)| (id, EntryStats::from_storage(e)))
        .collect();

    let cred_state = Arc::new(CredentialState::new());
    let cred_stats = Arc::new(CredentialStats::new());

    // ====== 构建 CredentialPool ======
    let pool = Arc::new(CredentialPool::new(
        cred_store.clone(),
        cred_state.clone(),
        cred_stats.clone(),
        Some(stats_file.clone()),
        config.clone(),
        resolver.clone(),
    ));

    // 装载初始 state（disabled + invalid_config）
    let invalid_config_ids: HashSet<u64> = validation_issues.iter().map(|i| i.id).collect();
    let initial_disabled_ids: HashSet<u64> = cred_store
        .snapshot()
        .iter()
        .filter(|(_, c)| c.disabled)
        .map(|(id, _)| *id)
        .collect();
    pool.install_initial_states(&invalid_config_ids, &initial_disabled_ids);
    pool.install_initial_stats(initial_stats);

    // ====== 处理 KIRO_API_KEY 环境变量 ======
    if let Ok(kiro_api_key) = std::env::var("KIRO_API_KEY") {
        if kiro_api_key.is_empty() {
            tracing::warn!("KIRO_API_KEY 环境变量已设置但为空，视为未配置");
        } else {
            tracing::info!("检测到 KIRO_API_KEY 环境变量，添加 API Key 凭据（最高优先级）");
            let api_key_cred = Credential {
                kiro_api_key: Some(kiro_api_key),
                auth_method: Some("api_key".to_string()),
                priority: 0,
                ..Default::default()
            };
            if let Err(e) = pool.add_credential(api_key_cred).await {
                tracing::warn!("添加 KIRO_API_KEY 凭据失败: {}", e);
            }
        }
    }

    tracing::info!("已加载 {} 个凭据配置", pool.total_count());

    // ====== API Key 鉴权 ======
    let api_key = config.api_key.clone().unwrap_or_else(|| {
        tracing::error!("配置文件中未设置 apiKey");
        std::process::exit(1);
    });

    // ====== 全局代理配置 ======
    let global_proxy = config.proxy.proxy_url.as_deref().map(|url| {
        let mut p = ProxyConfig::new(url);
        if let (Some(u), Some(pw)) = (
            &config.proxy.proxy_username,
            &config.proxy.proxy_password,
        ) {
            p = p.with_auth(u, pw);
        }
        p
    });
    if let Some(p) = &global_proxy {
        tracing::info!("已配置 HTTP 代理: {}", p.url);
    }

    // ====== 端点注册表 ======
    let mut endpoints_map: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
    {
        let ide = IdeEndpoint::new();
        endpoints_map.insert(ide.name().to_string(), Arc::new(ide));
    }
    let endpoints = match EndpointRegistry::new(&config.endpoint.default_endpoint, endpoints_map) {
        Ok(reg) => Arc::new(reg),
        Err(e) => {
            tracing::error!("构建端点注册表失败: {}", e);
            std::process::exit(1);
        }
    };

    // 校验所有凭据声明的端点都已注册
    for (id, cred) in cred_store.snapshot() {
        let name = cred
            .endpoint
            .as_deref()
            .unwrap_or(&config.endpoint.default_endpoint);
        if !endpoints.contains(name) {
            tracing::error!(
                "凭据 #{} 指定了未知端点 \"{}\"（已注册: {:?}）",
                id,
                name,
                endpoints.names()
            );
            std::process::exit(1);
        }
    }

    let endpoint_names: Vec<String> = endpoints.names();

    // ====== KiroClient ======
    let executor = Arc::new(RequestExecutor::new(config.clone(), global_proxy.clone()));
    let policy: Arc<dyn RetryPolicy> = Arc::new(DefaultRetryPolicy::new());
    let kiro_client = Arc::new(KiroClient::new(
        executor,
        pool.clone(),
        endpoints.clone(),
        policy,
    ));

    // ====== Anthropic 路由 ======
    let anthropic_app = http_anthropic::create_router(
        &api_key,
        Some(kiro_client),
        config.features.extract_thinking,
    );

    // ====== Admin 路由（如配置 admin_api_key）======
    let admin_key_valid = config
        .admin
        .admin_api_key
        .as_deref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);

    let app = if let Some(admin_key) = config.admin.admin_api_key.as_deref() {
        if admin_key.trim().is_empty() {
            tracing::warn!("adminApiKey 配置为空，Admin API 未启用");
            anthropic_app
        } else {
            // BalanceCacheStore 路径：与 credentials.json 同目录
            let cache_path = credentials_path
                .parent()
                .map(|d| d.join("kiro_balance_cache.json"));
            let balance_cache = Arc::new(BalanceCacheStore::new(cache_path));
            let admin_service =
                AdminService::new(pool.clone(), balance_cache, endpoint_names.clone());
            let admin_state = http_admin::AdminState::new(admin_key, admin_service);
            let admin_app = http_admin::create_admin_router(admin_state);
            let ui_app = http_ui::create_admin_ui_router();
            tracing::info!("Admin API 已启用");
            tracing::info!("Admin UI 已启用: /admin");
            anthropic_app
                .nest("/api/admin", admin_app)
                .nest("/admin", ui_app)
        }
    } else {
        anthropic_app
    };

    // ====== 启动 HTTP ======
    let addr = format!("{}:{}", config.net.host, config.net.port);
    tracing::info!("启动 Anthropic API 端点: {}", addr);
    // 仅打印前 4 字符 + 长度，避免日志泄露半个 API Key
    tracing::info!(
        "API Key: {}***（长度 {}）",
        &api_key[..api_key.len().min(4)],
        api_key.len()
    );
    tracing::info!("可用 API:");
    tracing::info!("  GET  /v1/models");
    tracing::info!("  POST /v1/messages");
    tracing::info!("  POST /v1/messages/count_tokens");
    tracing::info!("  POST /cc/v1/messages");
    tracing::info!("  POST /cc/v1/messages/count_tokens");
    if admin_key_valid {
        tracing::info!("Admin API:");
        tracing::info!("  GET    /api/admin/credentials");
        tracing::info!("  POST   /api/admin/credentials");
        tracing::info!("  DELETE /api/admin/credentials/:id");
        tracing::info!("  POST   /api/admin/credentials/:id/disabled");
        tracing::info!("  POST   /api/admin/credentials/:id/priority");
        tracing::info!("  POST   /api/admin/credentials/:id/reset");
        tracing::info!("  POST   /api/admin/credentials/:id/refresh");
        tracing::info!("  GET    /api/admin/credentials/:id/balance");
        tracing::info!("  GET    /api/admin/config/load-balancing");
        tracing::info!("  PUT    /api/admin/config/load-balancing");
        tracing::info!("Admin UI:");
        tracing::info!("  GET  /admin");
    }

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let pool_for_shutdown = pool.clone();
    // graceful shutdown：信号触发后等所有连接关闭再返回；用 timeout 兜底防止
    // 长 SSE 连接卡住 flush_stats（30s 上限）
    let serve = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());
    match tokio::time::timeout(std::time::Duration::from_secs(30), serve).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!("HTTP 服务异常退出: {}", e),
        Err(_) => tracing::warn!("graceful shutdown 超时 30s，强制退出（仍会 flush_stats）"),
    }

    // 进程退出前显式 flush stats，避免最后一窗口的统计因未到 30s debounce 丢失
    pool_for_shutdown.flush_stats();
    tracing::info!("stats 已落盘，进程退出");
}

async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("收到关停信号，准备 graceful shutdown");
}
