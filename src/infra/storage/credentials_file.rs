//! 凭据文件 I/O：单/多格式判定 + 仅多格式回写

use std::path::PathBuf;

use crate::domain::credential::{Credential, CredentialsFile};
use crate::domain::error::ConfigError;

pub struct CredentialsFileStore {
    path: Option<PathBuf>,
}

impl CredentialsFileStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    /// 加载凭据文件，返回 (按 priority 排序的凭据, is_multiple_format)
    ///
    /// - 文件不存在：返回 (vec![], false)
    /// - 文件为空：返回 (vec![], false)
    /// - 单对象格式：返回 (vec![cred], false)
    /// - 数组格式：返回 (sorted_vec, true)
    pub fn load(&self) -> Result<(Vec<Credential>, bool), ConfigError> {
        let path = match &self.path {
            Some(p) => p,
            None => return Ok((vec![], false)),
        };
        let file = CredentialsFile::load(path)?;
        let is_multiple = file.is_multiple();
        let creds = file.into_sorted_credentials();
        Ok((creds, is_multiple))
    }

    /// 回写凭据到文件（仅多格式时回写；单格式与缺失 path 时返回 Ok(false)）
    ///
    /// 序列化为 pretty JSON。Credential 字段顺序由 struct 定义决定。
    pub fn save(&self, creds: &[Credential], is_multiple: bool) -> Result<bool, ConfigError> {
        if !is_multiple {
            return Ok(false);
        }
        let path = match &self.path {
            Some(p) => p,
            None => return Ok(false),
        };
        let json = serde_json::to_string_pretty(creds)?;
        std::fs::write(path, json)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    const FIXTURE_ARRAY_MIXED: &str = include_str!("tests/fixtures/credentials_array_mixed.json");
    const FIXTURE_SINGLE_SOCIAL: &str =
        include_str!("tests/fixtures/credentials_single_social.json");
    const FIXTURE_WITH_MACHINE_ID: &str =
        include_str!("tests/fixtures/credentials_with_machine_id.json");

    fn tmp_path(tag: &str) -> PathBuf {
        let id = Uuid::new_v4();
        std::env::temp_dir().join(format!("kiro-rs-creds-test-{tag}-{id}.json"))
    }

    fn write_fixture(content: &str, tag: &str) -> PathBuf {
        let path = tmp_path(tag);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn load_array_mixed_returns_4_sorted_by_priority() {
        let path = write_fixture(FIXTURE_ARRAY_MIXED, "load-array");
        let store = CredentialsFileStore::new(Some(path.clone()));
        let (creds, is_multiple) = store.load().unwrap();
        let _ = fs::remove_file(&path);

        assert!(is_multiple);
        assert_eq!(creds.len(), 4);
        // priority 顺序：0, 1, 2, 3
        assert_eq!(creds[0].priority, 0);
        assert_eq!(creds[1].priority, 1);
        assert_eq!(creds[2].priority, 2);
        assert_eq!(creds[3].priority, 3);
        assert_eq!(creds[0].auth_method.as_deref(), Some("idc"));
        assert_eq!(creds[1].auth_method.as_deref(), Some("social"));
        assert_eq!(creds[2].auth_method.as_deref(), Some("api_key"));
        assert_eq!(creds[3].auth_method.as_deref(), Some("social"));
    }

    #[test]
    fn save_array_writes_pretty_json_with_struct_field_order() {
        let path = write_fixture(FIXTURE_ARRAY_MIXED, "save-array");
        let store = CredentialsFileStore::new(Some(path.clone()));
        let (creds, is_multiple) = store.load().unwrap();

        let written = store.save(&creds, is_multiple).unwrap();
        assert!(written);

        let after = fs::read_to_string(&path).unwrap();
        // pretty JSON 含换行
        assert!(after.contains('\n'));
        // Credential.id 在 struct 头部 — 但 fixture 没 id，所以序列化也无 id
        assert!(!after.contains("\"id\""));
        // priority=0 不序列化（is_zero）
        assert!(after.contains("\"priority\": 1"));
        // 4 条都在
        assert!(after.matches("\"authMethod\"").count() >= 4);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn save_single_format_does_not_write_file() {
        let path = write_fixture(FIXTURE_SINGLE_SOCIAL, "save-single");
        let store = CredentialsFileStore::new(Some(path.clone()));
        let (creds, is_multiple) = store.load().unwrap();
        assert!(!is_multiple);
        assert_eq!(creds.len(), 1);

        let original = fs::read_to_string(&path).unwrap();
        let written = store.save(&creds, is_multiple).unwrap();
        assert!(!written);

        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(original, after, "single 格式不应回写");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_single_with_machine_id_preserves_field() {
        let path = write_fixture(FIXTURE_WITH_MACHINE_ID, "load-machine-id");
        let store = CredentialsFileStore::new(Some(path.clone()));
        let (creds, is_multiple) = store.load().unwrap();
        let _ = fs::remove_file(&path);

        assert!(!is_multiple);
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].machine_id.as_deref().unwrap().len(), 64);
    }

    #[test]
    fn load_with_no_path_returns_empty() {
        let store = CredentialsFileStore::new(None);
        let (creds, is_multiple) = store.load().unwrap();
        assert!(creds.is_empty());
        assert!(!is_multiple);
    }

    #[test]
    fn save_with_no_path_returns_false() {
        let store = CredentialsFileStore::new(None);
        let written = store.save(&[], true).unwrap();
        assert!(!written);
    }
}
