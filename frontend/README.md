# Frontend - Admin UI

React 前端管理界面，用于管理 Kiro API 代理服务。

## 技术栈

- Vite + React + TypeScript
- Tailwind CSS 4
- Shadcn UI (Nova 风格)
- react-toastify (Toast 通知)

## 目录结构

| 文件/目录 | 地位 | 功能 |
|----------|------|------|
| `src/main.tsx` | 入口 | 应用入口文件 |
| `src/App.tsx` | 核心 | 根组件 |
| `src/assets/style/` | 样式 | CSS 样式文件 |
| `src/lib/` | 工具 | 工具函数 (cn 等) |
| `components.json` | 配置 | Shadcn UI 配置 |
| `vite.config.ts` | 配置 | Vite 构建配置 |

## 快速开始

```bash
# 安装依赖
npm install

# 开发模式
npm run dev

# 构建生产版本
npm run build
```

## 添加 Shadcn 组件

```bash
npx shadcn@latest add button
npx shadcn@latest add card
# ... 更多组件
```
