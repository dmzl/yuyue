<script setup lang="ts">
const props = defineProps<{
  mode: 'reading' | 'editing'
  editLabel: string
  doneLabel: string
  disabled?: boolean
}>()

const emit = defineEmits<{
  activate: []
}>()
</script>

<template>
  <button
    class="titlebar-btn titlebar-edit-action"
    type="button"
    data-edit-document
    :disabled="props.disabled"
    :aria-label="props.mode === 'editing' ? props.doneLabel : props.editLabel"
    @click="emit('activate')"
  >
    {{ props.mode === 'editing' ? props.doneLabel : props.editLabel }}
  </button>
</template>

<style scoped>
.titlebar-edit-action:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
</style>
