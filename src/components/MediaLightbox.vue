<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, shallowRef, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useLocale } from '../composables/useLocale'

export type LightboxMedia =
  | { kind: 'image'; src: string; alt: string }
  | { kind: 'svg'; element: SVGSVGElement; alt: string }

const props = defineProps<{
  media: LightboxMedia | null
}>()

const emit = defineEmits<{
  close: []
}>()

const MIN_ZOOM = 0.5
const MAX_ZOOM = Number.POSITIVE_INFINITY
const DEFAULT_ZOOM = 1
const ZOOM_STEP = 0.25
const SVG_CANVAS_PADDING = 24
const hasMaxZoom = Number.isFinite(MAX_ZOOM)

const svgContainer = ref<HTMLDivElement | null>(null)
const dialogRef = ref<HTMLDivElement | null>(null)
const closeButtonRef = ref<HTMLButtonElement | null>(null)
const previousFocus = shallowRef<HTMLElement | null>(null)
const zoom = shallowRef(DEFAULT_ZOOM)
const panX = shallowRef(0)
const panY = shallowRef(0)
const isDragging = shallowRef(false)
const hasMoved = shallowRef(false)
const pointerId = shallowRef<number | null>(null)
const pointerTarget = shallowRef<HTMLElement | null>(null)
const dragStartX = shallowRef(0)
const dragStartY = shallowRef(0)
const dragStartPanX = shallowRef(0)
const dragStartPanY = shallowRef(0)
const isPointerAttached = shallowRef(false)
const suppressNextClick = shallowRef(false)
const exportStatus = shallowRef('')
const { t } = useLocale()

const DRAG_THRESHOLD = 4

const zoomPercent = computed(() => Math.round(zoom.value * 100))
const canZoomOut = computed(() => zoom.value > MIN_ZOOM)
const canZoomIn = computed(() => (hasMaxZoom ? zoom.value < MAX_ZOOM : true))
const isZoomed = computed(() => zoom.value !== DEFAULT_ZOOM)
const canPan = computed(() => props.media !== null)
const svgBaseSize = shallowRef({ width: 0, height: 0 })

function parseNumericLength(value: string | null | undefined): number | null {
  const numericMatch = value?.match(/^(\d*\.?\d+)/)
  if (!numericMatch || !numericMatch[1]) return null
  const parsed = Number(numericMatch[1])
  return Number.isFinite(parsed) ? parsed : null
}

const mediaTransformStyle = computed(() => {
  const base = {
    cursor: isDragging.value ? 'grabbing' : canPan.value ? 'grab' : 'default',
    transition: isDragging.value ? 'none' : 'transform 0.16s ease-out',
  }

  if (props.media?.kind === 'svg' && svgBaseSize.value.width > 0 && svgBaseSize.value.height > 0) {
    return {
      ...base,
      transform: `translate(${panX.value}px, ${panY.value}px)`,
      width: `${svgBaseSize.value.width * zoom.value + SVG_CANVAS_PADDING * 2}px`,
      height: `${svgBaseSize.value.height * zoom.value + SVG_CANVAS_PADDING * 2}px`,
      maxWidth: 'none',
      maxHeight: 'none',
    }
  }

  return {
    ...base,
    transform: `translate(${panX.value}px, ${panY.value}px) scale(${zoom.value})`,
  }
})

