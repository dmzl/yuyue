<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, useTemplateRef, watch } from 'vue'
import { useLocale } from '../composables/useLocale'

interface OverflowTab {
  id: string
  fileName: string
  error?: string
}

const props = defineProps<{
  tabs: OverflowTab[]
  open: boolean
}>()

const emit = defineEmits<{
  activate: [tabId: string]
  'update:open': [open: boolean]
}>()

const menuId = 'tab-overflow-menu'
const menuRef = useTemplateRef<HTMLElement>('menu')
const rootRef = useTemplateRef<HTMLElement>('root')
const triggerRef = useTemplateRef<HTMLButtonElement>('trigger')
const { t } = useLocale()

function focusFirstItem() {
  menuRef.value?.querySelector<HTMLButtonElement>('.tab-overflow-item')?.focus()
}

function toggleMenu() {
  emit('update:open', !props.open)
}

function closeMenu(restoreFocus = false) {
  emit('update:open', false)
  if (restoreFocus) void nextTick(() => triggerRef.value?.focus())
}

function activateTab(tabId: string) {
  closeMenu()
  emit('activate', tabId)
}

function handleMenuKeydown(event: KeyboardEvent) {
  const items = Array.from(menuRef.value?.querySelectorAll<HTMLButtonElement>('.tab-overflow-item') ?? [])
  const currentIndex = items.findIndex((item) => item === document.activeElement)
  if (event.key === 'Escape') {
    event.preventDefault()
    closeMenu(true)
  } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    if (items.length === 0) return
    const offset = event.key === 'ArrowDown' ? 1 : -1
    items[(currentIndex + offset + items.length) % items.length]?.focus()
  } else if (event.key === 'Home') {
    event.preventDefault()
    items[0]?.focus()
  } else if (event.key === 'End') {
    event.preventDefault()
    items[items.length - 1]?.focus()
  }
}

function handleDocumentPointerDown(event: PointerEvent) {
  if (props.open && !rootRef.value?.contains(event.target as HTMLElement)) closeMenu()
}

watch(() => props.open, (isOpen) => {
  if (isOpen) void nextTick(focusFirstItem)
})
watch(() => props.tabs, (tabs) => {
  if (!props.open) return
  if (tabs.length === 0) {
    closeMenu()
    return
  }
  void nextTick(() => {
    if (!menuRef.value?.contains(document.activeElement)) focusFirstItem()
  })
})

onMounted(() => document.addEventListener('pointerdown', handleDocumentPointerDown))
onUnmounted(() => document.removeEventListener('pointerdown', handleDocumentPointerDown))
</script>

<template>
  <div ref="root" class="tab-overflow">
    <button
      ref="trigger"
      type="button"
      class="tab-overflow-trigger"
      :aria-controls="menuId"
      :aria-expanded="props.open"
      :aria-label="t('moreDocuments', { count: props.tabs.length })"
      :title="t('moreDocuments', { count: props.tabs.length })"
      aria-haspopup="menu"
      @click="toggleMenu"
    >
      <span class="tab-overflow-ellipsis" aria-hidden="true">•••</span>
      <span class="tab-overflow-count" aria-hidden="true">{{ props.tabs.length }}</span>
    </button>

    <Transition name="tab-overflow-menu">
      <div
        v-if="props.open"
        :id="menuId"
        ref="menu"
        class="tab-overflow-menu"
        role="menu"
        :aria-label="t('hiddenDocuments')"
        @keydown="handleMenuKeydown"
      >
        <button
          v-for="tab in props.tabs"
          :key="tab.id"
          type="button"
          role="menuitem"
          class="tab-overflow-item"
          :title="tab.fileName"
          @click="activateTab(tab.id)"
        >
          <span class="tab-overflow-item-icon" :class="{ 'has-error': tab.error }" aria-hidden="true">{{ tab.error ? '!' : '•' }}</span>
          <span class="tab-overflow-item-name">{{ tab.fileName }}</span>
          <span v-if="tab.error" class="tab-overflow-item-status">{{ t('needsAttention') }}</span>
        </button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.tab-overflow { position: relative; display: flex; align-items: center; flex-shrink: 0; padding-left: 4px; }
.tab-overflow-trigger { display: inline-flex; align-items: center; justify-content: center; gap: 3px; min-width: 38px; height: 28px; padding: 0 7px; border: 0; border-radius: 7px; color: var(--text-secondary); background: transparent; cursor: pointer; font: inherit; font-size: 12px; }
.tab-overflow-trigger:hover, .tab-overflow-trigger:focus-visible, .tab-overflow-trigger[aria-expanded="true"] { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 0; }
.tab-overflow-trigger:focus-visible { outline: 2px solid #4c6ef5; outline-offset: 1px; }
.tab-overflow-ellipsis { font-size: 14px; letter-spacing: -1px; line-height: 1; }
.tab-overflow-count { min-width: 9px; color: var(--text-muted); font-size: 10px; font-variant-numeric: tabular-nums; }
.tab-overflow-menu { position: absolute; top: calc(100% + 5px); right: 0; z-index: 60; width: min(310px, calc(100vw - 24px)); max-height: min(360px, calc(100vh - 72px)); padding: 5px; overflow-y: auto; border: 1px solid var(--border-color); border-radius: 10px; background: var(--bg-primary); box-shadow: 0 14px 32px rgba(20, 24, 40, 0.18); }
.tab-overflow-item { display: flex; align-items: center; width: 100%; min-height: 34px; gap: 7px; padding: 7px 8px; border: 0; border-radius: 6px; color: var(--text-secondary); background: transparent; cursor: pointer; font: inherit; font-size: 12px; text-align: left; }
.tab-overflow-item:hover, .tab-overflow-item:focus-visible { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 0; }
.tab-overflow-item:focus-visible { outline: 2px solid #4c6ef5; outline-offset: -2px; }
.tab-overflow-item-icon { width: 11px; flex-shrink: 0; color: #4c6ef5; font-size: 15px; line-height: 1; }
.tab-overflow-item-icon.has-error, .tab-overflow-item-status { color: #c2413b; }
.tab-overflow-item-name { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tab-overflow-item-status { flex-shrink: 0; font-size: 10px; }
.tab-overflow-menu-enter-active, .tab-overflow-menu-leave-active { transition: opacity 0.12s ease, transform 0.12s ease; }
.tab-overflow-menu-enter-from, .tab-overflow-menu-leave-to { opacity: 0; transform: translateY(-4px); }

@media print {
  .tab-overflow { display: none !important; }
}
</style>
