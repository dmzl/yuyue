# Implementation Spec: 屿阅 v0.1 开源准备

SPEC_STATUS：ready  
Revision：OSS-V0.1-SPEC-2026-08-10-R10

## Scope

将现有 macOS Tauri/Vue 阅读器实现为满足 OSS-V0.1-REQ-2026-08-10-R7 的隐私优先 v0.1，并建立可验证的 GitHub 开源分发基础。

## Non-Scope

编辑/保存、云/账号/遥测、插件、自动更新、整篇 HTML 导出、无对话框的自动/批量 PDF 写入、Windows/Linux installer、Apple 签名公证、最近文件数据库和持久化远程图片授权。

## Source Owners and Revisions

| Owner | Revision / evidence | Facts |
|---|---|---|
| `docs/PROJECT_CONTEXT.md` | PROJECT-CONTEXT-2026-08-10-R2 | 定位、范围、原则、分发 |
| `docs/ARCHITECTURE.md` | ARCH-YUYUE-R4-2026-08-10 | 系统与安全长期边界 |
| `REQUIREMENTS.md` | OSS-V0.1-REQ-2026-08-10-R7 | Requirement/Scenario/验收 |
| `TECH_DESIGN.md` | MDREADER-TECH-DESIGN-R11；canonical digest `58afd556e989b155ccfed6ee10b8330219646751b3e329f05750b4c07d48ce59` | 技术设计与冻结约束 |
| Architecture review | MDREADER-ARCH-R3 approved | 独立架构批准 |
| Current code baseline | 2026-08-09 audited workspace | 迁移起点，不覆盖目标事实 |

## Frozen Decisions and Constraints

- F-001 macOS-first；GitHub Release 提供 unsigned 可安装产物、SHA-256、来源与 Gatekeeper 说明。
- F-002 MIT License。
- F-003 unified/remark/rehype AST 管线和类型化 `RenderDocument`。
- F-004 raw HTML literal；sanitize 顺序和 post-sanitize trusted transform 边界不可改变。
- F-005 HTTPS 图片默认阻断，只在当前标签显式临时授权；HTTP/file/absolute/out-of-root 禁止。
- F-006 SafeSvg 禁用外部 resolver并 rasterize；本地/data SVG 与单图 PNG 导出共用。
- F-007 Mermaid 固定 opaque-origin sandbox、hashed single bundle、deny-network CSP、严格消息与返回 SVG allowlist。
- F-008 Worker supervisor、全局容量、active-only DOM、parent-directory watcher 和 minimal Rust opener 不得降级。
- F-009 File 菜单 PDF 导出仅触发当前已净化正文的 macOS 原生打印对话框；原生菜单先请求当前可见标签的 document ID，Rust 在 `DocumentRegistry` 中验证该 ID 并从原始 Markdown 路径派生同名 `.pdf`。系统打印参数固定为上 18 mm、左右 16 mm、下 20 mm 页边距，任务标题使用该名称，供用户自行选择“存储为 PDF”。不引入自动写文件或第三方 PDF 运行时，也不将路径传给前端。
- F-010 界面语言仅支持简体中文、English、繁體中文、日本語；首次启动遵循 macOS 系统语言映射，用户选择后仅本地保存并优先使用。不得翻译 Markdown、代码、图表 source，也不得因此加入网络、账号或遥测。

## Requirements and Scenarios

### IS-001 Structured render

系统必须实现 R-002、R-003、R-009；同一次 parse 生成 HTML、H1-H4 outline、resource/diagram side channels、diagnostics 和 stats。S-001 与 S-005 使用 renderer corpus 验收。

### IS-002 Document lifecycle

系统必须实现 R-004；Rust 负责 canonical identity、per-document revision 和原生 File 菜单事件，UI 负责 generation/reading state 与当前正文打印。S-002、S-003、S-008 使用 Rust integration 与 desktop flow 验收。

### IS-003 Resource security

系统必须实现 R-005 至 R-007。任何文档输入不得直接形成未授权 `src/href`、本机任意路径读取或未净化 SVG DOM。S-004、S-005 使用负向语料、网络观察与 protocol 测试验收。

### IS-004 Bounded operation

系统必须落实 ARCH-MDREADER-R3 的文件、AST、tab、retained data、image、SVG、Mermaid 和 deadline 上限。S-006 必须在超限时产生稳定诊断，其他标签保持可操作。

### IS-005 Reader UI and accessibility

系统必须实现 R-003、R-008：搜索、H1-H4 目录、内部跳转、copy code、宽内容、主题/缩放/宽度、键盘语义、可见焦点和 modal focus trap/restore。

### IS-006 OSS release

系统必须实现 R-001、R-010 和 S-007。Release workflow 只从 tag 生成资产；产物必须带 SHA-256 和 GitHub artifact attestation/build provenance；README 不得暗示 Apple 签名公证。

### IS-007 Interface locale

系统必须实现 R-011 和 S-009。`useLocale` 必须对初始系统语言映射与已保存偏好优先级提供可测试的纯函数；设置内语言选择必须即时更新所有应用 chrome、可访问名称和原生 File/Edit/View/Window 菜单。偏好仅保存在本机，用户文档内容不进入翻译管线。

### IS-008 Brand identity

系统必须实现 R-012 和 S-010。Tauri product name、窗口标题、应用菜单、欢迎页、macOS bundle/DMG 名称与图标必须统一为“屿阅”。图标从同一方形母版生成所需的 PNG、ICNS 与 ICO 投影；bundle identifier 使用 `com.zoulin.yuyue`，`mdreader-image` 与 `mdreader-locale` 保持不变。