function renderSvg() {
  if (!svgContainer.value || props.media?.kind !== 'svg') return
  const cloned = props.media.element.cloneNode(true)
  if (!(cloned instanceof SVGSVGElement)) return

  const viewBox = cloned.getAttribute('viewBox')
  const width = viewBox?.split(' ').map((item) => Number(item)).filter((n) => Number.isFinite(n))
  const rawWidth = cloned.getAttribute('width')
  const rawHeight = cloned.getAttribute('height')
  const hasNumericWidth = rawWidth ? /^\d*\.?\d+(?:px)?$/.test(rawWidth.trim()) : false
  const hasNumericHeight = rawHeight ? /^\d*\.?\d+(?:px)?$/.test(rawHeight.trim()) : false
  const attrWidth = hasNumericWidth ? parseNumericLength(rawWidth) : null
  const attrHeight = hasNumericHeight ? parseNumericLength(rawHeight) : null

  let svgWidth = attrWidth
  let svgHeight = attrHeight

  if (!hasNumericWidth) {
    cloned.removeAttribute('width')
  }
  if (!hasNumericHeight) {
    cloned.removeAttribute('height')
  }

  if (width && width.length >= 4) {
    const [, , vbWidth, vbHeight] = width
    svgWidth = svgWidth || vbWidth
    svgHeight = svgHeight || vbHeight
    if (!hasNumericWidth) cloned.setAttribute('width', String(svgWidth))
    if (!hasNumericHeight) cloned.setAttribute('height', String(svgHeight))
  }

  const safeWidth = svgWidth || 960
  const safeHeight = svgHeight || 540
  if (!hasNumericWidth) {
    cloned.setAttribute('width', String(safeWidth))
  }
  if (!hasNumericHeight) {
    cloned.setAttribute('height', String(safeHeight))
  }
  svgBaseSize.value = { width: safeWidth, height: safeHeight }

  cloned.setAttribute('preserveAspectRatio', 'xMidYMid meet')
  cloned.style.width = '100%'
  cloned.style.height = '100%'
  cloned.style.maxWidth = 'none'
  cloned.style.maxHeight = 'none'
  cloned.style.display = 'block'

  svgContainer.value.replaceChildren(cloned)
}

