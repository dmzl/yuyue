import { toString } from 'mdast-util-to-string'
import rehypeHighlight from 'rehype-highlight'
import rehypeKatex from 'rehype-katex'
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize'
import rehypeStringify from 'rehype-stringify'
import remarkGfm from 'remark-gfm'
import remarkMath from 'remark-math'
import remarkParse from 'remark-parse'
import remarkRehype from 'remark-rehype'
import { unified } from 'unified'
import { visit } from 'unist-util-visit'

const MAX_MARKDOWN_BYTES = 10 * 1024 * 1024
const MAX_AST_NODES = 200_000
const MAX_RENDERED_BYTES = 128 * 1024 * 1024
const MAX_MERMAID_DIAGRAMS = 50
const MAX_MERMAID_SOURCE_BYTES = 512 * 1024

export interface RenderOutlineItem {
  id: string
  level: number
  text: string
  sourcePosition?: { start: number; end: number }
}

export type ResourceKind = 'local' | 'https' | 'data' | 'blocked'

export interface RenderResource {
  id: string
  kind: ResourceKind
  source: string
  alt: string
  title?: string
}

export type RenderLinkKind = 'internal' | 'external' | 'blocked'

export interface RenderLink {
  id: string
  kind: RenderLinkKind
  url: string
  text: string
}

export interface RenderDiagram {
  id: string
  source: string
  error?: 'MERMAID_LIMIT'
}

export interface RenderDiagnostic {
  code: string
  severity: 'info' | 'warning' | 'error'
  message: string
  sourcePosition?: { start: number; end: number }
}

export interface RenderStats {
  inputBytes: number
  astNodes: number
  headingCount: number
  diagramCount: number
}

export interface RenderSourceBlock {
  id: string
  startLine: number
  endLine: number
  kind?: 'heading' | 'block'
}

export interface RenderOptions {
  renderLeaseId?: string
  contextEpoch?: number
  renderGeneration?: number
}

export interface RenderDocument {
  html: string
  outline: RenderOutlineItem[]
  resources: RenderResource[]
  links: RenderLink[]
  diagrams: RenderDiagram[]
  diagnostics: RenderDiagnostic[]
  stats: RenderStats
  /** Present on every Worker-produced document; optional here keeps existing reader fixtures concise. */
  sourceBlocks?: RenderSourceBlock[]
  renderLeaseId?: string
  contextEpoch?: number
  renderGeneration?: number
}

type MutableNode = {
  type: string
  value?: string
  depth?: number
  lang?: string | null
  children?: MutableNode[]
  data?: Record<string, unknown>
  url?: string
  title?: string | null
  alt?: string | null
  position?: any
}

type RenderContext = {
  outline: RenderOutlineItem[]
  resources: RenderResource[]
  links: RenderLink[]
  diagrams: RenderDiagram[]
  diagnostics: RenderDiagnostic[]
  usedHeadingIds: Map<string, number>
  mermaidSourceBytes: number
  sourceBlocks: RenderSourceBlock[]
  sourceBlockIds: Set<string>
  sourceBlockHash: string
  totalSourceLines: number
  nextSourceBlockSequence: number
}

const sanitizeSchema = {
  ...defaultSchema,
  clobberPrefix: '',
  tagNames: [...(defaultSchema.tagNames ?? []), 'aside', 'input', 'span'],
  attributes: {
    ...defaultSchema.attributes,
    '*': [
      ...(defaultSchema.attributes?.['*'] ?? []),
      'id',
      'role',
      'tabIndex',
      'aria-label',
      'aria-hidden',
      'data-md-resource-id',
      'data-md-link-id',
      'data-md-diagram-id',
      'data-md-source-block-id',
      'data-alert-kind',
    ],
    a: [
      ...(defaultSchema.attributes?.a ?? []),
      'data-md-link-id',
      'aria-label',
    ],
    blockquote: [
      ...(defaultSchema.attributes?.blockquote ?? []),
      ['className', /^markdown-alert(?:-(?:note|tip|important|warning|caution))?$/],
    ],
    input: [
      ...(defaultSchema.attributes?.input ?? []),
      'type',
      'checked',
      'disabled',
    ],
    code: [...(defaultSchema.attributes?.code ?? []), ['className', /^language-/]],
  },
  protocols: {
    ...defaultSchema.protocols,
    href: ['#'],
  },
} as any

function emptyStats(inputBytes: number): RenderStats {
  return { inputBytes, astNodes: 0, headingCount: 0, diagramCount: 0 }
}

function stableSourceHash(source: string) {
  let hash = 0x811c9dc5
  for (let index = 0; index < source.length; index += 1) {
    hash ^= source.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193)
  }
  return (hash >>> 0).toString(16).padStart(8, '0')
}

