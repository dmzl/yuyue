<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, shallowRef, useTemplateRef, watch } from 'vue'
import type { RenderDocument } from '../markdown/renderer'
import { useLocale } from '../composables/useLocale'
import TabOverflowMenu from './TabOverflowMenu.vue'

export interface Tab {
  id: string
  documentId: string
  fileName: string
  sourceRevision: number
  mode?: 'reading' | 'editing'
  document: RenderDocument | null
  sourceContent: string | null
  sourceBytes: number
  renderedBytes: number
  lastAccess: number
  isRendering: boolean
  renderGeneration: number
  paintOperationId?: string
  scrollRatio: number
  sourceBlockId?: string
  remoteImageAuthorized: boolean
  error?: string
}

const props = defineProps<{
  tabs: Tab[]
  activeTabId: string
}>()

const emit = defineEmits<{
  activate: [tabId: string]
  close: [tabId: string]
  closeOthers: [tabId: string]
  closeLeft: [tabId: string]
  closeRight: [tabId: string]
  closeAll: []
}>()

const contextMenu = shallowRef<{
  visible: boolean
  x: number
  y: number
  tabId: string
  tabIndex: number
}>({ visible: false, x: 0, y: 0, tabId: '', tabIndex: 0 })
const contextMenuRef = useTemplateRef<HTMLElement>('contextMenu')
const contextMenuReturnTarget = shallowRef<HTMLElement | null>(null)
const tabBarRef = useTemplateRef<HTMLElement>('tabBar')
const tabStripRef = useTemplateRef<HTMLElement>('tabStrip')
const availableWidth = shallowRef(0)
const overflowOpen = shallowRef(false)
const { t } = useLocale()
const MIN_TAB_WIDTH = 168
const OVERFLOW_TRIGGER_WIDTH = 50
let tabResizeObserver: { disconnect: () => void; observe: (target: Element) => void } | undefined

const visibleTabCount = computed(() => {
  const tabCount = props.tabs.length
  if (tabCount === 0) return 0
  if (availableWidth.value <= 0) return tabCount
  if (tabCount * MIN_TAB_WIDTH <= availableWidth.value) return tabCount
  return Math.max(1, Math.floor((availableWidth.value - OVERFLOW_TRIGGER_WIDTH) / MIN_TAB_WIDTH))
})
const visibleStartIndex = computed(() => {
  const count = visibleTabCount.value
  if (count >= props.tabs.length) return 0
  const activeIndex = Math.max(0, props.tabs.findIndex((tab) => tab.id === props.activeTabId))
  const preferredStart = activeIndex - Math.floor(count / 2)
  return Math.min(Math.max(preferredStart, 0), props.tabs.length - count)
})
const visibleTabs = computed(() => props.tabs.slice(visibleStartIndex.value, visibleStartIndex.value + visibleTabCount.value))
const hiddenTabs = computed(() => {
  const visibleIds = new Set(visibleTabs.value.map((tab) => tab.id))
  return props.tabs.filter((tab) => !visibleIds.has(tab.id))
})

function measureAvailableWidth() {
  availableWidth.value = tabBarRef.value?.clientWidth ?? 0
}

function scheduleAvailableWidthMeasurement() {
  void nextTick(() => {
    if (typeof window.requestAnimationFrame === 'function') {
      window.requestAnimationFrame(measureAvailableWidth)
    } else {
      measureAvailableWidth()
    }
  })
}

function activateByOffset(tabId: string, offset: number) {
  const index = props.tabs.findIndex((tab) => tab.id === tabId)
  if (index < 0 || props.tabs.length === 0) return
  const nextIndex = (index + offset + props.tabs.length) % props.tabs.length
  activateAndFocus(props.tabs[nextIndex].id)
}

function activateAndFocus(tabId: string) {
  emit('activate', tabId)
  void nextTick(() => {
    const target = Array.from(tabStripRef.value?.querySelectorAll<HTMLButtonElement>('[data-tab-id]') ?? [])
      .find((element) => element.dataset.tabId === tabId)
    target?.focus()
  })
}

function focusActiveTab() {
  const target = Array.from(tabStripRef.value?.querySelectorAll<HTMLButtonElement>('[data-tab-id]') ?? [])
    .find((element) => element.dataset.tabId === props.activeTabId)
  target?.focus()
}

