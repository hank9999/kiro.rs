#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""动态模型刷新验证脚本

通过本地 kiro.rs 服务的 `/v1/models` 接口，校验后台刷新任务是否成功
将上游 ListAvailableModels 的真实结果加载进缓存。

用法：
    # 默认从项目根 config.json 读取 host:port 和第一个有效 API Key
    python scripts/test_dynamic_models_refresh.py

    # 自定义服务地址 / API Key
    python scripts/test_dynamic_models_refresh.py --base-url http://127.0.0.1:8080 --api-key sk-xxx

    # 监听模式：每 N 秒轮询一次（默认 5 秒），观察从 fallback → dynamic 的切换
    python scripts/test_dynamic_models_refresh.py --watch 5

    # 同时调用 admin /api/admin/models 对比
    python scripts/test_dynamic_models_refresh.py --admin-key admin-xxx

依赖：
    pip install httpx
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

try:
    import httpx
except ImportError:
    sys.stderr.write("[错误] 需要 httpx。请先安装：pip install httpx\n")
    sys.exit(1)


PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONFIG_PATH = PROJECT_ROOT / "config.json"

# 与 src/anthropic/handlers.rs::fallback_models 保持同步
# 当 /v1/models 返回的 ID 集合 == 此集合时，说明仍在 fallback 状态
FALLBACK_MODEL_IDS: Set[str] = {
    "claude-opus-4-6",
    "claude-opus-4-6-thinking",
    "claude-sonnet-4-6",
    "claude-sonnet-4-6-thinking",
    "claude-opus-4-5-20251101",
    "claude-opus-4-5-20251101-thinking",
    "claude-sonnet-4-5-20250929",
    "claude-sonnet-4-5-20250929-thinking",
    "claude-haiku-4-5-20251001",
    "claude-haiku-4-5-20251001-thinking",
}


def load_config(path: Path) -> Dict[str, Any]:
    if not path.exists():
        raise FileNotFoundError(f"配置文件不存在: {path}")
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def pick_first_api_key(cfg: Dict[str, Any]) -> Optional[str]:
    """模拟 Config::collect_valid_keys 的逻辑，挑第一个能用的 key"""
    main_key = cfg.get("apiKey")
    if isinstance(main_key, str) and main_key.strip():
        return main_key.strip()

    for entry in cfg.get("apiKeys", []) or []:
        if not isinstance(entry, dict):
            continue
        if not entry.get("enabled", False):
            continue
        key = entry.get("key")
        if isinstance(key, str) and key.strip():
            return key.strip()
    return None


def derive_base_url(cfg: Dict[str, Any]) -> str:
    host = cfg.get("host") or "127.0.0.1"
    port = cfg.get("port") or 8080
    return f"http://{host}:{port}"


def fetch_v1_models(base_url: str, api_key: str, timeout: float) -> List[Dict[str, Any]]:
    url = f"{base_url.rstrip('/')}/v1/models"
    resp = httpx.get(
        url,
        headers={"x-api-key": api_key, "anthropic-version": "2023-06-01"},
        timeout=timeout,
    )
    resp.raise_for_status()
    body = resp.json()
    if not isinstance(body, dict):
        raise ValueError(f"非法响应：{body!r}")
    data = body.get("data")
    if not isinstance(data, list):
        raise ValueError(f"响应缺少 data 数组：{body!r}")
    return data


def fetch_admin_models(
    base_url: str, admin_key: str, timeout: float
) -> List[Dict[str, Any]]:
    url = f"{base_url.rstrip('/')}/api/admin/models"
    resp = httpx.get(
        url,
        headers={"x-api-key": admin_key},
        timeout=timeout,
    )
    resp.raise_for_status()
    body = resp.json()
    if isinstance(body, dict) and "data" in body:
        return body.get("data") or []
    if isinstance(body, list):
        return body
    raise ValueError(f"admin 响应格式异常：{body!r}")


def classify(models: List[Dict[str, Any]]) -> str:
    """判断当前模型集合处于 fallback 还是 dynamic 状态"""
    ids = {m.get("id") for m in models if isinstance(m, dict)}
    if ids == FALLBACK_MODEL_IDS:
        return "fallback"
    if ids.issubset(FALLBACK_MODEL_IDS):
        return "fallback-partial"
    if ids & FALLBACK_MODEL_IDS:
        return "mixed"
    return "dynamic"


