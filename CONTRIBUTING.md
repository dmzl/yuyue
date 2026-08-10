# Contributing to 屿阅

感谢你帮助改进屿阅。项目当前处于 v0.1 开源准备阶段，提交前请先确认改动没有扩大既定产品和安全边界。

## 开始之前

- 阅读 [`docs/PROJECT_CONTEXT.md`](docs/PROJECT_CONTEXT.md) 了解定位与非范围。
- 阅读 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) 了解解析、资源、Mermaid、SVG、外链和容量边界。
- 当前 v0.1 工作以 [`docs/features/open-source-v0.1/IMPLEMENTATION_SPEC.md`](docs/features/open-source-v0.1/IMPLEMENTATION_SPEC.md) 为执行契约。
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
- 改变冻结架构边界时，先更新技术设计并完成适用复核，不要只改代码。

## 提交内容

PR 描述至少应包括：

- 改了什么以及为什么。
- 对应 Requirement/Scenario 或 Issue。
- 实际执行的测试和结果。
- 截图或运行证据（用户可见行为变化时）。
- 安全、兼容、性能或平台限制。

## 文档同步

行为、范围、接口、安全默认值、容量或发布方式变化时，必须同步其唯一事实 Owner。README 只承担用户入口，不替代架构或 feature 规格。