function sourceLineCount(source: string) {
  return source.length === 0 ? 1 : source.split(/\r\n|\r|\n/).length
}

function defaultRenderLeaseId(sourceHash: string, sequence: number) {
  return `lease-${sourceHash}-${sequence}`
}

let renderLeaseSequence = 0

function diagnostic(
  code: string,
  message: string,
  severity: RenderDiagnostic['severity'] = 'error',
): RenderDiagnostic {
  return { code, severity, message }
}

function resourceKind(source: string): ResourceKind {
  const value = source.trim()
  const lower = value.toLowerCase()
  if (lower.startsWith('https://')) return 'https'
  if (lower.startsWith('data:image/')) return 'data'
  if (
    value.length > 0 &&
    !value.startsWith('/') &&
    !value.startsWith('\\') &&
    !lower.startsWith('file:') &&
    !lower.startsWith('http:') &&
    !/^[a-z][a-z\d+.-]*:/i.test(value)
  ) {
    return 'local'
  }
  return 'blocked'
}

function linkKind(url: string): RenderLinkKind {
  const value = url.trim().toLowerCase()
  if (value.startsWith('#')) return 'internal'
  if (value.startsWith('http://') || value.startsWith('https://')) return 'external'
  return 'blocked'
}

function stableHeadingId(text: string, usedHeadingIds: Map<string, number>): string {
  const base =
    text
      .trim()
      .toLocaleLowerCase()
      .replace(/[^\p{L}\p{N}]+/gu, '-')
      .replace(/^-+|-+$/g, '') || 'section'
  const count = usedHeadingIds.get(base) ?? 0
  usedHeadingIds.set(base, count + 1)
  return count === 0 ? base : `${base}-${count + 1}`
}

function positionOf(node: MutableNode): { start: number; end: number } | undefined {
  const start = node.position?.start?.line
  const end = node.position?.end?.line
  if (!start || !end) return undefined
  return { start, end }
}

function sourceBlockPosition(node: MutableNode, context: RenderContext) {
  const position = positionOf(node)
  if (!position) return undefined
  const startLine = Math.max(1, Math.min(position.start, context.totalSourceLines))
  const endLine = Math.max(startLine, Math.min(position.end, context.totalSourceLines))
  return { startLine, endLine }
}

function assignSourceBlock(node: MutableNode, context: RenderContext) {
  if (node.data?.sourceBlockId) return
  const position = sourceBlockPosition(node, context)
  if (!position) return
  const sequence = ++context.nextSourceBlockSequence
  const id = `sb-${context.sourceBlockHash}-${sequence}`
  node.data = {
    ...node.data,
    sourceBlockId: id,
    hProperties: {
      ...((node.data?.hProperties as Record<string, unknown>) ?? {}),
      'data-md-source-block-id': id,
    },
  }
  context.sourceBlocks.push({ id, ...position, kind: node.type === 'heading' ? 'heading' : 'block' })
  context.sourceBlockIds.add(id)
}

function isSourceBlockNode(node: MutableNode, parent?: MutableNode) {
  if (node.type === 'heading' || node.type === 'code' || node.type === 'table'
    || node.type === 'thematicBreak' || node.type === 'math') return true
  if (node.type === 'listItem') return true
  // A list item's own DOM element is the source target; avoid putting a
  // second target on its immediate paragraph child.
  return node.type === 'paragraph' && parent?.type !== 'listItem'
}

function makeElement(
  tagName: string,
  properties: Record<string, unknown>,
  children: unknown[] = [],
) {
  return { type: 'element', tagName, properties, children }
}