function handleTabKeydown(event: KeyboardEvent, tabId: string) {
  if (event.key === 'ArrowRight') {
    event.preventDefault()
    activateByOffset(tabId, 1)
  } else if (event.key === 'ArrowLeft') {
    event.preventDefault()
    activateByOffset(tabId, -1)
  } else if (event.key === 'Home') {
    event.preventDefault()
    activateAndFocus(props.tabs[0]?.id ?? tabId)
  } else if (event.key === 'End') {
    event.preventDefault()
    activateAndFocus(props.tabs[props.tabs.length - 1]?.id ?? tabId)
  } else if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    emit('activate', tabId)
  } else if (event.key === 'Delete' || (event.key === 'w' && (event.metaKey || event.ctrlKey))) {
    event.preventDefault()
    emit('close', tabId)
  }
}

function handleContextMenu(event: MouseEvent, tab: Tab) {
  event.preventDefault()
  contextMenuReturnTarget.value = event.currentTarget as HTMLElement
  contextMenu.value = {
    visible: true,
    x: Math.min(event.clientX, window.innerWidth - 180),
    y: Math.min(event.clientY, window.innerHeight - 210),
    tabId: tab.id,
    tabIndex: props.tabs.findIndex((item) => item.id === tab.id),
  }
  void nextTick(() => contextMenuRef.value?.querySelector<HTMLButtonElement>('.context-menu-item:not(:disabled)')?.focus())
}

function closeContextMenu(restoreFocus = false) {
  contextMenu.value = { ...contextMenu.value, visible: false }
  if (restoreFocus) void nextTick(() => contextMenuReturnTarget.value?.focus())
}

function handleMenuAction(action: 'close' | 'closeOthers' | 'closeLeft' | 'closeRight' | 'closeAll') {
  const tabId = contextMenu.value.tabId
  closeContextMenu()
  switch (action) {
    case 'close': emit('close', tabId); break
    case 'closeOthers': emit('closeOthers', tabId); break
    case 'closeLeft': emit('closeLeft', tabId); break
    case 'closeRight': emit('closeRight', tabId); break
    case 'closeAll': emit('closeAll'); break
  }
}

function handleContextMenuKeydown(event: KeyboardEvent) {
  const items = Array.from(contextMenuRef.value?.querySelectorAll<HTMLButtonElement>('.context-menu-item:not(:disabled)') ?? [])
  const currentIndex = items.findIndex((item) => item === document.activeElement)
  if (event.key === 'Escape') {
    event.preventDefault()
    closeContextMenu(true)
  } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    if (items.length === 0) return
    const offset = event.key === 'ArrowDown' ? 1 : -1
    items[(currentIndex + offset + items.length) % items.length]?.focus()
  }
}

function handleGlobalClick() {
  if (contextMenu.value.visible) closeContextMenu()
}

watch([() => props.tabs.length, () => props.activeTabId], () => {
  scheduleAvailableWidthMeasurement()
}, { flush: 'post' })
watch(hiddenTabs, (tabs) => {
  if (tabs.length === 0 && overflowOpen.value) {
    overflowOpen.value = false
    void nextTick(focusActiveTab)
  }
})

onMounted(() => {
  document.addEventListener('click', handleGlobalClick)
  window.addEventListener('resize', measureAvailableWidth)
  scheduleAvailableWidthMeasurement()
  if (window.ResizeObserver && tabBarRef.value) {
    tabResizeObserver = new window.ResizeObserver(measureAvailableWidth)
    tabResizeObserver.observe(tabBarRef.value)
  }
})
onUnmounted(() => {
  document.removeEventListener('click', handleGlobalClick)
  window.removeEventListener('resize', measureAvailableWidth)
  tabResizeObserver?.disconnect()
})
</script>

