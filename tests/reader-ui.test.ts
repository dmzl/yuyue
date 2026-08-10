// @vitest-environment jsdom

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { defineComponent } from 'vue'
import { mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import MediaLightbox from '../src/components/MediaLightbox.vue'
import ReaderSearch from '../src/components/ReaderSearch.vue'
import ReaderSettings from '../src/components/ReaderSettings.vue'
import ReadingMode from '../src/components/ReadingMode.vue'
import TabBar, { type Tab } from '../src/components/TabBar.vue'
import { useLocale } from '../src/composables/useLocale'
import { useTheme } from '../src/composables/useTheme'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => ({ url: 'data:image/png;base64,AA==' })),
}))

useLocale().setLocale('zh-Hans')

function makeTab(id: string, fileName: string): Tab {
  return {
    id,
    documentId: id,
    fileName,
    sourceRevision: 0,
    document: null,
    sourceContent: null,
    sourceBytes: 0,
    renderedBytes: 0,
    lastAccess: 0,
    isRendering: false,
    renderGeneration: 0,
    scrollRatio: 0,
    remoteImageAuthorized: false,
  }
}

describe('reader controls', () => {
  it('keeps search outside reading settings and closes settings from its backdrop', async () => {
    const wrapper = mount(ReadingMode, {
      attachTo: document.body,
      props: {
        document: {
          html: '<h1 id="heading">Heading</h1>',
          outline: [{ id: 'heading', text: 'Heading', level: 1 }],
          resources: [], links: [], diagrams: [], diagnostics: [],
          stats: { inputBytes: 1, astNodes: 1, headingCount: 1, diagramCount: 0 },
        },
        title: 'document.md', documentId: 'document-1', active: true,
        initialScrollRatio: 0, remoteImageAuthorized: false, settingsOpen: true,
      },
    })

    await vi.waitFor(() => expect(wrapper.find('.reader-settings').exists()).toBe(true))
    expect(wrapper.find('.reader-settings input[aria-label="搜索文档"]').exists()).toBe(false)
    await wrapper.get('[data-reader-settings-backdrop]').trigger('click')
    expect(wrapper.emitted('update-settings-open')).toContainEqual([false])
    wrapper.unmount()
  })

  it('opens and focuses the independent search popup with Ctrl/Cmd+F', async () => {
    const wrapper = mount(ReadingMode, {
      attachTo: document.body,
      props: {
        document: {
          html: '<h1 id="heading">Heading</h1>',
          outline: [{ id: 'heading', text: 'Heading', level: 1 }],
          resources: [], links: [], diagrams: [], diagnostics: [],
          stats: { inputBytes: 1, astNodes: 1, headingCount: 1, diagramCount: 0 },
        },
        title: 'document.md', documentId: 'document-1', active: true,
        initialScrollRatio: 0, remoteImageAuthorized: false,
      },
    })

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true }))
    await wrapper.vm.$nextTick()
    const search = wrapper.get('.reader-search input[aria-label="搜索文档"]')
    expect(document.activeElement).toBe(search.element)
    expect(wrapper.find('.reader-settings').exists()).toBe(false)
    expect(wrapper.emitted('update-settings-open')).toContainEqual([false])
    wrapper.unmount()
  })

  it('places the outline before the reading content in the reader body', async () => {
    const wrapper = mount(ReadingMode, {
      attachTo: document.body,
      props: {
        document: {
          html: '<h1 id="heading">Heading</h1><pre><code class="language-text">sample</code></pre>',
          outline: [{ id: 'heading', text: 'Heading', level: 1 }],
          resources: [],
          links: [],
          diagrams: [],
          diagnostics: [],
          stats: { inputBytes: 1, astNodes: 1, headingCount: 1, diagramCount: 0 },
        },
        title: 'document.md',
        documentId: 'document-1',
        active: true,
        initialScrollRatio: 0,
        remoteImageAuthorized: false,
      },
    })

    await vi.waitFor(() => expect(wrapper.find('.catalog-sidebar').exists()).toBe(true))
    const children = Array.from(wrapper.get('.reader-body').element.children)
    expect(children[0]?.classList.contains('catalog-sidebar')).toBe(true)
    expect(children[1]?.classList.contains('reading-content')).toBe(true)
    wrapper.unmount()
  })

  it('uses an icon-only copy action while retaining its accessible name', async () => {
    const wrapper = mount(ReadingMode, {
      attachTo: document.body,
      props: {
        document: {
          html: '<pre><code class="language-text">sample</code></pre>',
          outline: [],
          resources: [],
          links: [],
          diagrams: [],
          diagnostics: [],
          stats: { inputBytes: 1, astNodes: 1, headingCount: 0, diagramCount: 0 },
        },
        title: 'document.md',
        documentId: 'document-1',
        active: true,
        initialScrollRatio: 0,
        remoteImageAuthorized: false,
      },
    })

    await vi.waitFor(() => expect(wrapper.find('.code-copy-button').exists()).toBe(true))
    const copy = wrapper.get('button[aria-label="复制代码"]')
    expect(copy.find('[data-icon="copy"]').exists()).toBe(true)
    expect(copy.text()).toBe('')
    wrapper.unmount()
  })

  it('exposes independent search, width, and remote-image controls as semantic actions', async () => {
    const searchWrapper = mount(ReaderSearch, {
      attachTo: document.body,
      props: {
        open: true,
        searchQuery: '',
        searchMatchCount: 1,
        searchMatchIndex: 0,
      },
    })

    const search = searchWrapper.get('input[aria-label="搜索文档"]')
    search.element.focus()
    await search.setValue('head')
    await search.setValue('heading')
    const nextMatchButton = searchWrapper.get('button[aria-label="下一个匹配"]')
    nextMatchButton.element.focus()
    await nextMatchButton.trigger('click')
    await searchWrapper.vm.$nextTick()

    expect(searchWrapper.emitted('update-search')).toEqual([['head'], ['heading']])
    expect(document.activeElement).toBe(search.element)
    expect(searchWrapper.emitted('search-next')).toHaveLength(1)

    await search.trigger('keydown', { key: 'Enter' })
    await searchWrapper.vm.$nextTick()
    expect(document.activeElement).toBe(search.element)
    expect(searchWrapper.emitted('search-next')).toHaveLength(2)
    searchWrapper.unmount()

    const settingsWrapper = mount(ReaderSettings, {
      attachTo: document.body,
      props: {
        open: true,
        fontScale: 1,
        contentWidth: 'comfortable',
        hasRemoteImages: true,
        remoteImageAuthorized: false,
      },
    })

    await settingsWrapper.get('button.remote-image-button').trigger('click')

    expect(settingsWrapper.emitted('authorize-remote-images')).toHaveLength(1)
    expect(settingsWrapper.get('button.settings-width').attributes('aria-pressed')).toBe('false')
    settingsWrapper.unmount()
  })

  it('explains the current theme and confirms the selected mode', async () => {
    const ThemeHarness = defineComponent({
      setup() {
        const { themeButtonLabel, themeStatus, toggleTheme } = useTheme()
        return { themeButtonLabel, themeStatus, toggleTheme }
      },
      template: '<button :title="themeButtonLabel" @click="toggleTheme">theme</button><output>{{ themeStatus }}</output>',
    })
    const wrapper = mount(ThemeHarness)

    expect(wrapper.get('button').attributes('title')).toContain('当前：浅色')
    await wrapper.get('button').trigger('click')

    expect(wrapper.get('button').attributes('title')).toContain('当前：深色')
    expect(wrapper.get('output').text()).toContain('已切换到深色模式')
    wrapper.unmount()
  })

  it('reveals horizontal table scrollbars on hover and reserves matching SVG canvas padding', () => {
    const readingModeSource = readFileSync(resolve(process.cwd(), 'src/components/ReadingMode.vue'), 'utf8')
    const lightboxSource = readFileSync(resolve(process.cwd(), 'src/components/MediaLightbox.vue'), 'utf8')

    expect(readingModeSource).toContain('.preview-area :deep(table:hover) { scrollbar-color:')
    expect(lightboxSource).toContain('const SVG_CANVAS_PADDING = 24')
    expect(lightboxSource).toContain('width: `${svgBaseSize.value.width * zoom.value + SVG_CANVAS_PADDING * 2}px`')
    expect(lightboxSource).toContain('height: `${svgBaseSize.value.height * zoom.value + SVG_CANVAS_PADDING * 2}px`')
    expect(lightboxSource).toContain('.media-lightbox-svg {\n  box-sizing: border-box;')
  })

  it('keeps settings dismissal, native File actions, and PDF printing on explicit boundaries', () => {
    const appSource = readFileSync(resolve(process.cwd(), 'src/App.vue'), 'utf8')
    const rustSource = readFileSync(resolve(process.cwd(), 'src-tauri/src/lib.rs'), 'utf8')
    const searchSource = readFileSync(resolve(process.cwd(), 'src/components/ReaderSearch.vue'), 'utf8')
    const readingModeSource = readFileSync(resolve(process.cwd(), 'src/components/ReadingMode.vue'), 'utf8')
    const lightboxSource = readFileSync(resolve(process.cwd(), 'src/components/MediaLightbox.vue'), 'utf8')
    const tabBarSource = readFileSync(resolve(process.cwd(), 'src/components/TabBar.vue'), 'utf8')

    expect(appSource).toContain('data-icon="settings"')
    expect(appSource).toContain("document.addEventListener('pointerdown', closeSettingsOnOutside)")
    expect(appSource).toContain("listen<PrintErrorEvent>('print-error'")
    expect(appSource).toContain("listen('export-current-pdf', handleExportCurrentPdf)")
    expect(appSource).not.toContain("invoke('print_current_document')")
    expect(appSource).not.toContain('window.print()')
    expect(appSource).toContain('async function activateTab(tabId: string, ensureRendered = false)')
    expect(appSource).toContain("invoke('export_current_pdf', { documentId: tab.documentId })")
    expect(appSource).toContain("invoke('set_menu_locale', { locale: currentLocale.value })")
    expect(rustSource).toContain('MenuItemBuilder::with_id("open-document", labels.open_document)')
    expect(rustSource).toContain('MenuItemBuilder::with_id("export-current-pdf", labels.export_pdf)')
    expect(rustSource).toContain('fn set_menu_locale(app: AppHandle, locale: String)')
    expect(rustSource).toContain('fn request_print_export(app: &AppHandle)')
    expect(rustSource).toContain('fn open_print_dialog(app: &AppHandle, document_id: &str)')
    expect(rustSource).toContain('fn export_current_pdf(app: AppHandle, document_id: String)')
    expect(rustSource).toContain('fn suggested_pdf_file_name(file_name: &str) -> String')
    expect(rustSource).toContain('fn suggested_pdf_file_name_for_document(&self, document_id: &str)')
    expect(rustSource).toContain('print_info.setTopMargin(PRINT_MARGIN_TOP_POINTS)')
    expect(rustSource).toContain('print_info.setLeftMargin(PRINT_MARGIN_HORIZONTAL_POINTS)')
    expect(rustSource).toContain('print_info.setRightMargin(PRINT_MARGIN_HORIZONTAL_POINTS)')
    expect(rustSource).toContain('print_info.setBottomMargin(PRINT_MARGIN_BOTTOM_POINTS)')
    expect(rustSource).toContain('const PRINT_MARGIN_TOP_POINTS: f64 = 51.0;')
    expect(rustSource).toContain('const PRINT_MARGIN_HORIZONTAL_POINTS: f64 = 45.0;')
    expect(rustSource).toContain('const PRINT_MARGIN_BOTTOM_POINTS: f64 = 56.7;')
    expect(rustSource).toContain('print_operation.setJobTitle(Some(&job_title))')
    expect(searchSource).toContain('function focusSearch(select = true)')
    expect(searchSource).toContain('focusSearch(false)')
    expect(readingModeSource).toContain('.preview-area :deep(p)')
    expect(readingModeSource).toContain('@page { margin: 18mm 16mm 20mm; }')
    expect(readingModeSource).toContain('font-size: 10.5pt !important;')
    expect(readingModeSource).toContain('table-layout: auto;')
    expect(readingModeSource).toContain('overflow-wrap: anywhere;')
    expect(readingModeSource).toContain('.preview-area :deep(blockquote) { background: #fff !important; }')
    expect(readingModeSource).toContain('.preview-area :deep(th) { background: #f2f2f2 !important; }')
    expect(lightboxSource).toContain('@media print')
    expect(tabBarSource).toContain('@media print')
  })

  it('keeps tab selection and keyboard close behavior accessible', async () => {
    const wrapper = mount(TabBar, {
      props: { tabs: [makeTab('one', 'one.md'), makeTab('two', 'two.md')], activeTabId: 'one' },
      attachTo: document.body,
    })

    expect(wrapper.get('[role="tablist"]').attributes('aria-label')).toBe('打开的文档')
    expect(wrapper.get('[role="tab"]').attributes('aria-selected')).toBe('true')
    await wrapper.get('[role="tab"]').trigger('keydown', { key: 'ArrowRight' })
    await wrapper.vm.$nextTick()
    await wrapper.get('[role="tab"]').trigger('keydown', { key: 'w', metaKey: true })

    expect(wrapper.emitted('activate')).toEqual([['two']])
    expect(wrapper.emitted('close')).toEqual([['one']])
    expect(document.activeElement?.getAttribute('data-tab-id')).toBe('two')
    expect(wrapper.get('[role="tab"]').element.querySelector('button')).toBeNull()
    wrapper.unmount()
  })

  it('moves focus into the context menu and restores it on Escape', async () => {
    const wrapper = mount(TabBar, {
      props: { tabs: [makeTab('one', 'one.md'), makeTab('two', 'two.md')], activeTabId: 'one' },
      attachTo: document.body,
    })
    const tab = wrapper.get('[role="tab"]')
    tab.element.focus()
    await tab.trigger('contextmenu', { clientX: 10, clientY: 10 })
    await wrapper.vm.$nextTick()

    expect(document.activeElement?.getAttribute('role')).toBe('menuitem')
    const menu = document.body.querySelector<HTMLElement>('[role="menu"]')
    expect(menu).not.toBeNull()
    menu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await wrapper.vm.$nextTick()
    expect(document.body.querySelector('[role="menu"]')).toBeNull()
    expect(document.activeElement).toBe(tab.element)
    wrapper.unmount()
  })
})