function transformMarkdownTree(tree: MutableNode, context: RenderContext) {
  visit(tree as any, (node: MutableNode, _index: number | undefined, parent: MutableNode | undefined) => {
    if (node.type === 'html') {
      // Raw HTML is rendered as literal text before mdast is converted to hast.
      node.type = 'text'
      return
    }

    if (node.type === 'heading' && node.depth && node.depth <= 4) {
      const text = toString(node)
      const id = stableHeadingId(text, context.usedHeadingIds)
      node.data = {
        ...node.data,
        hProperties: { ...((node.data?.hProperties as Record<string, unknown>) ?? {}), id },
      }
      context.outline.push({ id, level: node.depth, text, sourcePosition: positionOf(node) })
    }

    if (isSourceBlockNode(node, parent)) assignSourceBlock(node, context)

    if (node.type === 'image') {
      const source = node.url ?? ''
      const id = `resource-${context.resources.length + 1}`
      context.resources.push({
        id,
        kind: resourceKind(source),
        source,
        alt: node.alt ?? '',
        ...(node.title ? { title: node.title } : {}),
      })
      node.data = { ...node.data, resourceId: id }
    }

    if (node.type === 'link') {
      const url = node.url ?? ''
      const id = `link-${context.links.length + 1}`
      context.links.push({ id, kind: linkKind(url), url, text: toString(node) })
      node.data = { ...node.data, linkId: id }
    }

    if (node.type === 'code' && node.lang?.trim().toLocaleLowerCase() === 'mermaid') {
      const id = `diagram-${context.diagrams.length + 1}`
      const source = node.value ?? ''
      const sourceBytes = new TextEncoder().encode(source).byteLength
      const overLimit = context.diagrams.length >= MAX_MERMAID_DIAGRAMS
        || context.mermaidSourceBytes + sourceBytes > MAX_MERMAID_SOURCE_BYTES
      context.mermaidSourceBytes += sourceBytes
      context.diagrams.push({ id, source, ...(overLimit ? { error: 'MERMAID_LIMIT' as const } : {}) })
      node.type = 'mdreader-mermaid'
      node.data = { ...node.data, diagramId: id, ...(overLimit ? { diagramError: 'MERMAID_LIMIT' } : {}) }
      if (overLimit && !context.diagnostics.some((item) => item.code === 'MERMAID_LIMIT')) {
        context.diagnostics.push(diagnostic('MERMAID_LIMIT', 'Mermaid 图表总量超过安全限制'))
      }
    }

    if (node.type === 'blockquote') {
      const firstChild = node.children?.[0]
      const marker = firstChild && toString(firstChild).match(/^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*/i)
      if (marker) {
        const kind = marker[1].toLocaleLowerCase()
        const firstText = firstChild?.children?.find((child) => child.type === 'text')
        if (firstText?.value) {
          firstText.value = firstText.value.replace(marker[0], '')
        }
        node.data = {
          ...node.data,
          hProperties: {
            ...((node.data?.hProperties as Record<string, unknown>) ?? {}),
            className: ['markdown-alert', `markdown-alert-${kind}`],
            'data-alert-kind': kind,
          },
        }
      }
    }
  })
}

function createHandlers() {
  return {
    image: (_state: any, node: MutableNode) => {
      const resourceId = String(node.data?.resourceId ?? '')
      return makeElement(
        'span',
        {
          className: ['markdown-image-placeholder'],
          'data-md-resource-id': resourceId,
          role: 'img',
          'aria-label': node.alt || '图片',
        },
        [{ type: 'text', value: node.alt || '图片' }],
      )
    },
    link: (state: any, node: MutableNode) => {
      const linkId = String(node.data?.linkId ?? '')
      const kind = linkKind(node.url ?? '')
      if (kind === 'internal') {
        return makeElement(
          'a',
          { href: node.url, 'data-md-link-id': linkId },
          state.all(node),
        )
      }
      return makeElement(
        'a',
        {
          href: '#',
          'data-md-link-id': linkId,
          ...(kind === 'blocked' ? { className: ['markdown-link-blocked'] } : {}),
        },
        state.all(node),
      )
    },
    'mdreader-mermaid': (_state: any, node: MutableNode) =>
      makeElement(
        'div',
        {
          className: ['mermaid-placeholder', ...(node.data?.diagramError ? ['mermaid-error'] : [])],
          'data-md-diagram-id': String(node.data?.diagramId ?? ''),
          ...(node.data?.sourceBlockId ? { 'data-md-source-block-id': String(node.data.sourceBlockId) } : {}),
          role: 'img',
          'aria-label': 'Mermaid 图表',
        },
        [{ type: 'text', value: node.data?.diagramError ? 'Mermaid 图表超过安全限制' : 'Mermaid 图表' }],
      ),
  }
}

function sourceBlockIdFromProperties(properties: Record<string, unknown> | undefined) {
  const value = properties?.['data-md-source-block-id'] ?? properties?.dataMdSourceBlockId
  return typeof value === 'string' ? value : undefined
}

function validateSourceBlockDom(tree: unknown, context: RenderContext) {
  const observed = new Set<string>()
  let valid = true
  visit(tree as any, 'element', (node: any) => {
    const id = sourceBlockIdFromProperties(node.properties)
    if (!id) return
    if (!/^sb-[a-f0-9]{8}-[0-9]{1,6}$/.test(id)
      || !context.sourceBlockIds.has(id)
      || observed.has(id)) valid = false
    observed.add(id)
  })
  if (!valid || observed.size !== context.sourceBlockIds.size) {
    throw new Error('RENDER_SOURCE_BLOCK_PROTOCOL')
  }
}