<template>
  <div ref="tabBar" class="tab-bar">
    <div ref="tabStrip" class="tab-strip" role="tablist" :aria-label="t('openDocuments')">
      <div
        v-for="tab in visibleTabs"
        :key="tab.id"
        :class="['tab-item', { active: tab.id === props.activeTabId, 'has-error': tab.error }]"
        role="presentation"
      >
        <button
          type="button"
          role="tab"
          class="tab-select"
          :data-tab-id="tab.id"
          :aria-label="t('switchTo', { name: tab.fileName })"
          :aria-selected="tab.id === props.activeTabId"
          :tabindex="tab.id === props.activeTabId ? 0 : -1"
          @click="emit('activate', tab.id)"
          @keydown="handleTabKeydown($event, tab.id)"
          @contextmenu="handleContextMenu($event, tab)"
        >
          <span class="tab-icon" aria-hidden="true">{{ tab.error ? '!' : '•' }}</span>
          <span class="tab-name">{{ tab.fileName }}</span>
          <span v-if="tab.error" class="tab-status" :title="t('documentHasError')">{{ t('needsAttention') }}</span>
          <span v-if="tab.id === props.activeTabId && !tab.error" class="tab-live" aria-hidden="true"></span>
        </button>
        <button
          type="button"
          class="tab-close"
          :aria-label="t('closeDocument')"
          :title="t('closeDocument')"
          @click.stop="emit('close', tab.id)"
          @keydown.enter.stop="emit('close', tab.id)"
          @keydown.space.prevent.stop="emit('close', tab.id)"
        >
          ×
        </button>
      </div>
    </div>
    <TabOverflowMenu v-if="hiddenTabs.length > 0" v-model:open="overflowOpen" :tabs="hiddenTabs" @activate="activateAndFocus" />
  </div>

  <Teleport to="body">
    <Transition name="context-menu">
      <div
        v-if="contextMenu.visible"
        ref="contextMenu"
        class="context-menu"
        role="menu"
        :aria-label="t('documentTabActions')"
        :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
        @keydown="handleContextMenuKeydown"
      >
        <button type="button" role="menuitem" class="context-menu-item" @click="handleMenuAction('close')">{{ t('close') }}</button>
        <button type="button" role="menuitem" class="context-menu-item" @click="handleMenuAction('closeOthers')">{{ t('closeOthers') }}</button>
        <button type="button" role="menuitem" class="context-menu-item" :disabled="contextMenu.tabIndex === 0" @click="handleMenuAction('closeLeft')">{{ t('closeLeft') }}</button>
        <button type="button" role="menuitem" class="context-menu-item" :disabled="contextMenu.tabIndex === props.tabs.length - 1" @click="handleMenuAction('closeRight')">{{ t('closeRight') }}</button>
        <div class="context-menu-separator" aria-hidden="true"></div>
        <button type="button" role="menuitem" class="context-menu-item" @click="handleMenuAction('closeAll')">{{ t('closeAll') }}</button>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.tab-bar { position: relative; display: flex; align-items: stretch; min-width: 0; height: 100%; }
.tab-strip { display: flex; min-width: 0; flex: 1; overflow: hidden; }
.tab-item { position: relative; display: flex; align-items: center; min-width: 168px; max-width: 240px; height: 38px; padding: 0 9px 0 12px; flex: 1 1 192px; border-right: 1px solid var(--border-color); border-radius: 8px 8px 0 0; color: var(--text-muted); background: transparent; user-select: none; }
.tab-item:hover { background: var(--tool-btn-hover-bg); color: var(--text-primary); }
.tab-item.active { color: var(--text-primary); background: var(--bg-primary); box-shadow: inset 0 -2px #4c6ef5; }
.tab-select { display: flex; align-items: center; gap: 7px; min-width: 0; flex: 1; height: 100%; padding: 0; border: 0; color: inherit; background: transparent; cursor: pointer; font: inherit; font-size: 12px; text-align: left; }
.tab-select:focus-visible { outline: 2px solid #4c6ef5; outline-offset: -2px; }
.tab-icon { width: 12px; color: #4c6ef5; font-size: 15px; line-height: 1; }
.tab-item.has-error .tab-icon, .tab-status { color: #c2413b; }
.tab-name { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tab-status { font-size: 10px; white-space: nowrap; }
.tab-live { width: 6px; height: 6px; border-radius: 50%; background: #4c6ef5; }
.tab-close { display: grid; width: 20px; height: 20px; place-items: center; flex-shrink: 0; border: 0; border-radius: 5px; color: var(--text-muted); background: transparent; cursor: pointer; font: inherit; font-size: 16px; line-height: 1; }
.tab-close:hover, .tab-close:focus-visible { color: #c2413b; background: rgba(194, 65, 59, 0.14); outline: 0; }
.context-menu { position: fixed; z-index: 10000; min-width: 170px; padding: 5px; border: 1px solid var(--border-color); border-radius: 10px; background: var(--bg-primary); box-shadow: 0 12px 28px rgba(20, 24, 40, 0.18); }
.context-menu-item { display: block; width: 100%; padding: 8px 10px; border: 0; border-radius: 6px; color: var(--text-primary); background: transparent; cursor: pointer; font: inherit; font-size: 13px; text-align: left; }
.context-menu-item:hover:not(:disabled), .context-menu-item:focus-visible:not(:disabled) { background: var(--tool-btn-hover-bg); outline: 0; }
.context-menu-item:disabled { cursor: not-allowed; color: var(--text-muted); opacity: 0.45; }
.context-menu-separator { height: 1px; margin: 4px 0; background: var(--border-color); }
.context-menu-enter-active, .context-menu-leave-active { transition: opacity 0.12s ease, transform 0.12s ease; }
.context-menu-enter-from, .context-menu-leave-to { opacity: 0; transform: scale(0.96); }

@media print {
  .context-menu { display: none !important; }
}
</style>
