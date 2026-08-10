<script setup lang="ts">
const props = defineProps<{
  searchQuery: string
  searchMatchCount: number
  searchMatchIndex: number
  fontScale: number
  contentWidth: 'comfortable' | 'wide'
  hasRemoteImages: boolean
  remoteImageAuthorized: boolean
}>()

const emit = defineEmits<{
  'update-search': [query: string]
  'search-next': []
  'search-previous': []
  'decrease-font': []
  'increase-font': []
  'reset-font': []
  'toggle-width': []
  'authorize-remote-images': []
}>()
</script>

<template>
  <div class="reader-toolbar" role="toolbar" aria-label="阅读工具">
    <label class="search-control">
      <span class="visually-hidden">搜索文档</span>
      <span class="search-icon" aria-hidden="true">⌕</span>
      <input
        :value="props.searchQuery"
        type="search"
        placeholder="搜索文档"
        aria-label="搜索文档"
        @input="emit('update-search', ($event.target as HTMLInputElement).value)"
        @keydown.enter.exact.prevent="emit('search-next')"
        @keydown.shift.enter.prevent="emit('search-previous')"
      >
      <span v-if="props.searchQuery" class="search-count" aria-live="polite">
        {{ props.searchMatchCount ? `${props.searchMatchIndex + 1}/${props.searchMatchCount}` : '无结果' }}
      </span>
    </label>
    <div class="toolbar-group" aria-label="搜索结果">
      <button type="button" class="toolbar-button" title="上一个匹配" aria-label="上一个匹配" :disabled="!props.searchMatchCount" @click="emit('search-previous')">↑</button>
      <button type="button" class="toolbar-button" title="下一个匹配" aria-label="下一个匹配" :disabled="!props.searchMatchCount" @click="emit('search-next')">↓</button>
    </div>
    <div class="toolbar-divider" aria-hidden="true"></div>
    <div class="toolbar-group" aria-label="文字大小">
      <button type="button" class="toolbar-button" title="缩小文字" aria-label="缩小文字" @click="emit('decrease-font')">A−</button>
      <button type="button" class="toolbar-value" title="恢复默认文字大小" @click="emit('reset-font')">{{ Math.round(props.fontScale * 100) }}%</button>
      <button type="button" class="toolbar-button" title="放大文字" aria-label="放大文字" @click="emit('increase-font')">A＋</button>
    </div>
    <button type="button" class="toolbar-button toolbar-width" :aria-pressed="props.contentWidth === 'wide'" title="切换内容宽度" @click="emit('toggle-width')">
      {{ props.contentWidth === 'wide' ? '窄版' : '宽版' }}
    </button>
    <button
      v-if="props.hasRemoteImages"
      type="button"
      class="toolbar-button remote-image-button"
      :disabled="props.remoteImageAuthorized"
      :title="props.remoteImageAuthorized ? '当前标签已允许远程图片' : '仅为当前标签临时加载 HTTPS 图片'"
      @click="emit('authorize-remote-images')"
    >
      {{ props.remoteImageAuthorized ? '远程图片已允许' : '加载远程图片' }}
    </button>
  </div>
</template>

<style scoped>
.reader-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 48px;
  padding: 6px 20px;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-secondary);
  color: var(--text-secondary);
}

.search-control {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 180px;
  max-width: 360px;
  flex: 1 1 260px;
  height: 32px;
  padding: 0 8px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
}

.search-control:focus-within { border-color: #4c6ef5; box-shadow: 0 0 0 2px rgba(76, 110, 245, 0.18); }
.search-icon { font-size: 18px; line-height: 1; color: var(--text-muted); }
.search-control input { min-width: 0; flex: 1; border: 0; outline: 0; color: var(--text-primary); background: transparent; font: inherit; }
.search-count { white-space: nowrap; font-size: 11px; color: var(--text-muted); }
.toolbar-group { display: inline-flex; align-items: center; gap: 2px; }
.toolbar-button, .toolbar-value {
  min-height: 32px;
  padding: 0 9px;
  border: 1px solid transparent;
  border-radius: 7px;
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
  font-size: 12px;
}
.toolbar-button:hover:not(:disabled), .toolbar-value:hover { background: var(--tool-btn-hover-bg); color: var(--text-primary); }
.toolbar-button:disabled { cursor: not-allowed; opacity: 0.45; }
.toolbar-button:focus-visible, .toolbar-value:focus-visible { outline: 2px solid #4c6ef5; outline-offset: 2px; }
.toolbar-value { min-width: 48px; font-variant-numeric: tabular-nums; }
.toolbar-width[aria-pressed="true"] { background: rgba(76, 110, 245, 0.14); color: #4c6ef5; }
.toolbar-divider { width: 1px; height: 20px; margin: 0 2px; background: var(--border-color); }
.remote-image-button { color: #4c6ef5; }
.visually-hidden { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0; }

@media (max-width: 720px) {
  .reader-toolbar { padding-inline: 10px; overflow-x: auto; }
  .search-control { min-width: 160px; }
  .remote-image-button { white-space: nowrap; }
}
</style>
