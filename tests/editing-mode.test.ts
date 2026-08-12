// @vitest-environment jsdom

import { mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { EditorView } from '@codemirror/view'
import EditingMode from '../src/components/EditingMode.vue'
import ReadingMode from '../src/components/ReadingMode.vue'
import { useLocale } from '../src/composables/useLocale'
import { createEditorSession, readEditorText } from '../src/editor/editorSession'
import editingModeSource from '../src/components/EditingMode.vue?raw'

describe('EditingMode', () => {
  it('places the narrow breakpoint above the configured 800 px minimum window width', () => {
    expect(editingModeSource).toContain('@media (max-width: 900px)')
  })
  it('uses theme-aware Markdown syntax colors instead of the light-only default highlighter', () => {
    expect(editingModeSource).not.toContain('defaultHighlightStyle')
    expect(editingModeSource).toContain('tags.processingInstruction')
    expect(editingModeSource).toContain("color: 'var(--editor-syntax-mark)'")
    expect(editingModeSource).toContain(':data-editor-theme="props.theme"')
    expect(editingModeSource).toContain(".editing-mode[data-editor-theme='dark']")
    expect(editingModeSource).toContain("fontWeight: '650'")
    expect(editingModeSource).toContain('--editor-text: #d7dbe5')
    expect(editingModeSource).toContain('--editor-syntax-heading: #f3f5f8')
    expect(editingModeSource).toContain('--editor-syntax-mark: #a7afbd')
    expect(editingModeSource).toContain('highlightActiveLineGutter()')
    expect(editingModeSource).toContain("{ dark: theme === 'dark' }")
    expect(editingModeSource).toContain('editorThemeCompartment.reconfigure')
  })
  it('uses a CodeMirror transaction for a compact formatting command', async () => {
    useLocale().setLocale('zh-Hans')
    const session = createEditorSession({ documentId: 'document-a', source: '' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session,
        writeStatus: 'pending',
        document: {
          html: '', outline: [], resources: [], links: [], diagrams: [], diagnostics: [], sourceBlocks: [],
          stats: { inputBytes: 0, astNodes: 0, headingCount: 0, diagramCount: 0 },
        },
        title: 'document-a.md',
        remoteImageAuthorized: false,
        renderGeneration: 1,
        inputFrozen: false,
      },
    })

    await wrapper.get('[data-editor-command="bold"]').trigger('click')

    expect(readEditorText(session.state.doc)).toBe('**文本**')
    expect(wrapper.emitted('session-change')).toEqual([[true]])

    await wrapper.get('[data-editor-command="bold"]').trigger('click')
    expect(wrapper.emitted('session-change')).toEqual([[true], [false]])
    wrapper.unmount()
  })

  it('inserts only a controlled relative image URL through a CodeMirror transaction', () => {
    const session = createEditorSession({ documentId: 'document-image', source: '' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'image.md', remoteImageAuthorized: false, renderGeneration: 1, inputFrozen: false,
        document: { html: '', outline: [], resources: [], links: [], diagrams: [], diagnostics: [], sourceBlocks: [], stats: { inputBytes: 0, astNodes: 0, headingCount: 0, diagramCount: 0 } },
      },
    })

    expect((wrapper.vm as unknown as { insertImage: (url: string, alt: string) => boolean })
      .insertImage('image.assets/%E9%85%8D%E5%9B%BE.png', '配图')).toBe(true)
    expect(readEditorText(session.state.doc)).toBe('![配图](image.assets/%E9%85%8D%E5%9B%BE.png)')
    expect((wrapper.vm as unknown as { insertImage: (url: string, alt: string) => boolean })
      .insertImage('../outside.png', 'bad')).toBe(false)
    wrapper.unmount()
  })

  it('requires a second explicit action before emitting conflict overwrite', async () => {
    const session = createEditorSession({ documentId: 'document-conflict', source: '# local' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'conflict', title: 'conflict.md', remoteImageAuthorized: false, renderGeneration: 1, inputFrozen: false,
        document: { html: '', outline: [], resources: [], links: [], diagrams: [], diagnostics: [], sourceBlocks: [], stats: { inputBytes: 0, astNodes: 0, headingCount: 0, diagramCount: 0 } },
      },
    })

    expect(wrapper.emitted('overwrite')).toBeUndefined()
    await wrapper.get('.editor-status-actions button:nth-child(2)').trigger('click')
    expect(wrapper.get('[role="alertdialog"]').exists()).toBe(true)
    expect(wrapper.emitted('overwrite')).toBeUndefined()
    await wrapper.get('.editor-confirm .editor-danger').trigger('click')
    expect(wrapper.emitted('overwrite')).toEqual([[]])
    wrapper.unmount()
  })

  it('freezes editor mutations while a leave operation is finalizing', async () => {
    const session = createEditorSession({ documentId: 'document-leave', source: 'safe' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'leave.md', remoteImageAuthorized: false, renderGeneration: 1, inputFrozen: true,
        document: { html: '', outline: [], resources: [], links: [], diagrams: [], diagnostics: [], sourceBlocks: [], stats: { inputBytes: 0, astNodes: 0, headingCount: 0, diagramCount: 0 } },
      },
    })

    expect(wrapper.get('[data-editor-command="bold"]').attributes('disabled')).toBeDefined()
    await wrapper.get('[data-editor-command="bold"]').trigger('click')
    expect(readEditorText(session.state.doc)).toBe('safe')
    wrapper.unmount()
  })

  it('keeps narrow-only formatting commands reachable through More and exposes Find', async () => {
    const session = createEditorSession({ documentId: 'document-narrow', source: '' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'narrow.md', remoteImageAuthorized: false, renderGeneration: 1, inputFrozen: false,
        document: { html: '', outline: [], resources: [], links: [], diagrams: [], diagnostics: [], sourceBlocks: [], stats: { inputBytes: 0, astNodes: 0, headingCount: 0, diagramCount: 0 } },
      },
    })

    expect(wrapper.get('.editor-find-trigger').attributes('aria-label')).toBe('查找与替换')
    await wrapper.get('.editor-more-trigger').trigger('click')
    const menu = wrapper.get('.editor-more-menu')
    expect(menu.findAll('[role="menuitem"]')).toHaveLength(8)
    await menu.findAll('[role="menuitem"]')[0].trigger('click')
    expect(readEditorText(session.state.doc)).toBe('*文本*')
    expect(wrapper.find('.editor-more-menu').exists()).toBe(false)
    wrapper.unmount()
  })

  it('localizes the CodeMirror find panel and refreshes an open panel when the app language changes', async () => {
    const locale = useLocale()
    locale.setLocale('zh-Hans')
    const session = createEditorSession({ documentId: 'document-find-locale', source: '查找 text' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'find.md', remoteImageAuthorized: false, renderGeneration: 1, inputFrozen: false,
        document: { html: '', outline: [], resources: [], links: [], diagrams: [], diagnostics: [], sourceBlocks: [], stats: { inputBytes: 11, astNodes: 1, headingCount: 0, diagramCount: 0 } },
      },
    })

    try {
      await wrapper.get('.editor-find-trigger').trigger('click')
      expect(wrapper.get('.cm-panel.cm-search input[name="search"]').attributes('placeholder')).toBe('查找')
      expect(wrapper.get('.cm-panel.cm-search input[name="replace"]').attributes('placeholder')).toBe('替换')
      expect(wrapper.get('.cm-panel.cm-search label').text()).toContain('区分大小写')
      expect(wrapper.get('.cm-panel.cm-search button[name="close"]').attributes('aria-label')).toBe('关闭')

      locale.setLocale('ja')
      await wrapper.vm.$nextTick()
      expect(wrapper.get('.cm-panel.cm-search input[name="search"]').attributes('placeholder')).toBe('検索')
      expect(wrapper.get('.cm-panel.cm-search input[name="replace"]').attributes('placeholder')).toBe('置換')
      expect(wrapper.get('.cm-panel.cm-search label').text()).toContain('大文字と小文字を区別')
      expect(wrapper.get('.cm-panel.cm-search button[name="close"]').attributes('aria-label')).toBe('閉じる')
    } finally {
      wrapper.unmount()
      locale.setLocale('zh-Hans')
    }
  })

  it('rebinds editor extensions when a retained session is remounted for another active tab', async () => {
    const locale = useLocale()
    locale.setLocale('zh-Hans')
    const session = createEditorSession({ documentId: 'document-remount-locale', source: '查找 text' })
    const renderDocument = {
      html: '', outline: [], resources: [], links: [], diagrams: [], diagnostics: [], sourceBlocks: [],
      stats: { inputBytes: 11, astNodes: 1, headingCount: 0, diagramCount: 0 },
    }
    const first = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'remount.md', remoteImageAuthorized: false,
        renderGeneration: 1, inputFrozen: false, document: renderDocument,
      },
    })

    try {
      await first.get('.editor-find-trigger').trigger('click')
      expect(first.get('.cm-panel.cm-search input[name="search"]').attributes('placeholder')).toBe('查找')
    } finally {
      first.unmount()
    }

    locale.setLocale('ja')
    const remounted = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'remount.md', remoteImageAuthorized: false,
        renderGeneration: 2, inputFrozen: false, document: renderDocument,
      },
    })

    try {
      await remounted.vm.$nextTick()
      const searchInput = remounted.find('.cm-panel.cm-search input[name="search"]')
      if (!searchInput.exists()) await remounted.get('.editor-find-trigger').trigger('click')
      expect(remounted.get('.cm-panel.cm-search input[name="search"]').attributes('placeholder')).toBe('検索')

      locale.setLocale('en')
      await remounted.vm.$nextTick()
      expect(remounted.get('.cm-panel.cm-search input[name="search"]').attributes('placeholder')).toBe('Find')
    } finally {
      remounted.unmount()
      locale.setLocale('zh-Hans')
    }
  })

  it('shows toolbar tips quickly without relying on delayed native title tooltips', async () => {
    vi.useFakeTimers()
    const session = createEditorSession({ documentId: 'document-tooltip', source: '' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'tooltip.md', remoteImageAuthorized: false, renderGeneration: 1, inputFrozen: false,
        document: { html: '', outline: [], resources: [], links: [], diagrams: [], diagnostics: [], sourceBlocks: [], stats: { inputBytes: 0, astNodes: 0, headingCount: 0, diagramCount: 0 } },
      },
    })

    try {
      const undoButton = wrapper.get('button[aria-label="撤销"]')
      expect(undoButton.attributes('title')).toBeUndefined()
      await undoButton.trigger('mouseenter')
      await vi.advanceTimersByTimeAsync(159)
      expect(wrapper.find('[role="tooltip"]').exists()).toBe(false)
      await vi.advanceTimersByTimeAsync(1)
      expect(wrapper.get('[role="tooltip"]').text()).toBe('撤销')
    } finally {
      wrapper.unmount()
      vi.useRealTimers()
    }
  })

  it('syncs the preview on every real editor scroll instead of waiting for a viewport boundary', async () => {
    const session = createEditorSession({ documentId: 'document-scroll', source: '# Heading\n\nBody' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'scroll.md', remoteImageAuthorized: false, renderGeneration: 1, inputFrozen: false,
        document: {
          html: '<h1 data-md-source-block-id="sb-aaaaaaaa-1">Heading</h1><p data-md-source-block-id="sb-aaaaaaaa-2">Body</p>',
          outline: [], resources: [], links: [], diagrams: [], diagnostics: [],
          sourceBlocks: [
            { id: 'sb-aaaaaaaa-1', startLine: 1, endLine: 1, kind: 'heading' },
            { id: 'sb-aaaaaaaa-2', startLine: 3, endLine: 3, kind: 'block' },
          ],
          stats: { inputBytes: 16, astNodes: 2, headingCount: 1, diagramCount: 0 },
        },
      },
    })
    const preview = wrapper.findComponent(ReadingMode)
    const previewScroller = preview.get('.reading-content').element as HTMLElement
    previewScroller.scrollTo = vi.fn()
    const originalRequestAnimationFrame = window.requestAnimationFrame
    window.requestAnimationFrame = (callback: FrameRequestCallback) => {
      callback(performance.now())
      return 1
    }

    try {
      wrapper.get('.cm-scroller').element.dispatchEvent(new Event('scroll'))
      await wrapper.vm.$nextTick()
      expect(previewScroller.scrollTo).toHaveBeenCalled()
    } finally {
      window.requestAnimationFrame = originalRequestAnimationFrame
      wrapper.unmount()
    }
  })

  it('suppresses the programmatic preview echo while keeping later user preview scroll active', async () => {
    const animationFrames: FrameRequestCallback[] = []
    const originalRequestAnimationFrame = window.requestAnimationFrame
    const originalCancelAnimationFrame = window.cancelAnimationFrame
    window.requestAnimationFrame = (callback: FrameRequestCallback) => {
      animationFrames.push(callback)
      return animationFrames.length
    }
    window.cancelAnimationFrame = vi.fn()
    const dispatch = vi.spyOn(EditorView.prototype, 'dispatch')
    const session = createEditorSession({ documentId: 'document-origin', source: '# Heading\n\nBody' })
    const wrapper = mount(EditingMode, {
      attachTo: document.body,
      props: {
        session, writeStatus: 'saved', title: 'origin.md', remoteImageAuthorized: false, renderGeneration: 1, inputFrozen: false,
        document: {
          html: '<h1 data-md-source-block-id="sb-aaaaaaaa-1">Heading</h1><p data-md-source-block-id="sb-aaaaaaaa-2">Body</p>',
          outline: [], resources: [], links: [], diagrams: [], diagnostics: [],
          sourceBlocks: [
            { id: 'sb-aaaaaaaa-1', startLine: 1, endLine: 1, kind: 'heading' },
            { id: 'sb-aaaaaaaa-2', startLine: 3, endLine: 3, kind: 'block' },
          ],
          stats: { inputBytes: 16, astNodes: 2, headingCount: 1, diagramCount: 0 },
        },
      },
    })
    const preview = wrapper.findComponent(ReadingMode)
    const previewScroller = preview.get('.reading-content').element as HTMLElement
    previewScroller.scrollTo = vi.fn()
    animationFrames.length = 0

    try {
      wrapper.get('.cm-scroller').element.dispatchEvent(new Event('scroll'))
      animationFrames.shift()?.(performance.now())
      expect(previewScroller.scrollTo).toHaveBeenCalled()

      dispatch.mockClear()
      preview.vm.$emit('source-block-scroll', 'sb-aaaaaaaa-1')
      await wrapper.vm.$nextTick()
      expect(dispatch).not.toHaveBeenCalled()

      while (animationFrames.length > 0) animationFrames.shift()?.(performance.now())
      preview.vm.$emit('source-block-scroll', 'sb-aaaaaaaa-2')
      await wrapper.vm.$nextTick()
      expect(dispatch).toHaveBeenCalled()
    } finally {
      dispatch.mockRestore()
      window.requestAnimationFrame = originalRequestAnimationFrame
      window.cancelAnimationFrame = originalCancelAnimationFrame
      wrapper.unmount()
    }
  })
})
