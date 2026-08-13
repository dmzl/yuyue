# 参与贡献 屿阅 / Contributing to Yuyue

感谢你帮助改进屿阅。项目当前已发布 v1.1.0；提交前请先确认改动没有扩大既定产品和安全边界。

Thank you for helping improve Yuyue. The project has released v1.1.0; before submitting a change, make sure it does not expand the established product or security boundaries.

## 开始之前 / Before you start

- 阅读 [`README.md`](README.md) 了解产品定位、功能边界与本地开发命令。

  Read [`README.md`](README.md) for the product position, feature boundaries, and local development commands.
- 阅读 [`SECURITY.md`](SECURITY.md) 了解解析、资源、Mermaid、SVG、外链和容量边界。

  Read [`SECURITY.md`](SECURITY.md) for parsing, resource, Mermaid, SVG, external-link, and capacity boundaries.
- 安全问题不要提交公开 Issue，按 [`SECURITY.md`](SECURITY.md) 私密报告。

  Do not file security issues publicly; report them privately as described in [`SECURITY.md`](SECURITY.md).

## 本地开发 / Local development

```bash
npm install
npm run tauri dev
```

提交前运行完整的本地质量检查：

Run the full local quality checks before submitting:

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

Do not treat a zero exit code with zero executed tests as a pass; `npm run test:ci` checks the actual Vitest execution count.

## 变更原则 / Change principles

- 一个 PR 聚焦一个可解释目标，说明用户影响、风险、验证和未覆盖项。

  Keep each PR focused on one explainable goal, and describe user impact, risks, verification, and uncovered areas.
- 新 Markdown 行为应先加入版本化语料和结构断言。

  Add new Markdown behavior to versioned corpus fixtures and structural assertions first.
- 不把文档内容直接拼进 HTML、URL、SVG 或错误提示。

  Do not concatenate document content directly into HTML, URLs, SVG, or error messages.
- 不新增任意文件系统 scope、通用 opener 权限、持久远程图片授权或未经批准的网络能力。

  Do not add arbitrary filesystem scope, a general opener capability, persistent remote-image authorization, or unapproved network access.
- 不随意增加 remark/rehype 插件；产品特有行为优先使用小型、可测试的项目 transform。

  Do not add remark/rehype plugins casually; prefer small, testable project transforms for product-specific behavior.
- 改变解析、安全、资源访问或发布边界时，先在 Issue 或 PR 中说明影响并取得维护者确认，不要只改代码。

  When changing parsing, security, resource access, or release boundaries, explain the impact in an Issue or PR and obtain maintainer confirmation before changing code alone.

## 提交内容 / Submission checklist

PR 描述至少应包括：

Every PR description should include at least:

- 改了什么以及为什么。

  What changed and why.
- 对应 Issue；若没有 Issue，说明需求来源和验收方式。

  The related Issue; if there is none, explain the request source and acceptance method.
- 实际执行的测试和结果。

  The tests actually run and their results.
- 截图或运行证据（用户可见行为变化时）。

  Screenshots or runtime evidence when user-visible behavior changes.
- 安全、兼容、性能或平台限制。

  Security, compatibility, performance, or platform limitations.

## 文档同步 / Documentation synchronization

行为、范围、接口、安全默认值、容量或发布方式变化时，必须同步相应的公开说明，避免 README、安装指引和安全策略与实际行为脱节。公开文档应同时提供简体中文和 English；命令、代码、标识符与协议名称保持原样。

When behavior, scope, interfaces, security defaults, capacity, or release methods change, update the corresponding public documentation so the README, installation guide, and security policy remain aligned with reality. Public documentation should provide both Simplified Chinese and English; commands, code, identifiers, and protocol names remain unchanged.
