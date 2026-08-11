# 安全策略 / Security Policy

## 支持版本 / Supported versions

目前支持 v1.0.0 公开版本以及默认分支上的最新代码。安全修复会优先针对仍受支持的版本发布。

The supported versions are the public v1.0.0 release and the latest code on the default branch. Security fixes are prioritized for versions that are still supported.

## 私密报告漏洞 / Report a vulnerability privately

请不要在公开 Issue、Discussion、PR 或社交媒体中披露尚未修复的漏洞细节。

Please do not disclose details of an unfixed vulnerability in a public Issue, Discussion, PR, or on social media.

优先使用 GitHub Private Vulnerability Reporting（**Security → Advisories → Report a vulnerability**）提交私密报告。如果该入口尚不可用，请只创建一个不包含技术细节的公开 Issue，询问维护者当前的私密安全联系方式；等待私密渠道建立后再发送复现信息。

Prefer GitHub Private Vulnerability Reporting (**Security → Advisories → Report a vulnerability**) for private reports. If that entry is not available, create only a public Issue without technical details to ask for the maintainer's current private contact method, then send reproduction information after a private channel is established.

报告建议包含：

A report should include:

- 受影响版本或 commit。

  The affected version or commit.
- 可复现步骤和最小样例文档。

  Reproduction steps and a minimal sample document.
- 实际影响，以及是否涉及文件读取、网络请求、脚本执行或资源耗尽。

  The practical impact, including whether it involves file reads, network requests, script execution, or resource exhaustion.
- 已验证的缓解方式（如有）。

  Any mitigations that have been verified.

## 响应原则 / Response principles

- 维护者应先确认收到报告，再私下复现、评估、修复和协调披露。

  Maintainers should acknowledge receipt first, then reproduce, assess, fix, and coordinate disclosure privately.
- 修复发布前，不要求报告者公开漏洞。

  Reporters are not asked to disclose the vulnerability publicly before a fix is released.
- 有效报告将在征得报告者同意后获得致谢。

  Valid reports receive credit only with the reporter's consent.
- 发布后的安全问题优先通过 GitHub Repository Security Advisory 协作和披露。

  Post-release security issues should be coordinated and disclosed through a GitHub Repository Security Advisory whenever possible.

## 核心安全预期 / Core security expectations

- 本地文档不上传。

  Local documents are not uploaded.
- 原始 HTML 不执行。

  Raw HTML is not executed.
- 远程图片默认阻断，授权不跨标签或重启持久化。

  Remote images are blocked by default, and authorization does not persist across tabs or restarts.
- 文档不能读取其根目录之外的本机资源。

  Documents cannot read local resources outside their root directory.
- Mermaid/SVG 不得绕过网络、脚本、文件或容量边界。

  Mermaid and SVG must not bypass network, script, file, or capacity boundaries.
- 外部链接只能在明确用户点击后由最小权限系统调用打开。

  External links can be opened only after an explicit user click through the minimum-privilege system call.

以上默认值构成本项目的公开安全边界。

These defaults define the project's public security boundary.
