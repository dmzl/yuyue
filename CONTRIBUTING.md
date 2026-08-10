# Contributing to 屿阅

感谢你帮助改进屿阅。项目当前处于 v0.1 开源准备阶段，提交前请先确认改动没有扩大既定产品和安全边界。

## 开始之前

- 阅读 [`README.md`](README.md) 了解产品定位、功能边界与本地开发命令。
- 阅读 [`SECURITY.md`](SECURITY.md) 了解解析、资源、Mermaid、SVG、外链和容量边界。
- 安全问题不要提交公开 Issue，按 [`SECURITY.md`](SECURITY.md) 私密报告。

## 本地开发

```bash
npm install
npm run tauri dev
```

提交前运行完整的本地质量检查：

```bash
npm run test:ci
npm run lint
npm run audit
npm run build
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cd ..
node scripts/check-release.mjs
```

不要把退出码为 0 但实际执行零测试视为通过；`npm run test:ci` 会检查 Vitest 的实际执行数量。

## 变更原则

- 一个 PR 聚焦一个可解释目标，说明用户影响、风险、验证和未覆盖项。
- 新 Markdown 行为应先加入版本化语料和结构断言。
- 不把文档内容直接拼进 HTML、URL、SVG 或错误提示。
- 不新增任意文件系统 scope、通用 opener 权限、持久远程图片授权或未经批准的网络能力。
- 不随意增加 remark/rehype 插件；产品特有行为优先使用小型、可测试的项目 transform。
- 改变解析、安全、资源访问或发布边界时，先在 Issue 或 PR 中说明影响并取得维护者确认，不要只改代码。

## 提交内容

PR 描述至少应包括：

- 改了什么以及为什么。
- 对应 Issue；若没有 Issue，说明需求来源和验收方式。
- 实际执行的测试和结果。
- 截图或运行证据（用户可见行为变化时）。
- 安全、兼容、性能或平台限制。

## 文档同步

行为、范围、接口、安全默认值、容量或发布方式变化时，必须同步相应的公开说明，避免 README、安装指引和安全策略与实际行为脱节。
