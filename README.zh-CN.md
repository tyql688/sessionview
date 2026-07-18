<p align="center">
  <a href="README.md">English</a> | <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <img src="assets/logo-text.svg" alt="SessionView" width="240">
</p>

<p align="center">
  <b>一个本地桌面工作台，用来阅读、搜索、分析和恢复 AI 编程会话。</b>
</p>

<p align="center">
  <a href="https://github.com/tyql688/sessionview/releases/latest"><img alt="Latest Release" src="https://img.shields.io/github/v/release/tyql688/sessionview?style=flat-square&color=blue"></a>
  <a href="https://github.com/tyql688/sessionview/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/tyql688/sessionview/ci.yml?branch=master&style=flat-square"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/tyql688/sessionview?style=flat-square"></a>
</p>

<p align="center">
  <a href="assets/show.png"><img src="assets/show.png" alt="SessionView — 会话浏览器" width="860"></a>
</p>

---

## 为什么需要 SessionView？

AI 编程工具会在你的机器上留下很多有价值的上下文：决策过程、补丁、工具调用、成本、图片和没做完的线程。问题是，每个工具保存历史的方式都不一样。

**SessionView 把这些本地历史整理成一个快速的桌面工作台。** 你可以在同一个资源管理器里浏览所有支持的工具，阅读完整会话，跨历史搜索，查看用量，导出归档，并在需要继续时回到终端恢复会话。

数据保留在本地。SessionView 只为搜索和分析建立本地索引，不上传你的对话。

## 可以做什么

- **把多工具历史放进一个工作台**：Claude Code、Codex CLI、Antigravity、Kimi Code、Cursor CLI、OpenCode、CC-Mirror、Pi 和 Grok Build。
- **像读文档一样阅读会话**：Markdown、代码块、Mermaid、KaTeX、内嵌图片、推理块和结构化工具调用输出。
- **不用翻目录也能搜索**：全局全文搜索，加上当前会话内查找。
- **看懂一次工作的形状**：Token 时间线、工具调用分布、上下文/缓存压力、成本趋势和模型拆分。
- **快速恢复现场**：源工具支持时，可以直接在对应终端代理中继续会话。
- **整理历史**：重命名、收藏、导出和屏蔽吵闹目录。
- **键盘友好**：标签页、分栏、搜索和常用动作都能用键盘完成。

## 会话分析

在侧边面板查看单个会话的 Token 时间线、工具调用分布、缓存/上下文压力和工作流信号，不用离开当前对话。

<p align="center">
  <a href="assets/session-analytics.png"><img src="assets/session-analytics.png" alt="SessionView 会话分析侧边面板" width="860"></a>
</p>

## 用量分析

在一个看板里查看每日花费、按模型拆分的 Token 总量、缓存读写和不同工具的趋势。

<p align="center">
  <a href="assets/usage.png"><img src="assets/usage.png" alt="SessionView 用量分析" width="860"></a>
</p>

## 支持的工具

SessionView 当前读取 Claude Code、Codex CLI、Antigravity、Kimi Code、Cursor CLI、OpenCode、CC-Mirror、Pi 和 Grok Build 的本地历史。

当源工具提供足够信息时，SessionView 也可以在对应终端代理中恢复所选会话；CC-Mirror 会跟随配置的变体。解析深度取决于各工具本地实际记录了什么；只要源数据提供，SessionView 会统一消息、工具调用、思考/推理块、Token 用量、图片和子会话。

## 安装

从 [**Releases**](https://github.com/tyql688/sessionview/releases) 下载最新构建：

| 平台 | 文件 |
| ------ | ------ |
| macOS | `.dmg` |
| Windows | `.exe`（NSIS 安装包） |
| Linux | `.deb` / `.AppImage` |

> **macOS Gatekeeper：** 根据发布构建是否拿到签名证书，macOS 可能会在首次启动时拦截。如果遇到这种情况，可以清除隔离标记：
>
> ```bash
> xattr -cr "/Applications/SessionView.app"
> ```

### Headless（浏览器模式）

更喜欢浏览器，或想在远程/开发机上使用？

```bash
npx sessionview          # 在 http://127.0.0.1:9921 提供完整应用
npx sessionview --open   # …并自动打开浏览器
```

Headless 服务与桌面应用共用同一个 Rust 核心、同一套 UI 和同一个索引
（`~/.sessionview`）——任意一端索引过的会话另一端立即可见。默认只绑定
localhost；如需对外暴露，请加 `--host 0.0.0.0 --token <secret>`（此后每个
API 请求都必须携带该 token）。

## 快速开始

1. 安装并启动 SessionView
2. 等待它索引本地工具历史
3. 打开任意会话、搜索历史，或从中断处继续

## 从源码构建

需要 [Rust](https://rustup.rs/) 和 [Node.js](https://nodejs.org/) 22.12+。

```bash
git clone https://github.com/tyql688/sessionview.git
cd sessionview
npm install
npm run tauri build              # 生产构建
npx tauri build --bundles dmg    # 仅 DMG
```

## 开发

```bash
npm run tauri dev                # 热重载开发
npm run check                    # 类型检查 + Biome + ESLint（前端）
npm test                         # 前端测试（Vitest）
cd src-tauri && cargo fmt --check
cd src-tauri && cargo test       # Rust 测试
cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings
npm run knip                     # 发布前死代码/依赖审计
```

代码规范见 [`style/ts.md`](style/ts.md) 与 [`style/rust.md`](style/rust.md)，由 Biome、ESLint、Clippy、Knip 和 lefthook 强制执行。Knip 作为发布门禁运行，不放进每次 push 的钩子。

## 技术栈

- [Tauri 2](https://v2.tauri.app/) —— 桌面壳与原生集成
- [React 19](https://react.dev/) —— 前端 UI 与 React Compiler
- [Rust](https://www.rust-lang.org/) —— provider 解析、索引、导出与会话生命周期管理
- [SQLite](https://www.sqlite.org/) + FTS5 —— 本地存储与全文搜索
- [Vitest](https://vitest.dev/)、[Biome](https://biomejs.dev/)、[ESLint](https://eslint.org/)、[Clippy](https://doc.rust-lang.org/clippy/) —— 测试与代码质量保障

## 许可证

[MIT](LICENSE) © tyql688
