# 屿阅架构

状态：implementation candidate；本地跨层 final verification complete；GitHub Release gate pending  
Revision：ARCH-YUYUE-R4-2026-08-10  
Independent review：MDREADER-ARCH-R3 approved；本次仅同步品牌术语

## 架构目标

屿阅采用 Tauri + Vue 的本地桌面架构。Rust 负责文件身份、资源访问和系统副作用；前端负责结构化 Markdown 渲染和阅读交互。任何来自文档的内容都视为不可信输入。

本文件定义 v0.1 的架构边界；当前代码已按实现规格落地，本地 unsigned macOS Release Candidate 已完成跨层验证。正式 GitHub tag、attestation 和公开下载验收仍属于外部发布门。

## 核心边界

```text
系统打开/拖入/文件对话框
          │
          ▼
Rust DocumentRegistry ── 文件身份、读取上限、watcher、资源快照
          │ DocumentPayload + revision
          ▼
RenderWorkerSupervisor ── remark/rehype AST、安全 HTML、目录、诊断
          │ RenderDocument
          ▼
Vue 阅读状态 ── 标签、搜索、目录、授权、焦点、阅读位置
          ├── Rust 本地图片解析
          ├── 受限 Mermaid renderer
          └── Rust 最小外链 opener
```

## 文档与文件状态

- `DocumentRegistry` 是打开文档身份、规范路径、根目录、资源快照、容量和 watcher 的唯一 Owner。
- 文档由 Rust 原生入口创建，不接受前端提交任意本机路径。
- 以规范文件身份去重；重复打开只激活已有标签。
- 每个 `document_id` 有独立 watcher 和单调 `revision`。
- watcher 监听父目录并处理 modify/create/remove/rename/atomic replace；替换后重新规范化、验证并 re-arm。
- 关闭标签先使前端 generation 和临时授权失效，再幂等关闭 Rust 上下文、watcher 和资源。

## Markdown 渲染契约

渲染结果不是单一 HTML 字符串，而是类型化 `RenderDocument`：

- `html`：净化后的正文。
- `outline`：H1-H4 标题、稳定 ID 和源码位置。
- `resources`：不透明 ID 对应的本地、HTTPS、data 或阻断资源。
- `diagrams`：不透明 ID 对应的 Mermaid 源码。
- `diagnostics`：稳定错误码、级别、提示和位置。
- `stats`：输入字节、AST 节点、标题和图表数量。

管线顺序：输入上限 → mdast → GFM/math → MDReader transforms → hast → allowlist sanitize → 可信 KaTeX/高亮 → serialize。

- 原始 HTML 节点在 mdast 阶段转换为普通文本，不执行、不直接丢失。
- 图片、Mermaid 和外链源不写入可直接加载的 HTML 属性。
- sanitize 之后不得运行会解释任意 HTML 的插件。

## Worker 与容量

- 单个受监督 Worker 串行处理，队列只保留每文档最新 generation，优先活动标签。
- 单 job 15 秒硬截止；timeout/crash/protocol error 会终止并重建 Worker，旧 epoch 结果不得提交。
- 不自动重试同一异常 generation；用户可在重载后显式重试。
- Markdown 单文件 10 MiB；mdast 200,000 节点；最多 32 标签；全局保留源码 64 MiB；序列化结果 128 MiB。
- 只有活动标签挂载完整正文 DOM；非活动结果按 LRU 回收并可重新渲染。

## 图片与 SVG

- 相对图片只能从当前 Markdown 目录及子目录解析；拒绝目录逃逸、符号链接逃逸、绝对路径和显式 scheme。
- Raster 输入限制单图 20 MiB、最大边 16,384、解码分配上限；文档/全局资源数量和字节继续受配额约束。
- SVG 原始 XML 永不直接进入 DOM。统一 `SafeSvg` 禁止 file/network/string resolver、嵌套 image/SVG 与 `foreignObject`，再由 resvg 转成受限 PNG。
- SafeSvg 输入最多 5 MiB，边长最多 16,384，总像素最多 64M；本地/data SVG 与 Mermaid PNG 导出共用同一边界。

## 远程图片与外链

- HTTPS 图片默认只显示阻断占位；用户可对当前标签执行一次临时加载，关闭标签即撤销，不持久化白名单。
- HTTP、file、绝对路径、目录外路径和未知 scheme 永远不加载。
- 远程图片设置 `no-referrer`；文档不能自行触发授权动作。
- 文档内锚点留在阅读器内。外链只在显式点击时调用唯一 Rust command；后端重新解析并仅允许 HTTP/HTTPS。
- 不向前端暴露通用 opener、文件 reveal、path、mailto 或 tel 权限。

## Mermaid 隔离

- Mermaid 不在正文 realm 运行，而在固定 `/mermaid-renderer.html` 中运行。
- iframe 使用 `sandbox="allow-scripts"`，没有 `allow-same-origin`；renderer 为单 IIFE、无动态 chunk/import/eval/runtime URL。
- renderer 页面使用构建时 SHA-256 script hash 和 deny-by-default CSP；主 CSP 只允许 `frame-src 'self'`。
- 消息校验 `event.source`、nonce、epoch、request ID、schema 和 2 MiB 输出上限。
- 返回 SVG 再经过 MDReader allowlist，拒绝 script、event、foreignObject、image、外部 href 和 URL-bearing CSS。
- 支持类型和复杂度受限：16 KiB/block、300 非空行、1000 字符/行、5000 token、200 edges、50 diagrams、512 KiB total source。
- watchdog 是非抢占式恢复信号，不宣称能硬中断同 UI 进程。最低支持 Mac 上，边界语料必须满足 tab/click 延迟小于 250 ms、单图渲染小于 2 秒；超标即降低限制或拒绝该类型。

## 故障与诊断

- 文件、读取、解析、资源、图表、外链和容量错误使用稳定公开错误码，UI 显示面向用户的说明，不暴露本机路径。
- stale revision/generation/epoch 结果必须被丢弃。
- 单图错误不得使正文失败；单标签渲染错误不得清空其他标签状态。

## 发布与回滚

- v0.1 从 tag 触发 GitHub Actions，生成 unsigned macOS 安装产物、SHA-256 和构建来源。
- 首次公开发布前不存在持久化 schema；解析器迁移失败可通过源码 revision 回滚。
- `RenderDocument` 与安全约束是 v0.1 后的内部冻结契约；HTML 字节输出不是公共 API。

## 架构复审触发

改变解析核心、sanitize 顺序、资源授权、SafeSvg、Mermaid sandbox/CSP/限额、外链 capability、Worker 容量/恢复、watcher identity/revision 或持久化边界时，必须重新判断架构复审。
