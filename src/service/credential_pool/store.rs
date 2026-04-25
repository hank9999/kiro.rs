//! CredentialStore：增删改 + 持久化协调
//!
//! 内部按 `HashMap<u64, Credential>` 存储，不依赖 Vec 索引。
//! 加载时补齐 ID/machineId 后立即回写（仅多格式）。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::Config;
use crate::domain::credential::Credential;
use crate::domain::error::ConfigError;
use crate::infra::machine_id::MachineIdResolver;
use crate::infra::storage::CredentialsFileStore;

/// 加载阶段产生的校验问题（不阻断启动；caller 据此把对应凭据 disable）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub id: u64,
    pub kind: ValidationKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationKind {
    /// 配置不完整（如 auth_method=api_key 但缺 kiroApiKey）
    InvalidConfig,
}

pub struct CredentialStore {
    inner: Mutex<HashMap<u64, Credential>>,
    file: Arc<CredentialsFileStore>,
    is_multiple: bool,
    next_id: Mutex<u64>,
    resolver: Arc<MachineIdResolver>,
    config: Arc<Config>,
}

impl CredentialStore {
    /// 从文件加载凭据；补齐 ID/machineId 后回写（多格式）
    ///
    /// 返回 (store, validation_issues)：issues 不阻断启动，caller 据此 disable 对应凭据。
    pub fn load(
        file: Arc<CredentialsFileStore>,
        config: Arc<Config>,
        resolver: Arc<MachineIdResolver>,
    ) -> Result<(Self, Vec<ValidationIssue>), ConfigError> {
        let (mut creds, is_multiple) = file.load()?;

        // 1. 分配缺失 id
        let mut max_id = 0u64;
        for c in &creds {
            if let Some(id) = c.id {
                max_id = max_id.max(id);
            }
        }
        for cred in &mut creds {
            if cred.id.is_none() {
                max_id += 1;
                cred.id = Some(max_id);
            }
        }

        // 2. 补齐 machineId（仅在凭据没显式 machineId 且 config.kiro.machine_id 也未设置时
        //    才用 resolver 派生写入文件 —— 防止覆盖用户全局配置的 machineId）
        for cred in &mut creds {
            if cred.machine_id.is_none() && config.kiro.machine_id.is_none() {
                cred.machine_id = Some(resolver.resolve(cred, &config));
            }
        }

        // 3. 仅多格式回写补齐结果
        if is_multiple {
            file.save(&creds, true)?;
        }

        // 4. 收集 ValidationIssue（api_key 凭据缺 kiroApiKey）
        let mut issues = Vec::new();
        for cred in &creds {
            let id = cred.id.expect("已补齐 id");
            if cred.is_api_key_credential()
                && cred
                    .kiro_api_key
                    .as_deref()
                    .map(str::is_empty)
                    .unwrap_or(true)
            {
                issues.push(ValidationIssue {
                    id,
                    kind: ValidationKind::InvalidConfig,
                    message: format!(
                        "凭据 {id} 配置无效：auth_method=api_key 但 kiroApiKey 缺失或为空"
                    ),
                });
            }
        }

        let map: HashMap<u64, Credential> = creds
            .into_iter()
            .map(|c| (c.id.expect("已补齐 id"), c))
            .collect();

        let store = Self {
            inner: Mutex::new(map),
            file,
            is_multiple,
            next_id: Mutex::new(max_id),
            resolver,
            config,
        };
        Ok((store, issues))
    }

    pub fn is_multiple(&self) -> bool {
        self.is_multiple
    }

    pub fn ids(&self) -> Vec<u64> {
        self.inner.lock().keys().copied().collect()
    }

    pub fn get(&self, id: u64) -> Option<Credential> {
        self.inner.lock().get(&id).cloned()
    }

    pub fn snapshot(&self) -> HashMap<u64, Credential> {
        self.inner.lock().clone()
    }

    pub fn count(&self) -> usize {
        self.inner.lock().len()
    }

    /// 添加新凭据（自动分配 id），返回新 id；持久化（仅多格式）
    pub fn add(&self, mut cred: Credential) -> Result<u64, ConfigError> {
        let id = {
            let mut next = self.next_id.lock();
            *next += 1;
            *next
        };
        cred.id = Some(id);
        cred.canonicalize_auth_method();

        if cred.machine_id.is_none() && self.config.kiro.machine_id.is_none() {
            cred.machine_id = Some(self.resolver.resolve(&cred, &self.config));
        }

        {
            let mut map = self.inner.lock();
            map.insert(id, cred);
        }
        self.persist()?;
        Ok(id)
    }

