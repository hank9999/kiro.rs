//! Kiro API 代理服务
//!
//! Input: .env 启动配置, SQLite 数据库运行时配置
//! Output: HTTP 服务
//! Pos: 应用入口

mod admin;
mod anthropic;
mod common;
mod db;
mod http_client;
mod jwt;
mod kiro;
mod model;
pub mod token;

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use kiro::provider::KiroProvider;
use kiro::token_manager::MultiTokenManager;
use model::arg::Args;
use model::config::Config;

use db::{Database, DbInitConfig};
use jwt::JwtManager;

#[tokio::main]
async fn main() {
    // 解析命令行参数
    let args = Args::parse();

    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    // 加载配置
    let config_path = args
        .config
        .unwrap_or_else(|| Config::default_config_path().to_string());
    let config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::error!("加载配置失败: {}", e);
        std::process::exit(1);
    });

    // 初始化数据库
    let db_init_config = DbInitConfig {
        admin_username: config.admin_username.clone(),
        admin_password: config.admin_password.clone(),
    };
    let db = Database::open_with_config(&config.db_path, db_init_config).unwrap_or_else(|e| {
        tracing::error!("初始化数据库失败: {}", e);
        std::process::exit(1);
    });
    tracing::info!("数据库已初始化: {}", config.db_path);

    // 从数据库加载运行时配置
    let runtime_config = db.get_runtime_config();
    tracing::info!(
        "运行时配置: kiro_version={}, system_version={}",
        runtime_config.kiro_version,
        runtime_config.system_version
    );

    // 从数据库加载凭据
    let credentials_list = db.load_credentials_list().unwrap_or_else(|e| {
        tracing::error!("加载凭据失败: {}", e);
        std::process::exit(1);
    });
    tracing::info!("已加载 {} 个凭据配置", credentials_list.len());

    // 获取第一个凭据用于日志显示
    let first_credentials = credentials_list.first().cloned().unwrap_or_default();
    tracing::debug!("主凭证: {:?}", first_credentials);

    // 获取 API Key
    let api_key = config.api_key.clone().unwrap_or_else(|| {
        tracing::error!("配置文件中未设置 apiKey");
        std::process::exit(1);
    });

    // 创建 MultiTokenManager 和 KiroProvider
    let token_manager = MultiTokenManager::new_with_db(
        runtime_config.clone(),
        credentials_list,
        db.clone(),
    )
    .unwrap_or_else(|e| {
        tracing::error!("创建 Token 管理器失败: {}", e);
        std::process::exit(1);
    });
    let token_manager = Arc::new(token_manager);
    let kiro_provider = KiroProvider::new(token_manager.clone());

    // 初始化 count_tokens 配置（从数据库运行时配置）
    token::init_config(token::CountTokensConfig {
        api_url: runtime_config.count_tokens_api_url.clone(),
        api_key: runtime_config.count_tokens_api_key.clone(),
        auth_type: runtime_config.count_tokens_auth_type.clone(),
        proxy_url: None,
    });

    // 构建 Anthropic API 路由（从第一个凭据获取 profile_arn）
    let anthropic_app = anthropic::create_router_with_provider(
        &api_key,
        Some(kiro_provider),
        first_credentials.profile_arn.clone(),
    );

    // 创建 JWT 管理器
    let jwt_secret = config.get_jwt_secret();
    let jwt_expiry_hours = config.jwt_expiry_hours;
    let jwt_manager = JwtManager::new(&jwt_secret, jwt_expiry_hours);

    // 构建 Admin API 路由
    let admin_service = admin::AdminService::new(token_manager.clone());
    let admin_state = admin::AdminState::new(db.clone(), jwt_manager, admin_service);
    let admin_app = admin::create_admin_router(admin_state);

    // 创建 Admin UI 路由
    let admin_ui_app = admin::create_admin_ui_router();

    tracing::info!("Admin API 已启用");
    tracing::info!("Admin UI 已启用: /admin");

    let app = anthropic_app
        .nest("/api/admin", admin_app)
        .nest("/admin", admin_ui_app);

    // 启动时刷新所有凭据余额
    let tm_for_refresh = token_manager.clone();
    tokio::spawn(async move {
        tracing::info!("启动时刷新所有凭据余额...");
        let (success, failures) = tm_for_refresh.refresh_all_balances().await;
        if failures.is_empty() {
            tracing::info!("启动刷新完成: 成功 {} 个凭据", success);
        } else {
            tracing::warn!(
                "启动刷新完成: 成功 {}, 失败 {} 个凭据",
                success,
                failures.len()
            );
        }
    });

    // 启动定时刷新任务（每 5 分钟）
    let tm_for_timer = token_manager.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 分钟
        interval.tick().await; // 跳过第一次立即执行（启动时已刷新）
        loop {
            interval.tick().await;
            tracing::debug!("定时刷新所有凭据余额...");
            let (success, failures) = tm_for_timer.refresh_all_balances().await;
            if !failures.is_empty() {
                tracing::warn!(
                    "定时刷新: 成功 {}, 失败 {} 个凭据",
                    success,
                    failures.len()
                );
            }
        }
    });

    // 启动服务器
    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("启动 Anthropic API 端点: {}", addr);
    tracing::info!("API Key: {}***", &api_key[..(api_key.len() / 2)]);
    tracing::info!("可用 API:");
    tracing::info!("  GET  /v1/models");
    tracing::info!("  POST /v1/messages");
    tracing::info!("  POST /v1/messages/count_tokens");
    tracing::info!("Admin API:");
    tracing::info!("  POST /api/admin/login");
    tracing::info!("  POST /api/admin/logout");
    tracing::info!("  GET  /api/admin/me");
    tracing::info!("  POST /api/admin/change-password");
    tracing::info!("  GET  /api/admin/credentials");
    tracing::info!("  POST /api/admin/credentials");
    tracing::info!("  POST /api/admin/credentials/refresh");
    tracing::info!("  DELETE /api/admin/credentials/:id");
    tracing::info!("Admin UI:");
    tracing::info!("  GET  /admin");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
