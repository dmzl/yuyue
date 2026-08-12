<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, shallowRef, useTemplateRef, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { RenderDocument, RenderLink, RenderResource } from '../markdown/renderer'
import { gateMermaidSource, sanitizeMermaidSvg } from '../markdown/mermaid'
import MediaLightbox, { type LightboxMedia } from './MediaLightbox.vue'
import ReaderSearch from './ReaderSearch.vue'
import { useLocale } from '../composables/useLocale'

const props = withDefaults(defineProps<{
  document: RenderDocument
  title: string
  documentId: string
  active: boolean
  initialScrollRatio: number
  remoteImageAuthorized: boolean
  renderGeneration?: number
  paintOperationId?: string
  errorMessage?: string
  fontScale?: number
  contentWidth?: 'comfortable' | 'wide'
  settingsOpen?: boolean
  embedded?: boolean
}>(), { renderGeneration: 0, paintOperationId: '', errorMessage: '', fontScale: 1, contentWidth: 'comfortable', settingsOpen: false, embedded: false })

const emit = defineEmits<{
  'scroll-ratio': [ratio: number]
  'authorize-remote-images': []
  'update-settings-open': [open: boolean]
  'content-painted': [operationId: string, documentId: string, generation: number]
  retry: []
  'source-block-activate': [blockId: string]
  'source-block-scroll': [blockId: string]
}>()

const MERMAID_AUTO_BUDGET_MS = 2_000
const MERMAID_DIAGRAM_TIMEOUT_MS = 8_000
const RESOURCE_PRELOAD_MARGIN = '640px 0px'

const previewRef = useTemplateRef<HTMLElement>('preview')
const catalogSidebarRef = useTemplateRef<HTMLElement>('catalog')
const readerSearchRef = useTemplateRef<InstanceType<typeof ReaderSearch>>('readerSearch')
const searchQuery = shallowRef('')
const searchMatches = shallowRef<Range[]>([])
const searchMatchIndex = shallowRef(0)
const searchOpen = shallowRef(false)
const activeHeadingId = shallowRef('')
const isReady = shallowRef(false)
const statusMessage = shallowRef('')
const enlargedMedia = shallowRef<LightboxMedia | null>(null)
const hydrationGeneration = shallowRef(0)
const lastPaintAcknowledgement = shallowRef('')
const mermaidCleanups: Array<() => void> = []
type ResourceObserverSession = { disconnect: () => void }
let resourceObserver: ResourceObserverSession | undefined
let activeRenderLeaseId = ''
let contentScrollFrame = 0
let sourceBlockElements: HTMLElement[] = []
let sourceBlockElementById = new Map<string, HTMLElement>()
let outlineHeadingElements: Array<{ id: string, element: HTMLElement }> = []
let documentElementCacheReady = false
const { currentLocale, t } = useLocale()

function refreshDocumentElementCaches(container: HTMLElement) {
  sourceBlockElements = Array.from(container.querySelectorAll<HTMLElement>('[data-md-source-block-id]'))
  sourceBlockElementById = new Map(sourceBlockElements.flatMap((element) => {
    const id = element.dataset.mdSourceBlockId
    return id ? [[id, element] as const] : []
  }))
  outlineHeadingElements = props.document.outline.flatMap((item) => {
    const element = document.getElementById(item.id)
    return element instanceof HTMLElement ? [{ id: item.id, element }] : []
  })
  documentElementCacheReady = true
}

const diagnosticMessage = computed(() => {
  const first = props.document.diagnostics[0]
  if (!first) return ''
  const localized: Record<string, 'documentTooLarge' | 'markdownAstLimit' | 'markdownRenderLimit' | 'markdownRenderFailed' | 'mermaidLimit'> = {
    MARKDOWN_TOO_LARGE: 'documentTooLarge',
    MARKDOWN_AST_LIMIT: 'markdownAstLimit',
    MARKDOWN_RENDER_LIMIT: 'markdownRenderLimit',
    MARKDOWN_PARSE_FAILED: 'markdownRenderFailed',
    MERMAID_LIMIT: 'mermaidLimit',
  }
  return localized[first.code] ? t(localized[first.code]) : first.message
})

function updateSearch(query: string) {
  searchQuery.value = query
  searchMatchIndex.value = 0
  void nextTick(refreshSearchMatches)
}

function refreshSearchMatches(revealFirstMatch = false) {
  const container = previewRef.value
  const query = searchQuery.value.trim().toLocaleLowerCase()
  if (!container || !query) {
    searchMatches.value = []
    searchMatchIndex.value = 0
    return
  }
  const matches: Range[] = []
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT)
  let current = walker.nextNode()
  while (current) {
    const value = current.nodeValue?.toLocaleLowerCase() ?? ''
    let offset = value.indexOf(query)
    while (offset >= 0) {
      const range = document.createRange()
      range.setStart(current, offset)
      range.setEnd(current, offset + query.length)
      matches.push(range)
      offset = value.indexOf(query, offset + query.length)
    }
    current = walker.nextNode()
  }
  searchMatches.value = matches
  if (revealFirstMatch && matches.length > 0) scrollToSearchMatch(0)
}

function scrollToSearchMatch(index: number) {
  const matches = searchMatches.value
  if (matches.length === 0) return
  const normalized = (index + matches.length) % matches.length
  searchMatchIndex.value = normalized
  const match = matches[normalized]
  match.startContainer.parentElement?.scrollIntoView({ behavior: 'smooth', block: 'center' })
  const selection = window.getSelection()
  selection?.removeAllRanges()
  selection?.addRange(match)
}

function nextSearchMatch() {
  scrollToSearchMatch(searchMatchIndex.value + 1)
}

function previousSearchMatch() {
  scrollToSearchMatch(searchMatchIndex.value - 1)
}

function openSearch() {
  emit('update-settings-open', false)
  searchOpen.value = true
  void nextTick(() => readerSearchRef.value?.focusSearch())
}

function getScrollRatio() {
  const container = previewRef.value
  if (!container) return 0
  const scrollHeight = container.scrollHeight - container.clientHeight
  return scrollHeight > 0 ? container.scrollTop / scrollHeight : 0
}

