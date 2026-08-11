<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, shallowRef, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { listen } from '@tauri-apps/api/event'
import { useTheme } from './composables/useTheme'
import { useMarkdown } from './composables/useMarkdown'
import { useLocale, type MessageKey } from './composables/useLocale'
import { useDocumentProcessing, type DocumentImportStartedEvent } from './composables/useDocumentProcessing'
import type { RenderDocument } from './markdown/renderer'
import type { RenderResult } from './markdown/workerClient'
import ReadingMode from './components/ReadingMode.vue'
import DocumentProcessingNotice from './components/DocumentProcessingNotice.vue'
import ReaderSettings from './components/ReaderSettings.vue'
import TabBar, { type Tab } from './components/TabBar.vue'
import yuyueIcon from './assets/yuyue-icon.png'

interface DocumentPayload {
  documentId: string
  fileName: string
  content: string
  sourceRevision: number
}

interface FileChangedEvent {
  documentId: string
  sourceRevision: number
}

interface DocumentOpenedEvent {
  operationId: string
  sequence: number
  document: DocumentPayload
}

interface DocumentImportErrorEvent {
  operationId: string
  sequence: number
  code: string
}

interface PendingImport {
  kind: 'opened' | 'error'
  operationId: string
  sequence: number
  fileName: string
  document?: DocumentPayload
  code?: string
}

interface DocumentErrorEvent {
  documentId?: string
  code: string
}

interface PrintErrorEvent {
  code: string
}

const { themeIcon, themeButtonLabel, themeStatus, toggleTheme } = useTheme()
const { currentLocale, t } = useLocale()
const { render, prepare: prepareMarkdown, cancel: cancelRender, dispose: disposeMarkdown } = useMarkdown()
const tabs = ref<Tab[]>([])
const activeTabId = shallowRef('')
const isDragOver = shallowRef(false)
const appError = shallowRef('')
let shellActivationIntent = 0
const processing = useDocumentProcessing((_operation, wasLatest) => {
  if (wasLatest) appError.value = humanizeError('RENDER_TIMEOUT')
})
const readerSettingsOpen = shallowRef(false)
const readerFontScale = shallowRef(1)
const readerContentWidth = shallowRef<'comfortable' | 'wide'>('comfortable')
const reloadTimers = new Map<string, ReturnType<typeof setTimeout>>()
const reloadRevisions = new Map<string, number>()
const RENDER_DEADLINE_MS = 15_000
const MAX_RETAINED_SOURCE_BYTES = 64 * 1024 * 1024
const MAX_RETAINED_RESULT_BYTES = 128 * 1024 * 1024
let retainedSourceBytes = 0
let retainedResultBytes = 0
let retentionSequence = 0
let unlistenDragDrop: (() => void) | null = null
let unlistenDocumentOpened: (() => void) | null = null
let unlistenImportStarted: (() => void) | null = null
let unlistenImportError: (() => void) | null = null
let unlistenDocumentError: (() => void) | null = null
let unlistenFileChanged: (() => void) | null = null
let unlistenPrintError: (() => void) | null = null
let unlistenExportPdf: (() => void) | null = null

const hasTabs = computed(() => tabs.value.length > 0)
const activeTab = computed(() => tabs.value.find((tab) => tab.id === activeTabId.value) ?? null)
const activeDocument = computed(() => activeTab.value?.document ?? emptyDocument(t('renderingDocument')))
const hasRemoteImages = computed(() => activeDocument.value.resources.some((resource) => resource.kind === 'https'))
const remoteImagesAuthorized = computed(() => activeTab.value?.remoteImageAuthorized ?? false)
const processingNotice = processing.visibleOperation
const processingMessage = computed(() => processingNotice.value?.phase === 'importing'
  ? t('importingDocument')
  : t('renderingDocument'))
const processingPhase = computed<'importing' | 'rendering' | 'awaiting-paint'>(() => {
  if (processingNotice.value?.phase === 'importing') return 'importing'
  if (processingNotice.value?.phase === 'awaiting-paint') return 'awaiting-paint'
  return 'rendering'
})

function emptyDocument(message: string): RenderDocument {
  return {
    html: `<p class="document-loading-message">${message}</p>`,
    outline: [],
    resources: [],
    links: [],
    diagrams: [],
    diagnostics: [],
    stats: { inputBytes: 0, astNodes: 0, headingCount: 0, diagramCount: 0 },
  }
}

function byteLength(value: string) {
  return new TextEncoder().encode(value).byteLength
}

