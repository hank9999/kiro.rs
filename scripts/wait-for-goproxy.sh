#!/usr/bin/env bash
set -euo pipefail

timeout_secs="${1:-300}"
interval_secs=5
deadline=$((SECONDS + timeout_secs))
min_http_ready="${MIN_HTTP_READY:-6}"
min_total_ready="${MIN_TOTAL_READY:-6}"

while (( SECONDS < deadline )); do
  if payload="$(curl -fsS http://127.0.0.1:7778/api/pool/status 2>/dev/null)"; then
    status_line="$(
      python3 - "$payload" "$min_total_ready" "$min_http_ready" <<'PY'
import json
import sys

data = json.loads(sys.argv[1])
state = data.get("State", "unknown")
total = data.get("Total", 0)
http = data.get("HTTP", 0)
socks5 = data.get("SOCKS5", 0)
ready = total >= int(sys.argv[2]) and http >= int(sys.argv[3])
print(f"{state} {total} {http} {socks5} {'ready' if ready else 'wait'}")
PY
    )"

    read -r state total http socks5 readiness <<<"$status_line"
    printf 'goproxy state=%s total=%s http=%s socks5=%s\n' "$state" "$total" "$http" "$socks5"

    if [[ "$readiness" == "ready" ]]; then
      exit 0
    fi
  fi

  sleep "$interval_secs"
done

echo "goproxy did not become ready within ${timeout_secs}s" >&2
exit 1