function sanitizePngFileName(name: string) {
  const cleaned = name
    .trim()
    .replace(/[<>:"/\\|?*\u0000-\u001f]+/g, '_')
    .replace(/\s+/g, ' ')
    .replace(/[. ]+$/g, '')
    .replace(/\.(md|markdown|txt)$/i, '')

  return `${cleaned || 'diagram'}.png`
}

function serializeCurrentSvg() {
  if (props.media?.kind !== 'svg') return null
  const currentSvg = svgContainer.value?.querySelector('svg') ?? props.media.element
  const cloned = currentSvg.cloneNode(true)
  if (!(cloned instanceof SVGSVGElement)) return null
  if (!cloned.getAttribute('xmlns')) {
    cloned.setAttribute('xmlns', 'http://www.w3.org/2000/svg')
  }
  return new XMLSerializer().serializeToString(cloned)
}

async function exportPngFile() {
  const svg = serializeCurrentSvg()
  if (!svg || props.media?.kind !== 'svg') return

  exportStatus.value = ''
  try {
    await invoke('export_png_dialog', {
      svg,
      suggestedFileName: sanitizePngFileName(props.media.alt || 'diagram'),
    })
    exportStatus.value = t('pngExported')
  } catch (error) {
    exportStatus.value = error === 'EXPORT_RENDER_LIMIT' ? t('pngExportLimit') : t('pngExportFailed')
  }
}

function resetDragState() {
  const target = pointerTarget.value
  const activePointerId = pointerId.value
  if (target && activePointerId !== null) {
    try {
      if (target.hasPointerCapture(activePointerId)) {
        target.releasePointerCapture(activePointerId)
      }
    } catch {
      // The pointer may already have been released while the media changed.
    }
  }

  isDragging.value = false
  hasMoved.value = false
  pointerId.value = null
  pointerTarget.value = null

  if (isPointerAttached.value) {
    window.removeEventListener('pointermove', handlePointerMove)
    window.removeEventListener('pointerup', endPan)
    window.removeEventListener('pointercancel', endPan)
    window.removeEventListener('mousemove', handleMouseMove)
    window.removeEventListener('mouseup', endMousePan)
    isPointerAttached.value = false
  }
}

function resetPan() {
  panX.value = 0
  panY.value = 0
}

function resetViewer() {
  resetDragState()
  zoom.value = DEFAULT_ZOOM
  resetPan()
}

function setZoom(nextZoom: number) {
  const normalizedMax = hasMaxZoom ? Math.min(nextZoom, MAX_ZOOM) : nextZoom
  const clampedZoom = Math.max(MIN_ZOOM, normalizedMax)
  zoom.value = Number(clampedZoom.toFixed(2))
  if (zoom.value <= DEFAULT_ZOOM) resetPan()
}

function zoomOut() {
  setZoom(zoom.value - ZOOM_STEP)
}

function zoomIn() {
  setZoom(zoom.value + ZOOM_STEP)
}

function handleWheel(event: WheelEvent) {
  event.preventDefault()
  event.stopPropagation()
  if (event.deltaY === 0) return
  setZoom(zoom.value + (event.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP))
}

function startPan(event: PointerEvent) {
  if (isDragging.value) return
  if (event.pointerType === 'mouse' && event.button !== 0) return

  const target = (event.currentTarget instanceof HTMLElement ? event.currentTarget : event.target) || null
  if (!(target instanceof HTMLElement)) return

  resetDragState()
  event.preventDefault()
  event.stopPropagation()

  pointerId.value = event.pointerId
  pointerTarget.value = target
  dragStartX.value = event.clientX
  dragStartY.value = event.clientY
  dragStartPanX.value = panX.value
  dragStartPanY.value = panY.value
  hasMoved.value = false
  suppressNextClick.value = false
  isDragging.value = true
  if (!isPointerAttached.value) {
    try {
      target.setPointerCapture(event.pointerId)
    } catch {
      // Pointer capture may fail on some platforms; window listeners still handle dragging.
    }
    window.addEventListener('pointermove', handlePointerMove, { passive: false })
    window.addEventListener('pointerup', endPan, { passive: false })
    window.addEventListener('pointercancel', endPan, { passive: false })
    isPointerAttached.value = true
  }
}

function startPanMouse(event: MouseEvent) {
  if (isDragging.value) return
  if (event.button !== 0) return
  const target = (event.currentTarget instanceof HTMLElement ? event.currentTarget : event.target) || null
  if (!(target instanceof HTMLElement)) return

  resetDragState()
  event.preventDefault()
  event.stopPropagation()

  pointerId.value = -1
  pointerTarget.value = target
  dragStartX.value = event.clientX
  dragStartY.value = event.clientY
  dragStartPanX.value = panX.value
  dragStartPanY.value = panY.value
  hasMoved.value = false
  suppressNextClick.value = false
  isDragging.value = true

  if (!isPointerAttached.value) {
    window.addEventListener('mousemove', handleMouseMove, { passive: false })
    window.addEventListener('mouseup', endMousePan, { passive: false })
    isPointerAttached.value = true
  }
}

function handlePointerMove(event: PointerEvent) {
  if (!isDragging.value) return

  event.preventDefault()
  event.stopPropagation()

  if (!hasMoved.value) {
    const deltaX = event.clientX - dragStartX.value
    const deltaY = event.clientY - dragStartY.value
    if (Math.abs(deltaX) >= DRAG_THRESHOLD || Math.abs(deltaY) >= DRAG_THRESHOLD) {
      hasMoved.value = true
    }
  }

  panX.value = dragStartPanX.value + event.clientX - dragStartX.value
  panY.value = dragStartPanY.value + event.clientY - dragStartY.value
}

function handleMouseMove(event: MouseEvent) {
  if (!isDragging.value) return
  event.preventDefault()
  if (!hasMoved.value) {
    const deltaX = event.clientX - dragStartX.value
    const deltaY = event.clientY - dragStartY.value
    if (Math.abs(deltaX) >= DRAG_THRESHOLD || Math.abs(deltaY) >= DRAG_THRESHOLD) {
      hasMoved.value = true
    }
  }
  panX.value = dragStartPanX.value + event.clientX - dragStartX.value
  panY.value = dragStartPanY.value + event.clientY - dragStartY.value
}

function endPan(event?: PointerEvent) {
  if (event && pointerId.value !== null && pointerId.value >= 0 && event.pointerId !== pointerId.value) return
  event?.stopPropagation()
  event?.preventDefault()
  if (hasMoved.value) suppressNextClick.value = true
  resetDragState()
}

function endMousePan(event?: MouseEvent) {
  event?.stopPropagation()
  event?.preventDefault()
  if (hasMoved.value) suppressNextClick.value = true
  resetDragState()
}

function handleStageClick(event: MouseEvent) {
  if (suppressNextClick.value) {
    event.preventDefault()
    event.stopPropagation()
    suppressNextClick.value = false
    return
  }
  closeLightbox()
}

function resetZoom() {
  resetViewer()
}

function closeLightbox() {
  resetViewer()
  emit('close')
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    closeLightbox()
    return
  }
  if (event.key !== 'Tab') return
  const dialog = dialogRef.value
  if (!dialog) return
  const focusable = Array.from(dialog.querySelectorAll<HTMLElement>('button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])'))
  if (focusable.length === 0) {
    event.preventDefault()
    dialog.focus()
    return
  }
  const first = focusable[0]
  const last = focusable[focusable.length - 1]
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first.focus()
  }
}