function touchTab(tab: Tab) {
  tab.lastAccess = ++retentionSequence
}

function setTabSource(tab: Tab, source: string | null) {
  retainedSourceBytes -= tab.sourceBytes
  tab.sourceContent = source
  tab.sourceBytes = source ? byteLength(source) : 0
  retainedSourceBytes += tab.sourceBytes
}

function setTabDocument(tab: Tab, document: RenderDocument | null, renderedBytes = 0) {
  retainedResultBytes -= tab.renderedBytes
  tab.document = document
  tab.renderedBytes = document ? renderedBytes : 0
  retainedResultBytes += tab.renderedBytes
}

function releaseTabRetention(tab: Tab) {
  setTabSource(tab, null)
  setTabDocument(tab, null)
}

function inactiveTabsOldest(predicate: (tab: Tab) => boolean) {
  return tabs.value
    .filter((tab) => tab.id !== activeTabId.value && predicate(tab))
    .sort((left, right) => left.lastAccess - right.lastAccess)
}

function enforceRetentionBudgets() {
  for (const tab of inactiveTabsOldest((candidate) => candidate.document !== null)) {
    if (retainedResultBytes <= MAX_RETAINED_RESULT_BYTES) break
    setTabDocument(tab, null)
  }
  for (const tab of inactiveTabsOldest((candidate) => candidate.sourceContent !== null)) {
    if (retainedSourceBytes <= MAX_RETAINED_SOURCE_BYTES) break
    setTabSource(tab, null)
  }
}

function humanizeError(code: string) {
  const messages: Record<string, MessageKey> = {
    DOCUMENT_OPEN_FAILED: 'documentOpenFailed', DOCUMENT_UNSUPPORTED_FORMAT: 'unsupportedFormat', DOCUMENT_TOO_LARGE: 'documentTooLarge',
    DOCUMENT_READ_FAILED: 'documentReadFailed', DOCUMENT_IDENTITY_CHANGED: 'documentIdentityChanged', DOCUMENT_NOT_UTF8: 'documentNotUtf8',
    DOCUMENT_TAB_LIMIT: 'documentTabLimit', DOCUMENT_WATCH_FAILED: 'documentWatchFailed', MARKDOWN_AST_LIMIT: 'markdownAstLimit',
    MARKDOWN_RENDER_LIMIT: 'markdownRenderLimit', MARKDOWN_RENDER_FAILED: 'markdownRenderFailed', RENDER_TIMEOUT: 'renderTimeout',
    RENDER_CRASH: 'renderCrash', RENDER_QUEUE_LIMIT: 'renderQueueLimit', RENDER_PROTOCOL_ERROR: 'renderProtocolError', RENDER_CANCELLED: 'renderCancelled',
  }
  return t(messages[code] ?? 'documentProcessingFailed')
}

async function renderIntoTab(
  tab: Tab,
  payload: DocumentPayload,
  preserveError = false,
  operationId?: string,
) {
  const generation = (tab.renderGeneration ?? 0) + 1
  tab.renderGeneration = generation
  if (tab.paintOperationId && tab.paintOperationId !== operationId) {
    processing.cancel(tab.paintOperationId)
  }
  tab.paintOperationId = undefined
  if (operationId) processing.beginRender(operationId, payload.documentId, generation)
  setTabSource(tab, payload.content)
  touchTab(tab)
  tab.isRendering = true
  const previousDocument = tab.document
  if (!previousDocument) setTabDocument(tab, emptyDocument(t('renderingDocument')))
  let deadlineTimer: ReturnType<typeof setTimeout> | undefined
  try {
    const result = await Promise.race([
      render(payload.content, payload.documentId, tab.id === activeTabId.value ? 1 : 0),
      new Promise<RenderResult>((_, reject) => {
        deadlineTimer = setTimeout(() => reject({ code: 'RENDER_TIMEOUT' }), RENDER_DEADLINE_MS)
      }),
    ])
    if (!tabs.value.some((item) => item.id === tab.id) || tab.renderGeneration !== generation) {
      if (operationId) processing.cancel(operationId)
      return
    }
    setTabDocument(tab, result.document, result.serializedBytes)
    tab.sourceRevision = payload.sourceRevision
    if (!preserveError) tab.error = undefined
    enforceRetentionBudgets()
    if (operationId) {
      if (tab.id === activeTabId.value) {
        tab.paintOperationId = operationId
        processing.awaitPaint(operationId, payload.documentId, generation)
      } else {
        processing.finishBackground(operationId, payload.documentId, generation)
      }
    }
  } catch (error) {
    if (!tabs.value.some((item) => item.id === tab.id) || tab.renderGeneration !== generation) {
      if (operationId) processing.cancel(operationId)
      return
    }
    const code = typeof error === 'object' && error && 'code' in error ? String(error.code) : 'MARKDOWN_RENDER_FAILED'
    if (!previousDocument) setTabDocument(tab, emptyDocument(humanizeError(code)))
    tab.error = humanizeError(code)
    if (tab.id === activeTabId.value) appError.value = tab.error
    if (operationId) processing.fail(operationId)
  } finally {
    if (deadlineTimer) clearTimeout(deadlineTimer)
    if (tab.renderGeneration === generation) tab.isRendering = false
  }
}

