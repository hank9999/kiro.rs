#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
ListAvailableModels / ListAvailableProfiles 探测脚本

读取项目根目录下的 `config.json` 与 `credentials.json`，按现网相同的
认证方式刷新 token，然后调用上游 Kiro / CodeWhisperer 的元数据接口：

  GET https://q.{region}.amazonaws.com/ListAvailableProfiles
  GET https://q.{region}.amazonaws.com/ListAvailableModels?origin=AI_EDITOR&profileArn={arn}

打印完整响应 JSON，便于据此设计 Rust 类型。

依赖:
    pip install "httpx[socks]"

用法:
    python scripts/test_list_models.py
    python scripts/test_list_models.py --credential-id 2
    python scripts/test_list_models.py --no-proxy
    python scripts/test_list_models.py --profile-arn "arn:aws:..."
    python scripts/test_list_models.py --skip-profiles  # 跳过 ListAvailableProfiles
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import sys
import uuid
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

try:
    import httpx
except ImportError:
    sys.stderr.write(
        "[错误] 需要 httpx。请先安装：pip install \"httpx[socks]\"\n"
    )
    sys.exit(1)


PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONFIG_PATH = PROJECT_ROOT / "config.json"
DEFAULT_CREDS_PATH = PROJECT_ROOT / "credentials.json"


def mask(value: Optional[str], head: int = 6, tail: int = 4) -> str:
    """安全地脱敏长字符串"""
    if not value:
        return "<空>"
    if len(value) <= head + tail:
        return value[:1] + "***"
    return f"{value[:head]}...{value[-tail:]}"


def sha256_hex(s: str) -> str:
    return hashlib.sha256(s.encode("utf-8")).hexdigest()


def normalize_machine_id(raw: Optional[str]) -> Optional[str]:
    """与 src/kiro/machine_id.rs::normalize_machine_id 行为对齐"""
    if not raw:
        return None
    trimmed = raw.strip()
    if len(trimmed) == 64 and all(c in "0123456789abcdefABCDEF" for c in trimmed):
        return trimmed.lower()
    without_dashes = trimmed.replace("-", "")
    if len(without_dashes) == 32 and all(
        c in "0123456789abcdefABCDEF" for c in without_dashes
    ):
        return (without_dashes + without_dashes).lower()
    return None


def derive_machine_id(
    cred: Dict[str, Any],
    config: Dict[str, Any],
) -> str:
    """与 src/kiro/machine_id.rs::generate_from_credentials 对齐"""
    cred_mid = normalize_machine_id(cred.get("machineId"))
    if cred_mid:
        return cred_mid
    cfg_mid = normalize_machine_id(config.get("machineId"))
    if cfg_mid:
        return cfg_mid
    auth_method = (cred.get("authMethod") or "").lower()
    if auth_method == "api_key":
        api_key = cred.get("kiroApiKey")
        if api_key:
            return sha256_hex(f"KiroAPIKey/{api_key}")
    refresh_token = cred.get("refreshToken")
    if refresh_token:
        return sha256_hex(f"KotlinNativeAPI/{refresh_token}")
    return sha256_hex(f"KiroFallback/{uuid.uuid4()}")


def load_credentials_file(path: Path) -> List[Dict[str, Any]]:
    """credentials.json 既支持单条对象也支持数组，统一返回 list"""
    if not path.exists():
        sys.stderr.write(f"[错误] 凭据文件不存在: {path}\n")
        sys.exit(1)
    data = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(data, dict):
        return [data]
    if isinstance(data, list):
        return data
    sys.stderr.write(f"[错误] 不识别的 credentials.json 结构: {type(data).__name__}\n")
    sys.exit(1)


def pick_credential(
    creds: List[Dict[str, Any]], target_id: Optional[int]
) -> Tuple[int, Dict[str, Any]]:
    """选择一条凭据，target_id=1-based 序号"""
    if target_id is not None:
        idx = target_id - 1
        if idx < 0 or idx >= len(creds):
            sys.stderr.write(
                f"[错误] credential-id={target_id} 越界，共 {len(creds)} 条\n"
            )
            sys.exit(1)
        return target_id, creds[idx]
    for i, c in enumerate(creds, start=1):
        if not c.get("disabled"):
            return i, c
    sys.stderr.write("[错误] 没有可用凭据（全部 disabled）\n")
    sys.exit(1)


