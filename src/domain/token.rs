//! TokenSource trait（占位，Phase 2 实现 Social/Idc/ApiKey 三种）

#![allow(dead_code)]

use futures::future::BoxFuture;

use crate::domain::credential::Credential;
use crate::domain::error::RefreshError;

/// Token 刷新结果
#[derive(Debug, Clone)]
pub struct RefreshOutcome {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub profile_arn: Option<String>,
    pub expires_at: Option<String>,
}

/// Token 刷新策略（RPITIT 形式，便于具体类型 impl）
pub trait TokenSource: Send + Sync {
    fn refresh(
        &self,
        cred: &Credential,
    ) -> impl std::future::Future<Output = Result<RefreshOutcome, RefreshError>> + Send;
}

/// `TokenSource` 的对象安全变体，供 `Arc<dyn DynTokenSource>` 使用。
///
/// 任何 `T: TokenSource` 都通过 blanket impl 自动满足 `DynTokenSource`，
/// 测试可注入 mock 而不影响生产代码路径。
pub trait DynTokenSource: Send + Sync {
    fn refresh<'a>(
        &'a self,
        cred: &'a Credential,
    ) -> BoxFuture<'a, Result<RefreshOutcome, RefreshError>>;
}

impl<T: TokenSource> DynTokenSource for T {
    fn refresh<'a>(
        &'a self,
        cred: &'a Credential,
    ) -> BoxFuture<'a, Result<RefreshOutcome, RefreshError>> {
        Box::pin(async move { TokenSource::refresh(self, cred).await })
    }
}