async function ensureTabRendered(tab: Tab) {
  touchTab(tab)
  if (tab.document || tab.isRendering) return
  if (tab.sourceContent !== null) {
    await renderIntoTab(tab, {
      documentId: tab.documentId,
      fileName: tab.fileName,
      content: tab.sourceContent,
      sourceRevision: tab.sourceRevision,
    })
  } else {
    await reloadDocument(tab.documentId, tab.sourceRevision)
  }
}

function bindRenderedOperation(tab: Tab, operationId: string) {
  const generation = tab.renderGeneration
  if (!processing.beginRender(operationId, tab.documentId, generation)) return
  if (tab.id === activeTabId.value) {
    tab.paintOperationId = operationId
    processing.awaitPaint(operationId, tab.documentId, generation)
  } else {
    processing.finishBackground(operationId, tab.documentId, generation)
  }
}

async function activateTab(tabId: string, ensureRendered = false) {
  const tab = tabs.value.find((item) => item.id === tabId)
  if (!tab) return false
  activeTabId.value = tab.id
  appError.value = ''
  if (ensureRendered) await ensureTabRendered(tab)
  return true
}

async function addDocument(payload: DocumentPayload, opened?: DocumentOpenedEvent) {
  const operationId = opened?.operationId
  const shouldActivate = !operationId || processing.shouldActivate(operationId, shellActivationIntent)
  const existing = tabs.value.find((tab) => tab.documentId === payload.documentId)
  if (existing) {
    if (shouldActivate && !await activateTab(existing.id)) return
    const sourceChanged = existing.sourceContent !== payload.content
    setTabSource(existing, payload.content)
    touchTab(existing)
    if (sourceChanged || payload.sourceRevision > existing.sourceRevision || existing.isRendering || !existing.document) {
      await renderIntoTab(existing, payload, false, operationId)
    } else if (operationId) {
      bindRenderedOperation(existing, operationId)
    } else {
      await ensureTabRendered(existing)
    }
    return
  }
  const tab: Tab = {
    id: payload.documentId,
    documentId: payload.documentId,
    fileName: payload.fileName,
    sourceRevision: payload.sourceRevision,
    document: null,
    sourceContent: null,
    sourceBytes: 0,
    renderedBytes: 0,
    lastAccess: 0,
    isRendering: false,
    renderGeneration: 0,
    scrollRatio: 0,
    remoteImageAuthorized: false,
  }
  tabs.value.push(tab)
  const reactiveTab = tabs.value[tabs.value.length - 1]
  if ((shouldActivate || !activeTabId.value) && !await activateTab(reactiveTab.id)) return
  await renderIntoTab(reactiveTab, payload, false, operationId)
  try {
    await invoke('start_document_watch', { documentId: reactiveTab.documentId })
  } catch {
    reactiveTab.error = t('documentWatchFailed')
  }
}

async function reloadDocument(documentId: string, sourceRevision?: number) {
  const tab = tabs.value.find((item) => item.documentId === documentId)
  if (!tab) return
  try {
    const payload = await invoke<DocumentPayload>('reload_document', { documentId, sourceRevision })
    await renderIntoTab(tab, payload)
  } catch (error) {
    const code = typeof error === 'string' ? error : 'DOCUMENT_READ_FAILED'
    tab.error = humanizeError(code)
    appError.value = tab.error
  }
}

async function retryTab(tab: Tab) {
  tab.error = undefined
  appError.value = ''
  if (tab.sourceContent !== null) {
    await renderIntoTab(tab, {
      documentId: tab.documentId,
      fileName: tab.fileName,
      content: tab.sourceContent,
      sourceRevision: tab.sourceRevision,
    })
  } else {
    await reloadDocument(tab.documentId, tab.sourceRevision)
  }
}

