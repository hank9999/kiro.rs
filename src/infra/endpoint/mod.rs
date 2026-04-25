//! Kiro 端点实现 + 注册表

#![allow(dead_code)]

pub mod ide;

pub use ide::IdeEndpoint;

use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::credential::Credential;
use crate::domain::endpoint::KiroEndpoint;
use crate::domain::error::ProviderError;

/// 端点注册表：default_endpoint + name → 实现 map
///
/// `resolve_for(&Credential)` 把"按凭据 endpoint 字段查表 / 否则用 default" 集中在一处。
pub struct EndpointRegistry {
    default_endpoint: String,
    endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
}

impl EndpointRegistry {
    pub fn new(
        default_endpoint: impl Into<String>,
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
    ) -> Result<Self, ProviderError> {
        let name = default_endpoint.into();
        if !endpoints.contains_key(&name) {
            return Err(ProviderError::EndpointResolution(format!(
                "default endpoint '{name}' 未在 endpoints 注册表中"
            )));
        }
        Ok(Self {
            default_endpoint: name,
            endpoints,
        })
    }

    pub fn default_name(&self) -> &str {
        &self.default_endpoint
    }

    pub fn names(&self) -> Vec<String> {
        self.endpoints.keys().cloned().collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.endpoints.contains_key(name)
    }

    /// 凭据 endpoint 字段命中 → 返回对应实现；缺失 → fallback default；未注册 → ProviderError::EndpointResolution
    pub fn resolve_for(&self, cred: &Credential) -> Result<Arc<dyn KiroEndpoint>, ProviderError> {
        let name = cred
            .endpoint
            .as_deref()
            .unwrap_or(&self.default_endpoint);
        self.endpoints.get(name).cloned().ok_or_else(|| {
            ProviderError::EndpointResolution(format!("unknown endpoint: {name}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with_ide() -> EndpointRegistry {
        let mut map: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        map.insert("ide".to_string(), Arc::new(IdeEndpoint::new()));
        EndpointRegistry::new("ide", map).unwrap()
    }

    #[test]
    fn resolve_for_credential_with_endpoint_hit() {
        let reg = registry_with_ide();
        let cred = Credential {
            endpoint: Some("ide".to_string()),
            ..Default::default()
        };
        let endpoint = reg.resolve_for(&cred).unwrap();
        assert_eq!(endpoint.name(), "ide");
    }

    #[test]
    fn resolve_for_credential_missing_endpoint_uses_default() {
        let reg = registry_with_ide();
        let cred = Credential::default(); // endpoint = None
        let endpoint = reg.resolve_for(&cred).unwrap();
        assert_eq!(endpoint.name(), "ide");
    }

    #[test]
    fn resolve_for_unknown_endpoint_returns_endpoint_resolution_error() {
        let reg = registry_with_ide();
        let cred = Credential {
            endpoint: Some("nonexistent".to_string()),
            ..Default::default()
        };
        match reg.resolve_for(&cred) {
            Err(ProviderError::EndpointResolution(msg)) => {
                assert!(msg.contains("nonexistent"));
            }
            Err(other) => panic!("expected EndpointResolution, got {other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn new_with_unregistered_default_returns_error() {
        let mut map: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        map.insert("ide".to_string(), Arc::new(IdeEndpoint::new()));
        let result = EndpointRegistry::new("nonexistent", map);
        assert!(result.is_err());
    }
}
