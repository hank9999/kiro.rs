#!/usr/bin/env pwsh
# Kiro.rs 并行调试启动脚本
# - 新窗口 1：cargo run (后端 http://localhost:8990)
# - 新窗口 2：npm run dev (前端 http://localhost:5173/admin/)
# - 主窗口：监控两个进程，任一退出时清理另一个

param(
    [switch]$Release,           # 使用 release 编译
    [switch]$SkipBuild,         # 跳过首次 dist 构建
    [switch]$Debug              # 开启代理池请求级 debug 日志
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$adminUi = Join-Path $root "admin-ui"

Write-Host "[kiro.rs] 根目录:   $root" -ForegroundColor Cyan
Write-Host "[kiro.rs] 前端目录: $adminUi" -ForegroundColor Cyan

# -------- 前置检查 --------
if (-not (Test-Path (Join-Path $adminUi "node_modules"))) {
    Write-Host "[kiro.rs] 前端依赖缺失，执行 npm install ..." -ForegroundColor Yellow
    Push-Location $adminUi
    try { npm install } finally { Pop-Location }
}

if (-not $SkipBuild -and -not (Test-Path (Join-Path $adminUi "dist\assets"))) {
    Write-Host "[kiro.rs] dist 缺失，执行一次 npm run build ..." -ForegroundColor Yellow
    Push-Location $adminUi
    try { npm run build } finally { Pop-Location }
}

# -------- 构造启动命令 --------
$cargoCmd = if ($Release) { "cargo run --release" } else { "cargo run" }
$rustLog = if ($Debug) {
    "`$env:RUST_LOG='kiro_rs::kiro::provider=debug,kiro_rs::kiro::token_manager=debug,info'; "
} else {
    "`$env:RUST_LOG='info'; "
}

$backendArgs = @(
    "-NoExit",
    "-Command",
    "Set-Location '$root'; $rustLog Write-Host '--- 后端 (cargo) ---' -ForegroundColor Green; $cargoCmd"
)

$frontendArgs = @(
    "-NoExit",
    "-Command",
    "Set-Location '$adminUi'; Write-Host '--- 前端 (vite) ---' -ForegroundColor Green; npm run dev"
)

# -------- 启动前后端 --------
Write-Host "[kiro.rs] 启动后端 ($cargoCmd) ..." -ForegroundColor Green
$backend = Start-Process -PassThru powershell -ArgumentList $backendArgs

Start-Sleep -Seconds 1

Write-Host "[kiro.rs] 启动前端 (npm run dev) ..." -ForegroundColor Green
$frontend = Start-Process -PassThru powershell -ArgumentList $frontendArgs

Write-Host ""
Write-Host "[kiro.rs] ============================================" -ForegroundColor Cyan
Write-Host "[kiro.rs] 后端 PID: $($backend.Id)  http://localhost:8990"
Write-Host "[kiro.rs] 前端 PID: $($frontend.Id)  http://localhost:5173/admin/"
Write-Host "[kiro.rs] 本窗口按 Ctrl+C 可一键停止全部进程" -ForegroundColor Cyan
Write-Host "[kiro.rs] ============================================" -ForegroundColor Cyan

function Stop-Tree([int]$pid_) {
    try {
        if ($pid_ -and -not (Get-Process -Id $pid_ -ErrorAction SilentlyContinue).HasExited) {
            taskkill /F /T /PID $pid_ 2>&1 | Out-Null
        }
    } catch {}
}

try {
    while ($true) {
        $backend.Refresh()
        $frontend.Refresh()
        if ($backend.HasExited) {
            Write-Host "[kiro.rs] 后端已退出 (ExitCode=$($backend.ExitCode))" -ForegroundColor Red
            break
        }
        if ($frontend.HasExited) {
            Write-Host "[kiro.rs] 前端已退出 (ExitCode=$($frontend.ExitCode))" -ForegroundColor Red
            break
        }
        Start-Sleep -Seconds 1
    }
} finally {
    Write-Host "[kiro.rs] 清理子进程树 ..." -ForegroundColor Yellow
    Stop-Tree $backend.Id
    Stop-Tree $frontend.Id
    Write-Host "[kiro.rs] 已停止" -ForegroundColor Yellow
}
