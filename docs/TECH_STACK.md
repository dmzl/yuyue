# 屿阅技术栈

状态：current  
Revision：TECH-STACK-2026-08-10

## 当前真实实现

- Desktop：Tauri 2.11.x、Rust 2021；macOS 打印使用系统 `WKWebView` / AppKit 绑定，仅配置页边距和打印任务标题，仍由系统对话框保存 PDF。
- UI：Vue 3、TypeScript、Vite 6。
- Markdown 解析：`unified`、`remark-parse`、`remark-gfm`、`remark-math`、`remark-rehype`、`rehype-sanitize`、`rehype-katex`、`rehype-highlight`、`rehype-stringify`。
- 渲染执行：受监督的 Web Worker；请求带 `documentKey` 和 `requestId`，超时/崩溃会重启并拒绝过期结果。
- Mermaid：自包含 IIFE bundle、完整性哈希和无同源权限的 `sandbox="allow-scripts"` iframe；输出回到父页面前再次净化。
- 文件监听：Rust `notify`，每个打开文档独立 watcher，事件带 `documentId`，前端按文档防抖重载。
- 图片验证与渲染：Rust `image`、`usvg`、`resvg`；本地和 data 图片由文档上下文授权，SVG 通过 SafeSvg 后统一快照为 PNG。
- 外链：仅允许 `http/https`，通过 Rust `open_external_url` 交给 macOS 系统浏览器。
- 包管理：npm 与 Cargo lockfile。
- Release 封装：先构建 unsigned `.app`，再由 `scripts/build-unsigned-dmg.sh` 使用 macOS `hdiutil` 生成 HFS+/UDZO DMG，跳过 Finder 自动化以兼容无 GUI CI。

本地 unsigned Release Candidate 已完成真实 macOS 运行、发布构建和交付审查；真实 GitHub tag、attestation 和公开下载仍需维护者权限。

## 已批准目标

- Markdown 管线：`unified`、`remark-parse`、`remark-gfm`、`remark-math`、`remark-rehype`、`rehype-sanitize`、`rehype-katex`、`rehype-stringify`。
- 产品专有能力使用仓库内 AST transform，不依赖随机第三方便利插件。
- Markdown 解析与可信 HTML 生成运行于受监督的 Web Worker。
- Mermaid 使用自包含、哈希脚本、无同源权限的受限 renderer iframe。
- 外链仅通过最小 Rust command 打开；本地资源继续由 Rust 文档上下文控制。

目标架构详见 [`ARCHITECTURE.md`](./ARCHITECTURE.md)。

## 当前可用命令

```bash
npm ci
npm run test:ci
npm run lint
npm run audit
npm run build
npm run tauri dev
npm run tauri -- build --bundles app --ci --no-sign
bash scripts/build-unsigned-dmg.sh
cd src-tauri && cargo test --locked
cd src-tauri && cargo fmt --check
cd src-tauri && cargo clippy --all-targets -- -D warnings
node scripts/check-release.mjs
```

CI 通过 `npm run test:ci` 拒绝零测试报告；`npm run release:check` 会在构建后检查开源文件、锁文件、unsigned Release 文档以及 Mermaid 完整性和单 bundle 约束。

## 依赖治理

- 必须提交 `package-lock.json` 和 `src-tauri/Cargo.lock`；CI 使用确定性安装。
- 解析器、安全净化、Mermaid、KaTeX、Highlight.js、Tauri、图片/SVG 依赖不得自动合并升级。
- v0.1 不允许存在未解释的生产高危依赖告警；`npm run audit` 固定使用官方 npm registry，传递依赖安全补丁通过 `package.json` overrides 和 lockfile 固定。
- 依赖升级必须运行 Markdown 语料、安全语料和关键桌面流程回归。