function scheduleReload(documentId: string, sourceRevision: number) {
  reloadRevisions.set(documentId, Math.max(reloadRevisions.get(documentId) ?? 0, sourceRevision))
  const previous = reloadTimers.get(documentId)
  if (previous) clearTimeout(previous)
  const timer = setTimeout(() => {
    reloadTimers.delete(documentId)
    const revision = reloadRevisions.get(documentId)
    reloadRevisions.delete(documentId)
    void reloadDocument(documentId, revision)
  }, 120)
  reloadTimers.set(documentId, timer)
}

async function handleOpenFile() {
  appError.value = ''
  try {
    await invoke('open_document_dialog')
  } catch {
    appError.value = t('openDialogFailed')
  }
}

function closeSettingsOnOutside(event: PointerEvent) {
  if (!readerSettingsOpen.value || !(event.target instanceof Element)) return
  if (event.target.closest('.reader-settings, .reader-settings-toggle')) return
  readerSettingsOpen.value = false
}

function closeSettingsOnEscape(event: KeyboardEvent) {
  if (event.key === 'Escape' && readerSettingsOpen.value) readerSettingsOpen.value = false
}

function decreaseReaderFont() {
  readerFontScale.value = Math.max(0.85, Number((readerFontScale.value - 0.05).toFixed(2)))
}

function increaseReaderFont() {
  readerFontScale.value = Math.min(1.35, Number((readerFontScale.value + 0.05).toFixed(2)))
}

function toggleReaderWidth() {
  readerContentWidth.value = readerContentWidth.value === 'comfortable' ? 'wide' : 'comfortable'
}

function authorizeActiveRemoteImages() {
  if (activeTab.value) authorizeRemoteImages(activeTab.value.id)
}

async function handleActivateTab(tabId: string) {
  shellActivationIntent += 1
  await activateTab(tabId, true)
}

async function closeTabs(closingTabs: Tab[]) {
  if (closingTabs.length === 0) return
  const closingIds = new Set(closingTabs.map((tab) => tab.id))
  const oldActiveIndex = tabs.value.findIndex((tab) => tab.id === activeTabId.value)
  await Promise.all(closingTabs.map(async (tab) => {
    reloadRevisions.delete(tab.documentId)
    const reloadTimer = reloadTimers.get(tab.documentId)
    if (reloadTimer) {
      clearTimeout(reloadTimer)
      reloadTimers.delete(tab.documentId)
    }
    tab.renderGeneration += 1
    processing.cancelDocument(tab.documentId)
    cancelRender(tab.documentId)
    releaseTabRetention(tab)
    try {
      await invoke('close_document', { documentId: tab.documentId })
    } catch {
      // Closing is idempotent from the user perspective; local state still needs to disappear.
    }
  }))
  tabs.value = tabs.value.filter((tab) => !closingIds.has(tab.id))
  if (!tabs.value.some((tab) => tab.id === activeTabId.value)) {
    const nextTab = tabs.value[Math.min(Math.max(oldActiveIndex, 0), tabs.value.length - 1)]
    if (nextTab) await activateTab(nextTab.id)
    else activeTabId.value = ''
  }
}

async function handleCloseTab(tabId: string) {
  shellActivationIntent += 1
  const tab = tabs.value.find((item) => item.id === tabId)
  if (tab) await closeTabs([tab])
}

async function handleCloseOthers(tabId: string) {
  shellActivationIntent += 1
  await closeTabs(tabs.value.filter((tab) => tab.id !== tabId))
  await activateTab(tabId)
}

async function handleCloseLeft(tabId: string) {
  shellActivationIntent += 1
  const index = tabs.value.findIndex((tab) => tab.id === tabId)
  if (index > 0) await closeTabs(tabs.value.slice(0, index))
}

async function handleCloseRight(tabId: string) {
  shellActivationIntent += 1
  const index = tabs.value.findIndex((tab) => tab.id === tabId)
  if (index >= 0) await closeTabs(tabs.value.slice(index + 1))
}

async function handleCloseAll() {
  shellActivationIntent += 1
  await closeTabs([...tabs.value])
}

function updateTabScrollRatio(tabId: string, ratio: number) {
  const tab = tabs.value.find((item) => item.id === tabId)
  if (tab) tab.scrollRatio = ratio
}

function isTauriRuntime() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

