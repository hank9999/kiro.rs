# 轮询模式（round_robin）端到端验证脚本
#
# 前置条件：
#   1. 服务已启动并监听 ServerUrl（默认 http://127.0.0.1:8080）
#   2. 已配置 Admin API Key 并通过参数 -ApiKey 传入
#   3. 至少配置了 2 个可用 Kiro 凭据
#
# 用法示例：
#   pwsh ./tests/round_robin_e2e.ps1 -ApiKey "ksk_xxx" -ServerUrl "http://127.0.0.1:8080" -RequestCount 6
#   pwsh ./tests/round_robin_e2e.ps1 -ApiKey "ksk_xxx" -SkipMessageRequests   # 仅验证 Admin API
#
# 退出码：
#   0 = 全部通过
#   1 = 至少一个用例失败

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ApiKey,

    [string]$ServerUrl = "http://127.0.0.1:8080",

    [int]$RequestCount = 6,

    [string]$Model = "claude-sonnet-4-20250514",

    [switch]$SkipMessageRequests
)

$ErrorActionPreference = "Stop"
$script:FailCount = 0
$script:PassCount = 0

function Write-Section($title) {
    Write-Host ""
    Write-Host "==== $title ====" -ForegroundColor Cyan
}

function Assert-True($cond, $msg) {
    if ($cond) {
        Write-Host "  [PASS] $msg" -ForegroundColor Green
        $script:PassCount++
    } else {
        Write-Host "  [FAIL] $msg" -ForegroundColor Red
        $script:FailCount++
    }
}

function Invoke-Admin {
    param(
        [Parameter(Mandatory = $true)] [string]$Method,
        [Parameter(Mandatory = $true)] [string]$Path,
        [object]$Body
    )
    $url = "$ServerUrl/api/admin$Path"
    $headers = @{ "x-api-key" = $ApiKey }
    if ($Body -ne $null) {
        $json = ($Body | ConvertTo-Json -Depth 6 -Compress)
        return Invoke-RestMethod -Method $Method -Uri $url -Headers $headers -ContentType "application/json" -Body $json
    } else {
        return Invoke-RestMethod -Method $Method -Uri $url -Headers $headers
    }
}

# ---------- 1. 切换到 round_robin 模式 ----------
Write-Section "1. 切换到 round_robin 模式"
try {
    $resp = Invoke-Admin -Method "PUT" -Path "/config/load-balancing" -Body @{ mode = "round_robin" }
    Assert-True ($resp.mode -eq "round_robin") "PUT /config/load-balancing 返回 mode=round_robin"
} catch {
    Assert-True $false "PUT /config/load-balancing 应该成功，但抛出: $_"
}

try {
    $get = Invoke-Admin -Method "GET" -Path "/config/load-balancing"
    Assert-True ($get.mode -eq "round_robin") "GET /config/load-balancing 返回 mode=round_robin"
} catch {
    Assert-True $false "GET /config/load-balancing 失败: $_"
}

# ---------- 2. 拒绝非法模式值 ----------
Write-Section "2. 非法模式值应被拒绝"
try {
    $null = Invoke-Admin -Method "PUT" -Path "/config/load-balancing" -Body @{ mode = "rotation_invalid" }
    Assert-True $false "非法模式应该被拒绝（HTTP 4xx），但请求成功了"
} catch {
    Assert-True $true "非法模式被正确拒绝"
}

# ---------- 3. 切换到 priority / balanced 都仍可用 ----------
Write-Section "3. 三模式互切"
foreach ($m in @("priority", "balanced", "round_robin")) {
    try {
        $resp = Invoke-Admin -Method "PUT" -Path "/config/load-balancing" -Body @{ mode = $m }
        Assert-True ($resp.mode -eq $m) "切换到 $m 成功"
    } catch {
        Assert-True $false "切换到 $m 失败: $_"
    }
}

# ---------- 4. round_robin 实际轮询验证（可选） ----------
if ($SkipMessageRequests) {
    Write-Host ""
    Write-Host "已通过 -SkipMessageRequests 跳过实际请求测试" -ForegroundColor Yellow
} else {
    Write-Section "4. 发起 $RequestCount 次请求，观察凭据轮换"
    # 切回 round_robin
    $null = Invoke-Admin -Method "PUT" -Path "/config/load-balancing" -Body @{ mode = "round_robin" }

    # 拍照请求前每个凭据的 success_count
    $before = Invoke-Admin -Method "GET" -Path "/credentials"
    $beforeMap = @{}
    foreach ($c in $before.credentials) {
        $beforeMap[$c.id] = $c.successCount
    }
    Write-Host ("  请求前 success_count: " + (($beforeMap.GetEnumerator() | ForEach-Object { "id=$($_.Key):$($_.Value)" }) -join ", "))

    $messageBody = @{
        model     = $Model
        max_tokens = 16
        messages  = @(@{ role = "user"; content = "ping" })
    }

    $sentOk = 0
    for ($i = 1; $i -le $RequestCount; $i++) {
        try {
            $null = Invoke-RestMethod -Method "POST" -Uri "$ServerUrl/v1/messages" `
                -Headers @{ "x-api-key" = $ApiKey; "anthropic-version" = "2023-06-01" } `
                -ContentType "application/json" `
                -Body ($messageBody | ConvertTo-Json -Depth 6 -Compress)
            $sentOk++
        } catch {
            Write-Host ("  请求 #${i} 失败: $_") -ForegroundColor Yellow
        }
    }
    Write-Host "  实际成功请求数: $sentOk / $RequestCount"

    # 拍照请求后每个凭据的 success_count
    Start-Sleep -Milliseconds 500   # 等 stats debounce 有机会更新
    $after = Invoke-Admin -Method "GET" -Path "/credentials"
    $diff = @{}
    $touched = 0
    foreach ($c in $after.credentials) {
        $beforeVal = if ($beforeMap.ContainsKey($c.id)) { $beforeMap[$c.id] } else { 0 }
        $delta = $c.successCount - $beforeVal
        $diff[$c.id] = $delta
        if ($delta -gt 0) { $touched++ }
    }
    Write-Host ("  请求后 success_count 变化: " + (($diff.GetEnumerator() | ForEach-Object { "id=$($_.Key):+$($_.Value)" }) -join ", "))

    if ($sentOk -gt 0) {
        Assert-True ($touched -ge 2) "至少 2 个凭据被轮到（轮询模式应该把请求分散到多个凭据）"
    } else {
        Write-Host "  所有请求都失败，跳过轮询断言（请检查凭据是否可用）" -ForegroundColor Yellow
    }
}

# ---------- 总结 ----------
Write-Host ""
Write-Host ("通过: $script:PassCount   失败: $script:FailCount") -ForegroundColor $(if ($script:FailCount -eq 0) { "Green" } else { "Red" })

if ($script:FailCount -gt 0) {
    exit 1
} else {
    exit 0
}
