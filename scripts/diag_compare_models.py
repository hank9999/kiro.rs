"""对比每个凭据返回的 ListAvailableModels 是否一致"""
import json
import subprocess
import sys
from pathlib import Path

results = {}
for cred_id in (1, 2, 3, 4):
    print(f"\n========== 测试凭据 #{cred_id} ==========")
    proc = subprocess.run(
        [
            sys.executable,
            "scripts/test_list_models.py",
            "--credential-id",
            str(cred_id),
            "--skip-profiles",
        ],
        cwd=Path(__file__).resolve().parent.parent,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if proc.returncode != 0:
        print(f"  失败 returncode={proc.returncode}")
        print(f"  stderr 后 500 字符: {proc.stderr[-500:]}")
        results[cred_id] = None
        continue
    try:
        body = json.loads(proc.stdout)
    except Exception as e:
        print(f"  解析失败: {e}")
        print(f"  stdout 前 500 字符: {proc.stdout[:500]}")
        results[cred_id] = None
        continue
    if body.get("status") != 200:
        print(f"  HTTP {body.get('status')}")
        results[cred_id] = None
        continue
    models = body.get("body", {}).get("models", [])
    default_model = body.get("body", {}).get("defaultModel", {})
    model_ids = sorted(m.get("modelId") for m in models)
    print(f"  模型数量: {len(models)}")
    print(f"  默认模型: {default_model.get('modelId')}")
    print(f"  模型列表: {', '.join(model_ids)}")
    results[cred_id] = {
        "default": default_model.get("modelId"),
        "model_ids": model_ids,
        "models": {m.get("modelId"): m for m in models},
    }

print("\n========== 对比结果 ==========")
all_ids = set()
for r in results.values():
    if r:
        all_ids.update(r["model_ids"])

baseline = next((r for r in results.values() if r), None)
if baseline:
    is_all_same = all(
        r and r["model_ids"] == baseline["model_ids"]
        for r in results.values()
    )
    if is_all_same:
        print("✅ 所有凭据返回的模型列表完全一致")
        print(f"   默认模型: {baseline['default']}")
        print(f"   模型 ID:  {', '.join(baseline['model_ids'])}")
    else:
        print("⚠️ 不同凭据返回的模型列表不一致！需要按凭据各自缓存")
        for cred_id, r in results.items():
            if r:
                print(
                    f"  #{cred_id}: default={r['default']}, "
                    f"models=[{', '.join(r['model_ids'])}]"
                )
            else:
                print(f"  #{cred_id}: 调用失败")
        print("\n  差异分析（出现在某些凭据但不在其他凭据的模型）:")
        for mid in sorted(all_ids):
            present = [
                str(cid)
                for cid, r in results.items()
                if r and mid in r["model_ids"]
            ]
            absent = [
                str(cid)
                for cid, r in results.items()
                if r and mid not in r["model_ids"]
            ]
            if absent:
                print(
                    f"    {mid}: 出现在 #{','.join(present)}，"
                    f"不在 #{','.join(absent)}"
                )
