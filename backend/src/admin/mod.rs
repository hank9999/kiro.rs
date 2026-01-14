//! Admin 模块
//!
//! Input: Database, JWT, TokenManager, rust-embed
//! Output: Admin API 路由、服务和 UI 静态文件
//! Pos: 管理后台 API 层和 UI 服务
//!
//! # 功能
//! - 用户认证（登录/登出/修改密码）
//! - 凭据管理（CRUD、启用/禁用、优先级）
//! - 余额查询
//! - 系统设置
//! - Admin UI 静态文件服务

mod error;
mod handlers;
mod middleware;
mod router;
mod service;
pub mod types;
mod ui;

pub use middleware::AdminState;
pub use router::create_admin_router;
pub use service::AdminService;
pub use ui::create_admin_ui_router;
