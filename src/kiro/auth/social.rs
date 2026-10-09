//! Kiro IDE Social 登录流程（Portal PKCE OAuth）
//!
//! 复现 Kiro IDE 的 portal-auth-provider 流程：
//! 1. 生成 PKCE code_verifier + code_challenge
//! 2. 启本地 HTTP 回调服务器
//! 3. 返回 portal URL 供用户在浏览器完成登录
//! 4. 捕获回调中的 authorization code
//! 5. 用 code + code_verifier 换取 access_token + refresh_token

use std::net::TcpListener;

use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::model::token_refresh::{SocialCreateTokenRequest, SocialCreateTokenResponse};
use crate::model::config::Config;

/// Portal 认证 URL（Kiro 网页版入口）
pub const KIRO_PORTAL_URL: &str = "https://app.kiro.dev";

/// Kiro auth service 默认端点
pub const KIRO_AUTH_ENDPOINT: &str = "https://prod.us-east-1.auth.desktop.kiro.dev";

/// 与 IDE 一致的本地回调端口候选列表
const CALLBACK_PORTS: &[u16] = &[3128, 4649, 6588, 8008, 9091, 49153, 50153, 51153, 52153, 53153];

/// OAuth 回调数据
#[derive(Debug, Clone)]
pub struct OAuthCallbackData {
    pub code: String,
    pub login_option: String,
    pub path: String,
    /// OAuth state 参数（用于 CSRF 验证）
    pub state: String,
}

/// 回调服务器关闭句柄
///
/// Drop 时自动向服务器发送关闭信号，服务器退出监听循环并释放端口。
pub struct ServerHandle {
    _shutdown_tx: oneshot::Sender<()>,
}

/// 启动本地回调服务器，返回端口号和关闭句柄
///
/// 关闭句柄 drop 时服务器自动停止。当收到有效的 OAuth 回调时，通过 channel 发送回调数据。
pub fn start_callback_server(
    tx: oneshot::Sender<OAuthCallbackData>,
) -> anyhow::Result<(u16, ServerHandle)> {
    // 直接持有已绑定的 socket，避免 probe-and-bind 的 TOCTOU 竞态
    let (port, std_listener) = bind_available_port()?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        run_callback_server(std_listener, tx, shutdown_rx).await;
    });

    Ok((port, ServerHandle { _shutdown_tx: shutdown_tx }))
}

fn bind_available_port() -> anyhow::Result<(u16, std::net::TcpListener)> {
    for &port in CALLBACK_PORTS {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                listener.set_nonblocking(true)?;
                return Ok((port, listener));
            }
            Err(_) => continue,
        }
    }
    anyhow::bail!("所有回调端口均被占用，请确保没有其他程序占用 {:?}", CALLBACK_PORTS)
}

async fn run_callback_server(
    std_listener: std::net::TcpListener,
    tx: oneshot::Sender<OAuthCallbackData>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let port = std_listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let listener = match TcpListener::from_std(std_listener) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Social 回调服务器初始化失败 (port {}): {}", port, e);
            return;
        }
    };

    tracing::info!("Social 回调服务器已启动: http://127.0.0.1:{}", port);

    // 只等待一次成功的回调，或关闭信号
    let mut tx = Some(tx);
    loop {
        let (mut stream, _addr) = tokio::select! {
            result = listener.accept() => match result {
                Ok(s) => s,
                Err(_) => break,
            },
            _ = &mut shutdown_rx => {
                tracing::info!("Social 回调服务器收到关闭信号，端口 {} 已释放", port);
                break;
            }
        };

        let mut buf = vec![0u8; 4096];
        let n = match stream.read(&mut buf).await {
            Ok(n) => n,
            Err(_) => continue,
        };

        let request = String::from_utf8_lossy(&buf[..n]);
        let first_line = request.lines().next().unwrap_or("");

        // GET /oauth/callback?... HTTP/1.1
        if let Some(path_and_query) = first_line
            .strip_prefix("GET ")
            .and_then(|s| s.strip_suffix(" HTTP/1.1").or_else(|| s.strip_suffix(" HTTP/1.0")))
        {
            if let Some(callback) = parse_callback(path_and_query) {
                let body = "<html><head><meta charset='utf-8'><title>登录成功</title></head><body style='font-family:sans-serif;text-align:center;padding:60px'><h2>&#10003; 登录成功</h2><p>Token 已更新，请返回 Kiro Admin UI。</p><p style='color:#888;font-size:13px'>此标签页可以关闭。</p></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;

                if let Some(sender) = tx.take() {
                    let _ = sender.send(callback);
                }
                break;
            } else if path_and_query.starts_with("/oauth/callback") || path_and_query.starts_with("/signin/callback") {
                // 有 error 参数的回调
                let error_msg = path_and_query
                    .split('?')
                    .nth(1)
                    .and_then(|q| {
                        let p = parse_query_string(q);
                        p.get("error_description").or_else(|| p.get("error")).cloned()
                    })
                    .unwrap_or_else(|| "未知错误".to_string());

                let body = format!(
                    "<html><head><meta charset='utf-8'><title>登录失败</title></head><body style='font-family:sans-serif;text-align:center;padding:60px'><h2>&#10007; 登录失败</h2><p>{}</p><p style='color:#888;font-size:13px'>请关闭此标签页并重试。</p></body></html>",
                    error_msg
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
                break;
            }
        }

        // 其他请求返回 404
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
    }
}