function syncNativeMenuLocale() {
  if (!isTauriRuntime()) return
  void invoke('set_menu_locale', { locale: currentLocale.value }).catch(() => {})
}

function handleExportCurrentPdf() {
  const tab = activeTab.value
  if (!tab) {
    appError.value = t('printDocumentMissing')
    return
  }
  void invoke('export_current_pdf', { documentId: tab.documentId }).catch(() => {
    appError.value = t('printDialogFailed')
  })
}

watch(currentLocale, syncNativeMenuLocale)

function authorizeRemoteImages(tabId: string) {
  const tab = tabs.value.find((item) => item.id === tabId)
  if (tab) tab.remoteImageAuthorized = true
}

async function setupDragDrop() {
  unlistenDragDrop = await getCurrentWebviewWindow().onDragDropEvent((event: any) => {
    isDragOver.value = event.payload.type === 'enter' || event.payload.type === 'over'
    if (event.payload.type === 'drop' || event.payload.type === 'leave') isDragOver.value = false
  })
}

function handleImportError(event: DocumentImportErrorEvent) {
  const wasVisible = processingNotice.value?.operationId === event.operationId
  processing.fail(event.operationId)
  if (wasVisible || !processingNotice.value) appError.value = humanizeError(event.code)
}

function handleContentPainted(operationId: string, documentId: string, generation: number) {
  processing.completePaint(operationId, documentId, generation)
}

onMounted(async () => {
  document.addEventListener('pointerdown', closeSettingsOnOutside)
  document.addEventListener('keydown', closeSettingsOnEscape)
  if (!isTauriRuntime()) return
  syncNativeMenuLocale()
  await setupDragDrop()
  unlistenImportStarted = await listen<DocumentImportStartedEvent>('document-import-started', (event) => {
    processing.start(event.payload, shellActivationIntent)
  })
  unlistenDocumentOpened = await listen<DocumentOpenedEvent>('document-opened', (event) => {
    processing.ensure({
      operationId: event.payload.operationId,
      sequence: event.payload.sequence,
      fileName: event.payload.document.fileName,
    }, shellActivationIntent)
    void addDocument(event.payload.document, event.payload)
  })
  unlistenImportError = await listen<DocumentImportErrorEvent>('document-import-error', (event) => {
    handleImportError(event.payload)
  })
  unlistenDocumentError = await listen<DocumentErrorEvent>('document-error', (event) => {
    const message = humanizeError(event.payload.code)
    const tab = event.payload.documentId
      ? tabs.value.find((item) => item.documentId === event.payload.documentId)
      : undefined
    if (tab) {
      tab.error = message
      if (tab.id === activeTabId.value) appError.value = message
    } else {
      appError.value = message
    }
  })
  unlistenFileChanged = await listen<FileChangedEvent>('file-changed', (event) => {
    scheduleReload(event.payload.documentId, event.payload.sourceRevision)
  })
  unlistenPrintError = await listen<PrintErrorEvent>('print-error', (event) => {
    appError.value = event.payload.code === 'PRINT_DOCUMENT_MISSING'
      ? t('printDocumentMissing')
      : t('printDialogFailed')
  })
  unlistenExportPdf = await listen('export-current-pdf', handleExportCurrentPdf)
  prepareMarkdown()
  try {
    const pending = await invoke<PendingImport | null>('take_pending_import')
    if (pending) {
      processing.ensure({
        operationId: pending.operationId,
        sequence: pending.sequence,
        fileName: pending.fileName,
      }, shellActivationIntent)
      if (pending.kind === 'opened' && pending.document) {
        await addDocument(pending.document, {
          operationId: pending.operationId,
          sequence: pending.sequence,
          document: pending.document,
        })
      } else {
        handleImportError({
          operationId: pending.operationId,
          sequence: pending.sequence,
          code: pending.code ?? 'DOCUMENT_OPEN_FAILED',
        })
      }
    }
  } catch {
    appError.value = t('appInitializeFailed')
  }
})

