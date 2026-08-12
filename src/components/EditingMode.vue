<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, shallowRef, toRaw, useTemplateRef, watch } from 'vue'
import { defaultKeymap, historyKeymap, indentWithTab, redo, undo } from '@codemirror/commands'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { markdown } from '@codemirror/lang-markdown'
import { closeSearchPanel, openSearchPanel, search, searchKeymap, searchPanelOpen } from '@codemirror/search'
import { Compartment, EditorState, type Transaction } from '@codemirror/state'
import { EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from '@codemirror/view'
import { tags } from '@lezer/highlight'
import { applyMarkdownCommand, markdownCommandIds, type MarkdownCommandId } from '../editor/markdownCommands'
import { createCodeMirrorPhrases } from '../editor/codeMirrorPhrases'
import type { EditorSession, EditorWriteStatus } from '../editor/editorSession'
import type { RenderDocument } from '../markdown/renderer'
import { useLocale, type MessageKey } from '../composables/useLocale'
import ReadingMode from './ReadingMode.vue'

interface ClipboardImageFile {
  size: number
  type: string
  name: string
  slice: (start: number, end: number) => { arrayBuffer: () => Promise<ArrayBuffer> }
}

const props = withDefaults(defineProps<{
  session: EditorSession
  writeStatus: EditorWriteStatus
  document: RenderDocument
  title: string
  remoteImageAuthorized: boolean
  renderGeneration: number
  inputFrozen: boolean
  leaving?: boolean
  theme?: 'light' | 'dark'
}>(), { theme: 'light' })

const emit = defineEmits<{
  'session-change': [writeStatusChanged: boolean]
  flush: []
  'authorize-remote-images': []
  retry: []
  'save-as': []
  'use-disk': []
  overwrite: []
  'insert-image': []
  'paste-image': [file: ClipboardImageFile]
  discard: []
  'recovery-action': [recordId: string, action: 'saveCurrentBufferCopy' | 'revealPreservedItem', actionToken: string]
  'recovery-ack': [recordId: string, ackToken?: string]
}>()

const editorHost = useTemplateRef<HTMLElement>('editorHost')
const previewRef = useTemplateRef<InstanceType<typeof ReadingMode>>('preview')
const { currentLocale, t } = useLocale()
const narrowPane = shallowRef<'source' | 'preview'>('source')
const overwriteConfirmationOpen = shallowRef(false)
const discardConfirmationOpen = shallowRef(false)
const narrowMoreOpen = shallowRef(false)
const toolbarTooltip = shallowRef<{ label: string, x: number, y: number } | null>(null)
let editorView: EditorView | null = null
let scrollFrame = 0
let scrollOriginReleaseFrame = 0
let tooltipTimer: ReturnType<typeof setTimeout> | null = null
let scrollSyncOrigin: 'source' | 'preview' | null = null
const editableCompartment = new Compartment()
const editorThemeCompartment = new Compartment()
const editorPhrasesCompartment = new Compartment()
const TOOLTIP_DELAY_MS = 160

function editorSession() {
  return toRaw(props.session)
}

const markdownHighlightStyle = HighlightStyle.define([
  { tag: tags.heading, color: 'var(--editor-syntax-heading)', fontWeight: '700' },
  { tag: tags.processingInstruction, color: 'var(--editor-syntax-mark)', fontWeight: '650' },
  { tag: [tags.link, tags.url], color: 'var(--editor-syntax-link)', textDecoration: 'underline' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strong, fontWeight: '700' },
  { tag: tags.strikethrough, textDecoration: 'line-through' },
  { tag: [tags.monospace, tags.literal, tags.string], color: 'var(--editor-syntax-code)' },
  { tag: [tags.keyword, tags.atom, tags.bool, tags.labelName], color: 'var(--editor-syntax-keyword)' },
  { tag: [tags.comment, tags.meta], color: 'var(--editor-syntax-comment)' },
  { tag: tags.invalid, color: 'var(--editor-syntax-invalid)', textDecoration: 'underline wavy' },
], { all: 'var(--editor-text)' })

const sourceBlocks = computed(() => props.document.sourceBlocks ?? [])
const headingSourceBlocks = computed(() => sourceBlocks.value.filter((block) => block.kind === 'heading'))
const sourceBlockById = computed(() => new Map(sourceBlocks.value.map((block) => [block.id, block])))

const commandLabels: Record<MarkdownCommandId, MessageKey> = {
  heading: 'editorHeading',
  bold: 'editorBold',
  italic: 'editorItalic',
  strikethrough: 'editorStrikethrough',
  'bullet-list': 'editorBulletList',
  'ordered-list': 'editorOrderedList',
  'task-list': 'editorTaskList',
  quote: 'editorQuote',
  link: 'editorLink',
  'inline-code': 'editorInlineCode',
  'code-block': 'editorCodeBlock',
}
const narrowMoreCommands: MarkdownCommandId[] = [
  'italic', 'strikethrough', 'ordered-list', 'task-list', 'quote', 'link', 'inline-code', 'code-block',
]

const statusLabels: Record<EditorWriteStatus, MessageKey> = {
  saved: 'editorSaved',
  pending: 'editorWritePending',
  writing: 'editorWriting',
  failed: 'editorWriteFailed',
  conflict: 'editorConflict',
  'recovery-required': 'editorRecoveryRequired',
}

function createEditorTheme(theme: 'light' | 'dark') {
  return EditorView.theme({
    '&': { height: '100%', color: 'var(--editor-text)', background: 'var(--editor-surface)' },
    '.cm-scroller': { overflow: 'auto', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', lineHeight: '1.68' },
    '.cm-content': { padding: '16px 18px 56px', caretColor: 'var(--editor-cursor)' },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--editor-cursor)' },
    '.cm-gutters': { borderRight: '1px solid var(--border-color)', color: 'var(--editor-line-number)', background: 'var(--editor-gutter-surface)' },
    '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'var(--editor-active-line-bg)' },
    '.cm-activeLineGutter': { color: 'var(--editor-active-line-number)' },
    '.cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection': { backgroundColor: 'var(--editor-selection-bg)' },
  }, { dark: theme === 'dark' })
}

function dispatchTransaction(transaction: Transaction) {
  if (!editorView || (props.inputFrozen && transaction.docChanged)) return
  const previousWriteStatus = editorSession().snapshot.writeStatus
  editorSession().accept(transaction)
  editorView.update([transaction])
  if (transaction.docChanged) {
    emit('session-change', previousWriteStatus !== editorSession().snapshot.writeStatus)
  }
}

function createEditorView() {
  if (!editorHost.value || editorView) return
  const extensions = [
    lineNumbers(),
    highlightActiveLine(),
    highlightActiveLineGutter(),
    EditorView.lineWrapping,
    editableCompartment.of(EditorView.editable.of(!props.inputFrozen)),
    EditorView.domEventHandlers({
      paste(event) {
        if (props.inputFrozen) return false
        const file = Array.from(event.clipboardData?.files ?? []).find((candidate) => candidate.type.startsWith('image/'))
        if (!file) return false
        event.preventDefault()
        emit('paste-image', file)
        return true
      },
    }),
    EditorView.updateListener.of((update) => {
      if (!update.docChanged && !update.viewportChanged) return
      schedulePreviewScrollSync()
    }),
    markdown(),
    syntaxHighlighting(markdownHighlightStyle),
    editorPhrasesCompartment.of(EditorState.phrases.of(createCodeMirrorPhrases(t))),
    search({ top: true }),
    keymap.of([
      { key: 'Mod-s', run: () => { emit('flush'); return true } },
      { key: 'Mod-z', run: undo },
      { key: 'Shift-Mod-z', run: redo },
      ...historyKeymap,
      ...searchKeymap,
      indentWithTab,
      ...defaultKeymap,
    ]),
    editorThemeCompartment.of(createEditorTheme(props.theme)),
  ]
  editorSession().configureEditor(extensions)
  editorView = new EditorView({
    state: editorSession().state,
    parent: editorHost.value,
    dispatch: dispatchTransaction,
  })
  editorView.scrollDOM.addEventListener('scroll', schedulePreviewScrollSync, { passive: true })
}

function blockForLine(line: number) {
  const blocks = sourceBlocks.value
  let low = 0
  let high = blocks.length - 1
  let precedingIndex = -1
  while (low <= high) {
    const middle = (low + high) >> 1
    if (blocks[middle].startLine <= line) {
      precedingIndex = middle
      low = middle + 1
    } else {
      high = middle - 1
    }
  }
  const nearest = blocks[Math.max(precedingIndex, 0)]
  if (nearest && line <= nearest.endLine) return nearest

  const headings = headingSourceBlocks.value
  low = 0
  high = headings.length - 1
  let headingIndex = -1
  while (low <= high) {
    const middle = (low + high) >> 1
    if (headings[middle].startLine <= line) {
      headingIndex = middle
      low = middle + 1
    } else {
      high = middle - 1
    }
  }
  return headings[Math.max(headingIndex, 0)] ?? nearest
}

function schedulePreviewScrollSync() {
  if (!editorView || scrollSyncOrigin === 'preview' || sourceBlocks.value.length === 0) return
  window.cancelAnimationFrame(scrollFrame)
  scrollFrame = requestAnimationFrame(() => {
    if (!editorView || scrollSyncOrigin === 'preview') return
    const lineBlock = editorView.lineBlockAtHeight(editorView.scrollDOM.scrollTop)
    const line = editorView.state.doc.lineAt(lineBlock.from).number
    const block = blockForLine(line)
    if (!block) return
    scrollSyncOrigin = 'source'
    const didScroll = previewRef.value?.scrollToSourceBlock(block.id) ?? false
    if (didScroll) scheduleScrollOriginRelease()
    else scrollSyncOrigin = null
  })
}

function scheduleScrollOriginRelease() {
  window.cancelAnimationFrame(scrollOriginReleaseFrame)
  scrollOriginReleaseFrame = requestAnimationFrame(() => {
    scrollOriginReleaseFrame = requestAnimationFrame(() => {
      scrollSyncOrigin = null
      scrollOriginReleaseFrame = 0
    })
  })
}

function revealLine(line: number, focusEditor = true) {
  if (!editorView) return
  const bounded = Math.min(Math.max(1, line), editorView.state.doc.lines)
  const position = editorView.state.doc.line(bounded).from
  editorView.dispatch({
    selection: { anchor: position },
    effects: EditorView.scrollIntoView(position, { y: 'center' }),
  })
  narrowPane.value = 'source'
  if (focusEditor) editorView.focus()
}

function revealOffset(offset = 0) {
  if (!editorView) return
  const bounded = Math.min(Math.max(0, offset), editorView.state.doc.length)
  const line = editorView.state.doc.lineAt(bounded).number
  revealLine(line)
}

function activateSourceBlock(blockId: string) {
  const block = sourceBlockById.value.get(blockId)
  if (block) revealLine(block.startLine)
}

function syncSourceFromPreview(blockId: string) {
  if (scrollSyncOrigin === 'source') return
  const block = sourceBlockById.value.get(blockId)
  if (!block || !editorView) return
  scrollSyncOrigin = 'preview'
  const position = editorView.state.doc.line(Math.min(block.startLine, editorView.state.doc.lines)).from
  editorView.dispatch({ effects: EditorView.scrollIntoView(position, { y: 'start' }) })
  scheduleScrollOriginRelease()
}

function runCommand(command: MarkdownCommandId) {
  if (!editorView || props.inputFrozen) return
  applyMarkdownCommand(editorView, command, t('editorPlaceholder'))
  narrowMoreOpen.value = false
}

function runFind() {
  if (!editorView) return
  openSearchPanel(editorView)
  editorView.focus()
}

function runUndo() {
  if (editorView && !props.inputFrozen) undo(editorView)
}

function runRedo() {
  if (editorView && !props.inputFrozen) redo(editorView)
}

function clearToolbarTooltip() {
  if (tooltipTimer) clearTimeout(tooltipTimer)
  tooltipTimer = null
  toolbarTooltip.value = null
}

function scheduleToolbarTooltip(event: unknown, label: string, delay = TOOLTIP_DELAY_MS) {
  clearToolbarTooltip()
  const target = (event as { currentTarget?: unknown }).currentTarget
  if (!(target instanceof HTMLElement)) return
  const rect = target.getBoundingClientRect()
  tooltipTimer = setTimeout(() => {
    const viewportWidth = document.documentElement.clientWidth || window.innerWidth
    toolbarTooltip.value = {
      label,
      x: Math.min(Math.max(rect.left + rect.width / 2, 48), Math.max(48, viewportWidth - 48)),
      y: rect.bottom + 7,
    }
    tooltipTimer = null
  }, delay)
}

function focus() {
  editorView?.focus()
}

function replaceState() {
  if (!editorView) return
  editorView.setState(editorSession().state)
  editorView.focus()
}

function insertImage(markdownUrl: string, alt: string) {
  const segments = markdownUrl.split('/')
  if (!editorView || props.inputFrozen
    || !/^(?:[A-Za-z0-9._~%-]+\/)*[A-Za-z0-9._~%-]+$/.test(markdownUrl)
    || segments.some((segment) => /^(?:\.|\.\.|%2e|%2e%2e)$/i.test(segment))) return false
  if (editorView.state !== editorSession().state) editorView.setState(editorSession().state)
  const before = props.session.snapshot.editGeneration
  const escapedAlt = alt.replace(/(\[|\]|\\)/g, (value) => `\\${value}`)
  const selection = editorView.state.selection.main
  dispatchTransaction(editorSession().state.update({
    changes: { from: selection.from, to: selection.to, insert: `![${escapedAlt}](${markdownUrl})` },
    selection: { anchor: selection.from + escapedAlt.length + markdownUrl.length + 5 },
    userEvent: 'input.image',
  }))
  editorView.focus()
  return props.session.snapshot.editGeneration > before
}

function handleConfirmKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    overwriteConfirmationOpen.value = false
    discardConfirmationOpen.value = false
    editorView?.focus()
    return
  }
  if (event.key !== 'Tab') return
  const dialog = (event.currentTarget as HTMLElement).closest('[role="alertdialog"]')
  const buttons = Array.from(dialog?.querySelectorAll<HTMLButtonElement>('button') ?? [])
  if (buttons.length < 2) return
  const first = buttons[0]
  const last = buttons[buttons.length - 1]
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first.focus()
  }
}

