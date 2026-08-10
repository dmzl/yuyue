# v0.1 验收记录

日期：2026-08-10  
状态：本地 unsigned macOS Release candidate；尚未发布 GitHub tag Release

## 自动化检查

- `npm run test:ci`：33 个前端测试，11 个 suite，实际执行。
- `npm run lint`：通过。
- `npm run build`：通过，包含 Mermaid 独立 renderer 构建与 CSP hash finalization。
- `cargo fmt --check`：通过。
- `cargo test --locked`：25 个测试通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过。
- `npm audit --registry=https://registry.npmjs.org --audit-level=high`：0 vulnerabilities。
- `node scripts/check-release.mjs`：通过。

## 真实 macOS Release 观察

验收使用 unsigned Release 的相同生产前端/Rust 路径；仅临时注入只读观察器记录结果，观察后已移除并重新构建干净产物。

- `tests/fixtures/ai-markdown.md`：1 个 Mermaid 图实际生成 SVG；从观察窗口开始计时的上界为 1009 ms；点击打开预览响应为 7 ms。
- `tests/fixtures/mermaid-at-limit.md`：50 个 Mermaid 图进入文档；首批 19 个图在 1002 ms 内生成；点击响应为 2 ms。其余图按产品的 2 秒 Mermaid 自动预算暂停并可继续渲染。
- `tests/fixtures/security.md`：远程 HTTPS 图片保持阻断；Performance Resource observer 记录 `example.test` 请求数为 0；原始 HTML、危险链接和 Mermaid 内容均保持阻断/字面化。
- unsigned DMG 通过无 Finder 自动化脚本生成；只读挂载后包含 `屿阅.app/Contents/MacOS/tauri-app` 和 Applications 入口，SHA-256 已计算并通过挂载检查。
- “屿阅”品牌包已从同一图标母版生成 PNG、ICNS 与 ICO；最终 `.app` 的 display name / bundle name 为“屿阅”，bundle identifier 为 `com.zoulin.yuyue`。在旧本地 MDReader 构建已运行时，最终 `.app` 仍可独立启动，未发生应用身份接管。
- 当前重新构建的 `.app` 已启动并完成两份真实文档验收：本地图进入预览，E-OP1 的 3 个 Mermaid 图均实际生成并显示；安全语料中的原始 HTML、远程图片、file 图片和危险 Mermaid 均保持阻断或字面化。
- 当前构建为 ad-hoc/unsigned（无 Apple Developer Team ID），未进行签名、公证或 Gatekeeper 信任背书；安装指引见 [`UNSIGNED_MACOS.md`](UNSIGNED_MACOS.md)。

## Release 追溯边界

本工作区没有 Git 元数据或 GitHub 发布凭据，因此未执行真实 tag workflow、GitHub attestation 和公开 Release 发布；workflow、checksum、attestation 配置已通过源码检查，正式发布仍需维护者在带 GitHub 权限的仓库中打 tag 执行。