describe('media lightbox focus management', () => {
  it('focuses the dialog, traps Tab, and restores the trigger focus on close', async () => {
    const trigger = document.createElement('button')
    document.body.append(trigger)
    trigger.focus()
    const wrapper = mount(MediaLightbox, { attachTo: document.body, props: { media: null } })

    await wrapper.setProps({ media: { kind: 'image', src: 'data:image/png;base64,AA==', alt: 'preview' } })
    await wrapper.vm.$nextTick()
    expect(document.activeElement?.getAttribute('aria-label')).toBe('关闭大图')

    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]')
    expect(dialog).not.toBeNull()
    dialog?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true, cancelable: true }))
    expect(document.activeElement?.getAttribute('aria-label')).toBe('关闭大图')

    document.body.querySelector<HTMLButtonElement>('button[aria-label="关闭大图"]')?.click()
    await wrapper.setProps({ media: null })
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(document.activeElement).toBe(trigger)
    wrapper.unmount()
    trigger.remove()
  })

  it('opens hydrated images with Enter and exposes a keyboard target', async () => {
    const wrapper = mount(ReadingMode, {
      attachTo: document.body,
      props: {
        document: {
          html: '<span data-md-resource-id="image-1"></span>',
          outline: [],
          resources: [{ id: 'image-1', kind: 'data', source: 'data:image/png;base64,AA==', alt: 'preview' }],
          links: [],
          diagrams: [],
          diagnostics: [],
          stats: { inputBytes: 1, astNodes: 1, headingCount: 0, diagramCount: 0 },
        },
        title: 'document.md',
        documentId: 'document-1',
        active: true,
        initialScrollRatio: 0,
        remoteImageAuthorized: false,
      },
    })

    await vi.waitFor(() => expect(wrapper.find('img.markdown-image').exists()).toBe(true))
    const image = wrapper.get('img.markdown-image')
    expect(image.attributes('tabindex')).toBe('0')
    await image.trigger('keydown', { key: 'Enter' })
    await wrapper.vm.$nextTick()
    expect(document.body.querySelector('[role="dialog"]')).not.toBeNull()
    wrapper.unmount()
  })

  it('shows a document error near the content and exposes an accessible retry action', async () => {
    const wrapper = mount(ReadingMode, {
      attachTo: document.body,
      props: {
        document: {
          html: '<p>content</p>',
          outline: [],
          resources: [],
          links: [],
          diagrams: [],
          diagnostics: [],
          stats: { inputBytes: 7, astNodes: 1, headingCount: 0, diagramCount: 0 },
        },
        title: 'document.md',
        documentId: 'document-1',
        active: true,
        initialScrollRatio: 0,
        remoteImageAuthorized: false,
        errorMessage: '文档渲染失败',
      },
    })

    expect(wrapper.get('[role="alert"]').text()).toContain('文档渲染失败')
    await wrapper.get('button[aria-label="重试此文档"]').trigger('click')
    expect(wrapper.emitted('retry')).toHaveLength(1)
    wrapper.unmount()
  })
})