onUnmounted(() => {
  document.removeEventListener('pointerdown', closeSettingsOnOutside)
  document.removeEventListener('keydown', closeSettingsOnEscape)
  if (!isTauriRuntime()) return
  if (unlistenDragDrop) unlistenDragDrop()
  if (unlistenImportStarted) unlistenImportStarted()
  if (unlistenDocumentOpened) unlistenDocumentOpened()
  if (unlistenImportError) unlistenImportError()
  if (unlistenDocumentError) unlistenDocumentError()
  if (unlistenFileChanged) unlistenFileChanged()
  if (unlistenPrintError) unlistenPrintError()
  if (unlistenExportPdf) unlistenExportPdf()
  for (const timer of reloadTimers.values()) clearTimeout(timer)
  reloadRevisions.clear()
  processing.dispose()
  disposeMarkdown()
  void Promise.all(tabs.value.map((tab) => invoke('close_document', { documentId: tab.documentId })))
  void invoke('stop_document_watch').catch(() => {})
})
</script>

<template>
  <div class="app-container">
    <header class="header-area" data-tauri-drag-region>
      <div class="titlebar">
        <div v-if="hasTabs" class="titlebar-tabs">
          <TabBar
            :tabs="tabs"
            :active-tab-id="activeTabId"
            @activate="handleActivateTab"
            @close="handleCloseTab"
            @close-others="handleCloseOthers"
            @close-left="handleCloseLeft"
            @close-right="handleCloseRight"
            @close-all="handleCloseAll"
          />
        </div>
        <div class="titlebar-brand" :class="{ compact: hasTabs }">
          <img class="brand-mark" :src="yuyueIcon" alt="" aria-hidden="true" />
          <span v-if="!hasTabs">{{ t('appName') }}</span>
        </div>
        <div class="titlebar-right">
          <button class="titlebar-btn" type="button" :title="t('openMarkdownFile')" :aria-label="t('openMarkdownFile')" @click="handleOpenFile">{{ t('open') }}</button>
          <button class="titlebar-btn icon-btn" type="button" :title="themeButtonLabel" :aria-label="themeButtonLabel" @click="toggleTheme">
            <svg data-icon="theme" viewBox="0 0 24 24" aria-hidden="true">
              <g v-if="themeIcon === '☀'">
                <circle cx="12" cy="12" r="3.25" />
                <path d="M12 2.75v2.1M12 19.15v2.1M5.46 5.46l1.49 1.49M17.05 17.05l1.49 1.49M2.75 12h2.1M19.15 12h2.1M5.46 18.54l1.49-1.49M17.05 6.95l1.49-1.49" />
              </g>
              <path v-else-if="themeIcon === '☾'" d="M19.4 14.75A7.5 7.5 0 0 1 9.25 4.6 7.5 7.5 0 1 0 19.4 14.75Z" />
              <g v-else>
                <path d="M12 4.5a7.5 7.5 0 1 0 0 15Z" />
                <path d="M12 4.5a7.5 7.5 0 0 1 0 15Z" />
              </g>
            </svg>
          </button>
          <button class="titlebar-btn icon-btn reader-settings-toggle" type="button" :title="t('readingSettings')" :aria-label="t('readingSettings')" :aria-expanded="readerSettingsOpen" @click="readerSettingsOpen = !readerSettingsOpen">
            <svg data-icon="settings" viewBox="0 0 24 24" aria-hidden="true">
              <path d="M12 15.25A3.25 3.25 0 1 0 12 8.75a3.25 3.25 0 0 0 0 6.5Z" />
              <path d="M19.43 13.5a7.92 7.92 0 0 0 .05-1.5 7.92 7.92 0 0 0-.05-1.5l2.02-1.57-2-3.46-2.4.97a8.06 8.06 0 0 0-2.59-1.5L14.1 2.4h-4l-.36 2.54a8.06 8.06 0 0 0-2.59 1.5l-2.4-.97-2 3.46 2.02 1.57A7.92 7.92 0 0 0 4.72 12c0 .51.02 1.01.05 1.5l-2.02 1.57 2 3.46 2.4-.97a8.06 8.06 0 0 0 2.59 1.5l.36 2.54h4l.36-2.54a8.06 8.06 0 0 0 2.59-1.5l2.4.97 2-3.46-2.02-1.57Z" />
            </svg>
          </button>
        </div>
      </div>
      <div v-if="appError" class="app-error" role="alert">
        <span>{{ appError }}</span>
        <button type="button" :aria-label="t('closeNotification')" @click="appError = ''">×</button>
      </div>
    </header>

    <Transition name="processing-notice">
      <DocumentProcessingNotice
        v-if="processingNotice"
        :file-name="processingNotice.fileName"
        :phase="processingPhase"
        :message="processingMessage"
      />
    </Transition>

    <main class="main-content">
      <div v-if="!hasTabs" class="welcome">
        <img class="welcome-mark" :src="yuyueIcon" alt="" aria-hidden="true" />
        <p class="eyebrow">PRIVATE · LOCAL · READABLE</p>
        <h1>{{ t('welcomeTitle') }}</h1>
        <p class="welcome-description">{{ t('welcomeDescription') }}</p>
        <button class="welcome-btn" type="button" @click="handleOpenFile">{{ t('openMarkdownFile') }}</button>
        <p class="welcome-hint">{{ t('welcomeHint') }}</p>
      </div>

      <template v-else-if="activeTab">
        <ReadingMode
          :document="activeDocument"
          :title="activeTab.fileName"
          :document-id="activeTab.documentId"
          :render-generation="activeTab.renderGeneration"
          :paint-operation-id="activeTab.paintOperationId"
          :active="true"
          :initial-scroll-ratio="activeTab.scrollRatio"
          :remote-image-authorized="activeTab.remoteImageAuthorized"
          :error-message="activeTab.error"
          :font-scale="readerFontScale"
          :content-width="readerContentWidth"
          :settings-open="readerSettingsOpen"
          @scroll-ratio="(ratio) => updateTabScrollRatio(activeTabId, ratio)"
          @authorize-remote-images="authorizeRemoteImages(activeTabId)"
          @retry="retryTab(activeTab)"
          @content-painted="handleContentPainted"
          @update-settings-open="readerSettingsOpen = $event"
        />
      </template>
    </main>

    <ReaderSettings
      :open="readerSettingsOpen"
      :font-scale="readerFontScale"
      :content-width="readerContentWidth"
      :has-remote-images="hasRemoteImages"
      :remote-image-authorized="remoteImagesAuthorized"
      @close="readerSettingsOpen = false"
      @decrease-font="decreaseReaderFont"
      @increase-font="increaseReaderFont"
      @reset-font="readerFontScale = 1"
      @toggle-width="toggleReaderWidth"
      @authorize-remote-images="authorizeActiveRemoteImages"
    />

    <div v-if="themeStatus" class="theme-status" role="status" aria-live="polite">{{ themeStatus }}</div>

    <Transition name="drop-overlay">
      <div v-if="isDragOver" class="drag-overlay" role="status" aria-live="polite">
        <div class="drag-overlay-content">
          <div class="drag-icon" aria-hidden="true">↓</div>
          <p>{{ t('dropToOpen') }}</p>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style>