    pub fn remove(&self, id: u64) -> Result<bool, ConfigError> {
        let removed = self.inner.lock().remove(&id).is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    /// 写回所有字段（用于 token 刷新后更新 access_token / expires_at / refresh_token / profile_arn）
    pub fn replace(&self, id: u64, new_cred: Credential) -> Result<bool, ConfigError> {
        let mut map = self.inner.lock();
        if !map.contains_key(&id) {
            return Ok(false);
        }
        map.insert(id, new_cred);
        drop(map);
        self.persist()?;
        Ok(true)
    }

    pub fn set_priority(&self, id: u64, priority: u32) -> Result<bool, ConfigError> {
        let updated = {
            let mut map = self.inner.lock();
            match map.get_mut(&id) {
                Some(c) => {
                    c.priority = priority;
                    true
                }
                None => false,
            }
        };
        if updated {
            self.persist()?;
        }
        Ok(updated)
    }

    pub fn set_endpoint(&self, id: u64, endpoint: Option<String>) -> Result<bool, ConfigError> {
        let updated = {
            let mut map = self.inner.lock();
            match map.get_mut(&id) {
                Some(c) => {
                    c.endpoint = endpoint;
                    true
                }
                None => false,
            }
        };
        if updated {
            self.persist()?;
        }
        Ok(updated)
    }

    /// 同步 disabled 字段到文件并回写（仅多格式）
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<bool, ConfigError> {
        let updated = {
            let mut map = self.inner.lock();
            match map.get_mut(&id) {
                Some(c) => {
                    c.disabled = disabled;
                    true
                }
                None => false,
            }
        };
        if updated {
            self.persist()?;
        }
        Ok(updated)
    }

    fn persist(&self) -> Result<bool, ConfigError> {
        if !self.is_multiple {
            return Ok(false);
        }
        // 按 priority 排序后落盘（与原文件字段顺序一致）
        let mut sorted: Vec<Credential> = self.inner.lock().values().cloned().collect();
        sorted.sort_by_key(|c| c.priority);
        self.file.save(&sorted, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    const FIXTURE_ARRAY_MIXED: &str =
        include_str!("../../infra/storage/tests/fixtures/credentials_array_mixed.json");
    const FIXTURE_SINGLE_SOCIAL: &str =
        include_str!("../../infra/storage/tests/fixtures/credentials_single_social.json");

    fn tmp_path(tag: &str) -> PathBuf {
        let id = Uuid::new_v4();
        std::env::temp_dir().join(format!("kiro-rs-store-test-{tag}-{id}.json"))
    }

    fn make_store_from(content: &str, tag: &str) -> (CredentialStore, Vec<ValidationIssue>, PathBuf) {
        let path = tmp_path(tag);
        fs::write(&path, content).unwrap();
        let file = Arc::new(CredentialsFileStore::new(Some(path.clone())));
        let config = Arc::new(Config::default());
        let resolver = Arc::new(MachineIdResolver::new());
        let (store, issues) = CredentialStore::load(file, config, resolver).unwrap();
        (store, issues, path)
    }

    #[test]
    fn load_array_mixed_returns_4_with_ids_assigned() {
        let (store, _issues, path) = make_store_from(FIXTURE_ARRAY_MIXED, "load-mixed");
        assert_eq!(store.count(), 4);
        let ids = store.ids();
        assert_eq!(ids.len(), 4);
        // 所有 id 都已补齐为 1..=4
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, vec![1, 2, 3, 4]);
        assert!(store.is_multiple());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn add_credential_then_persist_and_reload() {
        let (store, _issues, path) = make_store_from(FIXTURE_ARRAY_MIXED, "add-persist");
        let initial = store.count();
        let new_cred = Credential {
            refresh_token: Some("rt-new".to_string()),
            auth_method: Some("social".to_string()),
            priority: 10,
            ..Default::default()
        };
        let new_id = store.add(new_cred).unwrap();
        assert!(new_id > 0);
        assert_eq!(store.count(), initial + 1);

        // 从同一 path 重新加载，确保 5 条
        let file2 = Arc::new(CredentialsFileStore::new(Some(path.clone())));
        let (store2, _) =
            CredentialStore::load(file2, Arc::new(Config::default()), Arc::new(MachineIdResolver::new()))
                .unwrap();
        assert_eq!(store2.count(), initial + 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn single_format_load_does_not_rewrite_file() {
        let (store, _issues, path) = make_store_from(FIXTURE_SINGLE_SOCIAL, "single-noop");
        assert!(!store.is_multiple());
        assert_eq!(store.count(), 1);

        let original = fs::read_to_string(&path).unwrap();
        // set_priority 不应影响文件（单格式）
        let id = store.ids()[0];
        store.set_priority(id, 99).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(original, after, "single 格式不应被回写");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn api_key_missing_kiro_api_key_yields_validation_issue() {
        // 构造缺 kiroApiKey 的 api_key 凭据
        let json = r#"[{"authMethod":"api_key","priority":0}]"#;
        let (store, issues, path) = make_store_from(json, "api-key-missing");
        assert_eq!(store.count(), 1);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, ValidationKind::InvalidConfig);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn api_key_with_kiro_api_key_no_validation_issue() {
        let json = r#"[{"authMethod":"api_key","kiroApiKey":"ksk_x","priority":0}]"#;
        let (store, issues, path) = make_store_from(json, "api-key-ok");
        assert_eq!(store.count(), 1);
        assert!(issues.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn remove_credential_and_persist() {
        let (store, _, path) = make_store_from(FIXTURE_ARRAY_MIXED, "remove");
        let id = store.ids()[0];
        let removed = store.remove(id).unwrap();
        assert!(removed);
        assert_eq!(store.count(), 3);
        let _ = fs::remove_file(&path);
    }
}
