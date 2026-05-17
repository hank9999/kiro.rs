#!/bin/bash

# 获取当前日期，格式为 yyyyMMdd
DATE=$(date +%Y%m%d)
ROOT_DIR=$(pwd)
PUBLIC_DIR="$ROOT_DIR/public"
ADMIN_UI_DIR="$ROOT_DIR/admin-ui"
# 根据操作系统判断二进制文件名
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
    RELEASE_BIN="$ROOT_DIR/target/release/kiro-rs.exe"
    DEST_BIN="$PUBLIC_DIR/kiro-rs_$DATE.exe"
else
    RELEASE_BIN="$ROOT_DIR/target/release/kiro-rs"
    DEST_BIN="$PUBLIC_DIR/kiro-rs_$DATE"
fi

echo -e "\033[0;36m--- 开始自动化发布流程 ---\033[0m"

# 1. 编译 admin-ui
echo -e "\033[0;33m[1/3] 正在编译 admin-ui...\033[0m"
cd "$ADMIN_UI_DIR" || exit
# pnpm install
pnpm build
if [ $? -ne 0 ]; then
    echo -e "\033[0;31madmin-ui 编译失败！\033[0m"
    exit 1
fi
cd "$ROOT_DIR" || exit

# 2. 编译 Rust 项目 (Release 模式)
echo -e "\033[0;33m[2/3] 正在编译 Rust 主程序 (Release)...\033[0m"
cargo build --release
if [ $? -ne 0 ]; then
    echo -e "\033[0;31mRust 项目编译失败！\033[0m"
    exit 1
fi

# 3. 发布到 public 目录
echo -e "\033[0;33m[3/3] 正在收集产物到 public 目录...\033[0m"

# 创建 public 目录
mkdir -p "$PUBLIC_DIR"

# 复制并重命名主程序
if [ -f "$RELEASE_BIN" ]; then
    cp "$RELEASE_BIN" "$DEST_BIN"
    echo -e "\033[0;32m已发布主程序: $DEST_BIN\033[0m"
else
    echo -e "\033[0;31m找不到编译后的程序: $RELEASE_BIN\033[0m"
fi

# 复制示例配置文件
echo -e "\033[0;33m正在复制示例配置文件...\033[0m"
for config in config.example.json credentials.example.idc.json credentials.example.multiple.json credentials.example.social.json; do
    if [ -f "$ROOT_DIR/$config" ]; then
        cp "$ROOT_DIR/$config" "$PUBLIC_DIR/$config"
    fi
done

# 复制 admin-ui 构建产物
UI_DIST="$ADMIN_UI_DIR/dist"
if [ -d "$UI_DIST" ]; then
    rm -rf "$PUBLIC_DIR/admin-ui"
    cp -r "$UI_DIST" "$PUBLIC_DIR/admin-ui"
    echo -e "\033[0;32m已发布 Admin UI 静态文件到: $PUBLIC_DIR/admin-ui\033[0m"
fi

echo -e "\033[0;36m--- 发布完成！ ---\033[0m"