watch(
  () => props.media,
  async (media) => {
    resetViewer()
    if (!media) {
      const target = previousFocus.value
      previousFocus.value = null
      if (target?.isConnected) target.focus()
      return
    }
    if (!previousFocus.value) {
      previousFocus.value = document.activeElement instanceof HTMLElement ? document.activeElement : null
    }
    await nextTick()
    closeButtonRef.value?.focus()
    svgBaseSize.value = { width: 0, height: 0 }
    renderSvg()
  },
  { immediate: true }
)

onUnmounted(() => {
  resetDragState()
  const target = previousFocus.value
  previousFocus.value = null
  if (target?.isConnected) target.focus()
})
</script>

<template>
  <Teleport to="body">
    <Transition name="lightbox-fade">
      <div
        v-if="media"
        ref="dialogRef"
        class="media-lightbox"
        role="dialog"
        aria-modal="true"
        tabindex="-1"
        :aria-label="media.alt || t('imagePreview')"
        @keydown="handleKeydown"
        @click.self="closeLightbox"
      >
        <div class="media-lightbox-toolbar" role="toolbar" :aria-label="t('imagePreviewTools')">
          <span class="media-lightbox-caption">{{ media.alt || t('imagePreview') }}</span>
          <div class="media-lightbox-controls">
            <button
              class="media-lightbox-control"
              type="button"
              :aria-label="t('zoomOut')"
              :title="t('zoomOut')"
              :disabled="!canZoomOut"
              @click="zoomOut"
            >
              <span aria-hidden="true">−</span>
            </button>
            <button
              class="media-lightbox-zoom-value"
              type="button"
              :class="{ 'is-active': isZoomed }"
              :aria-label="t('zoomStatus', { percent: zoomPercent })"
              :title="t('resetZoom')"
              @click="resetZoom"
            >
              {{ zoomPercent }}%
            </button>
            <button
              class="media-lightbox-control"
              type="button"
              :aria-label="t('zoomIn')"
              :title="t('zoomIn')"
              :disabled="!canZoomIn"
              @click="zoomIn"
            >
              <span aria-hidden="true">＋</span>
            </button>
            <button
              v-if="media.kind === 'svg'"
              class="media-lightbox-export"
              type="button"
              :aria-label="t('exportPng')"
              :title="t('exportPng')"
              @click="exportPngFile"
            >
              {{ t('exportPng') }}
            </button>
            <button ref="closeButtonRef" class="media-lightbox-close" type="button" :aria-label="t('closeImagePreview')" :title="t('closeImagePreview')" @click="closeLightbox">
              <span aria-hidden="true">×</span>
            </button>
            <span v-if="exportStatus" class="media-lightbox-export-status" role="status" aria-live="polite">{{ exportStatus }}</span>
          </div>
        </div>

        <div
          class="media-lightbox-stage"
          @wheel.prevent.stop="handleWheel"
          @click.self="handleStageClick"
        >
          <div class="media-lightbox-media-shell" :style="mediaTransformStyle">
            <img
              v-if="media.kind === 'image'"
              class="media-lightbox-image"
              :src="media.src"
              :alt="media.alt"
              referrerpolicy="no-referrer"
              draggable="false"
              @dragstart.prevent
            />
            <div
              v-else
              ref="svgContainer"
              class="media-lightbox-svg"
            ></div>
            <div
              v-if="canPan"
              class="media-lightbox-pan-capture"
              :style="{ cursor: isDragging ? 'grabbing' : 'grab' }"
              @pointerdown="startPan"
              @pointermove="handlePointerMove"
              @pointerup="endPan"
              @mousedown="startPanMouse"
              @mousemove="handleMouseMove"
              @mouseup="endMousePan"
              @click.stop
              @dragstart.prevent
            />
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.media-lightbox {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  flex-direction: column;
  padding: 18px 22px;
  background: rgba(15, 15, 35, 0.88);
  backdrop-filter: blur(8px);
}

.media-lightbox-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  min-height: 40px;
  color: #f7fafc;
}

.media-lightbox-caption {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.875rem;
  opacity: 0.82;
}

