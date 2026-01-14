# Frontend - Admin UI

React 前端管理界面，用于管理 Kiro API 代理服务。

## 技术栈

- Vite + React + TypeScript
- Tailwind CSS 4
- Shadcn UI (Nova 风格)
- react-toastify (Toast 通知)
- 字体: Inter (主字体) + JetBrains Mono (等宽字体)

## 目录结构

| 文件/目录 | 地位 | 功能 |
|----------|------|------|
| `src/main.tsx` | 入口 | 应用入口文件 |
| `src/App.tsx` | 核心 | 根组件和路由配置 |
| `src/pages/` | 页面 | 页面组件 (Dashboard, Login, Settings) |
| `src/api/index.ts` | API | API 服务层 |
| `src/assets/style/` | 样式 | CSS 样式文件 |
| `src/components/ui/` | 组件 | Shadcn UI 组件 |
| `src/hooks/` | Hook | 自定义 React Hooks |
| `src/lib/` | 工具 | 工具函数 (cn 等) |
| `components.json` | 配置 | Shadcn UI 配置 |
| `vite.config.ts` | 配置 | Vite 构建配置 |

## 功能特性

- 凭据管理 (CRUD)
- 多选和批量操作
- 批量导入/导出凭据 (JSON 格式)
- 使用此账号 (快速切换当前凭据)
- 深色/浅色主题切换
- 响应式设计

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
npx shadcn@latest add checkbox
# ... 更多组件
```
