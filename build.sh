#!/bin/bash
# Kiro.rs 构建脚本
# 先编译前端（Vite），再编译后端（Rust）

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FRONTEND_DIR="$SCRIPT_DIR/frontend"
BACKEND_DIR="$SCRIPT_DIR/backend"

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

echo_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

echo_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# 清理 macOS 生成的 ._ 文件和残留的 .! 文件
cleanup_temp_files() {
    echo_info "清理临时文件..."
    find "$SCRIPT_DIR" -name "._*" -delete 2>/dev/null || true
    find "$SCRIPT_DIR" -name "*.!" -delete 2>/dev/null || true
}

# 编译前端
build_frontend() {
    echo_info "=========================================="
    echo_info "开始编译前端 (Vite + React)"
    echo_info "=========================================="

    cd "$FRONTEND_DIR"

    # 检查 node_modules 是否存在
    if [ ! -d "node_modules" ]; then
        echo_info "安装前端依赖..."
        if command -v pnpm &> /dev/null; then
            pnpm install
        elif command -v npm &> /dev/null; then
            npm install
        else
            echo_error "未找到 pnpm 或 npm，请先安装 Node.js"
            exit 1
        fi
    fi

    # 编译前端
    echo_info "执行 vite build..."
    if command -v pnpm &> /dev/null; then
        pnpm build
    else
        npm run build
    fi

    echo_info "前端编译完成，输出目录: $FRONTEND_DIR/dist"
}

# 编译后端
build_backend() {
    echo_info "=========================================="
    echo_info "开始编译后端 (Rust)"
    echo_info "=========================================="

    cd "$BACKEND_DIR"

    # 检查 Rust 工具链
    if ! command -v cargo &> /dev/null; then
        echo_error "未找到 cargo，请先安装 Rust 工具链"
        exit 1
    fi

    # 编译后端
    echo_info "执行 cargo build --release..."
    cargo build --release

    echo_info "后端编译完成，输出文件: $BACKEND_DIR/target/release/kiro-rs"
}

# 主流程
main() {
    echo_info "Kiro.rs 构建脚本"
    echo_info "项目目录: $SCRIPT_DIR"
    echo ""

    # 清理临时文件
    cleanup_temp_files

    # 编译前端
    build_frontend
    echo ""

    # 编译后端
    build_backend
    echo ""

    echo_info "=========================================="
    echo_info "构建完成!"
    echo_info "=========================================="
    echo_info "前端产物: $FRONTEND_DIR/dist"
    echo_info "后端产物: $BACKEND_DIR/target/release/kiro-rs"
}

# 支持参数
case "${1:-all}" in
    frontend)
        cleanup_temp_files
        build_frontend
        ;;
    backend)
        cleanup_temp_files
        build_backend
        ;;
    clean)
        cleanup_temp_files
        echo_info "清理前端构建产物..."
        rm -rf "$FRONTEND_DIR/dist"
        echo_info "清理后端构建产物..."
        cd "$BACKEND_DIR" && cargo clean
        echo_info "清理完成"
        ;;
    all|*)
        main
        ;;
esac