function handleContentScroll() {
  window.cancelAnimationFrame(contentScrollFrame)
  contentScrollFrame = window.requestAnimationFrame(emitContentScrollState)
}

function emitContentScrollState() {
  const container = previewRef.value
  if (!container) return
  emit('scroll-ratio', getScrollRatio())
  let low = 0
  let high = outlineHeadingElements.length - 1
  let currentIndex = 0
  while (low <= high) {
    const middle = (low + high) >> 1
    if (outlineHeadingElements[middle].element.offsetTop <= container.scrollTop + 72) {
      currentIndex = middle
      low = middle + 1
    } else {
      high = middle - 1
    }
  }
  const current = outlineHeadingElements[currentIndex]?.id ?? ''
  if (current && current !== activeHeadingId.value) {
    activeHeadingId.value = current
    nextTick(() => {
      const active = catalogSidebarRef.value?.querySelector('.catalog-item.active')
      active?.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
    })
  }
  const blockId = sourceBlockNearestViewportTop()
  if (blockId) emit('source-block-scroll', blockId)
}

function fallbackHeadingSourceBlockId(target?: Element | null) {
  const headingBlocks = (props.document.sourceBlocks ?? []).filter((block) => block.kind === 'heading')
  if (headingBlocks.length === 0) return ''
  const targetHeadingId = target?.closest<HTMLElement>('h1[id], h2[id], h3[id], h4[id], h5[id], h6[id]')?.id
  const outlineId = targetHeadingId || activeHeadingId.value
  const outlineIndex = props.document.outline.findIndex((item) => item.id === outlineId)
  return headingBlocks[Math.max(outlineIndex, 0)]?.id ?? headingBlocks[0]?.id ?? ''
}

function sourceBlockNearestViewportTop() {
  const container = previewRef.value
  if (!container) return ''
  if (!documentElementCacheReady) refreshDocumentElementCaches(container)
  const blocks = sourceBlockElements
  if (blocks.length === 0) return fallbackHeadingSourceBlockId()
  const threshold = container.getBoundingClientRect().top + 24
  let low = 0
  let high = blocks.length - 1
  let nearestIndex = 0
  while (low <= high) {
    const middle = (low + high) >> 1
    if (blocks[middle].getBoundingClientRect().top <= threshold) {
      nearestIndex = middle
      low = middle + 1
    } else {
      high = middle - 1
    }
  }
  const nearest = blocks[nearestIndex]
  return nearest.dataset.mdSourceBlockId ?? fallbackHeadingSourceBlockId(nearest)
}

function scrollToSourceBlock(blockId: string, behavior: 'auto' | 'smooth' = 'auto') {
  const container = previewRef.value
  if (!container || !/^sb-[a-f0-9]{8}-[0-9]{1,6}$/.test(blockId)) return false
  if (!documentElementCacheReady) refreshDocumentElementCaches(container)
  let block = sourceBlockElementById.get(blockId) ?? null
  if (!block) {
    const sourceBlocks = props.document.sourceBlocks ?? []
    const target = sourceBlocks.find((candidate) => candidate.id === blockId)
    const headings = sourceBlocks.filter((candidate) => candidate.kind === 'heading')
    const precedingHeadings = target
      ? headings.filter((candidate) => candidate.startLine <= target.startLine)
      : []
    const preceding = precedingHeadings[precedingHeadings.length - 1]
    const fallbackId = preceding?.id ?? fallbackHeadingSourceBlockId()
    block = fallbackId ? sourceBlockElementById.get(fallbackId) ?? null : null
    if (!block) {
      const headingIndex = headings.findIndex((candidate) => candidate.id === fallbackId)
      const outlineHeading = props.document.outline[Math.max(headingIndex, 0)]
      block = outlineHeading ? document.getElementById(outlineHeading.id) : null
    }
  }
  if (!block) return false
  container.scrollTo({ top: Math.max(block.offsetTop - 24, 0), behavior })
  return true
}

function scrollToHeading(id: string) {
  const heading = document.getElementById(id)
  const container = previewRef.value
  if (!heading || !container) {
    statusMessage.value = t('outlineLocationMissing')
    return
  }
  container.scrollTo({ top: Math.max(heading.offsetTop - 24, 0), behavior: 'smooth' })
  activeHeadingId.value = id
}

function restoreScrollPosition() {
  const container = previewRef.value
  if (!container) return
  const apply = () => {
    const scrollHeight = container.scrollHeight - container.clientHeight
    container.scrollTop = Math.max(0, props.initialScrollRatio * scrollHeight)
    handleContentScroll()
  }
  requestAnimationFrame(() => {
    apply()
    requestAnimationFrame(apply)
  })
}

function clearMermaidListeners() {
  while (mermaidCleanups.length > 0) mermaidCleanups.pop()?.()
}

function clearResourceObserver() {
  resourceObserver?.disconnect()
  resourceObserver = undefined
}

async function copyToClipboard(value: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value)
    return
  }
  const textarea = document.createElement('textarea')
  textarea.value = value
  textarea.setAttribute('readonly', '')
  textarea.style.position = 'fixed'
  textarea.style.opacity = '0'
  document.body.append(textarea)
  textarea.select()
  const copied = document.execCommand('copy')
  textarea.remove()
  if (!copied) throw new Error('CLIPBOARD_UNAVAILABLE')
}

