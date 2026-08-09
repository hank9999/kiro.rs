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
    /// 串行化运行期凭据快照与文件写入，避免并发写入让旧快照覆盖新值。
    /// 锁顺序固定为 persist_lock -> inner。
    persist_lock: Mutex<()>,
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

        // 4. 拒绝重复 id（避免后续 HashMap 静默覆盖凭据，导致 refresh_token 丢失）
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for cred in &creds {
            let id = cred.id.expect("已补齐 id");
            if !seen.insert(id) {
                return Err(ConfigError::Validation(format!(
                    "credentials.json 含重复 id {id}：拒绝启动以避免凭据被静默覆盖"
                )));
            }
        }

        // 5. 收集 ValidationIssue（api_key 凭据缺 kiroApiKey）
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
            persist_lock: Mutex::new(()),
            file,
            is_multiple,
            next_id: Mutex::new(max_id),
            resolver,
            config,
        };
        Ok((store, issues))
    }

    #[cfg(test)]
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

    /// 添加新凭据（自动分配 id），返回新 id；持久化（仅多格式）。
    /// 持久化失败时回滚内存（next_id 不回滚以避免与并发 add 竞态产生 id 重复）。
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
        if let Err(e) = self.persist() {
            self.inner.lock().remove(&id);
            return Err(e);
        }
        Ok(id)
    }

    /// 持久化失败时回滚内存
    pub fn remove(&self, id: u64) -> Result<bool, ConfigError> {
        let removed = {
            let mut map = self.inner.lock();
            map.remove(&id)
        };
        let Some(cred) = removed else {
            return Ok(false);
        };
        if let Err(e) = self.persist() {
            self.inner.lock().insert(id, cred);
            return Err(e);
        }
        Ok(true)
    }

    /// 将候选快照保存到磁盘（仅多格式），按 priority/id 排序。
    /// 调用方必须持有 `persist_lock`。
    fn save_candidate_locked(&self, mut creds: Vec<Credential>) -> Result<bool, ConfigError> {
        if !self.is_multiple {
            return Ok(false);
        }
        creds.sort_by_key(|c| (c.priority, c.id.unwrap_or(0)));
        self.file.save(&creds, true)
    }

    /// Best-effort 写回：先更新内存再持久化；磁盘失败时内存仍保留新值。
    ///
    /// 用于请求路径（token 刷新）：刷新成功但磁盘抖动时，内存更新让请求继续。
    pub fn replace_best_effort(&self, id: u64, new_cred: Credential) -> Result<bool, ConfigError> {
        let mut map = self.inner.lock();
        if !map.contains_key(&id) {
            return Ok(false);
        }
        map.insert(id, new_cred);
        drop(map);
        self.persist()?;
        Ok(true)
    }

    /// 严格持久化：先写盘，成功后才更新内存。
    ///
    /// 用于 admin 显式写路径：API 返回失败时调用方不应观察到部分成功。
    pub fn replace_persisted(&self, id: u64, new_cred: Credential) -> Result<bool, ConfigError> {
        let _persist_guard = self.persist_lock.lock();
        let mut map = self.inner.lock();
        if !map.contains_key(&id) {
            return Ok(false);
        }
        let mut candidate = map.clone();
        candidate.insert(id, new_cred.clone());
        self.save_candidate_locked(candidate.into_values().collect())?;
        map.insert(id, new_cred);
        Ok(true)
    }

    /// 设置 priority：先写盘，成功后才更新内存。
    pub fn set_priority(&self, id: u64, priority: u32) -> Result<bool, ConfigError> {
        let _persist_guard = self.persist_lock.lock();
        let mut map = self.inner.lock();
        if !map.contains_key(&id) {
            return Ok(false);
        }
        let mut candidate = map.clone();
        if let Some(c) = candidate.get_mut(&id) {
            c.priority = priority;
        }
        self.save_candidate_locked(candidate.into_values().collect())?;
        if let Some(c) = map.get_mut(&id) {
            c.priority = priority;
        }
        Ok(true)
    }

    /// 设置 disabled：先写盘，成功后才更新内存。
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<bool, ConfigError> {
        let _persist_guard = self.persist_lock.lock();
        let mut map = self.inner.lock();
        if !map.contains_key(&id) {
            return Ok(false);
        }
        let mut candidate = map.clone();
        if let Some(c) = candidate.get_mut(&id) {
            c.disabled = disabled;
        }
        self.save_candidate_locked(candidate.into_values().collect())?;
        if let Some(c) = map.get_mut(&id) {
            c.disabled = disabled;
        }
        Ok(true)
    }

    fn persist(&self) -> Result<bool, ConfigError> {
        if !self.is_multiple {
            return Ok(false);
        }
        // 必须在取得快照前加锁，并持有到写盘结束；否则旧快照可能后写覆盖新值。
        let _persist_guard = self.persist_lock.lock();
        // 按 priority/id 排序后落盘（与原文件字段顺序一致）
        let mut sorted: Vec<Credential> = self.inner.lock().values().cloned().collect();
        sorted.sort_by_key(|c| (c.priority, c.id.unwrap_or(0)));
        self.file.save(&sorted, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    const FIXTURE_ARRAY_MIXED: &str =
        include_str!("../../infra/storage/tests/fixtures/credentials_array_mixed.json");
    const FIXTURE_SINGLE_SOCIAL: &str =
        include_str!("../../infra/storage/tests/fixtures/credentials_single_social.json");

    fn tmp_path(tag: &str) -> PathBuf {
        let id = Uuid::new_v4();
        std::env::temp_dir().join(format!("kiro-rs-store-test-{tag}-{id}.json"))
    }

    fn make_store_from(
        content: &str,
        tag: &str,
    ) -> (CredentialStore, Vec<ValidationIssue>, PathBuf) {
        let path = tmp_path(tag);
        fs::write(&path, content).unwrap();
        let file = Arc::new(CredentialsFileStore::new(Some(path.clone())));
        let config = Arc::new(Config::default());
        let resolver = Arc::new(MachineIdResolver::new());
        let (store, issues) = CredentialStore::load(file, config, resolver).unwrap();
        (store, issues, path)
    }

    fn tied_priority_credentials_json(ids: &[u64]) -> String {
        let creds: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": id,
                    "refreshToken": format!("rt-{id}"),
                    "authMethod": "social",
                    "priority": 0,
                })
            })
            .collect();
        serde_json::to_string_pretty(&creds).unwrap()
    }

    fn credential_ids_in_file(path: &PathBuf) -> Vec<u64> {
        let content = fs::read_to_string(path).unwrap();
        let creds: Vec<Credential> = serde_json::from_str(&content).unwrap();
        creds.into_iter().map(|c| c.id.unwrap()).collect()
    }

    fn wait_for_refresh_token(store: &CredentialStore, id: u64, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if store.get(id).and_then(|c| c.refresh_token).as_deref() == Some(expected) {
                return;
            }
            assert!(Instant::now() < deadline, "等待凭据 {id} 更新到内存超时");
            thread::yield_now();
        }
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
        let (store2, _) = CredentialStore::load(
            file2,
            Arc::new(Config::default()),
            Arc::new(MachineIdResolver::new()),
        )
        .unwrap();
        assert_eq!(store2.count(), initial + 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn set_disabled_persists_tied_priorities_in_id_order() {
        let json = tied_priority_credentials_json(&[9, 3, 6, 1, 8, 2, 7, 4, 5]);
        let (store, _issues, path) = make_store_from(&json, "disabled-stable-order");

        store.set_disabled(6, true).unwrap();

        assert_eq!(
            credential_ids_in_file(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn add_persists_tied_priorities_in_id_order() {
        let json = tied_priority_credentials_json(&[9, 3, 6, 1, 8, 2, 7, 4, 5]);
        let (store, _issues, path) = make_store_from(&json, "add-stable-order");

        let new_id = store
            .add(Credential {
                refresh_token: Some("rt-10".to_string()),
                auth_method: Some("social".to_string()),
                priority: 0,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(new_id, 10);
        assert_eq!(
            credential_ids_in_file(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        );
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

    fn try_load_from(
        content: &str,
        tag: &str,
    ) -> (
        Result<(CredentialStore, Vec<ValidationIssue>), ConfigError>,
        PathBuf,
    ) {
        let path = tmp_path(tag);
        fs::write(&path, content).unwrap();
        let file = Arc::new(CredentialsFileStore::new(Some(path.clone())));
        let config = Arc::new(Config::default());
        let resolver = Arc::new(MachineIdResolver::new());
        let res = CredentialStore::load(file, config, resolver);
        (res, path)
    }

    #[test]
    fn load_rejects_duplicate_ids() {
        let json = r#"[
            {"id":1,"refreshToken":"rt-a","authMethod":"social"},
            {"id":1,"refreshToken":"rt-b","authMethod":"social"}
        ]"#;
        let (res, path) = try_load_from(json, "dup-id");
        match res {
            Err(ConfigError::Validation(msg)) => {
                assert!(
                    msg.contains("重复 id 1"),
                    "expected message to contain 重复 id 1, got: {msg}"
                );
            }
            Err(other) => panic!("expected ConfigError::Validation, got {other:?}"),
            Ok(_) => panic!("expected ConfigError::Validation, got Ok"),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_accepts_distinct_explicit_ids() {
        let json = r#"[
            {"id":1,"refreshToken":"rt-a","authMethod":"social"},
            {"id":2,"refreshToken":"rt-b","authMethod":"social"}
        ]"#;
        let (res, path) = try_load_from(json, "distinct-ids");
        let (store, _) = res.expect("distinct ids should load");
        assert_eq!(store.count(), 2);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_accepts_all_missing_ids_auto_assign() {
        let json = r#"[
            {"refreshToken":"rt-a","authMethod":"social"},
            {"refreshToken":"rt-b","authMethod":"social"}
        ]"#;
        let (res, path) = try_load_from(json, "auto-id");
        let (store, _) = res.expect("auto-assigned ids should not be flagged duplicate");
        assert_eq!(store.count(), 2);
        let _ = fs::remove_file(&path);
    }

    /// 构造 store，凭据文件在可删除的子目录中；返回 (store, 子目录路径)。
    /// 删除子目录后 CredentialsFileStore::save 会因父目录不存在而失败。
    fn make_store_with_deletable_dir(content: &str, tag: &str) -> (CredentialStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kiro-rs-store-rm-{}-{}", tag, Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("creds.json");
        fs::write(&path, content).unwrap();
        let file = Arc::new(CredentialsFileStore::new(Some(path)));
        let config = Arc::new(Config::default());
        let resolver = Arc::new(MachineIdResolver::new());
        let (store, _) = CredentialStore::load(file, config, resolver).unwrap();
        (store, dir)
    }

    #[test]
    fn set_priority_persist_failure_does_not_modify_memory() {
        let (store, dir) = make_store_with_deletable_dir(FIXTURE_ARRAY_MIXED, "priority-rollback");
        let id = store.ids()[0];
        let old_priority = store.get(id).unwrap().priority;

        // 删除目录使后续 save 失败
        fs::remove_dir_all(&dir).unwrap();

        let err = store.set_priority(id, 999).unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)));
        assert_eq!(
            store.get(id).unwrap().priority,
            old_priority,
            "持久化失败后 priority 应保持旧值"
        );
    }

    #[test]
    fn set_disabled_persist_failure_does_not_modify_memory() {
        let (store, dir) = make_store_with_deletable_dir(FIXTURE_ARRAY_MIXED, "disabled-rollback");
        let id = store.ids()[0];
        assert!(!store.get(id).unwrap().disabled);

        fs::remove_dir_all(&dir).unwrap();

        let err = store.set_disabled(id, true).unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)));
        assert!(
            !store.get(id).unwrap().disabled,
            "持久化失败后 disabled 应保持 false"
        );
    }

    #[test]
    fn replace_persisted_persist_failure_does_not_modify_memory() {
        let (store, dir) =
            make_store_with_deletable_dir(FIXTURE_ARRAY_MIXED, "replace-persisted-rollback");
        let id = store.ids()[0];
        let old_cred = store.get(id).unwrap();
        let old_token = old_cred.access_token.clone();

        fs::remove_dir_all(&dir).unwrap();

        let mut new_cred = old_cred;
        new_cred.access_token = Some("new-token".to_string());
        let err = store.replace_persisted(id, new_cred).unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)));
        assert_eq!(
            store.get(id).unwrap().access_token,
            old_token,
            "持久化失败后 access_token 应保持旧值"
        );
    }

    #[test]
    fn replace_best_effort_persist_failure_still_modifies_memory() {
        let (store, dir) =
            make_store_with_deletable_dir(FIXTURE_ARRAY_MIXED, "replace-best-effort");
        let id = store.ids()[0];
        let old_cred = store.get(id).unwrap();

        fs::remove_dir_all(&dir).unwrap();

        let mut new_cred = old_cred;
        new_cred.access_token = Some("new-token".to_string());
        let err = store.replace_best_effort(id, new_cred).unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)));
        assert_eq!(
            store.get(id).unwrap().access_token.as_deref(),
            Some("new-token"),
            "best-effort 语义：磁盘失败但内存已更新"
        );
    }

    #[test]
    fn concurrent_best_effort_replacements_serialize_snapshot_and_write() {
        let json = r#"[
            {"id":1,"refreshToken":"rt-a-old","authMethod":"social"},
            {"id":2,"refreshToken":"rt-b-old","authMethod":"social"}
        ]"#;
        let (store, _, path) = make_store_from(json, "concurrent-persist");
        let store = Arc::new(store);

        let mut cred_a = store.get(1).unwrap();
        cred_a.refresh_token = Some("rt-a-new".to_string());
        let mut cred_b = store.get(2).unwrap();
        cred_b.refresh_token = Some("rt-b-new".to_string());

        // 暂停持久化，让两个不同 id 的内存更新同时等待同一把锁。
        // persist_lock 在快照前获取，因此此时磁盘必须仍是旧值。
        let persist_guard = store.persist_lock.lock();
        let store_a = Arc::clone(&store);
        let worker_a = thread::spawn(move || store_a.replace_best_effort(1, cred_a));
        wait_for_refresh_token(&store, 1, "rt-a-new");

        let store_b = Arc::clone(&store);
        let worker_b = thread::spawn(move || store_b.replace_best_effort(2, cred_b));
        wait_for_refresh_token(&store, 2, "rt-b-new");

        let before_release: Vec<Credential> =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            before_release
                .iter()
                .find(|c| c.id == Some(1))
                .and_then(|c| c.refresh_token.as_deref()),
            Some("rt-a-old")
        );
        assert_eq!(
            before_release
                .iter()
                .find(|c| c.id == Some(2))
                .and_then(|c| c.refresh_token.as_deref()),
            Some("rt-b-old")
        );

        drop(persist_guard);
        assert!(worker_a.join().unwrap().unwrap());
        assert!(worker_b.join().unwrap().unwrap());

        let persisted: Vec<Credential> =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            persisted
                .iter()
                .find(|c| c.id == Some(1))
                .and_then(|c| c.refresh_token.as_deref()),
            Some("rt-a-new")
        );
        assert_eq!(
            persisted
                .iter()
                .find(|c| c.id == Some(2))
                .and_then(|c| c.refresh_token.as_deref()),
            Some("rt-b-new")
        );
        let _ = fs::remove_file(&path);
    }
}
