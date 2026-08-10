<script setup lang="ts">
import { nextTick, useTemplateRef, watch } from 'vue'
import { useLocale } from '../composables/useLocale'

const props = defineProps<{
  open: boolean
  searchQuery: string
  searchMatchCount: number
  searchMatchIndex: number
}>()

const emit = defineEmits<{
  close: []
  'update-search': [query: string]
  'search-next': []
  'search-previous': []
}>()

const searchInputRef = useTemplateRef<HTMLInputElement>('searchInput')
const { t } = useLocale()

function focusSearch(select = true) {
  searchInputRef.value?.focus()
  if (select) searchInputRef.value?.select()
}

function searchNext() {
  emit('search-next')
  void nextTick(() => focusSearch(false))
}

function searchPrevious() {
  emit('search-previous')
  void nextTick(() => focusSearch(false))
}

watch(() => props.open, (open) => {
  if (open) void nextTick(focusSearch)
})

defineExpose({ focusSearch })
</script>

<template>
  <Transition name="reader-search">
    <div v-if="props.open" class="reader-search-layer" @click.self="emit('close')">
      <section class="reader-search" role="dialog" aria-modal="true" :aria-label="t('searchDocument')" @keydown.escape.prevent="emit('close')">
        <label class="reader-search-input">
          <span class="search-icon" aria-hidden="true">⌕</span>
          <input
            ref="searchInput"
            :value="props.searchQuery"
            type="search"
            :placeholder="t('searchDocument')"
            :aria-label="t('searchDocument')"
            @input="emit('update-search', ($event.target as HTMLInputElement).value)"
            @keydown.enter.exact.prevent="searchNext"
            @keydown.shift.enter.prevent="searchPrevious"
          >
          <span v-if="props.searchQuery" class="search-count" aria-live="polite">
            {{ props.searchMatchCount ? `${props.searchMatchIndex + 1}/${props.searchMatchCount}` : t('noResults') }}
          </span>
        </label>
        <div class="reader-search-actions" :aria-label="t('searchResults')">
          <button type="button" :title="t('previousMatch')" :aria-label="t('previousMatch')" :disabled="!props.searchMatchCount" @click="searchPrevious">↑</button>
          <button type="button" :title="t('nextMatch')" :aria-label="t('nextMatch')" :disabled="!props.searchMatchCount" @click="searchNext">↓</button>
          <button type="button" class="reader-search-close" :title="t('closeSearch')" :aria-label="t('closeSearch')" @click="emit('close')">×</button>
        </div>
      </section>
    </div>
  </Transition>
</template>

<style scoped>
.reader-search-layer { position: absolute; z-index: 30; inset: 0; display: flex; justify-content: center; padding: 18px 20px; background: transparent; }
.reader-search { display: flex; align-items: center; width: min(620px, 100%); height: min-content; padding: 7px; border: 1px solid var(--border-color); border-radius: 12px; color: var(--text-primary); background: var(--bg-primary); box-shadow: 0 18px 44px rgba(20, 24, 40, 0.18); }
.reader-search-input { display: flex; min-width: 0; flex: 1; align-items: center; gap: 8px; height: 38px; padding: 0 10px; border: 1px solid var(--border-color); border-radius: 8px; background: var(--bg-secondary); }
.reader-search-input:focus-within { border-color: #4c6ef5; box-shadow: 0 0 0 2px rgba(76, 110, 245, 0.18); }
.search-icon { color: var(--text-muted); font-size: 20px; line-height: 1; }
.reader-search-input input { min-width: 0; flex: 1; border: 0; outline: 0; color: var(--text-primary); background: transparent; font: inherit; font-size: 14px; }
.search-count { white-space: nowrap; color: var(--text-muted); font-size: 12px; }
.reader-search-actions { display: flex; align-items: center; gap: 4px; margin-left: 7px; }
.reader-search-actions button { display: grid; width: 32px; height: 32px; place-items: center; border: 1px solid var(--border-color); border-radius: 7px; color: var(--text-secondary); background: var(--bg-primary); cursor: pointer; font: inherit; font-size: 14px; }
.reader-search-actions button:hover:not(:disabled), .reader-search-actions button:focus-visible { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 2px solid rgba(76, 110, 245, 0.28); outline-offset: 1px; }
.reader-search-actions button:disabled { cursor: not-allowed; opacity: 0.45; }
.reader-search-close { margin-left: 2px; font-size: 19px !important; }
.reader-search-enter-active, .reader-search-leave-active { transition: opacity 0.14s ease, transform 0.14s ease; }
.reader-search-enter-from, .reader-search-leave-to { opacity: 0; transform: translateY(-6px); }
@media (max-width: 640px) { .reader-search-layer { padding: 10px; } .reader-search { align-items: stretch; flex-wrap: wrap; } .reader-search-actions { width: 100%; justify-content: flex-end; margin: 7px 0 0; } }
</style>
