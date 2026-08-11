import { computed, onMounted, onUnmounted, ref, shallowRef, watchEffect } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useLocale } from './useLocale'

export type Theme = 'light' | 'dark' | 'system'

const THEME_KEY = 'mdreader-theme'
const storedTheme = typeof localStorage === 'undefined' ? null : localStorage.getItem(THEME_KEY)
const currentTheme = ref<Theme>(storedTheme === 'dark' || storedTheme === 'system' ? storedTheme : 'light')
const systemTheme = ref<'light' | 'dark'>('light')
const resolvedTheme = computed(() => currentTheme.value === 'system' ? systemTheme.value : currentTheme.value)
const themeIcon = computed(() => currentTheme.value === 'light' ? '☀' : currentTheme.value === 'dark' ? '☾' : '◐')
const { t } = useLocale()
function themeLabel(theme: Theme | 'light' | 'dark') {
  return theme === 'system'
    ? t('themeFollowingSystem', { theme: t(systemTheme.value === 'dark' ? 'themeDark' : 'themeLight') })
    : t(theme === 'dark' ? 'themeDark' : 'themeLight')
}
const themeModeLabel = computed(() => themeLabel(currentTheme.value))
const nextThemeLabel = computed(() => themeLabel(currentTheme.value === 'light' ? 'dark' : currentTheme.value === 'dark' ? 'system' : 'light'))
const themeButtonLabel = computed(() => t('currentThemeAction', { current: themeModeLabel.value, next: nextThemeLabel.value }))
const themeStatus = shallowRef('')
let themeStatusTimer: ReturnType<typeof setTimeout> | undefined

let systemMediaQuery: MediaQueryList | null = null
function syncSystemTheme() {
  systemTheme.value = systemMediaQuery?.matches ? 'dark' : 'light'
}

function syncNativeWindowTheme(theme: Theme) {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return
  void invoke('set_native_window_theme', { theme })
    .then(() => {
      if (theme === 'system') window.requestAnimationFrame(syncSystemTheme)
    })
    .catch(() => {})
}

export function useTheme() {
  function setTheme(theme: Theme) {
    currentTheme.value = theme
    localStorage.setItem(THEME_KEY, theme)
  }

  function toggleTheme() {
    const nextTheme: Theme = currentTheme.value === 'light' ? 'dark' : currentTheme.value === 'dark' ? 'system' : 'light'
    setTheme(nextTheme)
    themeStatus.value = t('themeChanged', { theme: themeLabel(nextTheme) })
    if (themeStatusTimer) clearTimeout(themeStatusTimer)
    themeStatusTimer = setTimeout(() => { themeStatus.value = '' }, 2_400)
  }

  watchEffect(() => {
    const theme = resolvedTheme.value
    document.documentElement.setAttribute('data-theme', theme)
    syncNativeWindowTheme(currentTheme.value)
  })

  onMounted(() => {
    if (typeof window.matchMedia !== 'function') return
    systemMediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
    syncSystemTheme()
    systemMediaQuery.addEventListener('change', syncSystemTheme)
  })

  onUnmounted(() => {
    systemMediaQuery?.removeEventListener('change', syncSystemTheme)
    systemMediaQuery = null
    if (themeStatusTimer) clearTimeout(themeStatusTimer)
  })

  return {
    currentTheme,
    resolvedTheme,
    themeIcon,
    themeButtonLabel,
    themeStatus,
    setTheme,
    toggleTheme,
  }
}
