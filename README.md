# 屿阅

屿阅是一个隐私优先的桌面 Markdown 阅读器，面向“AI 生成了 Markdown，但接收者只想像普通文档一样查看”的场景。

## 当前状态

项目已完成 v0.1 开源前整改和本地跨层验证，当前提供可安装的 unsigned macOS Release Candidate；尚未发布正式 GitHub Release。源码、测试、依赖审计和 unsigned macOS Release workflow 已就绪，正式 Release 仍需由维护者在具备 GitHub 权限的仓库中打 tag 后生成。

v0.1 完成后将优先提供：

- macOS unsigned 可安装产物，用户无需自行构建。
- GFM、脚注、GitHub Alerts、KaTeX、代码高亮和受限 Mermaid。
- 长文档搜索、H1-H4 目录、稳定锚点、多标签和独立文件刷新。
- 本地图片安全加载，HTTPS 远程图片默认阻断并按当前标签临时授权。
- MIT License、自动测试、依赖审计、校验和和可追溯 GitHub Release。

## 为什么做屿阅

AI 生成的方案、报告、教程和技术文档经常以 Markdown 交付。对熟悉开发工具的人这很方便，但对普通接收者来说，直接打开 `.md` 往往只是带符号的纯文本。屿阅的目标是把“把文档发给别人”变成“对方下载后直接阅读”。

## 隐私与安全默认值

- 文档在本机读取，不上传到云端。
- 原始 HTML 不执行。
- HTTPS 远程图片默认不请求；授权只对当前打开标签有效。
- HTTP、`file:`、绝对路径和目录外图片不加载。
- 外部链接只在用户点击后交给系统浏览器。
- 文档、图片、SVG 和 Mermaid 都有资源上限。

漏洞报告方式见 [`SECURITY.md`](SECURITY.md)。

## 从源码运行

### 前置环境

- Node.js 与 npm
- Rust toolchain
- Tauri 2 的 macOS 系统依赖

```bash
npm ci
npm run tauri dev
```

本地质量检查：

```bash
npm run test:ci
npm run lint
npm run audit
cd src-tauri && cargo fmt --check && cargo test --locked && cargo clippy --all-targets -- --deny warnings
```

构建前端和 Mermaid 隔离 renderer：

```bash
npm run build
```

构建本地 unsigned macOS Release Candidate：

```bash
npm run tauri -- build --bundles app --ci --no-sign
bash scripts/build-unsigned-dmg.sh
```

## 安装包

完成 v0.1 验收后，GitHub Releases workflow 将在维护者推送版本 tag 时提供 unsigned macOS 安装产物、SHA-256 和构建来源。由于不使用 Apple Developer ID 签名或公证，macOS 可能显示 Gatekeeper 提示；Release 说明会提供明确的校验和手动安装步骤，详见 [`docs/release/UNSIGNED_MACOS.md`](docs/release/UNSIGNED_MACOS.md)。

在首个 Release 出现之前，不存在官方预构建下载。

## 贡献

请先阅读 [`CONTRIBUTING.md`](CONTRIBUTING.md) 和 [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)。

## License

[MIT](LICENSE)
