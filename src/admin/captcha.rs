//! CAPTCHA 生成和验证模块
//!
//! 提供基于图像的 CAPTCHA 验证功能,用于防止暴力破解攻击

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use uuid::Uuid;

/// CAPTCHA 会话信息
#[derive(Debug, Clone)]
struct CaptchaSession {
    /// 正确答案(小写)
    answer: String,
    /// 创建时间
    created_at: Instant,
    /// 验证尝试次数
    attempts: u8,
}

/// 速率限制条目
#[derive(Debug, Clone)]
struct RateLimitEntry {
    /// 尝试次数
    attempts: u8,
    /// 时间窗口开始时间
    window_start: Instant,
}

/// CAPTCHA 存储
pub struct CaptchaStore {
    /// CAPTCHA 会话存储 (token -> session)
    sessions: RwLock<HashMap<String, CaptchaSession>>,
    /// 速率限制存储 (IP -> limit entry)
    rate_limits: RwLock<HashMap<String, RateLimitEntry>>,
}

impl CaptchaStore {
    /// 创建新的 CAPTCHA 存储
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            rate_limits: RwLock::new(HashMap::new()),
        }
    }

    /// 生成 CAPTCHA
    ///
    /// 返回 (token, PNG 图像字节)
    pub fn generate(&self) -> (String, Vec<u8>) {
        let (answer, png_bytes) = generate_captcha_image();
        let token = Uuid::new_v4().to_string();

        let session = CaptchaSession {
            answer: answer.to_lowercase(),
            created_at: Instant::now(),
            attempts: 0,
        };

        self.sessions.write().insert(token.clone(), session);

        (token, png_bytes)
    }

    /// 验证 CAPTCHA 答案
    ///
    /// # 错误
    /// - `CaptchaError::TokenNotFound`: Token 不存在或已过期
    /// - `CaptchaError::AnswerIncorrect`: 答案错误
    /// - `CaptchaError::TooManyAttempts`: 尝试次数过多
    pub fn verify(&self, token: &str, answer: &str) -> Result<(), CaptchaError> {
        let mut sessions = self.sessions.write();

        let session = sessions
            .get_mut(token)
            .ok_or(CaptchaError::TokenNotFound)?;

        // 检查尝试次数
        if session.attempts >= 3 {
            sessions.remove(token);
            return Err(CaptchaError::TooManyAttempts);
        }

        session.attempts += 1;

        // 不区分大小写比较
        if session.answer == answer.to_lowercase() {
            // 验证成功,删除 token(一次性使用)
            sessions.remove(token);
            Ok(())
        } else {
            Err(CaptchaError::AnswerIncorrect)
        }
    }

    /// 检查速率限制
    ///
    /// 15 分钟内最多 5 次尝试
    pub fn check_rate_limit(&self, ip: &str) -> Result<(), CaptchaError> {
        const MAX_ATTEMPTS: u8 = 5;
        const WINDOW_DURATION: Duration = Duration::from_secs(15 * 60); // 15 分钟

        let mut rate_limits = self.rate_limits.write();
        let now = Instant::now();

        let entry = rate_limits.entry(ip.to_string()).or_insert(RateLimitEntry {
            attempts: 0,
            window_start: now,
        });

        // 检查时间窗口是否过期
        if now.duration_since(entry.window_start) > WINDOW_DURATION {
            // 重置窗口
            entry.attempts = 0;
            entry.window_start = now;
        }

        // 检查是否超过限制
        if entry.attempts >= MAX_ATTEMPTS {
            return Err(CaptchaError::TooManyAttempts);
        }

        entry.attempts += 1;
        Ok(())
    }

    /// 清理过期的 CAPTCHA 会话和速率限制条目
    pub fn cleanup_expired(&self) {
        const SESSION_TTL: Duration = Duration::from_secs(5 * 60); // 5 分钟
        const RATE_LIMIT_TTL: Duration = Duration::from_secs(15 * 60); // 15 分钟

        let now = Instant::now();

        // 清理过期的 CAPTCHA 会话
        self.sessions
            .write()
            .retain(|_, session| now.duration_since(session.created_at) < SESSION_TTL);

        // 清理过期的速率限制条目
        self.rate_limits
            .write()
            .retain(|_, entry| now.duration_since(entry.window_start) < RATE_LIMIT_TTL);
    }
}

/// CAPTCHA 错误类型
#[derive(Debug, Clone)]
pub enum CaptchaError {
    /// Token 不存在或已过期
    TokenNotFound,
    /// 答案错误
    AnswerIncorrect,
    /// 尝试次数过多
    TooManyAttempts,
}

impl std::fmt::Display for CaptchaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptchaError::TokenNotFound => write!(f, "CAPTCHA token not found or expired"),
            CaptchaError::AnswerIncorrect => write!(f, "Incorrect CAPTCHA answer"),
            CaptchaError::TooManyAttempts => write!(f, "Too many attempts"),
        }
    }
}

impl std::error::Error for CaptchaError {}

/// 生成 CAPTCHA 图像
///
/// 返回 (答案字符串, PNG 图像字节)
fn generate_captcha_image() -> (String, Vec<u8>) {
    use captcha::filters::{Noise, Wave};
    use captcha::Captcha;

    let mut captcha = Captcha::new();

    // 生成 4 个字符
    captcha
        .add_chars(4)
        .apply_filter(Noise::new(0.1)) // 轻度噪声
        .apply_filter(Wave::new(2.0, 10.0)) // 波浪扭曲
        .view(160, 60); // 图像尺寸

    let answer = captcha.chars_as_string();
    let png = captcha.as_png().expect("Failed to generate CAPTCHA PNG");

    (answer, png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_captcha_generation() {
        let store = CaptchaStore::new();
        let (token, png_bytes) = store.generate();

        assert!(!token.is_empty());
        assert!(!png_bytes.is_empty());
        assert!(png_bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47])); // PNG 文件头
    }

    #[test]
    fn test_case_insensitive_verification() {
        let store = CaptchaStore::new();
        let (token, _) = store.generate();

        // 获取答案
        let answer = {
            let sessions = store.sessions.read();
            sessions.get(&token).unwrap().answer.clone()
        };

        // 测试不同大小写
        assert!(store.verify(&token, &answer.to_uppercase()).is_ok());
    }

    #[test]
    fn test_one_time_use() {
        let store = CaptchaStore::new();
        let (token, _) = store.generate();

        let answer = {
            let sessions = store.sessions.read();
            sessions.get(&token).unwrap().answer.clone()
        };

        // 第一次验证成功
        assert!(store.verify(&token, &answer).is_ok());

        // 第二次验证失败(token 已被删除)
        assert!(matches!(
            store.verify(&token, &answer),
            Err(CaptchaError::TokenNotFound)
        ));
    }

    #[test]
    fn test_max_attempts() {
        let store = CaptchaStore::new();
        let (token, _) = store.generate();

        // 3 次错误尝试
        for _ in 0..3 {
            let _ = store.verify(&token, "wrong");
        }

        // 第 4 次尝试应该失败并返回 TooManyAttempts
        assert!(matches!(
            store.verify(&token, "wrong"),
            Err(CaptchaError::TooManyAttempts)
        ));
    }

    #[test]
    fn test_rate_limiting() {
        let store = CaptchaStore::new();
        let ip = "192.168.1.1";

        // 前 5 次应该成功
        for _ in 0..5 {
            assert!(store.check_rate_limit(ip).is_ok());
        }

        // 第 6 次应该被限流
        assert!(matches!(
            store.check_rate_limit(ip),
            Err(CaptchaError::TooManyAttempts)
        ));
    }
}