onMounted(() => {
  createEditorView()
  focus()
})
watch(() => props.inputFrozen, (frozen) => {
  editorView?.dispatch({ effects: editableCompartment.reconfigure(EditorView.editable.of(!frozen)) })
})
watch(() => props.theme, (theme) => {
  editorView?.dispatch({ effects: editorThemeCompartment.reconfigure(createEditorTheme(theme)) })
})
watch(currentLocale, () => {
  if (!editorView) return
  const hadFocus = editorView.hasFocus
  const scrollTop = editorView.scrollDOM.scrollTop
  const reopenSearch = searchPanelOpen(editorView.state)
  const transaction = editorSession().dispatch({
    effects: editorPhrasesCompartment.reconfigure(EditorState.phrases.of(createCodeMirrorPhrases(t))),
  })
  editorView.setState(transaction.state)
  if (reopenSearch) {
    closeSearchPanel(editorView)
    openSearchPanel(editorView)
  }
  editorView.scrollDOM.scrollTop = scrollTop
  if (hadFocus) editorView.focus()
})
watch([overwriteConfirmationOpen, discardConfirmationOpen], ([overwriteOpen, discardOpen], previous) => {
  if (!overwriteOpen && !discardOpen && previous?.some(Boolean)) requestAnimationFrame(() => editorView?.focus())
})
onBeforeUnmount(() => {
  window.cancelAnimationFrame(scrollFrame)
  window.cancelAnimationFrame(scrollOriginReleaseFrame)
  clearToolbarTooltip()
  editorView?.scrollDOM.removeEventListener('scroll', schedulePreviewScrollSync)
  editorView?.destroy()
  editorView = null
})