## State / Permission Contract

- Tab state：`documentId`, `sourceRevision`, `renderGeneration`, optional `RenderDocument`, heading/ratio position, search, remoteImageAuthorization, diagram states。
- Remote authorization：`blocked → allowed-for-live-tab → destroyed-on-close`；无持久状态。
- Worker：epoch + generation；超时/crash 后 old epoch 永远不可提交。
- Mermaid：renderer epoch + nonce + request；child 无路径、documentId 或授权信息。
- 权限：前端只有必要 core/dialog IPC；通用 filesystem/opener capability 不暴露。

## UI Contract

- 未授权远程图片显示可理解占位及一次性“加载此文档远程图片”动作。
- 错误在相关内容附近或文档状态区显示，不只写 console。
- 目录项、标签、上下文动作和媒体预览使用语义控件；可键盘操作并有 focus ring。
- 关闭/reload/worker restart/diagram failure 不丢失其他标签状态。
- Mermaid 超过长期任务阈值时暂停该文档后续自动图表渲染，并提供显式继续入口。
- 阅读设置包含四种支持语言的选择入口；首次启动遵循系统语言映射，选择后的本地偏好优先于系统语言，界面即时刷新但文档正文不翻译。
- 应用显示“屿阅”，标题栏与欢迎页使用同一图标母版的品牌图形；该用户可见名称不改变内部 URI 协议或本地偏好键，并使用新的 `com.zoulin.yuyue` bundle identifier 避免与旧本地构建冲突。
- 顶栏为单行紧凑布局：标签在左，打开、主题与阅读设置入口在右；搜索、字体和宽度不常驻占用正文高度。
- Ctrl/Cmd+F 搜索浮窗中的连续输入、Enter 匹配切换和再次编辑不得丢失焦点或字符；阅读设置在任意面板外区域关闭，目录显示在左侧，宽版使用除目录外的可用正文宽度。
- File 菜单包含“打开… / Cmd+O”和“导出为 PDF… / Cmd+P”；导出仅打印当前标签正文并交由 macOS 原生对话框保存 PDF，默认上 18 mm、左右 16 mm、下 20 mm 页边距，且默认文件名为当前 Markdown 的同名 `.pdf`；无当前文档时显示明确提示。
- 表格、代码与公式横向滚动条仅在悬停或聚焦时显示；大图预览画布在可用区域垂直居中，上下留白对称。
- 主题入口显示当前模式和下一次切换结果；代码复制使用带可访问名称的图标按钮。
- Mermaid 安全检查逐项净化展示样式，不因可安全删除的样式整图失败；本地 SVG 栅格化必须保留系统字体文字。

## API / Data / Error Contract

- Rust document payload/event 不返回规范本机路径；事件包含 document ID 和 monotonic revision。
- Resource resolver 输入为 document ID + classified source，输出 document-scoped URL 或稳定错误码。
- External opener 输入为 URL 字符串，后端重新解析并只允许 HTTP/HTTPS。
- 错误码至少区分 unsupported/open/read/too-large/not-found、image scheme/path/type/quota、SVG unsafe/limit、render timeout/crash/limit、Mermaid unsafe/limit/timeout、external URL rejected。

## Compatibility / Migration / Rollback

- 无用户持久化 schema 或历史数据迁移。
- 解析输出可能与 markdown-it 不同；以版本化语料和 Requirement 为兼容基线，不以 HTML 字节一致为目标。
- 架构迁移应保持一个可复核批次；首次公开发布前若关键安全/语料验收失败，可源码回滚到迁移前 revision，但不得发布旧的不安全实现。

## Non-Functional Requirements

- NFR-001 最低支持 Mac 上 Mermaid at-limit corpus：tab/click latency `<250 ms`，单图 `<2 s`；不合格则降限或拒绝类型。
- NFR-002 Markdown worker job 15 秒后可终止重建；故障不得阻断后续最新请求。
- NFR-003 生产高危依赖无未解释项；锁文件与确定性 CI 安装。
- NFR-004 Release 产物可校验、可追溯到 tag/workflow，unsigned 状态明确。
- NFR-005 用户文档、Owner、实现和验收必须一致。

## Acceptance and Test Seams

| Requirement | Primary seam |
|---|---|
| R-001 / IS-006 | tagged GitHub Actions、checksum/attestation、真实 unsigned macOS 安装 |
| R-002 / IS-001 | renderer unit + AI Markdown corpus |
| R-003 / IS-005 | renderer structural + Vue component + desktop interaction |
| R-004 / IS-002 | Rust watcher/file-name unit + multi-tab desktop flow |
| R-005 / IS-003 | network observer + resolver/SVG negative tests |
| R-006 | opener unit/capability + exactly-once desktop flow |
| R-007 / IS-004 | typed diagnostic assertions + cross-tab failure tests |
| R-008 | component accessibility + keyboard/focus runtime |
| R-009 | type/API tests and removal of markdown-it/DOM outline derivation |
| R-010 | CI executed-test counts, audit, docs/release checks |
| R-011 / IS-007 | locale unit + settings interaction + macOS system-locale startup flow |
| R-012 / IS-008 | generated icon assets + bundled app/DMG names + desktop launch observation |

## Traceability

- IS-001 → R-002, R-003, R-009 → S-001, S-005。
- IS-002 → R-004 → S-002, S-003, S-008。
- IS-003 → R-005, R-006, R-007 → S-004, S-005。
- IS-004 → R-007 → S-006。
- IS-005 → R-003, R-008。
- IS-006 → R-001, R-010 → S-007。
- IS-007 → R-011 → S-009。
- IS-008 → R-012 → S-010。