@import 'highlight.js/styles/github.css';
@import 'katex/dist/katex.min.css';

* { box-sizing: border-box; }
html, body, #app { height: 100%; overflow: hidden; }
body { margin: 0; font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif; }
button, input { font: inherit; }
:root, [data-theme='light'] {
  --bg-primary: #fcfcfd;
  --bg-secondary: #f2f3f6;
  --text-primary: #171923;
  --text-secondary: #4f5667;
  --text-heading: #171923;
  --text-muted: #7d8494;
  --border-color: #e4e6eb;
  --tool-btn-hover-bg: #e8eaf0;
  --code-bg: #f0f1f4;
  --code-color: #a0443f;
  --pre-bg: #202434;
  --pre-color: #edf1fb;
  --table-header-bg: #f0f2f6;
  --titlebar-bg: #f7f7f9;
  --titlebar-border: #e1e3e9;
}
[data-theme='dark'] {
  --bg-primary: #171923;
  --bg-secondary: #202434;
  --text-primary: #f3f5fb;
  --text-secondary: #c2c8d6;
  --text-heading: #f6f7fb;
  --text-muted: #8f98aa;
  --border-color: #303646;
  --tool-btn-hover-bg: #303646;
  --code-bg: #2a2f40;
  --code-color: #f0bd78;
  --pre-bg: #10121b;
  --pre-color: #edf1fb;
  --table-header-bg: #2a2f40;
  --titlebar-bg: #1d202c;
  --titlebar-border: #303646;
}
</style>