function decorateCodeBlocks() {
  const container = previewRef.value
  if (!container) return
  for (const pre of Array.from(container.querySelectorAll('pre'))) {
    if (pre.dataset.codeDecorated === 'true') continue
    const code = pre.querySelector('code')
    if (!code) continue
    pre.dataset.codeDecorated = 'true'
    const toolbar = document.createElement('div')
    toolbar.className = 'code-toolbar'
    const label = document.createElement('span')
    label.className = 'code-language'
    const languageClass = Array.from(code.classList).find((name) => name.startsWith('language-'))
    label.textContent = languageClass ? t('codeLanguage', { language: languageClass.slice('language-'.length) }) : t('code')
    const copyActions = document.createElement('span')
    copyActions.className = 'code-copy-actions'
    const button = document.createElement('button')
    const copyIcon = '<svg data-icon="copy" viewBox="0 0 24 24" aria-hidden="true"><rect x="9" y="9" width="10" height="11" rx="2"></rect><path d="M15 9V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"></path></svg>'
    const copiedIcon = '<svg data-icon="check" viewBox="0 0 24 24" aria-hidden="true"><path d="m5 12.5 4.2 4.2L19 7"></path></svg>'
    let feedbackTimer: ReturnType<typeof window.setTimeout> | undefined
    const resetCopyFeedback = () => {
      if (feedbackTimer !== undefined) {
        window.clearTimeout(feedbackTimer)
        feedbackTimer = undefined
      }
      delete button.dataset.copied
      button.setAttribute('aria-label', t('copyCode'))
      button.title = t('copyCode')
      button.innerHTML = copyIcon
      copyActions.querySelector('.code-copy-feedback')?.remove()
    }
    button.type = 'button'
    button.className = 'code-copy-button'
    button.setAttribute('aria-label', t('copyCode'))
    button.title = t('copyCode')
    button.innerHTML = copyIcon
    button.addEventListener('click', async () => {
      try {
        await copyToClipboard(code.textContent ?? '')
        statusMessage.value = ''
        resetCopyFeedback()
        button.dataset.copied = 'true'
        button.setAttribute('aria-label', t('codeCopied'))
        button.title = t('codeCopied')
        button.innerHTML = copiedIcon
        copyActions.querySelector('.code-copy-feedback')?.remove()
        const feedback = document.createElement('span')
        feedback.className = 'code-copy-feedback'
        feedback.setAttribute('role', 'status')
        feedback.setAttribute('aria-live', 'polite')
        feedback.textContent = t('codeCopied')
        copyActions.append(feedback)
        feedbackTimer = window.setTimeout(() => {
          resetCopyFeedback()
        }, 1_500)
      } catch {
        resetCopyFeedback()
        statusMessage.value = t('copyCodeFailed')
      }
    })
    copyActions.append(button)
    toolbar.append(label, copyActions)
    pre.parentElement?.insertBefore(toolbar, pre)
  }
}

function showImageError(placeholder: Element, message = t('imageUnableToLoad')) {
  placeholder.className = 'markdown-image-error'
  placeholder.textContent = message
}

function createRemoteImagePrompt(placeholder: Element, resource: RenderResource) {
  placeholder.className = 'markdown-image-placeholder remote-image-placeholder'
  placeholder.textContent = ''
  const label = document.createElement('span')
  label.textContent = resource.alt ? t('remoteImageBlockedWithAlt', { alt: resource.alt }) : t('remoteImageBlocked')
  const button = document.createElement('button')
  button.type = 'button'
  button.className = 'inline-action'
  button.textContent = t('loadForCurrentTab')
  button.addEventListener('click', () => emit('authorize-remote-images'), { once: true })
  placeholder.append(label, button)
}

async function hydrateResource(resource: RenderResource, generation: number) {
  const container = previewRef.value
  const placeholder = container?.querySelector(`[data-md-resource-id="${resource.id}"]`)
  if (!placeholder || generation !== hydrationGeneration.value) return
  if (resource.kind === 'https' && !props.remoteImageAuthorized) {
    createRemoteImagePrompt(placeholder, resource)
    return
  }
  if (resource.kind === 'blocked') {
    showImageError(placeholder, t('imageSourceBlocked'))
    return
  }
  try {
    const resolved = await invoke<{ url: string }>('resolve_image_source', {
      documentId: props.documentId,
      source: resource.source,
      allowRemote: resource.kind === 'https' && props.remoteImageAuthorized,
      renderLeaseId: props.document.renderLeaseId ?? 'lease-reader-legacy',
    })
    if (generation !== hydrationGeneration.value) return
    const image = document.createElement('img')
    image.className = 'markdown-image'
    image.alt = resource.alt
    image.tabIndex = 0
    image.setAttribute('role', 'button')
    image.setAttribute('aria-label', resource.alt ? t('openImagePreviewWithAlt', { alt: resource.alt }) : t('openImagePreview'))
    image.loading = 'lazy'
    image.referrerPolicy = 'no-referrer'
    image.src = resolved.url
    image.addEventListener('error', () => showImageError(image, resource.alt ? `${t('imageUnableToLoad')}：${resource.alt}` : t('imageUnableToLoad')), { once: true })
    placeholder.replaceWith(image)
  } catch (error) {
    if (generation !== hydrationGeneration.value) return
    const code = typeof error === 'string' ? error : ''
    showImageError(placeholder, code === 'IMAGE_REMOTE_BLOCKED' ? t('remoteImageBlocked') : t('imageUnableToLoad'))
  }
}

