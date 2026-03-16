//! 代理 IP 池管理
//!
//! 提供代理 IP 的 CRUD 操作、自动分配和持久化

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 代理池条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyEntry {
    /// 唯一 ID
    pub id: u64,
    /// 代理 URL（如 http://1.2.3.4:8080 或 socks5://5.6.7.8:1080）
    pub url: String,
    /// 认证用户名
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// 认证密码
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// 备注标签（如 "IPRoyal-US-01"）
    #[serde(default)]
    pub label: String,
    /// 是否禁用（不参与自动分配）
    #[serde(default)]
    pub disabled: bool,
    /// 绑定的凭据 ID，None 表示空闲
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_to: Option<u64>,
}

/// 代理池管理器
pub struct ProxyPool {
    entries: RwLock<Vec<ProxyEntry>>,
    next_id: AtomicU64,
    file_path: Option<PathBuf>,
}

impl ProxyPool {
    /// 创建代理池，从指定目录加载 proxy_pool.json
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        let file_path = data_dir.map(|d| d.join("proxy_pool.json"));
        let entries = Self::load_from_file(&file_path);
        let max_id = entries.iter().map(|e| e.id).max().unwrap_or(0);

        Self {
            entries: RwLock::new(entries),
            next_id: AtomicU64::new(max_id + 1),
            file_path,
        }
    }

    /// 获取所有代理
    pub fn list(&self) -> Vec<ProxyEntry> {
        self.entries.read().clone()
    }

    /// 根据 ID 获取代理
    pub fn get(&self, id: u64) -> Option<ProxyEntry> {
        self.entries.read().iter().find(|e| e.id == id).cloned()
    }

    /// 添加代理，返回新 ID
    pub fn add(&self, url: String, username: Option<String>, password: Option<String>, label: String) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = ProxyEntry {
            id,
            url,
            username,
            password,
            label,
            disabled: false,
            assigned_to: None,
        };
        self.entries.write().push(entry);
        self.save();
        id
    }

    /// 批量添加代理，返回新 ID 列表
    /// 每行格式：url 或 url:username:password 或 url:username:password:label
    pub fn batch_add(&self, lines: &[String]) -> Vec<u64> {
        let mut ids = Vec::new();
        let mut entries = self.entries.write();

        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // 需要特殊处理 URL 中的 :// 
            // 用空格或制表符分隔
            let (url, username, password, label) = Self::parse_proxy_line(line);

            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            entries.push(ProxyEntry {
                id,
                url,
                username,
                password,
                label,
                disabled: false,
                assigned_to: None,
            });
            ids.push(id);
        }

        drop(entries);
        self.save();
        ids
    }

    /// 解析一行代理配置
    /// 支持格式：
    /// - url
    /// - url username password
    /// - url username password label
    /// 分隔符：空格或制表符
    fn parse_proxy_line(line: &str) -> (String, Option<String>, Option<String>, String) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.len() {
            0 => (String::new(), None, None, String::new()),
            1 => (parts[0].to_string(), None, None, String::new()),
            2 => (parts[0].to_string(), Some(parts[1].to_string()), None, String::new()),
            3 => (parts[0].to_string(), Some(parts[1].to_string()), Some(parts[2].to_string()), String::new()),
            _ => (
                parts[0].to_string(),
                Some(parts[1].to_string()),
                Some(parts[2].to_string()),
                parts[3..].join(" "),
            ),
        }
    }

    /// 编辑代理
    pub fn update(&self, id: u64, url: Option<String>, username: Option<Option<String>>, password: Option<Option<String>>, label: Option<String>, disabled: Option<bool>) -> Result<(), String> {
        let mut entries = self.entries.write();
        let entry = entries.iter_mut().find(|e| e.id == id).ok_or_else(|| format!("代理 #{} 不存在", id))?;

        if let Some(url) = url {
            entry.url = url;
        }
        if let Some(username) = username {
            entry.username = username;
        }
        if let Some(password) = password {
            entry.password = password;
        }
        if let Some(label) = label {
            entry.label = label;
        }
        if let Some(disabled) = disabled {
            entry.disabled = disabled;
        }

        drop(entries);
        self.save();
        Ok(())
    }

    /// 删除代理
    pub fn delete(&self, id: u64) -> Result<ProxyEntry, String> {
        let mut entries = self.entries.write();
        let pos = entries.iter().position(|e| e.id == id).ok_or_else(|| format!("代理 #{} 不存在", id))?;

        if entries[pos].assigned_to.is_some() {
            return Err(format!("代理 #{} 正在被凭据使用，请先解绑", id));
        }

        let removed = entries.remove(pos);
        drop(entries);
        self.save();
        Ok(removed)
    }

    /// 设置禁用状态
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<(), String> {
        self.update(id, None, None, None, None, Some(disabled))
    }

    /// 自动分配一个空闲代理给凭据，返回代理信息
    pub fn auto_assign(&self, credential_id: u64) -> Option<ProxyEntry> {
        let mut entries = self.entries.write();
        let entry = entries.iter_mut().find(|e| !e.disabled && e.assigned_to.is_none())?;
        entry.assigned_to = Some(credential_id);
        let result = entry.clone();
        drop(entries);
        self.save();
        Some(result)
    }

    /// 手动分配指定代理给凭据
    pub fn assign(&self, proxy_id: u64, credential_id: u64) -> Result<ProxyEntry, String> {
        let mut entries = self.entries.write();
        let entry = entries.iter_mut().find(|e| e.id == proxy_id).ok_or_else(|| format!("代理 #{} 不存在", proxy_id))?;

        if entry.disabled {
            return Err(format!("代理 #{} 已禁用", proxy_id));
        }
        if let Some(existing) = entry.assigned_to {
            if existing != credential_id {
                return Err(format!("代理 #{} 已被凭据 #{} 使用", proxy_id, existing));
            }
        }

        entry.assigned_to = Some(credential_id);
        let result = entry.clone();
        drop(entries);
        self.save();
        Ok(result)
    }

    /// 释放凭据绑定的代理
    pub fn release_by_credential(&self, credential_id: u64) {
        let mut entries = self.entries.write();
        for entry in entries.iter_mut() {
            if entry.assigned_to == Some(credential_id) {
                entry.assigned_to = None;
            }
        }
        drop(entries);
        self.save();
    }

    /// 获取凭据绑定的代理
    pub fn get_by_credential(&self, credential_id: u64) -> Option<ProxyEntry> {
        self.entries.read().iter().find(|e| e.assigned_to == Some(credential_id)).cloned()
    }

    /// 获取空闲代理数量
    pub fn available_count(&self) -> usize {
        self.entries.read().iter().filter(|e| !e.disabled && e.assigned_to.is_none()).count()
    }

    // ============ 持久化 ============

    fn load_from_file(file_path: &Option<PathBuf>) -> Vec<ProxyEntry> {
        let path = match file_path {
            Some(p) => p,
            None => return Vec::new(),
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        match serde_json::from_str(&content) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("解析 proxy_pool.json 失败: {}", e);
                Vec::new()
            }
        }
    }

    fn save(&self) {
        let path = match &self.file_path {
            Some(p) => p,
            None => return,
        };

        let entries = self.entries.read();
        match serde_json::to_string_pretty(&*entries) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    tracing::warn!("保存 proxy_pool.json 失败: {}", e);
                }
            }
            Err(e) => tracing::warn!("序列化 proxy_pool.json 失败: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_proxy_line_url_only() {
        let (url, user, pass, label) = ProxyPool::parse_proxy_line("http://1.2.3.4:8080");
        assert_eq!(url, "http://1.2.3.4:8080");
        assert!(user.is_none());
        assert!(pass.is_none());
        assert!(label.is_empty());
    }

    #[test]
    fn test_parse_proxy_line_with_auth() {
        let (url, user, pass, label) = ProxyPool::parse_proxy_line("http://1.2.3.4:8080 user pass");
        assert_eq!(url, "http://1.2.3.4:8080");
        assert_eq!(user.unwrap(), "user");
        assert_eq!(pass.unwrap(), "pass");
    }

    #[test]
    fn test_parse_proxy_line_with_label() {
        let (url, user, pass, label) = ProxyPool::parse_proxy_line("http://1.2.3.4:8080 user pass US Server 01");
        assert_eq!(url, "http://1.2.3.4:8080");
        assert_eq!(user.unwrap(), "user");
        assert_eq!(pass.unwrap(), "pass");
        assert_eq!(label, "US Server 01");
    }

    #[test]
    fn test_auto_assign() {
        let pool = ProxyPool::new(None);
        pool.add("http://1.1.1.1:8080".into(), None, None, "test1".into());
        pool.add("http://2.2.2.2:8080".into(), None, None, "test2".into());

        let assigned = pool.auto_assign(100).unwrap();
        assert_eq!(assigned.url, "http://1.1.1.1:8080");
        assert_eq!(assigned.assigned_to, Some(100));

        let assigned2 = pool.auto_assign(200).unwrap();
        assert_eq!(assigned2.url, "http://2.2.2.2:8080");

        // 没有空闲的了
        assert!(pool.auto_assign(300).is_none());
    }

    #[test]
    fn test_release_by_credential() {
        let pool = ProxyPool::new(None);
        pool.add("http://1.1.1.1:8080".into(), None, None, "test".into());
        pool.auto_assign(100);

        assert_eq!(pool.available_count(), 0);
        pool.release_by_credential(100);
        assert_eq!(pool.available_count(), 1);
    }
}
