# 屿阅 v0.1 技术设计

状态：approved  
Revision：MDREADER-TECH-DESIGN-R11  
Design digest：`canonical-sha256: 58afd556e989b155ccfed6ee10b8330219646751b3e329f05750b4c07d48ce59`  
Independent review：MDREADER-ARCH-R3 approved

## 设计摘要

本轮用 unified/remark/rehype 替换字符串式 `markdown-it` 渲染，并建立类型化 `RenderDocument`、受监督 Worker、per-document watcher、统一 SafeSvg、无同源权限 Mermaid renderer、最小外链命令和开源 Release 验证链。

长期边界与容量详见 [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)。本文件记录本 feature 的实现设计与审查冻结项。

## 解析与展示

- 新建与 Vue 无关的 Markdown renderer；Vue composable 只负责调用、状态和 DOM 激活。
- 处理顺序固定为：size gate → remark parse/GFM/math → MDReader mdast transforms → remark-rehype → rehype-sanitize → trusted KaTeX/highlight → stringify。
- mdast transforms 负责 raw HTML literal、Alerts、标题/目录、链接图片分类、Mermaid 占位和诊断。
- `ReadingMode` 接收 `RenderDocument`，不再从 DOM 提取目录；仅活动标签挂载正文 DOM。
- 搜索使用结构化正文/DOM 映射，不能重新解析不可信 HTML；Ctrl/Cmd+F 打开独立搜索浮窗，输入状态与匹配定位分离，输入时不得移动焦点。
- App shell 保持单行紧凑顶部栏；标签在左，打开、主题和阅读设置入口在右。设置使用可访问名称的 20px SVG 图标；目录位于正文左侧，宽版使用除目录外的可用正文宽度。
- 阅读设置是低频控制面板，承载字体缩放、舒适/宽版和适用的远程图片授权；其可见状态由 App shell 单一持有，并通过 document-level pointerdown 判定任意面板外区域关闭。搜索浮窗独立承载搜索和匹配切换；匹配移动后显式恢复搜索输入焦点。
- 界面语言由独立 `useLocale` composable 统一提供简体中文、English、繁體中文和日本語的受类型约束文案。首次无本地偏好时将 `navigator.language` 映射为系统语言（`zh-TW`/`zh-HK`/`zh-MO` 为繁中，`en`/`ja` 对应英语/日语，其余为简中）；用户在阅读设置中选择后写入单个本地偏好键并即时更新，并通过最小 IPC 刷新原生 File/Edit/View/Window 菜单标签。只翻译应用 chrome、状态、提示和可访问名称；Markdown、代码、图表 source 与用户文件不参与翻译，也不产生网络、账号或遥测状态。
- 原生 File 菜单提供 `open-document` 与 `export-current-pdf` 事件。打开复用既有受限文件对话框；PDF 菜单先由 Rust 请求前端提供当前可见标签的 document ID，再由 Rust 在 `DocumentRegistry` 中重新验证该 ID，并从受信任的原始 Markdown 路径派生同名 `.pdf`。随后通过 Tauri `WebviewWindow::with_webview` 使用 macOS `WKWebView` 的原生打印操作。仅设置 print-info 的上 18 mm、左右 16 mm、下 20 mm 页边距和打印任务标题，用于系统“存储为 PDF”面板的默认文件名。该事件链使当前正文与默认文件名在用户选择导出时绑定，不依赖异步标签同步。Tauri 固定在 2.11.x，原生绑定只在 macOS target 编译；不新增 PDF 运行时、不自动写文件、不将路径暴露给前端。原生失败仅向前端发送稳定错误码。print CSS 只保留当前标签阅读正文。
- 横向可滚动的表格、代码和公式复用非常驻滚动条样式，仅在悬停或聚焦时可见；大图预览 stage 使用剩余可视空间垂直居中画布，确保上下留白对称。

## Rust 文件与资源

- `DocumentRegistry` 增加规范身份去重、pre-read size gate、per-document watcher registry 和 revision event。
- watcher 监听父目录，覆盖 atomic replace/rename/delete-recreate/rapid saves。
- 本地 raster 继续使用 no-follow 相对路径与快照协议。
- 新建 `SafeSvg`：禁用外部 resolver，拒绝 nested image/SVG/foreignObject，统一 rasterize 为受限 PNG；PNG 导出复用该边界。
- 公开错误只传稳定代码，不传本机路径；本地 SVG 栅格化加载系统字体以保留中文文本，仍禁用外部 resolver。

## Worker 与标签状态

- `RenderWorkerSupervisor` 只保留每文档最新请求；active first；15 秒超时可终止/recreate。
- response 同时匹配 worker epoch、document revision 和 render generation。
- 标签保存 heading ID + ratio；关闭先 invalidate，再调用幂等 Rust close。
- 全局保留源/结果与标签数使用批准上限；LRU 回收 inactive render result。

## Mermaid

- 固定独立 build entry `/mermaid-renderer.html`，生成单 IIFE 和 build-time hash CSP。
- iframe 只启用 scripts，不启用 same-origin；child CSP deny network。
- postMessage 验证 source、nonce、epoch、request、schema、size。
- Mermaid source 先做类型/复杂度 gate；返回 SVG 再做项目 allowlist。
- Mermaid SVG/CSS 按元素、属性、选择器和声明逐条净化：只保留无外部资源的展示样式，删除动画和未知声明；脚本、外部 URL、CSS escape/comment 绕过和不支持元素仍拒绝整个输出。
- watchdog 非抢占；最低支持 Mac 上以 250 ms 交互延迟和 2 s 图表渲染阈值验收。

## 图片授权与外链

- 图片 AST 只产生 opaque resource ID；初始 HTML 不携带可请求远程 URL。
- HTTPS 加载按钮只改变当前 tab 的内存状态；关闭即销毁。
- 外链点击调用唯一 Rust `open_external_url` 类命令；后端只接受 normalized HTTP/HTTPS，并使用 Rust-side opener API。

## 测试策略

- 版本化 Markdown corpus 同时覆盖典型 AI 文档、边界语法和 adversarial input。
- Renderer、Vue、Rust、desktop flow 分层验证；关键安全边界必须有负向测试。
- CI 必须证明实际执行测试，不接受仅退出码为 0 但零测试的假绿。
- Release 必须在真实 macOS 对 unsigned 安装路径、文件关联、多 watcher、远程资源和 Mermaid 性能做验收。

## 冻结约束

- unified/remark/rehype 是 v0.1 长期解析核心。
- raw HTML literal；remote HTTPS 当前 tab 临时授权；SafeSvg rasterization；Mermaid opaque-origin sandbox；minimal Rust opener；Worker/global budgets；parent-directory watcher 不得在实现中降级。
- 等价实现可调整文件组织；改变上述边界必须重新判断架构复审。