function renderResultContext(sourceHash: string, options: RenderOptions) {
  const contextEpoch = options.contextEpoch ?? 0
  const renderGeneration = options.renderGeneration ?? 0
  const renderLeaseId = options.renderLeaseId ?? defaultRenderLeaseId(sourceHash, ++renderLeaseSequence)
  return { contextEpoch, renderGeneration, renderLeaseId }
}

function emptyRenderDocument(
  inputBytes: number,
  sourceHash: string,
  options: RenderOptions,
  diagnostics: RenderDiagnostic[] = [],
): RenderDocument {
  return {
    html: '',
    outline: [],
    resources: [],
    links: [],
    diagrams: [],
    diagnostics,
    stats: emptyStats(inputBytes),
    sourceBlocks: [],
    ...renderResultContext(sourceHash, options),
  }
}

export async function renderMarkdown(source: string, options: RenderOptions = {}): Promise<RenderDocument> {
  const inputBytes = new TextEncoder().encode(source).byteLength
  const sourceHash = stableSourceHash(source)
  const context: RenderContext = {
    outline: [],
    resources: [],
    links: [],
    diagrams: [],
    diagnostics: [],
    usedHeadingIds: new Map(),
    mermaidSourceBytes: 0,
    sourceBlocks: [],
    sourceBlockIds: new Set(),
    sourceBlockHash: sourceHash,
    totalSourceLines: sourceLineCount(source),
    nextSourceBlockSequence: 0,
  }

  if (inputBytes > MAX_MARKDOWN_BYTES) {
    return emptyRenderDocument(
      inputBytes,
      sourceHash,
      options,
      [diagnostic('MARKDOWN_TOO_LARGE', 'Markdown 文件超过 10 MiB 限制')],
    )
  }

  try {
    const processor = unified()
      .use(remarkParse)
      .use(remarkGfm)
      .use(remarkMath)
      .use(() => (tree: any) => transformMarkdownTree(tree as MutableNode, context))
      .use(remarkRehype, { clobberPrefix: '', handlers: createHandlers() as any })
      .use(() => (tree: any) => {
        visit(tree, 'element', (node: any) => {
          if (/^h[1-4]$/.test(node.tagName) && typeof node.properties?.id === 'string') {
            node.properties.id = node.properties.id.replace(/^user-content-/, '')
          }
        })
      })
      .use(rehypeSanitize, sanitizeSchema)
      .use(rehypeKatex)
      .use(rehypeHighlight)
      .use(() => (tree: any) => validateSourceBlockDom(tree, context))
      .use(rehypeStringify)
    const tree = processor.parse(source)
    const astNodes = countNodes(tree as MutableNode)
    if (astNodes > MAX_AST_NODES) {
      return {
        ...emptyRenderDocument(
          inputBytes,
          sourceHash,
          options,
          [diagnostic('MARKDOWN_AST_LIMIT', 'Markdown 结构超过安全限制')],
        ),
        stats: {
          inputBytes,
          astNodes,
          headingCount: 0,
          diagramCount: 0,
        },
      }
    }
    const transformedTree = await processor.run(tree)
    const html = String(processor.stringify(transformedTree))
    if (new TextEncoder().encode(html).byteLength > MAX_RENDERED_BYTES) {
      return {
        html: '',
        outline: context.outline,
        resources: context.resources,
        links: context.links,
        diagrams: context.diagrams,
        diagnostics: [diagnostic('MARKDOWN_RENDER_LIMIT', 'Markdown 渲染结果超过安全限制')],
        stats: {
          inputBytes,
          astNodes,
          headingCount: context.outline.length,
          diagramCount: context.diagrams.length,
        },
        sourceBlocks: context.sourceBlocks,
        ...renderResultContext(sourceHash, options),
      }
    }
    return {
      html,
      outline: context.outline,
      resources: context.resources,
      links: context.links,
      diagrams: context.diagrams,
      diagnostics: context.diagnostics,
      stats: {
        inputBytes,
        astNodes,
        headingCount: context.outline.length,
        diagramCount: context.diagrams.length,
      },
      sourceBlocks: context.sourceBlocks,
      ...renderResultContext(sourceHash, options),
    }
  } catch (error) {
    const protocolFailure = error instanceof Error && error.message === 'RENDER_SOURCE_BLOCK_PROTOCOL'
    return emptyRenderDocument(
      inputBytes,
      sourceHash,
      options,
      [diagnostic(
        protocolFailure ? 'RENDER_PROTOCOL_ERROR' : 'MARKDOWN_PARSE_FAILED',
        protocolFailure ? 'Markdown 源码映射校验失败' : 'Markdown 无法解析',
      )],
    )
  }
}

function countNodes(node: MutableNode): number {
  return 1 + (node.children?.reduce((total, child) => total + countNodes(child), 0) ?? 0)
}