def resolve_proxy(
    config: Dict[str, Any],
    cred: Dict[str, Any],
    no_proxy: bool,
) -> Optional[str]:
    """凭据级 proxyUrl 优先；否则 config.proxyUrl；再否则随机选一个 proxyPool.urls"""
    if no_proxy:
        return None
    cred_proxy = cred.get("proxyUrl")
    if cred_proxy:
        return cred_proxy
    global_proxy = config.get("proxyUrl")
    if global_proxy:
        return global_proxy
    pool = config.get("proxyPool") or {}
    if pool.get("enabled"):
        urls = pool.get("urls") or []
        if urls:
            return random.choice(urls)
    return None


def make_client(proxy: Optional[str], timeout: float = 60.0) -> httpx.Client:
    if proxy:
        sys.stderr.write(f"[信息] 使用代理: {proxy}\n")
        return httpx.Client(proxy=proxy, timeout=timeout, verify=True)
    return httpx.Client(timeout=timeout, verify=True)


def build_kiro_headers(
    config: Dict[str, Any],
    cred: Dict[str, Any],
    machine_id: str,
    access_token: str,
) -> Dict[str, str]:
    """与 src/kiro/endpoint/ide.rs::decorate_api 保持一致"""
    kiro_version = config.get("kiroVersion", "0.11.107")
    system_version = config.get("systemVersion", "darwin#24.6.0")
    node_version = config.get("nodeVersion", "22.22.0")
    user_agent = (
        f"aws-sdk-js/1.0.34 ua/2.1 os/{system_version} lang/js "
        f"md/nodejs#{node_version} api/codewhispererstreaming#1.0.34 m/E "
        f"KiroIDE-{kiro_version}-{machine_id}"
    )
    x_amz_user_agent = f"aws-sdk-js/1.0.34 KiroIDE-{kiro_version}-{machine_id}"
    headers = {
        "Authorization": f"Bearer {access_token}",
        "x-amzn-codewhisperer-optout": "true",
        "x-amzn-kiro-agent-mode": "vibe",
        "x-amz-user-agent": x_amz_user_agent,
        "user-agent": user_agent,
        "amz-sdk-invocation-id": str(uuid.uuid4()),
        "amz-sdk-request": "attempt=1; max=3",
    }
    if (cred.get("authMethod") or "").lower() == "api_key":
        headers["tokentype"] = "API_KEY"
    return headers


def refresh_token(
    config: Dict[str, Any],
    cred: Dict[str, Any],
    machine_id: str,
    proxy: Optional[str],
) -> Tuple[str, Optional[str]]:
    """根据 authMethod 走不同刷新流程，返回 (access_token, profile_arn)"""
    auth_method = (cred.get("authMethod") or "").lower()
    if auth_method == "api_key":
        api_key = cred.get("kiroApiKey")
        if not api_key:
            sys.stderr.write("[错误] api_key 凭据缺少 kiroApiKey\n")
            sys.exit(1)
        sys.stderr.write("[信息] 使用 api_key 凭据，不需要刷新 token\n")
        return api_key, None
    if auth_method in ("idc", "builder-id", "iam"):
        return refresh_idc_token(config, cred, machine_id, proxy)
    return refresh_social_token(config, cred, machine_id, proxy)