.media-lightbox-controls {
  display: flex;
  flex: 0 1 auto;
  align-items: center;
  justify-content: flex-end;
  gap: 6px;
  flex-wrap: wrap;
}

.media-lightbox-control,
.media-lightbox-zoom-value,
.media-lightbox-export,
.media-lightbox-close {
  display: grid;
  flex: 0 0 auto;
  place-items: center;
  width: 36px;
  height: 36px;
  border: 1px solid rgba(255, 255, 255, 0.22);
  border-radius: 50%;
  color: #f7fafc;
  background: rgba(255, 255, 255, 0.1);
  cursor: pointer;
  line-height: 1;
  transition: background 0.15s ease, transform 0.15s ease;
}

.media-lightbox-export-status { color: #dce5ff; font-size: 0.75rem; }

.media-lightbox-control,
.media-lightbox-zoom-value {
  min-width: 36px;
  padding: 0 8px;
  color: #f7fafc;
  font-size: 1.2rem;
}

.media-lightbox-export {
  min-width: 92px;
  padding: 0 12px;
  color: #f7fafc;
  font-size: 0.78rem;
  letter-spacing: 0.02em;
}

.media-lightbox-zoom-value {
  min-width: 62px;
  border-color: rgba(255, 255, 255, 0.36);
  font-size: 0.8rem;
  font-variant-numeric: tabular-nums;
}

.media-lightbox-zoom-value.is-active {
  color: #ffffff;
  background: rgba(76, 110, 245, 0.42);
  border-color: rgba(164, 180, 255, 0.72);
}

.media-lightbox-close {
  font-size: 1.6rem;
}

.media-lightbox-control:hover:not(:disabled),
.media-lightbox-zoom-value:hover,
.media-lightbox-export:hover,
.media-lightbox-close:hover {
  background: rgba(255, 255, 255, 0.2);
  transform: scale(1.05);
}

.media-lightbox-control:disabled {
  opacity: 0.36;
  cursor: not-allowed;
}

.media-lightbox-control:focus-visible,
.media-lightbox-zoom-value:focus-visible,
.media-lightbox-export:focus-visible,
.media-lightbox-close:focus-visible {
  outline: 2px solid rgba(164, 180, 255, 0.95);
  outline-offset: 2px;
}

.media-lightbox-stage {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: center;
  min-height: 0;
  overflow: hidden;
  padding: 30px 0;
  overscroll-behavior: contain;
  touch-action: none;
  user-select: none;
}

.media-lightbox-media-shell {
  position: relative;
  flex: 0 0 auto;
  width: fit-content;
  max-width: min(94vw, 1400px);
  max-height: calc(100vh - 150px);
  border-radius: 10px;
  box-shadow: 0 24px 80px rgba(0, 0, 0, 0.4);
  overflow: hidden;
}

.media-lightbox-image,
.media-lightbox-svg {
  flex: 0 0 auto;
  touch-action: none;
  user-select: none;
  -webkit-user-drag: none;
  pointer-events: none;
}

.media-lightbox-image {
  display: block;
  width: auto;
  height: auto;
  max-width: min(94vw, 1400px);
  max-height: calc(100vh - 150px);
  object-fit: contain;
  transform-origin: 0 0;
}

.media-lightbox-svg {
  box-sizing: border-box;
  display: flex;
  align-items: flex-start;
  justify-content: flex-start;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  padding: 24px;
  background: #ffffff;
}

.media-lightbox-pan-capture {
  position: absolute;
  inset: 0;
  z-index: 1;
  display: flex;
  background: transparent;
  touch-action: none;
  user-select: none;
}

.media-lightbox-svg :deep(svg) {
  display: block;
  width: 100%;
  height: 100%;
  transform-origin: 0 0;
  shape-rendering: geometricPrecision;
  text-rendering: geometricPrecision;
  pointer-events: none;
}

.lightbox-fade-enter-active,
.lightbox-fade-leave-active {
  transition: opacity 0.18s ease;
}

.lightbox-fade-enter-from,
.lightbox-fade-leave-to {
  opacity: 0;
}

@media print {
  .media-lightbox { display: none !important; }
}

@media (max-width: 640px) {
  .media-lightbox {
    padding: 12px;
  }

  .media-lightbox-stage {
    padding: 8px 0 16px;
  }
}
</style>
