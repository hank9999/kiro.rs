mod admin;
mod admin_ui;
mod anthropic;
mod common;
mod http_client;
mod image_resize;
mod kiro;
mod model;
pub mod token;

use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use kiro::endpoint::{CliEndpoint, IdeEndpoint, KiroEndpoint};
use kiro::model::credentials::{CredentialsConfig, KiroCredentials};
use kiro::provider::KiroProvider;
use kiro::token_manager::MultiTokenManager;
use model::arg::Args;
use model::config::Config;

#[tokio::main]
async fn main() {
    // 解析命令行参数
    let args = Args::parse();

    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 解析配置/凭证路径
    let config_path = args
        .config
        .unwrap_or_else(|| Config::default_config_path().to_string());
    let credentials_path = args
        .credentials
        .unwrap_or_else(|| KiroCredentials::default_credentials_path().to_string());

    // 文件不存在时自动初始化（Docker 首次部署友好）
    ensure_config_files(&config_path, &credentials_path);

    // 加载配置
    let config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::error!("加载配置失败: {}", e);
        std::process::exit(1);
    });

    // 加载凭证（支持单对象或数组格式）
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

    // 仅显示安全的元数据，避免在日志里泄露 token / client_secret
    let first_credentials = credentials_list.first().cloned().unwrap_or_default();
    tracing::debug!(
        id = ?first_credentials.id,
        email = ?first_credentials.email,
        auth_method = ?first_credentials.auth_method,
        priority = first_credentials.priority,
        endpoint = ?first_credentials.endpoint,
        "已选定主凭证"
    );

    // 获取 API Key
    let api_key = config.api_key.clone().unwrap_or_else(|| {
        tracing::error!("配置文件中未设置 apiKey");
        std::process::exit(1);
    });

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

    // 构建端点注册表
    let mut endpoints: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
    {
        let ide = IdeEndpoint::new();
        endpoints.insert(ide.name().to_string(), Arc::new(ide));
        let cli = CliEndpoint::new();
        endpoints.insert(cli.name().to_string(), Arc::new(cli));
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
        Some(credentials_path.into()),
        is_multiple_format,
    )
    .unwrap_or_else(|e| {
        tracing::error!("创建 Token 管理器失败: {}", e);
        std::process::exit(1);
    });
    let token_manager = Arc::new(token_manager);
    let kiro_provider = KiroProvider::with_proxy(
        token_manager.clone(),
        proxy_config.clone(),
        endpoints,
        config.default_endpoint.clone(),
    );

    // 初始化 count_tokens 配置
    token::init_config(token::CountTokensConfig {
        api_url: config.count_tokens_api_url.clone(),
        api_key: config.count_tokens_api_key.clone(),
        auth_type: config.count_tokens_auth_type.clone(),
        proxy: proxy_config,
        tls_backend: config.tls_backend,
    });

    // 客户端 Key 管理器 + 用量记录器 + 聚合器（与凭据文件同目录）
    let cache_dir = token_manager
        .cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let client_keys_path = admin::client_keys::default_path_in(&cache_dir);
    let client_key_manager = std::sync::Arc::new(
        admin::ClientKeyManager::load(&client_keys_path).unwrap_or_else(|e| {
            tracing::warn!("加载客户端 Key 失败 ({}): {}", client_keys_path.display(), e);
            admin::ClientKeyManager::new()
        }),
    );
    let usage_recorder = std::sync::Arc::new(admin::UsageRecorder::with_retention(
        cache_dir.clone(),
        config.usage_log_retention_days as i64,
    ));
    let usage_aggregator = std::sync::Arc::new(admin::UsageAggregator::new());
    usage_aggregator.rebuild_from_logs(&cache_dir);

    // 请求链路追踪存储（SQLite，traces.db）。失败不致命：trace 不可用但服务正常。
    let trace_store: Option<admin::SharedTraceStore> = match admin::TraceStore::open(
        cache_dir.join("traces.db"),
        config.trace_enabled,
        config.trace_retention_days,
    ) {
        Ok(s) => Some(std::sync::Arc::new(s)),
        Err(e) => {
            tracing::warn!("打开 traces.db 失败，请求链路追踪不可用: {}", e);
            None
        }
    };

    // 启动后定期清理过期 usage_log 与 trace 记录
    {
        let recorder = usage_recorder.clone();
        let trace_store = trace_store.clone();
        tokio::spawn(async move {
            let day = std::time::Duration::from_secs(24 * 3600);
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                recorder.cleanup_old_logs();
                if let Some(ts) = &trace_store {
                    ts.cleanup();
                }
                tokio::time::sleep(day).await;
            }
        });
    }

    // 构建 Anthropic API 路由（profile_arn 由 provider 层根据实际凭据动态注入）
    // 把 api_key 包成 Arc<RwLock<...>>，以便 Admin 模块运行时改 key 后立刻生效
    let shared_api_key = std::sync::Arc::new(parking_lot::RwLock::new(api_key.clone()));

    // PromptCache：基于 cache_control 断点的进程内提示词缓存
    // 持久化到 cache_dir/prompt_cache.json，启动时自动加载有效条目
    let prompt_cache = std::sync::Arc::new(anthropic::prompt_cache::PromptCache::new(Some(
        cache_dir.join("prompt_cache.json"),
    )));
    prompt_cache.clone().spawn_background();

    let anthropic_app = anthropic::create_router_with_shared_key(
        shared_api_key.clone(),
        Some(kiro_provider),
        config.extract_thinking,
        Some(client_key_manager.clone()),
        Some(usage_recorder.clone()),
        Some(usage_aggregator.clone()),
        Some(prompt_cache.clone()),
        trace_store.clone(),
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
            // Admin 查询需要一个确定的 store；traces.db 打开失败时用内存兜底（仅本进程有效）
            let admin_trace_store = trace_store.clone().unwrap_or_else(|| {
                std::sync::Arc::new(
                    admin::TraceStore::open_in_memory()
                        .expect("内存 trace store 初始化失败"),
                )
            });
            let admin_service =
                admin::AdminService::new(token_manager.clone(), endpoint_names.clone())
                    .with_log_governance(
                        Some(admin_trace_store.clone()),
                        Some(usage_recorder.clone()),
                    );
            let admin_state = admin::AdminState::new(
                admin_key,
                shared_api_key.clone(),
                admin_service,
                client_key_manager.clone(),
                usage_aggregator.clone(),
                admin_trace_store,
            );

            // 启动余额后台刷新调度器（每 5 分钟一次，与缓存 TTL 对齐）
            admin_state
                .service
                .start_balance_refresher(std::time::Duration::from_secs(300));

            // 启动代理池健康检查调度器（每 5 分钟一次）
            admin_state
                .service
                .start_proxy_health_checker(std::time::Duration::from_secs(300));

            // 启动自动更新调度器：每分钟检查一次本地时间，到达 update_auto_apply_time
            // 且开启 update_auto_apply 时执行一次更新；否则静默等待。
            admin_state.service.start_auto_update_scheduler();

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
    tracing::info!("API Key: {}***", &api_key[..(api_key.len() / 2)]);
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

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// 文件不存在时初始化配置/凭证文件
///
/// - `config.json`：写入带随机 `apiKey` / `adminApiKey` 的最小默认配置，
///   `host` 设为 `0.0.0.0` 以适配容器场景，端口/默认端点等其余字段沿用代码默认值。
/// - `credentials.json`：写入空数组 `[]`，便于后续通过 Admin UI 添加凭据。
///
/// 任一步失败都仅打印警告，不中断启动；后续 `Config::load` / `CredentialsConfig::load`
/// 仍会按既有逻辑处理（失败再退出）。
fn ensure_config_files(config_path: &str, credentials_path: &str) {
    let config_p = std::path::Path::new(config_path);
    if !config_p.exists() {
        if let Some(parent) = config_p.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!("创建配置目录失败 {}: {}", parent.display(), e);
                }
            }
        }
        let api_key = format!("sk-kiro-rs-{}", random_token(24));
        let admin_api_key = format!("sk-admin-{}", random_token(24));
        let default = serde_json::json!({
            "host": "0.0.0.0",
            "port": 8990,
            "apiKey": api_key,
            "adminApiKey": admin_api_key,
            "region": "us-east-1",
            "tlsBackend": "rustls",
            "defaultEndpoint": "ide"
        });
        match serde_json::to_string_pretty(&default)
            .map_err(anyhow::Error::from)
            .and_then(|s| std::fs::write(config_p, s).map_err(anyhow::Error::from))
        {
            Ok(_) => {
                tracing::info!("已生成默认配置: {}", config_p.display());
                tracing::info!("  apiKey      = {}", api_key);
                tracing::info!("  adminApiKey = {}", admin_api_key);
                tracing::info!("请妥善保存上述密钥，可在配置文件中修改");
            }
            Err(e) => tracing::warn!("写入默认配置失败 {}: {}", config_p.display(), e),
        }
    }

    let cred_p = std::path::Path::new(credentials_path);
    if !cred_p.exists() {
        if let Some(parent) = cred_p.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!("创建凭证目录失败 {}: {}", parent.display(), e);
                }
            }
        }
        if let Err(e) = std::fs::write(cred_p, "[]\n") {
            tracing::warn!("写入空凭证文件失败 {}: {}", cred_p.display(), e);
        } else {
            tracing::info!("已生成空凭证文件: {}（可通过 Admin UI 添加凭据）", cred_p.display());
        }
    }
}

/// 生成一段长度为 `len` 的字母数字随机字符串，用于默认 API Key
fn random_token(len: usize) -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    (0..len)
        .map(|_| {
            let idx = fastrand::usize(..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}