def refresh_social_token(
    config: Dict[str, Any],
    cred: Dict[str, Any],
    machine_id: str,
    proxy: Optional[str],
) -> Tuple[str, Optional[str]]:
    refresh = cred.get("refreshToken")
    if not refresh:
        sys.stderr.write("[错误] social 凭据缺少 refreshToken\n")
        sys.exit(1)
    region = (
        cred.get("authRegion")
        or cred.get("region")
        or config.get("authRegion")
        or config.get("region")
        or "us-east-1"
    )
    url = f"https://prod.{region}.auth.desktop.kiro.dev/refreshToken"
    domain = f"prod.{region}.auth.desktop.kiro.dev"
    kiro_version = config.get("kiroVersion", "0.11.107")
    headers = {
        "Accept": "application/json, text/plain, */*",
        "Content-Type": "application/json",
        "User-Agent": f"KiroIDE-{kiro_version}-{machine_id}",
        "Accept-Encoding": "gzip, compress, deflate, br",
        "host": domain,
        "Connection": "close",
    }
    sys.stderr.write(f"[信息] 刷新 social token: {url}\n")
    with make_client(proxy) as client:
        resp = client.post(url, json={"refreshToken": refresh}, headers=headers)
    if resp.status_code != 200:
        sys.stderr.write(
            f"[错误] social token 刷新失败: {resp.status_code}\n{resp.text}\n"
        )
        sys.exit(1)
    data = resp.json()
    access = data.get("accessToken")
    profile_arn = data.get("profileArn")
    if not access:
        sys.stderr.write(f"[错误] social 响应缺少 accessToken: {data}\n")
        sys.exit(1)
    sys.stderr.write(
        f"[信息] social token 刷新成功，access_token={mask(access)}, "
        f"profile_arn={profile_arn or '<空>'}\n"
    )
    return access, profile_arn


def refresh_idc_token(
    config: Dict[str, Any],
    cred: Dict[str, Any],
    machine_id: str,
    proxy: Optional[str],
) -> Tuple[str, Optional[str]]:
    client_id = cred.get("clientId")
    client_secret = cred.get("clientSecret")
    refresh = cred.get("refreshToken")
    if not (client_id and client_secret and refresh):
        sys.stderr.write("[错误] idc 凭据缺少 clientId/clientSecret/refreshToken\n")
        sys.exit(1)
    region = (
        cred.get("authRegion")
        or cred.get("region")
        or config.get("authRegion")
        or config.get("region")
        or "us-east-1"
    )
    url = f"https://oidc.{region}.amazonaws.com/token"
    system_version = config.get("systemVersion", "darwin#24.6.0")
    node_version = config.get("nodeVersion", "22.22.0")
    headers = {
        "content-type": "application/json",
        "x-amz-user-agent": "aws-sdk-js/3.980.0 KiroIDE",
        "user-agent": (
            f"aws-sdk-js/3.980.0 ua/2.1 os/{system_version} lang/js "
            f"md/nodejs#{node_version} api/sso-oidc#3.980.0 m/E KiroIDE"
        ),
        "host": f"oidc.{region}.amazonaws.com",
        "amz-sdk-invocation-id": str(uuid.uuid4()),
        "amz-sdk-request": "attempt=1; max=4",
        "Connection": "close",
    }
    body = {
        "clientId": client_id,
        "clientSecret": client_secret,
        "refreshToken": refresh,
        "grantType": "refresh_token",
    }
    sys.stderr.write(f"[信息] 刷新 idc token: {url}\n")
    with make_client(proxy) as client:
        resp = client.post(url, json=body, headers=headers)
    if resp.status_code != 200:
        sys.stderr.write(
            f"[错误] idc token 刷新失败: {resp.status_code}\n{resp.text}\n"
        )
        sys.exit(1)
    data = resp.json()
    access = data.get("accessToken")
    profile_arn = data.get("profileArn")
    if not access:
        sys.stderr.write(f"[错误] idc 响应缺少 accessToken: {data}\n")
        sys.exit(1)
    sys.stderr.write(
        f"[信息] idc token 刷新成功，access_token={mask(access)}, "
        f"profile_arn={profile_arn or '<空>'}\n"
    )
    return access, profile_arn


def call_list_profiles(
    api_host: str,
    headers: Dict[str, str],
    proxy: Optional[str],
) -> Tuple[int, Any]:
    url = f"{api_host}/ListAvailableProfiles"
    sys.stderr.write(f"[信息] GET {url}\n")
    with make_client(proxy) as client:
        resp = client.get(url, headers=headers)
    text = resp.text
    try:
        body = resp.json()
    except Exception:
        body = text
    return resp.status_code, body


def call_list_models(
    api_host: str,
    headers: Dict[str, str],
    proxy: Optional[str],
    profile_arn: Optional[str],
) -> Tuple[int, Any]:
    url = f"{api_host}/ListAvailableModels"
    params: Dict[str, str] = {"origin": "AI_EDITOR"}
    if profile_arn:
        params["profileArn"] = profile_arn
    sys.stderr.write(f"[信息] GET {url}?{ '&'.join(f'{k}={v}' for k,v in params.items()) }\n")
    with make_client(proxy) as client:
        resp = client.get(url, headers=headers, params=params)
    text = resp.text
    try:
        body = resp.json()
    except Exception:
        body = text
    return resp.status_code, body


