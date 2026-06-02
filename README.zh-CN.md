<p align="center">
  <a href="README.md">English</a> | <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <img src="assets/logo-text.svg" alt="CC Session" width="240">
</p>

<p align="center">
  <b>一个桌面应用，浏览、搜索、恢复和管理你所有的 AI 编程会话。</b>
</p>

<p align="center">
  <a href="https://github.com/tyql688/cc-session/releases/latest"><img alt="Latest Release" src="https://img.shields.io/github/v/release/tyql688/cc-session?style=flat-square&color=blue"></a>
  <a href="https://github.com/tyql688/cc-session/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/tyql688/cc-session/ci.yml?branch=master&style=flat-square"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/tyql688/cc-session?style=flat-square"></a>
</p>

<p align="center">
  <a href="assets/show.png"><img src="assets/show.png" alt="CC Session — 会话浏览器" width="860"></a>
</p>

---

## 为什么需要 CC Session？

Claude Code、Codex、Antigravity、Kimi Code、Cursor CLI 等工具都把会话数据存在本地 —— 但各有各的格式、各自的目录，回头查找全无门路。**CC Session 把所有 provider 汇到一个原生快应用里：** 阅读完整对话历史、一次性跨工具搜索、导出干净的归档，并直接在终端里恢复任意会话。

> 💡 **所有本地编程会话，一个窗口搞定** —— 不用再翻 `~/.claude`、`~/.codex` 和一堆其它目录。

## ✨ 功能

- 🗂️ **统一视图** —— 所有支持的 provider 的会话，集中在一个资源管理器里
- 🔍 **全文搜索** —— 跨所有会话内容的即时搜索（SQLite FTS5），外加会话内查找
- ↩️ **一键恢复** —— 直接跳回终端继续任意会话
- 📊 **用量分析** —— 成本、Token、按模型拆分，区分缓存命中/写入
- 🎨 **丰富渲染** —— Markdown、语法高亮、Mermaid 图表、KaTeX 数学公式、内嵌图片、结构化工具调用 diff
- 👀 **实时监听** —— 文件型 provider 通过 OS watcher 自动刷新；OpenCode 使用 provider-aware 轮询
- 📤 **导出** —— JSON、Markdown 或独立 HTML（暗色模式、可折叠工具与思考块）
- 🗃️ **会话管理** —— 重命名、收藏、回收站/恢复、批量操作
- ⌨️ **键盘优先** —— 不碰鼠标也能导航和操作
- 🔄 **自动更新**、🌐 **中文 / English**、🚫 **屏蔽文件夹**（隐藏吵闹的项目目录）

## 📊 用量分析

清楚看到自己在每个 provider 上的实际花费 —— 每日成本趋势、按模型的 Token 总量、缓存效率，全在一个看板里。

<p align="center">
  <a href="assets/usage.png"><img src="assets/usage.png" alt="CC Session — 用量分析" width="860"></a>
</p>

## 🧩 支持的工具

| Provider | 源格式 | 实时监听 | 恢复命令 |
|----------|--------|:--------:|----------|
| **Claude Code** | JSONL | FS | `claude --resume` |
| **Codex CLI** | JSONL | FS | `codex resume` |
| **Antigravity** | JSONL | FS | `agy --conversation` |
| **Kimi Code** | JSONL | FS | `kimi --session` |
| **Cursor CLI** | JSONL + SQLite | FS | `cursor agent --resume` |
| **OpenCode** | SQLite | 轮询 | `opencode -s` |
| **CC-Mirror** | JSONL | FS | 按变体 |

跨工具统一解析：消息、工具调用（含输入/输出）、思考/推理块、Token 用量、内嵌图片，以及源格式支持时的 Markdown、Mermaid 图表和 KaTeX 数学公式 —— 含子代理/子会话。

## 📥 安装

从 [**Releases**](https://github.com/tyql688/cc-session/releases) 下载最新版本：

| 平台 | 文件 |
|------|------|
| macOS | `.dmg` |
| Windows | `.exe`（NSIS 安装包） |
| Linux | `.deb` / `.AppImage` |

> **macOS Gatekeeper：** 应用未经代码签名，首次打开时 macOS 可能会阻止运行，执行以下命令清除隔离标记：
>
> ```bash
> xattr -cr "/Applications/CC Session.app"
> ```

## 🚀 快速开始

1. 安装并启动 CC Session
2. 等待它索引本地的 provider 数据
3. 打开任意会话、搜索历史，或从中断处继续

## 🛠️ 从源码构建

需要 [Rust](https://rustup.rs/) 和 [Node.js](https://nodejs.org/) 20+。

```bash
git clone https://github.com/tyql688/cc-session.git
cd cc-session
npm install
npm run tauri build              # 生产构建
npx tauri build --bundles dmg    # 仅 DMG
```

## 💻 开发

```bash
npm run tauri dev                # 热重载开发
npm run check                    # 类型检查 + Biome + ESLint（前端）
npm test                         # 前端测试（Vitest）
cd src-tauri && cargo test       # Rust 测试
cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings
```

代码规范见 [`style/ts.md`](style/ts.md) 与 [`style/rust.md`](style/rust.md)，由 Biome、ESLint、Clippy 和 lefthook 预提交钩子强制执行。在 macOS 上，文件型实时监听使用 `notify` 的 `kqueue` 后端，以保证文件级更新更稳定。

## 🏗️ 技术栈

- [Tauri 2](https://v2.tauri.app/) —— 桌面壳与原生集成
- [SolidJS](https://www.solidjs.com/) —— 响应式前端 UI
- [Rust](https://www.rust-lang.org/) —— provider 解析、索引、导出与会话生命周期管理
- [SQLite](https://www.sqlite.org/) + FTS5 —— 本地存储与全文搜索
- [Vitest](https://vitest.dev/)、[Biome](https://biomejs.dev/)、[ESLint](https://eslint.org/)、[Clippy](https://doc.rust-lang.org/clippy/) —— 测试与代码质量保障

## 📄 许可证

[MIT](LICENSE) © tyql688