function renderMermaidPlaceholder(placeholder: Element, source: string, diagramId: string, generation: number): Promise<'rendered' | 'failed' | 'timeout'> {
  const gate = gateMermaidSource(source)
  if (!gate.allowed) {
    placeholder.className = 'mermaid-error'
    placeholder.textContent = gate.code === 'MERMAID_LIMIT' ? t('mermaidLimit') : t('mermaidBlocked')
    return Promise.resolve('failed')
  }
  return new Promise((resolve) => {
    const frame = document.createElement('iframe')
    frame.className = 'mermaid-frame'
    frame.title = t('mermaidDiagram')
    frame.setAttribute('sandbox', 'allow-scripts')
    frame.setAttribute('referrerpolicy', 'no-referrer')
    const nonce = globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`
    const requestId = `${diagramId}-${nonce}`
    const rendererEpoch = `${generation}-${nonce}`
    let timeout: ReturnType<typeof setTimeout> | undefined
    let finished = false
    const dispose = () => {
      window.removeEventListener('message', handleMessage)
      if (timeout) clearTimeout(timeout)
    }
    const finish = (result: 'rendered' | 'failed' | 'timeout') => {
      if (finished) return
      finished = true
      dispose()
      resolve(result)
    }
    const handleMessage = (event: MessageEvent<unknown>) => {
      if (generation !== hydrationGeneration.value) return
      const data = event.data
      if (!data || typeof data !== 'object') return
      const response = data as { type?: string; nonce?: string; requestId?: string; rendererEpoch?: string; svg?: string; code?: string }
      const allowedRendererOrigin = event.origin === 'null' || import.meta.env.DEV && event.origin === window.location.origin
      if (!allowedRendererOrigin) return
      if (event.source !== frame.contentWindow) return
      if (response.type === 'ready') {
        if (response.nonce !== nonce) return
        frame.contentWindow?.postMessage({ type: 'render', nonce, requestId, rendererEpoch, source }, '*')
        return
      }
      if (response.nonce !== nonce || response.requestId !== requestId || response.rendererEpoch !== rendererEpoch) return
      if (response.type === 'failed') {
        placeholder.className = 'mermaid-error'
        placeholder.textContent = response.code === 'MERMAID_LIMIT' ? t('mermaidLimit') : t('mermaidRenderFailed')
        finish('failed')
        return
      }
      if (response.type !== 'rendered' || typeof response.svg !== 'string') return
      const safeSvg = sanitizeMermaidSvg(response.svg)
      if (!safeSvg) {
        placeholder.className = 'mermaid-error'
        placeholder.textContent = t('mermaidUnsafeOutput')
        finish('failed')
        return
      }
      const parsed = new DOMParser().parseFromString(safeSvg, 'image/svg+xml')
      const svg = parsed.documentElement
      if (!(svg instanceof SVGSVGElement)) {
        finish('failed')
        return
      }
      svg.setAttribute('data-md-diagram', 'mermaid')
      svg.setAttribute('tabindex', '0')
      svg.setAttribute('role', 'button')
      svg.setAttribute('aria-label', t('openMermaidDiagram', { title: props.title }))
      const wrapper = document.createElement('div')
      wrapper.className = 'mermaid-svg'
      wrapper.append(svg)
      placeholder.className = 'mermaid-container'
      placeholder.replaceChildren(wrapper)
      finish('rendered')
    }
    window.addEventListener('message', handleMessage)
    timeout = setTimeout(() => {
      placeholder.className = 'mermaid-error'
      placeholder.textContent = t('mermaidTimeout')
      finish('timeout')
    }, MERMAID_DIAGRAM_TIMEOUT_MS)
    mermaidCleanups.push(() => finish('failed'))
    placeholder.className = 'mermaid-container'
    placeholder.replaceChildren(frame)
    frame.src = `${import.meta.env.BASE_URL}mermaid-renderer.html?capability=${encodeURIComponent(nonce)}`
  })
}

function showMermaidContinuePrompt(generation: number, startIndex: number) {
  const firstDiagram = props.document.diagrams[startIndex]
  const firstPlaceholder = firstDiagram && previewRef.value?.querySelector(`[data-md-diagram-id="${firstDiagram.id}"]`)
  if (!firstPlaceholder || !firstDiagram) return
  const notice = document.createElement('div')
  notice.className = 'mermaid-deferred'
  const label = document.createElement('span')
  label.textContent = t('mermaidDeferred')
  const button = document.createElement('button')
  button.type = 'button'
  button.textContent = t('continueRendering')
  button.setAttribute('aria-label', t('continueMermaidRendering'))
  button.addEventListener('click', () => {
    notice.remove()
    void hydrateMermaidDiagrams(generation, startIndex)
  }, { once: true })
  notice.append(label, button)
  firstPlaceholder.parentElement?.insertBefore(notice, firstPlaceholder)
}

async function hydrateMermaidDiagrams(generation: number, startIndex = 0) {
  const startedAt = Date.now()
  for (let index = startIndex; index < props.document.diagrams.length; index += 1) {
    if (generation !== hydrationGeneration.value) return
    if (index > startIndex && Date.now() - startedAt >= MERMAID_AUTO_BUDGET_MS) {
      showMermaidContinuePrompt(generation, index)
      return
    }
    const diagram = props.document.diagrams[index]
    if (diagram.error) continue
    const placeholder = previewRef.value?.querySelector(`[data-md-diagram-id="${diagram.id}"]`)
    if (placeholder) await renderMermaidPlaceholder(placeholder, diagram.source, diagram.id, generation)
    if (generation !== hydrationGeneration.value) return
    if (index + 1 < props.document.diagrams.length && Date.now() - startedAt >= MERMAID_AUTO_BUDGET_MS) {
      showMermaidContinuePrompt(generation, index + 1)
      return
    }
  }
}

async function hydrateResources(generation: number) {
  const container = previewRef.value
  if (!container || generation !== hydrationGeneration.value) return
  clearResourceObserver()

  const ResourceObserver = window.IntersectionObserver
  if (!ResourceObserver) {
    for (const resource of props.document.resources) {
      await hydrateResource(resource, generation)
      if (generation !== hydrationGeneration.value) return
    }
    return
  }

  const resourcesByPlaceholder = new Map<Element, RenderResource>()
  let session: ResourceObserverSession | undefined
  const observer = new ResourceObserver((entries) => {
    if (generation !== hydrationGeneration.value || resourceObserver !== session) return
    for (const entry of entries) {
      if (!entry.isIntersecting) continue
      const resource = resourcesByPlaceholder.get(entry.target)
      observer.unobserve(entry.target)
      resourcesByPlaceholder.delete(entry.target)
      if (resource) void hydrateResource(resource, generation)
    }
  }, {
    root: container,
    rootMargin: RESOURCE_PRELOAD_MARGIN,
    threshold: 0,
  })
  session = {
    disconnect() {
      observer.disconnect()
      resourcesByPlaceholder.clear()
    },
  }
  resourceObserver = session

  for (const resource of props.document.resources) {
    if (generation !== hydrationGeneration.value) return
    const placeholder = container.querySelector(`[data-md-resource-id="${resource.id}"]`)
    if (!placeholder) continue
    resourcesByPlaceholder.set(placeholder, resource)
    observer.observe(placeholder)
  }
}

async function hydrateDeferredContent(generation: number) {
  const container = previewRef.value
  if (!container || generation !== hydrationGeneration.value) return
  container.querySelectorAll('.mermaid-deferred').forEach((notice) => notice.remove())
  decorateCodeBlocks()
  await hydrateResources(generation)
  if (generation !== hydrationGeneration.value) return
  await hydrateMermaidDiagrams(generation)
}

async function hydrateContent() {
  const generation = hydrationGeneration.value + 1
  hydrationGeneration.value = generation
  clearMermaidListeners()
  clearResourceObserver()
  await nextTick()
  if (!previewRef.value || generation !== hydrationGeneration.value) return
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
  if (!previewRef.value || generation !== hydrationGeneration.value) return
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
  if (!previewRef.value || generation !== hydrationGeneration.value) return
  const previewArea = previewRef.value.querySelector('.preview-area')
  refreshDocumentElementCaches(previewRef.value)
  const paintKey = `${props.paintOperationId}:${props.documentId}:${props.renderGeneration}`
  const hasCommittedContent = props.document.html.length === 0 || Boolean(previewArea?.childNodes.length)
  if (
    props.active &&
    props.paintOperationId &&
    hasCommittedContent &&
    lastPaintAcknowledgement.value !== paintKey
  ) {
    lastPaintAcknowledgement.value = paintKey
    emit('content-painted', props.paintOperationId, props.documentId, props.renderGeneration)
  }
  isReady.value = true
  const nextLease = props.document.renderLeaseId ?? ''
  if (activeRenderLeaseId && activeRenderLeaseId !== nextLease) {
    void invoke('release_render_lease', {
      documentId: props.documentId,
      renderLeaseId: activeRenderLeaseId,
    }).catch(() => {})
  }
  activeRenderLeaseId = nextLease
  restoreScrollPosition()
  refreshSearchMatches()
  requestAnimationFrame(() => {
    void hydrateDeferredContent(generation)
  })
}

function openMediaFromTarget(target: Element) {
  const image = target.closest('img.markdown-image')
  if (image instanceof HTMLImageElement && (image.currentSrc || image.src)) {
    enlargedMedia.value = { kind: 'image', src: image.currentSrc || image.src, alt: image.alt }
    return true
  }
  const svg = target.closest('svg[data-md-diagram="mermaid"]')
  if (svg instanceof SVGSVGElement) {
    enlargedMedia.value = { kind: 'svg', element: svg, alt: props.title }
    return true
  }
  return false
}

function handlePreviewClick(event: MouseEvent) {
  const target = event.target
  if (!(target instanceof Element)) return
  const linkElement = target.closest('a[data-md-link-id]')
  if (linkElement) {
    event.preventDefault()
    const linkId = linkElement.getAttribute('data-md-link-id')
    const link = props.document.links.find((item) => item.id === linkId)
    if (link) void handleLink(link)
    return
  }
  openMediaFromTarget(target)
}

function handlePreviewDoubleClick(event: MouseEvent) {
  if (!props.embedded) return
  const target = event.target
  if (!(target instanceof Element)) return
  const block = target.closest<HTMLElement>('[data-md-source-block-id]')
  const blockId = block?.dataset.mdSourceBlockId ?? fallbackHeadingSourceBlockId(target)
  if (blockId) emit('source-block-activate', blockId)
}

function handlePreviewKeydown(event: KeyboardEvent) {
  if (event.key !== 'Enter' && event.key !== ' ') return
  const target = event.target
  if (!(target instanceof Element)) return
  if (!target.closest('img.markdown-image, svg[data-md-diagram="mermaid"]')) return
  event.preventDefault()
  openMediaFromTarget(target)
}

async function handleLink(link: RenderLink) {
  if (link.kind === 'internal') {
    scrollToHeading(link.url.slice(1))
  } else if (link.kind === 'external') {
    try {
      await invoke('open_external_url', { url: link.url })
    } catch {
      statusMessage.value = t('externalLinkFailed')
    }
  } else {
    statusMessage.value = t('linkTypeBlocked')
  }
}

function handleShortcut(event: KeyboardEvent) {
  if ((event.metaKey || event.ctrlKey) && event.key.toLocaleLowerCase() === 'f') {
    event.preventDefault()
    openSearch()
  } else if (event.key === 'Escape' && searchOpen.value) {
    searchOpen.value = false
  } else if (event.key === 'Escape' && props.settingsOpen) {
    emit('update-settings-open', false)
  }
}

watch([() => props.document, () => props.paintOperationId, () => props.renderGeneration], () => {
  isReady.value = false
  documentElementCacheReady = false
  activeHeadingId.value = props.document.outline[0]?.id ?? ''
  void hydrateContent()
})

watch(() => props.remoteImageAuthorized, () => void hydrateContent())
watch(currentLocale, () => {
  if (!previewRef.value) return
  documentElementCacheReady = false
  previewRef.value.innerHTML = props.document.html
  void hydrateContent()
})
onMounted(() => {
  window.addEventListener('keydown', handleShortcut)
  activeHeadingId.value = props.document.outline[0]?.id ?? ''
  void hydrateContent()
})

onUnmounted(() => {
  window.cancelAnimationFrame(contentScrollFrame)
  hydrationGeneration.value += 1
  clearMermaidListeners()
  clearResourceObserver()
  sourceBlockElements = []
  sourceBlockElementById.clear()
  outlineHeadingElements = []
  documentElementCacheReady = false
  window.removeEventListener('keydown', handleShortcut)
  window.getSelection()?.removeAllRanges()
  if (activeRenderLeaseId) {
    void invoke('release_render_lease', {
      documentId: props.documentId,
      renderLeaseId: activeRenderLeaseId,
    }).catch(() => {})
  }
})

defineExpose({ scrollToHeading, scrollToSourceBlock })
</script>

<template>
  <div class="reading-mode" :class="{ 'reading-mode-embedded': props.embedded }">
    <ReaderSearch
      ref="readerSearch"
      :open="searchOpen"
      :search-query="searchQuery"
      :search-match-count="searchMatches.length"
      :search-match-index="searchMatchIndex"
      @close="searchOpen = false"
      @update-search="updateSearch"
      @search-next="nextSearchMatch"
      @search-previous="previousSearchMatch"
    />
    <div class="reader-body">
      <aside v-if="!props.embedded && isReady && props.document.outline.length > 0" ref="catalog" class="catalog-sidebar" :aria-label="t('outline')">
        <div class="catalog-title">{{ t('outline') }}</div>
        <nav class="catalog-list" :aria-label="t('headingNavigation')">
          <button
            v-for="item in props.document.outline"
            :key="item.id"
            type="button"
            :class="['catalog-item', `catalog-item-level-${item.level}`, { active: activeHeadingId === item.id }]"
            @click="scrollToHeading(item.id)"
          >
            {{ item.text }}
          </button>
        </nav>
      </aside>
      <div ref="preview" class="reading-content" @scroll="handleContentScroll">
        <div
          class="preview-area"
          :class="{ 'preview-area-wide': props.contentWidth === 'wide' }"
          :style="{ fontSize: `${props.fontScale}rem` }"
          @click="handlePreviewClick"
          @dblclick="handlePreviewDoubleClick"
          @keydown="handlePreviewKeydown"
          v-html="props.document.html"
        ></div>
      </div>
    </div>

    <div v-if="props.errorMessage || diagnosticMessage || statusMessage" class="reader-status" :class="{ 'reader-error': props.errorMessage }" :role="props.errorMessage ? 'alert' : 'status'" aria-live="polite">
      <span class="status-dot" aria-hidden="true"></span>
      <span>{{ props.errorMessage || statusMessage || diagnosticMessage }}</span>
      <button v-if="props.errorMessage" type="button" :aria-label="t('retryDocument')" @click="emit('retry')">{{ t('retry') }}</button>
    </div>

    <MediaLightbox :media="enlargedMedia" @close="enlargedMedia = null" />
  </div>
</template>

<style scoped>
.reading-mode { position: relative; display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-primary); }
.reading-mode-embedded .reading-content { padding: 22px 28px 48px; }
.reading-mode-embedded .preview-area :deep([data-md-source-block-id]) { border-radius: 5px; }
.reading-mode-embedded .preview-area :deep([data-md-source-block-id]:hover) { background: color-mix(in srgb, var(--tool-btn-hover-bg) 52%, transparent); }
.reader-body { display: flex; flex: 1; min-height: 0; }
.reading-content { min-width: 0; flex: 1; overflow-y: auto; padding: 28px 40px 48px; overscroll-behavior: contain; scrollbar-width: thin; scrollbar-color: transparent transparent; }
.reading-content:hover, .reading-content:focus-within { scrollbar-color: rgba(118, 126, 141, 0.42) transparent; }
.reading-content::-webkit-scrollbar, .catalog-sidebar::-webkit-scrollbar { width: 10px; height: 10px; }
.reading-content::-webkit-scrollbar-track, .catalog-sidebar::-webkit-scrollbar-track { background: transparent; }
.reading-content::-webkit-scrollbar-thumb, .catalog-sidebar::-webkit-scrollbar-thumb { border: 3px solid transparent; border-radius: 999px; background: transparent; background-clip: content-box; }
.reading-content:hover::-webkit-scrollbar-thumb, .reading-content:focus-within::-webkit-scrollbar-thumb, .catalog-sidebar:hover::-webkit-scrollbar-thumb, .catalog-sidebar:focus-within::-webkit-scrollbar-thumb { background-color: rgba(118, 126, 141, 0.42); }
.preview-area { max-width: 800px; margin: 0 auto; color: var(--text-primary); line-height: 1.82; }
.preview-area-wide { max-width: none; }
.preview-area :deep(h1), .preview-area :deep(h2), .preview-area :deep(h3), .preview-area :deep(h4), .preview-area :deep(h5), .preview-area :deep(h6) { color: var(--text-heading); scroll-margin-top: 24px; }
.preview-area :deep(h1) { margin: 0 0 28px; padding-bottom: 14px; border-bottom: 1px solid var(--border-color); font-size: 2.2em; line-height: 1.25; }
.preview-area :deep(h2) { margin: 36px 0 16px; padding-top: 12px; border-top: 1px solid var(--border-color); font-size: 1.55em; line-height: 1.3; }
.preview-area :deep(h3) { margin: 28px 0 12px; font-size: 1.28em; line-height: 1.35; }
.preview-area :deep(h4) { margin: 24px 0 10px; font-size: 1.1em; }
.preview-area :deep(h5), .preview-area :deep(h6) { margin: 20px 0 8px; font-size: 1em; }
.preview-area :deep(p) { margin: 0 0 17px; color: var(--text-secondary); }
.preview-area :deep(ul), .preview-area :deep(ol) { margin: 0 0 18px; padding-left: 26px; color: var(--text-secondary); }
.preview-area :deep(li) { margin: 5px 0; }
.preview-area :deep(.contains-task-list) { list-style: none; padding-left: 0; }
.preview-area :deep(input[type="checkbox"]) { width: 1em; height: 1em; margin: 0 7px 0 0; vertical-align: -0.1em; accent-color: #4c6ef5; }
.preview-area :deep(blockquote) { margin: 20px 0; padding: 12px 18px; border-left: 3px solid #4c6ef5; border-radius: 0 8px 8px 0; background: var(--bg-secondary); color: var(--text-secondary); }
.preview-area :deep(.markdown-alert) { border-left-color: #4c6ef5; }
.preview-area :deep(.markdown-alert-warning), .preview-area :deep(.markdown-alert-caution) { border-left-color: #c98b24; }
.preview-area :deep(.markdown-alert-important) { border-left-color: #9a5cc7; }
.preview-area :deep(.markdown-alert-tip) { border-left-color: #2f9e78; }
.preview-area :deep(a) { color: #4c6ef5; text-decoration: underline; text-decoration-color: rgba(76, 110, 245, 0.35); text-underline-offset: 3px; cursor: pointer; }
.preview-area :deep(a:hover) { text-decoration-color: currentColor; }
.preview-area :deep(.markdown-link-blocked) { color: var(--text-muted); text-decoration-style: dotted; }
.preview-area :deep(code) { padding: 2px 5px; border-radius: 5px; color: var(--code-color); background: var(--code-bg); font-family: 'SFMono-Regular', Menlo, Monaco, Consolas, monospace; font-size: 0.88em; }
.preview-area :deep(pre) { margin: 20px 0; padding: 17px; overflow-x: auto; border: 1px solid rgba(255, 255, 255, 0.04); border-radius: 9px; color: var(--pre-color); background: var(--pre-bg); line-height: 1.55; tab-size: 2; scrollbar-width: thin; scrollbar-color: transparent transparent; }
.preview-area :deep(pre:hover) { scrollbar-color: rgba(202, 210, 228, 0.5) transparent; }
.preview-area :deep(table:hover) { scrollbar-color: rgba(151, 160, 182, 0.5) transparent; }
.preview-area :deep(.katex-display:hover) { scrollbar-color: rgba(151, 160, 182, 0.5) transparent; }
.preview-area :deep(pre::-webkit-scrollbar), .preview-area :deep(table::-webkit-scrollbar), .preview-area :deep(.katex-display::-webkit-scrollbar) { width: 10px; height: 10px; }
.preview-area :deep(pre::-webkit-scrollbar-track), .preview-area :deep(table::-webkit-scrollbar-track), .preview-area :deep(.katex-display::-webkit-scrollbar-track) { background: transparent; }
.preview-area :deep(pre::-webkit-scrollbar-thumb), .preview-area :deep(table::-webkit-scrollbar-thumb), .preview-area :deep(.katex-display::-webkit-scrollbar-thumb) { border: 3px solid transparent; border-radius: 999px; background: transparent; background-clip: content-box; }
.preview-area :deep(pre:hover::-webkit-scrollbar-thumb), .preview-area :deep(table:hover::-webkit-scrollbar-thumb), .preview-area :deep(.katex-display:hover::-webkit-scrollbar-thumb) { background-color: rgba(151, 160, 182, 0.5); }
.preview-area :deep(.code-toolbar) { display: flex; align-items: center; justify-content: space-between; gap: 10px; min-height: 30px; margin: 20px 0 0; padding: 5px 8px 0; border: 1px solid var(--border-color); border-bottom: 0; border-radius: 9px 9px 0 0; color: var(--text-muted); background: var(--code-bg); font-size: 11px; }
.preview-area :deep(.code-toolbar + pre) { margin-top: 0; border-top-left-radius: 0; border-top-right-radius: 0; }
.preview-area :deep(.code-copy-button), .preview-area :deep(.mermaid-deferred button) { padding: 4px 8px; border: 1px solid var(--border-color); border-radius: 5px; color: var(--text-secondary); background: var(--bg-primary); cursor: pointer; font: inherit; font-size: 11px; }
.preview-area :deep(.code-copy-button) { display: grid; width: 27px; height: 27px; place-items: center; padding: 0; }
.preview-area :deep(.code-copy-actions) { display: inline-flex; align-items: center; gap: 6px; }
.preview-area :deep(.code-copy-button svg) { width: 15px; height: 15px; fill: none; stroke: currentColor; stroke-linecap: round; stroke-linejoin: round; stroke-width: 1.8; }
.preview-area :deep(.code-copy-button[data-copied="true"]) { color: #2f9e78; }
.preview-area :deep(.code-copy-feedback) { display: inline-flex; align-items: center; min-height: 22px; padding: 0 6px; border-radius: 5px; color: #26765d; background: rgba(47, 158, 120, 0.12); font-size: 10px; font-weight: 650; white-space: nowrap; }
.preview-area :deep(.code-copy-button:hover), .preview-area :deep(.code-copy-button:focus-visible), .preview-area :deep(.mermaid-deferred button:hover), .preview-area :deep(.mermaid-deferred button:focus-visible) { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 2px solid rgba(76, 110, 245, 0.24); outline-offset: 1px; }
.preview-area :deep(pre code) { padding: 0; color: inherit; background: transparent; font-size: 0.86em; }
.preview-area :deep(table) { display: block; width: 100%; margin: 20px 0; overflow-x: auto; border-collapse: collapse; white-space: nowrap; scrollbar-width: thin; scrollbar-color: transparent transparent; }
.preview-area :deep(th), .preview-area :deep(td) { min-width: 100px; padding: 9px 13px; border: 1px solid var(--border-color); text-align: left; }
.preview-area :deep(th) { color: var(--text-primary); background: var(--table-header-bg); font-weight: 650; }
.preview-area :deep(td) { color: var(--text-secondary); }
.preview-area :deep(.markdown-image-placeholder), .preview-area :deep(.markdown-image-error) { display: inline-flex; align-items: center; gap: 10px; min-height: 38px; margin: 6px 0; padding: 8px 12px; border: 1px dashed var(--border-color); border-radius: 8px; color: var(--text-muted); background: var(--bg-secondary); font-size: 0.85em; }
.preview-area :deep(.markdown-image-error) { border-color: rgba(194, 65, 59, 0.35); color: #b33a35; }
.preview-area :deep(.inline-action) { padding: 5px 9px; border: 1px solid rgba(76, 110, 245, 0.36); border-radius: 6px; color: #4c6ef5; background: transparent; cursor: pointer; font: inherit; font-size: 0.92em; }
.preview-area :deep(.inline-action:hover), .preview-area :deep(.inline-action:focus-visible) { background: rgba(76, 110, 245, 0.12); outline: 0; }
.preview-area :deep(.markdown-image) { display: block; max-width: 100%; margin: 16px 0; border-radius: 8px; cursor: zoom-in; }
.preview-area :deep(.markdown-image:hover) { box-shadow: 0 8px 24px rgba(26, 26, 46, 0.14); }
.preview-area :deep(.markdown-image:focus-visible), .preview-area :deep(.mermaid-svg svg:focus-visible) { outline: 2px solid #4c6ef5; outline-offset: 4px; }
.preview-area :deep(.mermaid-container) { margin: 24px 0; padding: 14px; overflow: hidden; border: 1px solid var(--border-color); border-radius: 9px; text-align: center; background: var(--bg-secondary); }
.preview-area :deep(.mermaid-deferred) { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin: 20px 0 8px; padding: 10px 12px; border: 1px dashed var(--border-color); border-radius: 8px; color: var(--text-muted); background: var(--bg-secondary); font-size: 12px; }
.preview-area :deep(.mermaid-frame) { display: block; width: 100%; min-height: 220px; border: 0; background: transparent; }
.preview-area :deep(.mermaid-svg) { display: flex; justify-content: center; }
.preview-area :deep(.mermaid-svg svg) { max-width: 100%; height: auto; cursor: zoom-in; }
.preview-area :deep(.mermaid-error) { margin: 20px 0; padding: 14px; border: 1px solid rgba(194, 65, 59, 0.3); border-radius: 8px; color: #b33a35; background: rgba(194, 65, 59, 0.08); }
.preview-area :deep(.katex-display) { overflow-x: auto; padding: 8px 0; }
.catalog-sidebar { width: 232px; flex-shrink: 0; padding: 28px 10px 28px 14px; overflow-y: auto; border-right: 1px solid var(--border-color); background: var(--bg-primary); scrollbar-width: thin; scrollbar-color: transparent transparent; }
.catalog-sidebar:hover, .catalog-sidebar:focus-within { scrollbar-color: rgba(118, 126, 141, 0.42) transparent; }
.catalog-title { margin: 0 10px 12px; color: var(--text-muted); font-size: 11px; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; }
.catalog-list { display: flex; flex-direction: column; gap: 3px; }
.catalog-item { width: 100%; padding: 7px 10px; overflow: hidden; border: 0; border-radius: 6px; color: var(--text-secondary); background: transparent; cursor: pointer; font: inherit; font-size: 12px; text-align: left; text-overflow: ellipsis; white-space: nowrap; }
.catalog-item:hover, .catalog-item:focus-visible { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 0; }
.catalog-item.active { color: #fff; background: #4c6ef5; }
.catalog-item-level-1 { color: var(--text-primary); font-weight: 650; }
.catalog-item-level-2 { padding-left: 20px; }
.catalog-item-level-3 { padding-left: 30px; font-size: 11px; }
.catalog-item-level-4 { padding-left: 40px; font-size: 11px; }
.reader-status { display: flex; align-items: center; gap: 8px; min-height: 32px; padding: 6px 20px; border-top: 1px solid var(--border-color); color: var(--text-muted); background: var(--bg-secondary); font-size: 11px; }
.reader-status.reader-error { color: #b33a35; background: rgba(194, 65, 59, 0.08); }
.reader-status button { margin-left: auto; padding: 4px 9px; border: 1px solid currentColor; border-radius: 5px; color: inherit; background: transparent; cursor: pointer; font: inherit; }
.reader-status button:focus-visible { outline: 2px solid #4c6ef5; outline-offset: 2px; }
.status-dot { width: 6px; height: 6px; flex-shrink: 0; border-radius: 50%; background: #c98b24; }

@media print {
  @page { margin: 18mm 16mm 20mm; }

  .reading-mode, .reader-body, .reading-content { height: auto !important; min-height: 0; overflow: visible !important; }
  .catalog-sidebar, .reader-search-layer, .reader-settings-layer, .reader-status { display: none !important; }
  .reading-content { padding: 0 !important; }
  .preview-area {
    max-width: none !important;
    margin: 0 !important;
    color: #111 !important;
    font-size: 10.5pt !important;
    line-height: 1.62 !important;
    -webkit-print-color-adjust: exact;
    print-color-adjust: exact;
  }
  .preview-area :deep(h1), .preview-area :deep(h2), .preview-area :deep(h3), .preview-area :deep(h4), .preview-area :deep(h5), .preview-area :deep(h6), .preview-area :deep(p), .preview-area :deep(li), .preview-area :deep(th), .preview-area :deep(td), .preview-area :deep(blockquote) { color: #111 !important; }
  .preview-area :deep(h1), .preview-area :deep(h2), .preview-area :deep(h3), .preview-area :deep(h4), .preview-area :deep(h5), .preview-area :deep(h6) { break-after: avoid-page; break-inside: avoid-page; }
  .preview-area :deep(h1) { margin: 0 0 12pt; padding-bottom: 8pt; font-size: 22pt; }
  .preview-area :deep(h2) { margin: 24pt 0 10pt; padding-top: 8pt; font-size: 16pt; }
  .preview-area :deep(h3) { margin: 18pt 0 8pt; font-size: 13pt; }
  .preview-area :deep(h4) { margin: 15pt 0 6pt; font-size: 11.5pt; }
  .preview-area :deep(h5), .preview-area :deep(h6) { margin: 12pt 0 5pt; font-size: 10.5pt; }
  .preview-area :deep(p) { margin: 0 0 9pt; }
  .preview-area :deep(ul), .preview-area :deep(ol) { margin: 0 0 9pt; padding-left: 18pt; }
  .preview-area :deep(li) { margin: 2pt 0; }
  .preview-area :deep(blockquote) { background: #fff !important; }
  .preview-area :deep(th) { background: #f2f2f2 !important; }
  .preview-area :deep(pre) { overflow: visible !important; border-color: #d6d6d6 !important; color: #111 !important; background: #f5f5f5 !important; font-size: 8.5pt; white-space: pre-wrap; overflow-wrap: anywhere; }
  .preview-area :deep(pre code) { color: inherit !important; }
  .preview-area :deep(table) { display: table !important; table-layout: auto; white-space: normal; font-size: 9pt; break-inside: auto; }
  .preview-area :deep(th), .preview-area :deep(td) { min-width: 0; padding: 5pt 6pt; white-space: normal; overflow-wrap: anywhere; }
  .preview-area :deep(tr) { break-inside: avoid; }
  .preview-area :deep(.markdown-image), .preview-area :deep(.mermaid-svg svg) { max-width: 100% !important; height: auto !important; }
  .preview-area :deep(.code-toolbar), .preview-area :deep(.code-copy-button) { display: none !important; }
  .preview-area :deep(.mermaid-container), .preview-area :deep(pre), .preview-area :deep(blockquote) { break-inside: avoid; }
}

@media (max-width: 900px) {
  .reading-content { padding-inline: 22px; }
  .catalog-sidebar { width: 190px; }
}
@media (max-width: 720px) {
  .reading-content { padding: 20px 14px 36px; }
  .catalog-sidebar { display: none; }
}
</style>