<style scoped>
.app-container { position: relative; display: flex; flex-direction: column; height: 100%; color: var(--text-primary); background: var(--bg-primary); }
.header-area { flex-shrink: 0; border-bottom: 1px solid var(--titlebar-border); background: var(--titlebar-bg); }
.titlebar { display: flex; align-items: center; gap: 8px; min-height: 40px; padding: 2px 10px 0 82px; }
.titlebar-tabs { min-width: 0; flex: 1; align-self: stretch; }
.titlebar-brand { display: flex; align-items: center; gap: 8px; flex-shrink: 0; color: var(--text-primary); font-size: 13px; font-weight: 700; letter-spacing: -0.01em; }
.titlebar-brand.compact { display: none; }
.brand-mark, .welcome-mark { display: block; width: 28px; height: 28px; border-radius: 8px; background: #faf7f0; object-fit: cover; }
.titlebar-right { display: flex; align-items: center; gap: 4px; flex-shrink: 0; margin-left: auto; padding-bottom: 2px; }
.titlebar-btn { min-height: 28px; padding: 0 9px; border: 0; border-radius: 7px; color: var(--text-secondary); background: transparent; cursor: pointer; font-size: 12px; }
.titlebar-btn:hover, .titlebar-btn:focus-visible { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 0; }
.titlebar-btn.icon-btn { display: grid; width: 38px; min-height: 38px; place-items: center; padding: 0; }
.titlebar-btn.icon-btn svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-linecap: round; stroke-linejoin: round; stroke-width: 1.8; }
.reader-settings-toggle svg { width: 15px; height: 15px; }
.theme-status { position: absolute; right: 12px; bottom: 10px; z-index: 30; padding: 7px 10px; border: 1px solid var(--border-color); border-radius: 7px; color: var(--text-secondary); background: var(--bg-primary); box-shadow: 0 8px 20px rgba(20, 24, 40, 0.12); font-size: 12px; }
.app-error { display: flex; align-items: center; justify-content: space-between; gap: 10px; min-height: 32px; padding: 6px 16px; color: #9d3732; background: rgba(194, 65, 59, 0.09); font-size: 12px; }
.app-error button { width: 22px; height: 22px; padding: 0; border: 0; color: inherit; background: transparent; cursor: pointer; font-size: 18px; line-height: 1; }
.main-content { min-height: 0; flex: 1; overflow: hidden; }
.welcome { display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; padding: 32px; text-align: center; }
.welcome-mark { width: 54px; height: 54px; margin-bottom: 22px; border-radius: 15px; box-shadow: 0 10px 26px rgba(49, 73, 168, 0.22); }
.eyebrow { margin: 0 0 12px; color: #4c6ef5; font-size: 10px; font-weight: 750; letter-spacing: 0.13em; }
.welcome h1 { margin: 0; color: var(--text-heading); font-size: clamp(1.7rem, 3vw, 2.35rem); letter-spacing: -0.04em; }
.welcome-description { max-width: 350px; margin: 13px 0 22px; color: var(--text-secondary); font-size: 14px; line-height: 1.7; }
.welcome-btn { min-height: 42px; padding: 0 21px; border: 0; border-radius: 9px; color: #fff; background: #4c6ef5; box-shadow: 0 7px 16px rgba(76, 110, 245, 0.22); cursor: pointer; font-size: 13px; font-weight: 650; }
.welcome-btn:hover, .welcome-btn:focus-visible { background: #4263eb; outline: 2px solid rgba(76, 110, 245, 0.3); outline-offset: 3px; }
.welcome-hint { margin: 18px 0 0; color: var(--text-muted); font-size: 11px; }
.drag-overlay { position: absolute; z-index: 9999; inset: 0; display: flex; align-items: center; justify-content: center; border: 2px dashed #4c6ef5; background: rgba(76, 110, 245, 0.12); pointer-events: none; }
.drag-overlay-content { padding: 28px 36px; border: 1px solid rgba(76, 110, 245, 0.3); border-radius: 14px; color: #4c6ef5; background: var(--bg-primary); text-align: center; box-shadow: 0 16px 40px rgba(20, 24, 40, 0.14); }
.drag-icon { margin-bottom: 10px; font-size: 28px; }
.drag-overlay-content p { margin: 0; font-size: 14px; font-weight: 650; }
.drop-overlay-enter-active, .drop-overlay-leave-active { transition: opacity 0.15s ease; }
.drop-overlay-enter-from, .drop-overlay-leave-to { opacity: 0; }
.processing-notice-enter-active, .processing-notice-leave-active { transition: opacity 0.14s ease, transform 0.14s ease; }
.processing-notice-enter-from, .processing-notice-leave-to { opacity: 0; transform: translate(-50%, -5px); }

@media print {
  :global(html), :global(body), :global(#app) { height: auto !important; overflow: visible !important; background: #fff !important; }
  .app-container, .main-content { height: auto !important; overflow: visible !important; background: #fff !important; }
  .header-area, .theme-status, .drag-overlay, :deep(.document-processing-notice) { display: none !important; }
}
</style>
