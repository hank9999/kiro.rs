# 获取当前日期，格式为 yyyyMMdd
$Date = Get-Date -Format "yyyyMMdd"
$RootDir = $PSScriptRoot
$PublicDir = Join-Path $RootDir "public"
$AdminUiDir = Join-Path $RootDir "admin-ui"
$ReleaseBin = Join-Path $RootDir "target\release\kiro-rs.exe"
$DestBin = Join-Path $PublicDir "kiro-rs_$Date.exe"

Write-Host "--- 开始自动化发布流程 ---" -ForegroundColor Cyan

# 1. 编译 admin-ui
Write-Host "[1/3] 正在编译 admin-ui..." -ForegroundColor Yellow
Push-Location $AdminUiDir
try {
    # 确保依赖已安装 (可选，如果确定环境已有可跳过，为了全自动建议保留)
    # pnpm install
    pnpm build
    if ($LASTEXITCODE -ne 0) {
        Write-Error "admin-ui 编译失败！"
        exit $LASTEXITCODE
    }
} finally {
    Pop-Location
}

# 2. 编译 Rust 项目 (Release 模式)
# 因为 Rust 项目嵌入了 admin-ui/dist，所以必须在 admin-ui 之后编译
Write-Host "[2/3] 正在编译 Rust 主程序 (Release)..." -ForegroundColor Yellow
cargo build --release
if ($LASTEXITCODE -ne 0) {
    Write-Error "Rust 项目编译失败！"
    exit $LASTEXITCODE
}

# 3. 发布到 public 目录
Write-Host "[3/3] 正在收集产物到 public 目录..." -ForegroundColor Yellow

# 创建 public 目录
if (-not (Test-Path $PublicDir)) {
    New-Item -ItemType Directory -Path $PublicDir | Out-Null
}

# 复制并重命名主程序
if (Test-Path $ReleaseBin) {
    Copy-Item $ReleaseBin $DestBin -Force
    Write-Host "已发布主程序: $DestBin" -ForegroundColor Green
} else {
    Write-Error "找不到编译后的程序: $ReleaseBin"
}

# 复制示例配置文件
Write-Host "正在复制示例配置文件..." -ForegroundColor Yellow
$Configs = @("config.example.json", "credentials.example.idc.json", "credentials.example.multiple.json", "credentials.example.social.json")
foreach ($cfg in $Configs) {
    $src = Join-Path $RootDir $cfg
    if (Test-Path $src) {
        Copy-Item $src (Join-Path $PublicDir $cfg) -Force
    }
}

# 复制 admin-ui 构建产物 (作为备份或独立发布)
$UiDist = Join-Path $AdminUiDir "dist"
if (Test-Path $UiDist) {
    $DestUi = Join-Path $PublicDir "admin-ui"
    if (Test-Path $DestUi) { Remove-Item $DestUi -Recurse -Force }
    Copy-Item $UiDist $DestUi -Recurse -Force
    Write-Host "已发布 Admin UI 静态文件到: $DestUi" -ForegroundColor Green
}

# 4. 部署到上一级目录并重启服务
Write-Host "[4/4] 正在部署到上一级目录并重启服务..." -ForegroundColor Yellow

$ParentDir = Split-Path $RootDir -Parent
$ParentExe = Join-Path $ParentDir "kiro-rs.exe"
$ParentExeBackup = Join-Path $ParentDir "kiro-rs_$Date.exe"
$ServiceName = "kiro-2api"

# 检查管理员权限
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "警告: 未以管理员身份运行，将尝试使用 sc.exe 操作服务..." -ForegroundColor DarkYellow
}

# 停止服务（以便替换文件）
Write-Host "正在停止服务 $ServiceName..." -ForegroundColor Yellow
$stopResult = sc.exe stop $ServiceName 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Host "服务已停止，等待进程退出..." -ForegroundColor Green
    Start-Sleep -Seconds 3
} elseif ($stopResult -match "1062") {
    Write-Host "服务 $ServiceName 当前未运行" -ForegroundColor DarkYellow
} elseif ($stopResult -match "1060") {
    Write-Host "服务 $ServiceName 不存在，跳过服务操作" -ForegroundColor DarkYellow
} else {
    Write-Host "停止服务返回: $stopResult" -ForegroundColor DarkYellow
    Start-Sleep -Seconds 2
}

# 备份旧版本
if (Test-Path $ParentExe) {
    if (Test-Path $ParentExeBackup) {
        Remove-Item $ParentExeBackup -Force
    }
    Rename-Item $ParentExe $ParentExeBackup -Force
    Write-Host "已备份旧版本: $ParentExeBackup" -ForegroundColor Green
}

# 复制新版本到上一级目录
if (Test-Path $DestBin) {
    Copy-Item $DestBin $ParentExe -Force
    Write-Host "已部署新版本: $ParentExe" -ForegroundColor Green
} else {
    Write-Error "找不到新版本文件: $DestBin"
    exit 1
}

# 重启服务
Write-Host "正在启动服务 $ServiceName..." -ForegroundColor Yellow
$startResult = sc.exe start $ServiceName 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Host "服务 $ServiceName 已成功启动" -ForegroundColor Green
} elseif ($startResult -match "1060") {
    Write-Host "服务 $ServiceName 不存在，请手动启动程序" -ForegroundColor DarkYellow
} else {
    Write-Error "启动服务 $ServiceName 失败: $startResult"
    Write-Host "请尝试以管理员身份运行此脚本，或手动启动服务" -ForegroundColor Yellow
}

Write-Host "--- 发布完成！ ---" -ForegroundColor Cyan
