<script setup lang="ts">
defineProps<{
  fileName: string
  phase: 'importing' | 'rendering' | 'awaiting-paint'
  message: string
}>()
</script>

<template>
  <div class="document-processing-notice" :data-phase="phase" role="status" aria-live="polite" aria-atomic="true">
    <span class="processing-indicator" aria-hidden="true"></span>
    <span class="processing-copy">
      <strong>{{ fileName }}</strong>
      <span>{{ message }}</span>
    </span>
  </div>
</template>

<style scoped>
.document-processing-notice {
  position: fixed;
  z-index: 40;
  top: 52px;
  left: 50%;
  display: flex;
  max-width: min(440px, calc(100vw - 32px));
  align-items: center;
  gap: 10px;
  padding: 9px 13px 9px 11px;
  transform: translateX(-50%);
  border: 1px solid color-mix(in srgb, var(--border-color) 82%, #4c6ef5 18%);
  border-radius: 10px;
  color: var(--text-secondary);
  background: color-mix(in srgb, var(--bg-primary) 94%, transparent);
  box-shadow: 0 9px 28px rgba(24, 31, 51, 0.13);
  pointer-events: none;
  backdrop-filter: blur(14px);
  font-size: 12px;
}

.processing-indicator {
  width: 15px;
  height: 15px;
  flex: 0 0 auto;
  border: 2px solid color-mix(in srgb, #4c6ef5 24%, transparent);
  border-top-color: #4c6ef5;
  border-radius: 50%;
  animation: processing-spin 0.8s linear infinite;
}

.processing-copy {
  display: flex;
  min-width: 0;
  align-items: baseline;
  gap: 7px;
}

.processing-copy strong {
  min-width: 0;
  overflow: hidden;
  color: var(--text-primary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.processing-copy span { flex: 0 0 auto; }

@keyframes processing-spin { to { transform: rotate(360deg); } }

@media (prefers-reduced-motion: reduce) {
  .processing-indicator { animation: none; }
}
</style>
