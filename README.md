# 屿阅 / Yuyue

屿阅是一个隐私优先的桌面 Markdown 阅读器，面向“AI 生成了 Markdown，但接收者只想像普通文档一样查看”的场景。

Yuyue is a privacy-first desktop Markdown reader for the situation where AI generates Markdown but the recipient simply wants to read it like a normal document.

## 当前状态 / Current status

当前版本为 v1.0.0。源码、测试、依赖审计、unsigned macOS Release workflow、校验和与构建来源证明均已就绪；GitHub Releases 已提供可安装的 macOS 包。

The current version is v1.0.0. The source, tests, dependency audit, unsigned macOS release workflow, checksum, and build provenance are ready, and GitHub Releases provides an installable macOS package.

v1.0 提供：

v1.0 provides:

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

GitHub Releases workflow 会在推送 `v1.0.0` 这类版本标签时提供 unsigned macOS 安装产物、SHA-256 和构建来源证明。由于不使用 Apple Developer ID 签名或公证，macOS 可能显示 Gatekeeper 提示；Release 说明会提供明确的校验和与手动安装步骤，详见 [`docs/release/UNSIGNED_MACOS.md`](docs/release/UNSIGNED_MACOS.md)。

When a version tag such as `v1.0.0` is pushed, the GitHub Releases workflow publishes an unsigned macOS installer, SHA-256 checksums, and build provenance. Because the build is not signed or notarized with an Apple Developer ID, macOS may show a Gatekeeper warning; the release notes provide checksum and manual-install steps in [`docs/release/UNSIGNED_MACOS.md`](docs/release/UNSIGNED_MACOS.md).

当前公开版本：[`v1.0.0 Release`](https://github.com/dmzl/yuyue/releases/tag/v1.0.0)。

Current public release: [`v1.0.0 Release`](https://github.com/dmzl/yuyue/releases/tag/v1.0.0).

## 贡献 / Contributing

请先阅读 [`CONTRIBUTING.md`](CONTRIBUTING.md) 和 [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)。

Please read [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) first.

## License / 许可证

[MIT](LICENSE)