fn parse_callback(path_and_query: &str) -> Option<OAuthCallbackData> {
    let (path, query) = if let Some(idx) = path_and_query.find('?') {
        (&path_and_query[..idx], &path_and_query[idx + 1..])
    } else {
        return None;
    };

    if path != "/oauth/callback" && path != "/signin/callback" {
        return None;
    }

    let params = parse_query_string(query);

    // 必须有 code 且没有 error
    if params.contains_key("error") {
        return None;
    }

    let code = params.get("code")?.clone();
    let login_option = params.get("login_option").cloned().unwrap_or_default();
    let state = params.get("state").cloned().unwrap_or_default();

    Some(OAuthCallbackData {
        code,
        login_option,
        path: path.to_string(),
        state,
    })
}

/// base64url 编码（无填充），与 Kiro IDE 行为一致
fn base64url_encode(data: &[u8]) -> String {
    // 标准 base64 → 替换 +/= 为 base64url 规范
    let b64 = base64_encode_standard(data);
    b64.replace('+', "-").replace('/', "_").replace('=', "")
}

/// 标准 base64 编码（用于内部转换）
fn base64_encode_standard(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 { chunk[1] as usize } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as usize } else { 0 };
        out.push(CHARS[(b0 >> 2)] as char);
        out.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 { out.push(CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char); } else { out.push('='); }
        if chunk.len() > 2 { out.push(CHARS[b2 & 0x3f] as char); } else { out.push('='); }
    }
    out
}

/// 生成 PKCE code_verifier 和 code_challenge
pub fn generate_pkce() -> (String, String) {
    // 32 字节随机数作为 verifier（与 IDE crypto.randomBytes(32).toString("base64url") 等价）
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = fastrand::u8(..).wrapping_add(i as u8);
    }
    // 使用 uuid v4 的随机性来增强
    let uuid_bytes = uuid::Uuid::new_v4().as_bytes().to_owned();
    for (i, b) in bytes.iter_mut().enumerate() {
        *b ^= uuid_bytes[i % 16];
    }

    let verifier = base64url_encode(&bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    let challenge = base64url_encode(&digest);

    (verifier, challenge)
}

/// 构建供用户在浏览器中访问的 portal URL
pub fn build_portal_url(state: &str, code_challenge: &str, redirect_uri: &str) -> String {
    let params = format!(
        "state={}&code_challenge={}&code_challenge_method=S256&redirect_uri={}&redirect_from=KiroIDE",
        urlencoding::encode(state),
        urlencoding::encode(code_challenge),
        urlencoding::encode(redirect_uri),
    );
    format!("{}/signin?{}", KIRO_PORTAL_URL, params)
}

/// 简易 query string 解析（不依赖 url crate）
fn parse_query_string(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let mut iter = pair.splitn(2, '=');
            let key = iter.next()?.to_string();
            let val = iter.next().map(|v| {
                // 简单的 percent-decode（处理 %XX 和 + 号）
                let with_space = v.replace('+', " ");
                urlencoding::decode(&with_space).map(|s| s.into_owned()).unwrap_or_else(|_| with_space)
            }).unwrap_or_default();
            Some((key, val))
        })
        .collect()
}

/// 用 authorization code 换取 access_token + refresh_token
pub async fn exchange_code_for_token(
    auth_endpoint: &str,
    code: &str,
    code_verifier: &str,
    full_redirect_uri: &str,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<SocialCreateTokenResponse> {
    let url = format!("{}/oauth/token", auth_endpoint);
    let client = build_client(proxy, 30, config.tls_backend)?;

    let body = SocialCreateTokenRequest {
        code: code.to_string(),
        code_verifier: code_verifier.to_string(),
        redirect_uri: full_redirect_uri.to_string(),
        invitation_code: None,
    };

    let kiro_version = &config.kiro_version;
    let user_agent = format!("KiroIDE-{}", kiro_version);

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("User-Agent", &user_agent)
        .header("host", auth_endpoint.trim_start_matches("https://"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Social token 交换失败 {}: {}", status, body_text);
    }

    resp.json::<SocialCreateTokenResponse>()
        .await
        .map_err(|e| anyhow::anyhow!("解析 Social token 响应失败: {}", e))
}