defineExpose({ focus, revealLine, replaceState, insertImage })
</script>

<template>
  <section class="editing-mode" data-editing-mode :data-editor-theme="props.theme">
    <div class="editing-toolbar" :aria-label="t('editorToolbar')" role="toolbar">
      <button
        class="editor-tool"
        type="button"
        data-editor-history="undo"
        :disabled="props.inputFrozen"
        :aria-label="t('editorUndo')"
        :data-tooltip="t('editorUndo')"
        @mouseenter="scheduleToolbarTooltip($event, t('editorUndo'))"
        @mouseleave="clearToolbarTooltip"
        @focus="scheduleToolbarTooltip($event, t('editorUndo'), 0)"
        @blur="clearToolbarTooltip"
        @click="runUndo"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 14 4 9l5-5" /><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11" /></svg>
      </button>
      <button
        class="editor-tool"
        type="button"
        data-editor-history="redo"
        :disabled="props.inputFrozen"
        :aria-label="t('editorRedo')"
        :data-tooltip="t('editorRedo')"
        @mouseenter="scheduleToolbarTooltip($event, t('editorRedo'))"
        @mouseleave="clearToolbarTooltip"
        @focus="scheduleToolbarTooltip($event, t('editorRedo'), 0)"
        @blur="clearToolbarTooltip"
        @click="runRedo"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m15 14 5-5-5-5" /><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13" /></svg>
      </button>
      <span class="editor-toolbar-separator" aria-hidden="true" />
      <button
        v-for="command in markdownCommandIds"
        :key="command"
        class="editor-tool"
        type="button"
        :disabled="props.inputFrozen"
        :data-editor-command="command"
        :aria-label="t(commandLabels[command])"
        :data-tooltip="t(commandLabels[command])"
        @mouseenter="scheduleToolbarTooltip($event, t(commandLabels[command]))"
        @mouseleave="clearToolbarTooltip"
        @focus="scheduleToolbarTooltip($event, t(commandLabels[command]), 0)"
        @blur="clearToolbarTooltip"
        @click="runCommand(command)"
      >
        <span v-if="command === 'heading'">H</span>
        <strong v-else-if="command === 'bold'">B</strong>
        <em v-else-if="command === 'italic'">I</em>
        <s v-else-if="command === 'strikethrough'">S</s>
        <span v-else-if="command === 'bullet-list'">•</span>
        <span v-else-if="command === 'ordered-list'">1.</span>
        <span v-else-if="command === 'task-list'">☐</span>
        <span v-else-if="command === 'quote'">❝</span>
        <span v-else-if="command === 'link'">↗</span>
        <span v-else-if="command === 'inline-code'">&lt;/&gt;</span>
        <span v-else>```</span>
      </button>
      <button class="editor-tool editor-find-trigger" type="button" :aria-label="t('editorFind')" :data-tooltip="t('editorFind')" @mouseenter="scheduleToolbarTooltip($event, t('editorFind'))" @mouseleave="clearToolbarTooltip" @focus="scheduleToolbarTooltip($event, t('editorFind'), 0)" @blur="clearToolbarTooltip" @click="runFind">⌕</button>
      <button class="editor-tool" type="button" :disabled="props.inputFrozen" :aria-label="t('editorInsertImage')" :data-tooltip="t('editorInsertImage')" @mouseenter="scheduleToolbarTooltip($event, t('editorInsertImage'))" @mouseleave="clearToolbarTooltip" @focus="scheduleToolbarTooltip($event, t('editorInsertImage'), 0)" @blur="clearToolbarTooltip" @click="emit('insert-image')">▧</button>
      <span class="editor-more">
        <button class="editor-tool editor-more-trigger" type="button" :disabled="props.inputFrozen" :aria-label="t('editorMore')" :data-tooltip="t('editorMore')" :aria-expanded="narrowMoreOpen" @mouseenter="scheduleToolbarTooltip($event, t('editorMore'))" @mouseleave="clearToolbarTooltip" @focus="scheduleToolbarTooltip($event, t('editorMore'), 0)" @blur="clearToolbarTooltip" @click="narrowMoreOpen = !narrowMoreOpen">•••</button>
        <span v-if="narrowMoreOpen" class="editor-more-menu" role="menu">
          <button v-for="command in narrowMoreCommands" :key="command" type="button" role="menuitem" @click="runCommand(command)">{{ t(commandLabels[command]) }}</button>
        </span>
      </span>
      <span class="editor-write-status" :data-write-status="props.writeStatus" role="status" aria-live="polite">
        {{ t(statusLabels[props.writeStatus]) }}
      </span>
      <span v-if="props.writeStatus === 'failed'" class="editor-status-actions">
        <button type="button" :disabled="props.leaving" @click="emit('retry')">{{ t('editorRetry') }}</button>
        <button type="button" :disabled="props.leaving" @click="emit('save-as')">{{ t('editorSaveAs') }}</button>
        <button type="button" :disabled="props.leaving" @click="discardConfirmationOpen = true">{{ t('editorDiscard') }}</button>
      </span>
      <span v-else-if="props.writeStatus === 'conflict'" class="editor-status-actions">
        <button type="button" :disabled="props.leaving" @click="emit('use-disk')">{{ t('editorUseDisk') }}</button>
        <button type="button" :disabled="props.leaving" @click="overwriteConfirmationOpen = true">{{ t('editorOverwrite') }}</button>
        <button type="button" :disabled="props.leaving" @click="emit('save-as')">{{ t('editorSaveAs') }}</button>
        <button type="button" :disabled="props.leaving" @click="discardConfirmationOpen = true">{{ t('editorDiscard') }}</button>
      </span>
      <span v-else-if="props.writeStatus === 'recovery-required'" class="editor-status-actions recovery-actions">
        <template v-for="record in props.session.snapshot.recoveryRecords" :key="record.recordId">
          <span v-if="!record.acknowledged" class="recovery-record">
            <button v-if="!record.actionCompleted" type="button" :disabled="props.leaving" @click="emit('recovery-action', record.recordId, record.action, record.actionToken)">
              {{ record.action === 'saveCurrentBufferCopy' ? t('editorRecoverySaveCopy') : t('editorRecoveryReveal') }} {{ record.ordinal }}
            </button>
            <button v-else type="button" :disabled="props.leaving" @click="emit('recovery-ack', record.recordId, record.ackToken)">
              {{ t('editorRecoveryAcknowledge') }} {{ record.ordinal }}
            </button>
          </span>
        </template>
        <button v-if="props.session.snapshot.recoveryRecords?.length && props.session.snapshot.recoveryRecords.every((record) => record.acknowledged)" type="button" :disabled="props.leaving" @click="emit('save-as')">
          {{ t('editorSaveAs') }}
        </button>
      </span>
    </div>
    <div
      v-if="toolbarTooltip"
      class="editor-tooltip"
      role="tooltip"
      :style="{ left: `${toolbarTooltip.x}px`, top: `${toolbarTooltip.y}px` }"
    >
      {{ toolbarTooltip.label }}
    </div>
    <div v-if="overwriteConfirmationOpen" class="editor-confirm-backdrop" role="presentation" @click.self="overwriteConfirmationOpen = false">
      <section class="editor-confirm" role="alertdialog" aria-modal="true" :aria-label="t('editorConfirmOverwrite')" @keydown="handleConfirmKeydown">
        <p>{{ t('editorConfirmOverwrite') }}</p>
        <div>
          <button type="button" autofocus @click="overwriteConfirmationOpen = false">{{ t('editorCancel') }}</button>
          <button type="button" class="editor-danger" @click="overwriteConfirmationOpen = false; emit('overwrite')">{{ t('editorOverwriteConfirmed') }}</button>
        </div>
      </section>
    </div>
    <div v-if="discardConfirmationOpen" class="editor-confirm-backdrop" role="presentation" @click.self="discardConfirmationOpen = false">
      <section class="editor-confirm" role="alertdialog" aria-modal="true" :aria-label="t('editorConfirmDiscard')" @keydown="handleConfirmKeydown">
        <p>{{ t('editorConfirmDiscard') }}</p>
        <div>
          <button type="button" autofocus @click="discardConfirmationOpen = false">{{ t('editorCancel') }}</button>
          <button type="button" class="editor-danger" @click="discardConfirmationOpen = false; emit('discard')">{{ t('editorDiscardConfirmed') }}</button>
        </div>
      </section>
    </div>
    <div class="editing-narrow-tabs" role="tablist" :aria-label="t('editorWorkspace')">
      <button type="button" role="tab" :aria-selected="narrowPane === 'source'" @click="narrowPane = 'source'">{{ t('editorSource') }}</button>
      <button type="button" role="tab" :aria-selected="narrowPane === 'preview'" @click="narrowPane = 'preview'">{{ t('editorPreview') }}</button>
    </div>
    <div class="editing-workspace" :data-narrow-pane="narrowPane">
      <aside v-if="props.document.outline.length > 0" class="editing-outline" :aria-label="t('outline')">
        <div class="editing-outline-title">{{ t('outline') }}</div>
        <button
          v-for="item in props.document.outline"
          :key="item.id"
          type="button"
          :class="`editing-outline-level-${item.level}`"
          @click="revealOffset(item.sourcePosition?.start)"
        >
          {{ item.text }}
        </button>
      </aside>
      <div ref="editorHost" class="editor-host" aria-label="Markdown" />
      <div class="editing-preview">
        <ReadingMode
          ref="preview"
          embedded
          :document="props.document"
          :title="props.title"
          :document-id="props.session.documentId"
          :active="true"
          :initial-scroll-ratio="0"
          :remote-image-authorized="props.remoteImageAuthorized"
          :render-generation="props.renderGeneration"
          content-width="wide"
          @authorize-remote-images="emit('authorize-remote-images')"
          @source-block-activate="activateSourceBlock"
          @source-block-scroll="syncSourceFromPreview"
        />
      </div>
    </div>
  </section>
</template>

<style scoped>
.editing-mode {
  --editor-text: var(--text-primary);
  --editor-surface: var(--bg-primary);
  --editor-gutter-surface: var(--bg-secondary);
  --editor-line-number: var(--text-muted);
  --editor-active-line-number: var(--text-primary);
  --editor-active-line-bg: color-mix(in srgb, var(--tool-btn-hover-bg) 52%, transparent);
  --editor-syntax-heading: #3159c7;
  --editor-syntax-mark: #667085;
  --editor-syntax-link: #3159c7;
  --editor-syntax-code: #a0443f;
  --editor-syntax-keyword: #7146a8;
  --editor-syntax-comment: #6f7788;
  --editor-syntax-invalid: #c23b3b;
  --editor-cursor: #3159c7;
  --editor-selection-bg: rgba(76, 110, 245, .22);
  --editor-selection-match-bg: rgba(76, 110, 245, .13);
  --editor-search-match-bg: rgba(245, 190, 54, .3);
  --editor-search-match-border: rgba(173, 119, 0, .42);
  --editor-search-selected-bg: rgba(76, 110, 245, .3);
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  color-scheme: light;
  background: var(--bg-primary);
}
.editing-mode[data-editor-theme='dark'] {
  --editor-text: #d7dbe5;
  --editor-surface: #171923;
  --editor-gutter-surface: #191c27;
  --editor-line-number: #747e91;
  --editor-active-line-number: #cbd1dc;
  --editor-active-line-bg: rgba(143, 152, 170, .09);
  --editor-syntax-heading: #f3f5f8;
  --editor-syntax-mark: #a7afbd;
  --editor-syntax-link: #82aaff;
  --editor-syntax-code: #e6b673;
  --editor-syntax-keyword: #c5a3e7;
  --editor-syntax-comment: #8c96a8;
  --editor-syntax-invalid: #ff8a8a;
  --editor-cursor: #b8c5ff;
  --editor-selection-bg: rgba(126, 156, 255, .32);
  --editor-selection-match-bg: rgba(126, 156, 255, .2);
  --editor-search-match-bg: rgba(255, 203, 92, .28);
  --editor-search-match-border: rgba(255, 216, 132, .5);
  --editor-search-selected-bg: rgba(126, 156, 255, .4);
  color-scheme: dark;
}
.editing-toolbar { display: flex; align-items: center; gap: 3px; min-height: 40px; padding: 4px 12px; border-bottom: 1px solid var(--border-color); background: var(--titlebar-bg); overflow-x: auto; }
.editor-tool { display: grid; width: 29px; min-width: 29px; height: 29px; place-items: center; padding: 0; border: 0; border-radius: 6px; color: var(--text-secondary); background: transparent; cursor: pointer; font-size: 12px; }
.editor-tool svg { width: 17px; height: 17px; fill: none; stroke: currentColor; stroke-width: 1.75; stroke-linecap: round; stroke-linejoin: round; }
.editor-tool:hover, .editor-tool:focus-visible { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 0; }
.editor-tooltip { position: fixed; z-index: 120; transform: translateX(-50%); max-width: 180px; padding: 5px 7px; border: 1px solid color-mix(in srgb, var(--border-color) 84%, transparent); border-radius: 5px; color: var(--text-primary); background: var(--bg-primary); box-shadow: 0 5px 16px rgba(0, 0, 0, .18); pointer-events: none; font-size: 11px; line-height: 1.25; white-space: nowrap; }
.editor-toolbar-separator { width: 1px; height: 18px; margin: 0 3px; background: var(--border-color); }
.editor-more { position: relative; display: none; }
.editor-more-menu { position: absolute; z-index: 70; top: 33px; right: 0; display: grid; width: 144px; padding: 5px; border: 1px solid var(--border-color); border-radius: 7px; background: var(--bg-primary); box-shadow: 0 10px 30px rgba(0, 0, 0, .18); }
.editor-more-menu button { padding: 7px 8px; border: 0; border-radius: 4px; color: var(--text-secondary); background: transparent; font: inherit; font-size: 12px; text-align: left; }
.editor-more-menu button:hover, .editor-more-menu button:focus-visible { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 0; }
.editor-write-status { margin-left: auto; padding-left: 10px; color: var(--text-muted); font-size: 12px; white-space: nowrap; }
.editor-status-actions { display: inline-flex; gap: 4px; }
.editor-status-actions button, .editor-confirm button { padding: 5px 8px; border: 1px solid var(--border-color); border-radius: 5px; color: var(--text-secondary); background: var(--bg-primary); cursor: pointer; font: inherit; font-size: 11px; white-space: nowrap; }
.editor-status-actions button:hover, .editor-status-actions button:focus-visible, .editor-confirm button:focus-visible { color: var(--text-primary); border-color: #4c6ef5; outline: 0; }
.editor-confirm-backdrop { position: absolute; z-index: 80; inset: 0; display: grid; place-items: center; padding: 24px; background: rgba(14, 17, 24, .48); }
.editor-confirm { width: min(390px, 100%); padding: 18px; border: 1px solid var(--border-color); border-radius: 10px; color: var(--text-primary); background: var(--bg-primary); box-shadow: 0 18px 60px rgba(0, 0, 0, .28); }
.editor-confirm p { margin: 0 0 16px; line-height: 1.55; }
.editor-confirm div { display: flex; justify-content: flex-end; gap: 8px; }
.editor-confirm .editor-danger { color: #b33a35; }
.editing-workspace { display: grid; grid-template-columns: minmax(132px, 188px) minmax(320px, 1fr) minmax(320px, 1fr); min-height: 0; flex: 1; }
.editing-outline { min-width: 0; padding: 14px 8px; overflow: auto; border-right: 1px solid var(--border-color); background: var(--bg-secondary); }
.editing-outline-title { margin: 0 8px 9px; color: var(--text-muted); font-size: 11px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; }
.editing-outline button { display: block; width: 100%; padding: 6px 8px; overflow: hidden; border: 0; border-radius: 5px; color: var(--text-secondary); background: transparent; cursor: pointer; font: inherit; font-size: 11px; text-align: left; text-overflow: ellipsis; white-space: nowrap; }
.editing-outline button:hover, .editing-outline button:focus-visible { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 0; }
.editing-outline-level-2 { padding-left: 16px !important; }
.editing-outline-level-3, .editing-outline-level-4 { padding-left: 24px !important; }
.editor-host, .editing-preview { min-width: 0; min-height: 0; overflow: hidden; }
.editor-host { border-right: 1px solid var(--border-color); }
.editor-host :deep(.cm-selectionMatch) { background-color: var(--editor-selection-match-bg) !important; }
.editor-host :deep(.cm-searchMatch) { background-color: var(--editor-search-match-bg) !important; outline: 1px solid var(--editor-search-match-border); }
.editor-host :deep(.cm-searchMatch.cm-searchMatch-selected) { background-color: var(--editor-search-selected-bg) !important; }
.editor-host :deep(.cm-panels) { color: var(--text-primary) !important; background-color: var(--bg-secondary) !important; }
.editor-host :deep(.cm-panels.cm-panels-top) { border-bottom-color: var(--border-color) !important; }
.editor-host :deep(.cm-panel.cm-search) { background-color: var(--bg-secondary) !important; }
.editor-host :deep(.cm-panel.cm-search label),
.editor-host :deep(.cm-panel.cm-search [name="close"]) { color: var(--text-secondary); }
.editor-host :deep(.cm-panel.cm-search input[type="checkbox"]) { accent-color: var(--editor-cursor); }
.editor-host :deep(.cm-textfield) { border: 1px solid var(--border-color); border-radius: 4px; color: var(--text-primary) !important; background-color: var(--bg-primary) !important; }
.editor-host :deep(.cm-textfield:focus) { border-color: var(--editor-cursor); outline: 0; box-shadow: 0 0 0 2px var(--editor-selection-match-bg); }
.editor-host :deep(.cm-button) { border: 1px solid var(--border-color); border-radius: 4px; color: var(--text-secondary) !important; background-color: var(--bg-primary) !important; background-image: none !important; }
.editor-host :deep(.cm-button:hover),
.editor-host :deep(.cm-button:focus-visible) { color: var(--text-primary) !important; background-color: var(--tool-btn-hover-bg) !important; outline: 0; }
.editing-narrow-tabs { display: none; }

@media (max-width: 1120px) {
  .editing-workspace { grid-template-columns: minmax(300px, 1fr) minmax(300px, 1fr); }
  .editing-outline { display: none; }
}

@media (max-width: 900px) {
  .editing-toolbar { padding-inline: 8px; overflow: visible; }
  .editor-more { display: inline-flex; }
  .editor-tool[data-editor-command="italic"],
  .editor-tool[data-editor-command="strikethrough"],
  .editor-tool[data-editor-command="ordered-list"],
  .editor-tool[data-editor-command="task-list"],
  .editor-tool[data-editor-command="quote"],
  .editor-tool[data-editor-command="link"],
  .editor-tool[data-editor-command="inline-code"],
  .editor-tool[data-editor-command="code-block"] { display: none; }
  .editing-narrow-tabs { display: flex; height: 34px; padding: 3px 8px; border-bottom: 1px solid var(--border-color); background: var(--bg-secondary); }
  .editing-narrow-tabs button { flex: 1; border: 0; border-radius: 5px; color: var(--text-muted); background: transparent; font: inherit; font-size: 12px; }
  .editing-narrow-tabs button[aria-selected="true"] { color: var(--text-primary); background: var(--bg-primary); box-shadow: 0 1px 3px rgba(0, 0, 0, .12); }
  .editing-workspace { display: block; position: relative; }
  .editor-host, .editing-preview { position: absolute; inset: 0; }
  .editing-workspace[data-narrow-pane="source"] .editing-preview,
  .editing-workspace[data-narrow-pane="preview"] .editor-host { visibility: hidden; pointer-events: none; }
}
</style>