def main() -> int:
    parser = argparse.ArgumentParser(
        description="探测 Kiro 上游 ListAvailableModels / ListAvailableProfiles 接口",
    )
    parser.add_argument(
        "--config", type=Path, default=DEFAULT_CONFIG_PATH, help="config.json 路径"
    )
    parser.add_argument(
        "--credentials",
        type=Path,
        default=DEFAULT_CREDS_PATH,
        help="credentials.json 路径",
    )
    parser.add_argument(
        "--credential-id",
        type=int,
        default=None,
        help="选择第几条凭据（1-based），默认第一条非 disabled",
    )
    parser.add_argument("--no-proxy", action="store_true", help="禁用代理直连")
    parser.add_argument(
        "--profile-arn",
        default=None,
        help="手动指定 profileArn（绕过 ListAvailableProfiles）",
    )
    parser.add_argument(
        "--skip-profiles", action="store_true", help="跳过 ListAvailableProfiles 调用"
    )
    args = parser.parse_args()

    config = json.loads(args.config.read_text(encoding="utf-8"))
    creds = load_credentials_file(args.credentials)
    cred_idx, cred = pick_credential(creds, args.credential_id)
    machine_id = derive_machine_id(cred, config)
    proxy = resolve_proxy(config, cred, args.no_proxy)

    api_region = (
        cred.get("apiRegion")
        or cred.get("region")
        or config.get("apiRegion")
        or config.get("region")
        or "us-east-1"
    )
    api_host = f"https://q.{api_region}.amazonaws.com"

    sys.stderr.write("\n========== 准备 ==========\n")
    sys.stderr.write(f"配置文件:   {args.config}\n")
    sys.stderr.write(f"凭据文件:   {args.credentials}\n")
    sys.stderr.write(f"凭据序号:   #{cred_idx}/{len(creds)}\n")
    sys.stderr.write(f"认证方式:   {cred.get('authMethod') or 'social'}\n")
    sys.stderr.write(f"machineId:  {machine_id[:12]}...{machine_id[-4:]}\n")
    sys.stderr.write(f"api_region: {api_region}\n")
    sys.stderr.write(f"api_host:   {api_host}\n")
    sys.stderr.write(f"代理:       {proxy or '<直连>'}\n")
    sys.stderr.write("\n")

    access_token, profile_arn_from_refresh = refresh_token(
        config, cred, machine_id, proxy
    )
    headers = build_kiro_headers(config, cred, machine_id, access_token)

    profile_arn: Optional[str] = (
        args.profile_arn
        or profile_arn_from_refresh
        or cred.get("profileArn")
    )

    if not args.skip_profiles:
        sys.stderr.write("\n========== ListAvailableProfiles ==========\n")
        status, body = call_list_profiles(api_host, headers, proxy)
        sys.stderr.write(f"[响应] HTTP {status}\n")
        print(json.dumps(
            {"endpoint": "ListAvailableProfiles", "status": status, "body": body},
            indent=2,
            ensure_ascii=False,
        ))
        if status == 200 and isinstance(body, dict) and not profile_arn:
            profiles = body.get("profiles") or []
            if profiles and isinstance(profiles[0], dict):
                profile_arn = profiles[0].get("arn")
                sys.stderr.write(
                    f"[信息] 自动选用第一个 profileArn: {profile_arn}\n"
                )

    sys.stderr.write("\n========== ListAvailableModels ==========\n")
    status, body = call_list_models(api_host, headers, proxy, profile_arn)
    sys.stderr.write(f"[响应] HTTP {status}\n")
    print(json.dumps(
        {
            "endpoint": "ListAvailableModels",
            "status": status,
            "profileArn": profile_arn,
            "body": body,
        },
        indent=2,
        ensure_ascii=False,
    ))

    if status == 200:
        sys.stderr.write("\n[完成] 调用成功，请检查上方 JSON 内容确认 schema\n")
        return 0
    sys.stderr.write(f"\n[失败] 调用未成功，HTTP {status}\n")
    return 1


if __name__ == "__main__":
    sys.exit(main())