def summarize(models: List[Dict[str, Any]]) -> Dict[str, Any]:
    return {
        "count": len(models),
        "ids": sorted(
            m.get("id", "<no-id>")
            for m in models
            if isinstance(m, dict)
        ),
        "owned_by_unique": sorted(
            {
                m.get("ownedBy") or m.get("owned_by") or "?"
                for m in models
                if isinstance(m, dict)
            }
        ),
    }


def print_report(state: str, summary: Dict[str, Any], label: str) -> None:
    print(f"---- {label} ----")
    print(f"  状态: {state}")
    print(f"  数量: {summary['count']}")
    print(f"  归属: {', '.join(summary['owned_by_unique'])}")
    print(f"  模型 ID 列表:")
    for mid in summary["ids"]:
        print(f"    - {mid}")


def run_once(
    base_url: str,
    api_key: str,
    admin_key: Optional[str],
    timeout: float,
) -> int:
    print(f"\n========== {time.strftime('%Y-%m-%d %H:%M:%S')} ==========")
    print(f"目标服务: {base_url}")

    try:
        v1_models = fetch_v1_models(base_url, api_key, timeout)
    except Exception as e:
        print(f"  /v1/models 调用失败: {e}", file=sys.stderr)
        return 2

    state = classify(v1_models)
    print_report(state, summarize(v1_models), "GET /v1/models")

    if admin_key:
        try:
            admin_models = fetch_admin_models(base_url, admin_key, timeout)
            admin_state = classify(admin_models)
            print_report(
                admin_state,
                summarize(admin_models),
                "GET /api/admin/models",
            )
            if admin_state != state:
                print(
                    "  ⚠ 警告：/v1/models 与 /api/admin/models 状态不一致，"
                    "排查共享缓存是否被注入两套实例。",
                    file=sys.stderr,
                )
        except Exception as e:
            print(f"  /api/admin/models 调用失败: {e}", file=sys.stderr)

    if state == "dynamic":
        print("\n  ✅ 动态拉取生效：当前缓存为上游真实模型列表。")
        return 0
    if state == "fallback":
        print("\n  ⏳ 仍在 fallback 状态：可能后台任务尚未首次成功，或 dynamicModels.enabled=false。")
        return 1
    print(f"\n  ⚠ 未知状态：{state}，请检查 fallback_models 是否被改动。")
    return 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="动态模型刷新验证脚本")
    parser.add_argument(
        "--config",
        default=str(DEFAULT_CONFIG_PATH),
        help=f"配置文件路径（默认 {DEFAULT_CONFIG_PATH}）",
    )
    parser.add_argument(
        "--base-url",
        default=None,
        help="服务 base URL（覆盖 config.json 的 host/port）",
    )
    parser.add_argument(
        "--api-key",
        default=None,
        help="访问 /v1/models 的 API Key（覆盖 config.json 中的第一个有效 key）",
    )
    parser.add_argument(
        "--admin-key",
        default=None,
        help="可选 admin API Key，提供后会同时校验 /api/admin/models",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=10.0,
        help="单次请求超时（秒），默认 10",
    )
    parser.add_argument(
        "--watch",
        type=float,
        default=0.0,
        metavar="SECS",
        help="监听模式：每 SECS 秒轮询一次（默认关闭）",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    config_path = Path(args.config)
    cfg: Dict[str, Any] = {}
    if config_path.exists():
        try:
            cfg = load_config(config_path)
        except Exception as e:
            print(f"[警告] 读取配置失败：{e}", file=sys.stderr)

    base_url = args.base_url or derive_base_url(cfg)
    api_key = args.api_key or pick_first_api_key(cfg)
    admin_key = args.admin_key or cfg.get("adminApiKey") or None
    if isinstance(admin_key, str) and not admin_key.strip():
        admin_key = None

    if not api_key:
        print(
            "[错误] 未指定 API Key，且 config.json 未提供有效的 apiKey/apiKeys。",
            file=sys.stderr,
        )
        return 2

    if args.watch and args.watch > 0:
        print(f"监听模式：每 {args.watch:.1f} 秒刷新一次，Ctrl+C 退出")
        try:
            while True:
                run_once(base_url, api_key, admin_key, args.timeout)
                time.sleep(args.watch)
        except KeyboardInterrupt:
            print("\n已退出监听。")
            return 0

    return run_once(base_url, api_key, admin_key, args.timeout)


if __name__ == "__main__":
    sys.exit(main())
