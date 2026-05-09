<p align="center">
  <a href="README.md">English</a> | <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <img src="assets/logo-text.svg" alt="CC Session" width="240">
</p>

<p align="center">
  浏览、搜索、恢复和管理你的 AI 编程会话，一个桌面应用搞定。
</p>

<p align="center">
  <a href="https://github.com/tyql688/cc-session/releases/latest"><img alt="Latest Release" src="https://img.shields.io/github/v/release/tyql688/cc-session?style=flat-square&color=blue"></a>
  <a href="https://github.com/tyql688/cc-session/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/tyql688/cc-session/ci.yml?branch=master&style=flat-square"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/tyql688/cc-session?style=flat-square"></a>
</p>

---

## 为什么需要 CC Session？

Claude Code、Codex、Gemini CLI、Qwen Code 等 AI 编程工具会在本地存储会话数据，但没有方便的方式来浏览、搜索和回顾历史对话。CC Session 将所有工具的会话集中到一个统一界面 — 查看完整对话历史、跨工具全文搜索、导出记录、或直接在终端中恢复任意会话。

> **把所有本地 AI 编程会话集中到一个应用里**
>
> 浏览历史、跨工具搜索、恢复误删会话、导出归档，并且一键继续当前会话。

## 功能

- **统一视图** — 多种 AI 编程工具的所有会话集中展示
- **全文搜索** — SQLite FTS5，跨所有会话内容搜索
- **恢复会话** — 直接从当前会话跳回终端继续
- **实时监听** — 文件型 provider 通过 OS watcher 自动刷新；Gemini 和 OpenCode 使用 provider-aware 轮询
- **丰富渲染** — Markdown、语法高亮、Mermaid 图表、KaTeX 数学公式、内嵌图片、结构化工具调用 diff
- **Token 统计** — 单条消息和会话级别 Token 计数，区分缓存命中/写入
- **导出** — JSON、Markdown 或独立 HTML（暗色模式、可折叠工具和思考块）
- **会话管理** — 重命名、回收站/恢复、收藏、批量操作
- **自动更新** — 内置更新器自动检查新版本
- **键盘友好** — 常用导航和操作都可以快速用键盘完成
- **双语** — 英文 / 中文
- **屏蔽文件夹** — 隐藏特定项目目录的会话

## 支持的工具

当前支持：

- Claude Code
- Codex CLI
- Gemini CLI
- Kimi CLI
- OpenCode
- Qwen Code
- CC-Mirror

跨工具统一解析：消息、工具调用（含输入/输出）、思考/推理块、Token 用量、内嵌图片，以及源格式支持时的 Markdown、Mermaid 图表和 KaTeX 数学公式。

## 安装

从 [Releases](https://github.com/tyql688/cc-session/releases) 下载最新版本：

- **macOS** — `.dmg`
- **Windows** — `.exe`（NSIS 安装包）
- **Linux** — `.deb` / `.AppImage`

> **macOS Gatekeeper：** 应用未经代码签名。首次打开时 macOS 可能会阻止运行，执行以下命令修复：
>
> ```bash
> xattr -cr "/Applications/CC Session.app"
> ```

## 快速开始

1. 安装并启动 CC Session
2. 等待它索引本地支持的 provider 数据
3. 打开任意会话，搜索历史记录，或从中断处继续

## 从源码构建

需要 [Rust](https://rustup.rs/) 和 [Node.js](https://nodejs.org/) 18+。

```bash
git clone https://github.com/tyql688/cc-session.git
cd cc-session
npm install
npm run tauri build              # 生产构建
npx tauri build --bundles dmg    # 仅 DMG
```

## 开发

```bash
npm run tauri dev                # 热重载开发
npm test                         # 前端测试
npx tsc --noEmit                 # 前端类型检查
cd src-tauri && cargo test       # Rust 测试
cd src-tauri && cargo clippy     # Rust 检查
```

在 macOS 上，文件型实时监听使用 `notify` 的 `kqueue` 后端，以保证文件级更新更稳定。

## 技术栈

- [Tauri 2](https://v2.tauri.app/)：桌面壳与原生集成
- [SolidJS](https://www.solidjs.com/)：前端 UI
- [Rust](https://www.rust-lang.org/)：provider 解析、索引、导出与会话生命周期管理
- [SQLite](https://www.sqlite.org/) + FTS5：本地存储与全文搜索
- [Vitest](https://vitest.dev/)、[ESLint](https://eslint.org/)、[Prettier](https://prettier.io/)、[Clippy](https://doc.rust-lang.org/clippy/)：测试与代码质量保障

## 许可证

[MIT](LICENSE)
