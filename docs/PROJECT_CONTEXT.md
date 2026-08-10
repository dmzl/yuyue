# 屿阅项目上下文

状态：current  
Revision：PROJECT-CONTEXT-2026-08-10-R2

## 产品定位

屿阅是一个隐私优先的桌面 Markdown 阅读器。它面向 AI 大量生成 Markdown、但接收者不熟悉 Markdown 工具的场景，让用户可以像阅读普通文档一样打开、浏览和理解 `.md` 文件。

屿阅是阅读工具，不是 Markdown 编辑器、知识库、云文档平台或 AI 对话客户端。

## 目标用户

- 收到 AI 生成 Markdown 文档、希望直接阅读的非技术用户。
- 需要检查包含表格、脚注、任务列表、数学公式、代码和 Mermaid 图的长文档用户。
- 希望文档默认留在本机、远程资源未经确认不自动加载的隐私敏感用户。

## 产品原则

1. 打开即读：不要求用户安装编辑器、浏览器插件或理解构建工具。
2. 忠实可读：优先正确呈现 AI 常见 Markdown 结构，并对无法呈现的内容给出可见诊断。
3. 隐私默认：本地文档不上传；远程图片默认阻断；外部链接不在应用 WebView 内导航。
4. 失败可理解：文件、解析、图片、图表和容量错误必须显示给用户，不得静默丢失。
5. 有界运行：长文档、图片、SVG 和 Mermaid 必须受明确资源限制，单份异常内容不得拖垮全部标签页。
6. 开源可维护：核心行为由清晰的长期文档、类型化契约、测试语料和可复现 Release 流程约束。

## v0.1 范围

- macOS 桌面客户端；支持打开、拖入、系统文件关联和多标签阅读 `.md`、`.markdown`、`.txt`。
- GFM 表格、删除线、任务列表、脚注、GitHub Alerts、代码块、KaTeX 和受限 Mermaid。
- H1-H4 目录、稳定标题锚点、文档内跳转、全文搜索、代码复制与语言标签。
- 长表格/代码横向查看、主题/字体缩放/内容宽度、键盘和焦点可访问性。
- 每个标签独立监听文件变化并保留阅读位置。
- 文档目录内相对图片受控加载；HTTPS 远程图片默认阻断，只能在当前标签显式临时授权。
- GitHub 公共源码、MIT License、unsigned macOS 可安装产物、SHA-256、构建来源和安装说明。

## 非范围

- 编辑、保存、所见即所得或双栏编辑器。
- 账号、云同步、上传、遥测、广告或在线 AI 功能。
- 插件系统、自动更新、最近文件数据库或跨重启恢复。
- 整篇文档 PDF/HTML 导出；单个已净化 Mermaid 图导出 PNG 可保留。
- v0.1 的 Windows/Linux 安装包。
- Apple Developer ID 签名和公证。

## 分发原则

- v0.1 通过 GitHub Releases 发布 unsigned macOS 安装产物，用户不需要自行构建。
- Release 必须同时提供源码、校验和、未签名提示、Gatekeeper 手动安装说明、构建工作流来源和从源码构建方法。
- Windows 只作为后续路线，不属于 v0.1 验收。

## 事实归属

- 当前技术栈与命令：[`TECH_STACK.md`](./TECH_STACK.md)
- 长期系统边界：[`ARCHITECTURE.md`](./ARCHITECTURE.md)
- 本轮开源准备需求：[`features/open-source-v0.1/REQUIREMENTS.md`](./features/open-source-v0.1/REQUIREMENTS.md)
- 本轮批准技术设计：[`features/open-source-v0.1/TECH_DESIGN.md`](./features/open-source-v0.1/TECH_DESIGN.md)
- 本轮执行契约：[`features/open-source-v0.1/IMPLEMENTATION_SPEC.md`](./features/open-source-v0.1/IMPLEMENTATION_SPEC.md)
