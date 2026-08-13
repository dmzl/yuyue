# 屿阅 / Yuyue

屿阅是一个隐私优先的桌面 Markdown 阅读器，面向“AI 生成了 Markdown，但接收者只想像普通文档一样查看”的场景。

Yuyue is a privacy-first desktop Markdown reader for the situation where AI generates Markdown but the recipient simply wants to read it like a normal document.

## 当前状态 / Current status

当前公开版本为 v1.1.0。源码、测试、依赖审计、unsigned macOS Release workflow、校验和与构建来源证明均已就绪；GitHub Releases 提供 unsigned macOS 安装包。

The current public version is v1.1.0. The source, tests, dependency audit, unsigned macOS release workflow, checksums, and build provenance are ready; GitHub Releases provides the unsigned macOS installer.

v1.0 阅读基线提供：

v1.0 reading baseline provides:

- macOS unsigned 可安装产物，用户无需自行构建。

  An unsigned macOS build that users can install without building it themselves.
- GFM、脚注、GitHub Alerts、KaTeX、代码高亮和受限 Mermaid。

  GFM, footnotes, GitHub Alerts, KaTeX, syntax highlighting, and bounded Mermaid rendering.
- 长文档搜索、H1-H4 目录、稳定锚点、多标签和独立文件刷新；标签会先缩窄至可读宽度，再收进“更多”菜单。

  Full-text search, an H1-H4 outline, stable anchors, tabs, and independent file refresh; tabs narrow to a readable minimum before overflow moves them into the “More” menu.
- 空白状态也可直接使用右上角的打开、主题和设置入口；设置会沿用到随后打开的文档。

  Open, theme, and settings controls remain available in the upper-right empty state, and settings carry over to documents opened later.
- 本地图片安全加载，HTTPS 远程图片默认阻断并按当前标签临时授权。

  Safe local-image loading; HTTPS remote images are blocked by default and can be authorized temporarily for the current tab.
- MIT License、自动测试、依赖审计、校验和、构建来源证明和可追溯 GitHub Release。

  The MIT License, automated tests, dependency auditing, checksums, build provenance, and traceable GitHub Releases.

v1.1 增加本地 Markdown 编辑：

v1.1 adds local Markdown editing:

- 从阅读位置进入源码编辑，并在同源预览中即时查看结果；支持常用 Markdown 格式命令、查找、撤销和重做。

  Enter source editing from the reading position and preview the same document in-app; common Markdown commands, find, undo, and redo are supported.
- 编辑内容在短暂停顿后默认自动写回原文件；支持 Cmd+S、Save As、外部冲突保护、失败恢复和离开前保存确认。

  Edits are automatically written back to the original file after a short pause; Cmd+S, Save As, external-conflict protection, failure recovery, and close/exit save confirmation are supported.
- 源码与预览支持双向定位和滚动同步，本地图片可受控插入；编辑器按需加载，预览解析和写回保持异步并受资源上限约束。

  Source and preview support bidirectional navigation and scroll sync, with controlled local-image insertion; the editor loads on demand, while preview rendering and writeback stay asynchronous and bounded.
- 编辑界面支持简体中文、繁体中文、英文和日文，并适配浅色、深色主题。

  The editing interface supports Simplified Chinese, Traditional Chinese, English, and Japanese, with light and dark themes.

## 为什么做屿阅 / Why Yuyue

AI 生成的方案、报告、教程和技术文档经常以 Markdown 交付。对熟悉开发工具的人这很方便，但对普通接收者来说，直接打开 `.md` 往往只是带符号的纯文本。屿阅的目标是把“把文档发给别人”变成“对方下载后直接阅读”。

AI-generated plans, reports, tutorials, and technical documents are often delivered as Markdown. That is convenient for people familiar with developer tools, but opening an `.md` file directly can look like symbol-filled plain text to everyone else. Yuyue turns “send someone a document” into “let them download it and start reading.”

## 隐私与安全默认值 / Privacy and security defaults

- 文档在本机读取，不上传到云端。

  Documents are read locally and are not uploaded to the cloud.
- 原始 HTML 不执行。

  Raw HTML is rendered as text and is not executed.
- HTTPS 远程图片默认不请求；授权只对当前打开标签有效。

  HTTPS remote images are not requested by default; authorization only applies to the current open tab.
- HTTP、`file:`、绝对路径和目录外图片不加载。

  HTTP, `file:`, absolute-path, and out-of-directory images are not loaded.
- 外部链接只在用户点击后交给系统浏览器。

  External links are handed to the system browser only after an explicit user click.
- 文档、图片、SVG 和 Mermaid 都有资源上限。

  Documents, images, SVG, and Mermaid inputs are subject to resource limits.

漏洞报告方式见 [`SECURITY.md`](SECURITY.md)。

See [`SECURITY.md`](SECURITY.md) for vulnerability-reporting instructions.

## 从源码运行 / Run from source

### 前置环境 / Prerequisites

- Node.js 与 npm / Node.js and npm
- Rust toolchain / Rust toolchain
- Tauri 2 的 macOS 系统依赖 / macOS system dependencies for Tauri 2

```bash
npm ci
npm run tauri dev
```

本地质量检查：

Local quality checks:

```bash
npm run test:ci
npm run lint
npm run audit
cd src-tauri && cargo fmt --check && cargo test --locked && cargo clippy --all-targets -- --deny warnings
```

构建前端和 Mermaid 隔离 renderer：

Build the frontend and isolated Mermaid renderer:

```bash
npm run build
```

构建本地 unsigned macOS Release Candidate：

Build a local unsigned macOS release candidate:

```bash
npm run tauri -- build --bundles app --ci --no-sign
bash scripts/build-unsigned-dmg.sh
```

## 安装包 / Installers

GitHub Releases workflow 会在推送 `v1.1.0` 这类版本标签时提供 unsigned macOS 安装产物、SHA-256 和构建来源证明。由于不使用 Apple Developer ID 签名或公证，macOS 可能显示 Gatekeeper 提示；Release 说明会提供明确的校验和与手动安装步骤，详见 [`docs/release/UNSIGNED_MACOS.md`](docs/release/UNSIGNED_MACOS.md)。

When a version tag such as `v1.1.0` is pushed, the GitHub Releases workflow publishes an unsigned macOS installer, SHA-256 checksums, and build provenance. Because the build is not signed or notarized with an Apple Developer ID, macOS may show a Gatekeeper warning; the release notes provide checksum and manual-install steps in [`docs/release/UNSIGNED_MACOS.md`](docs/release/UNSIGNED_MACOS.md).

当前公开版本：[`v1.1.0 Release`](https://github.com/dmzl/yuyue/releases/tag/v1.1.0)。

Current public release: [`v1.1.0 Release`](https://github.com/dmzl/yuyue/releases/tag/v1.1.0).

v1.1.0 另生成了一个仅供测试的 Windows x64 NSIS 安装包。该包通过 Tauri 官方实验性交叉编译路径生成，未签名，尚未在 Windows 真机运行验收；Windows 编辑写回不属于当前支持范围，测试包也不会上传到 GitHub Release。

Version 1.1.0 also has a Windows x64 NSIS installer for testing only. It was produced through Tauri's experimental cross-compilation path, is unsigned, and has not been runtime-verified on Windows; Windows editing writeback is outside the current support scope, and the test installer is not uploaded to GitHub Releases.

## 贡献 / Contributing

请先阅读 [`CONTRIBUTING.md`](CONTRIBUTING.md) 和 [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)。

Please read [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) first.

## License / 许可证

[MIT](LICENSE)
