<script setup lang="ts">
import { useLocale, type Locale } from '../composables/useLocale'

const props = defineProps<{
  open: boolean
  fontScale: number
  contentWidth: 'comfortable' | 'wide'
  hasRemoteImages: boolean
  remoteImageAuthorized: boolean
}>()

const emit = defineEmits<{
  close: []
  'decrease-font': []
  'increase-font': []
  'reset-font': []
  'toggle-width': []
  'authorize-remote-images': []
}>()

const { currentLocale, localeOptions, setLocale, t } = useLocale()

</script>

<template>
  <Transition name="reader-settings">
    <div v-if="props.open" class="reader-settings-layer" data-reader-settings-backdrop @click.self="emit('close')">
      <aside class="reader-settings" :aria-label="t('readingSettings')">
        <div class="reader-settings-header">
          <span>{{ t('readingSettings') }}</span>
          <button type="button" class="reader-settings-close" :aria-label="t('closeReadingSettings')" :title="t('closeReadingSettings')" @click="emit('close')">×</button>
        </div>

        <section class="reader-settings-section" :aria-label="t('readingAppearance')">
          <span class="settings-label">{{ t('fontSize') }}</span>
          <div class="settings-row">
            <button type="button" class="settings-button" :title="t('decreaseFont')" :aria-label="t('decreaseFont')" @click="emit('decrease-font')">A−</button>
            <button type="button" class="settings-value" :title="t('resetFont')" @click="emit('reset-font')">{{ Math.round(props.fontScale * 100) }}%</button>
            <button type="button" class="settings-button" :title="t('increaseFont')" :aria-label="t('increaseFont')" @click="emit('increase-font')">A＋</button>
          </div>
          <button type="button" class="settings-width" :aria-pressed="props.contentWidth === 'wide'" @click="emit('toggle-width')">
            {{ props.contentWidth === 'wide' ? t('switchComfortableWidth') : t('switchFullWidth') }}
          </button>
        </section>

        <section class="reader-settings-section" :aria-label="t('language')">
          <label class="settings-label" for="reader-language">{{ t('language') }}</label>
          <select id="reader-language" class="settings-select" :value="currentLocale" :aria-label="t('language')" @change="setLocale(($event.target as HTMLSelectElement).value as Locale)">
            <option v-for="option in localeOptions" :key="option.value" :value="option.value">{{ option.label }}</option>
          </select>
        </section>

        <section v-if="props.hasRemoteImages" class="reader-settings-section" :aria-label="t('remoteImages')">
          <button
            type="button"
            class="settings-width remote-image-button"
            :disabled="props.remoteImageAuthorized"
            :title="props.remoteImageAuthorized ? t('remoteImagesAllowed') : t('loadRemoteImagesHint')"
            @click="emit('authorize-remote-images')"
          >
            {{ props.remoteImageAuthorized ? t('remoteImagesAllowed') : t('loadRemoteImages') }}
          </button>
        </section>
      </aside>
    </div>
  </Transition>
</template>

<style scoped>
.reader-settings-layer { position: absolute; z-index: 20; inset: 0; background: transparent; }
.reader-settings { position: absolute; top: 12px; right: 16px; width: min(310px, calc(100vw - 32px)); padding: 12px; border: 1px solid var(--border-color); border-radius: 12px; color: var(--text-primary); background: var(--bg-primary); box-shadow: 0 16px 40px rgba(20, 24, 40, 0.16); }
.reader-settings-header, .settings-row { display: flex; align-items: center; }
.reader-settings-header { justify-content: space-between; padding: 2px 2px 10px; color: var(--text-primary); font-size: 13px; font-weight: 700; }
.reader-settings-close, .settings-button, .settings-value, .settings-width, .settings-select { min-height: 30px; border: 1px solid var(--border-color); border-radius: 7px; color: var(--text-secondary); background: var(--bg-primary); cursor: pointer; font: inherit; font-size: 12px; }
.reader-settings-close { display: grid; width: 28px; place-items: center; padding: 0; border-color: transparent; font-size: 18px; line-height: 1; }
.reader-settings-section { padding: 11px 2px; border-top: 1px solid var(--border-color); }
.settings-label { display: block; margin-bottom: 8px; color: var(--text-muted); font-size: 11px; font-weight: 650; }
.settings-row { gap: 5px; }
.settings-button { min-width: 32px; padding: 0 9px; }
.settings-value { min-width: 54px; padding: 0 9px; font-variant-numeric: tabular-nums; }
.settings-width { width: 100%; margin-top: 9px; padding: 0 10px; text-align: left; }
.settings-select { width: 100%; padding: 0 9px; }
.settings-width[aria-pressed="true"] { border-color: rgba(76, 110, 245, 0.36); color: #4c6ef5; background: rgba(76, 110, 245, 0.12); }
.remote-image-button { color: #4c6ef5; }
.reader-settings-close:hover, .reader-settings-close:focus-visible, .settings-button:hover:not(:disabled), .settings-value:hover, .settings-width:hover:not(:disabled), .settings-select:hover { color: var(--text-primary); background: var(--tool-btn-hover-bg); outline: 0; }
.settings-button:disabled, .settings-width:disabled { cursor: not-allowed; opacity: 0.45; }
.reader-settings-close:focus-visible, .settings-button:focus-visible, .settings-value:focus-visible, .settings-width:focus-visible, .settings-select:focus-visible { outline: 2px solid #4c6ef5; outline-offset: 2px; }
.reader-settings-enter-active, .reader-settings-leave-active { transition: opacity 0.14s ease, transform 0.14s ease; }
.reader-settings-enter-from, .reader-settings-leave-to { opacity: 0; transform: translateY(-6px); }
</style>
