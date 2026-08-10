// @vitest-environment jsdom

import { mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import ReadingMode from '../src/components/ReadingMode.vue'
import TabBar, { type Tab } from '../src/components/TabBar.vue'
import { useLocale } from '../src/composables/useLocale'

function makeTab(id: string): Tab {
  return {
    id,
    documentId: id,
    fileName: `${id}.md`,
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

describe('tab overflow and copy feedback', () => {
  it('keeps the active tab visible and offers hidden tabs from a menu when space is tight', async () => {
    useLocale().setLocale('zh-Hans')
    const tabs = ['one', 'two', 'three', 'four', 'five', 'six'].map(makeTab)
    const wrapper = mount(TabBar, {
      attachTo: document.body,
      props: { tabs, activeTabId: 'four' },
    })
    Object.defineProperty(wrapper.get('.tab-bar').element, 'clientWidth', { configurable: true, value: 500 })
    window.dispatchEvent(new Event('resize'))
    await wrapper.vm.$nextTick()

    expect(wrapper.findAll('[role="tab"]')).toHaveLength(2)
    expect(wrapper.get('[data-tab-id="four"]').attributes('aria-selected')).toBe('true')
    const overflow = wrapper.get('.tab-overflow-trigger')
    expect(overflow.attributes('aria-label')).toBe('更多文档（4 个）')

    await overflow.trigger('click')
    await wrapper.vm.$nextTick()
    expect(wrapper.get('[role="menu"]').attributes('aria-label')).toBe('其他打开的文档')
    expect(wrapper.findAll('[role="menuitem"]')).toHaveLength(4)
    expect(document.activeElement?.getAttribute('role')).toBe('menuitem')

    await wrapper.get('[role="menuitem"]').trigger('click')
    expect(wrapper.emitted('activate')).toEqual([['one']])
    wrapper.unmount()
  })

  it('returns focus to the active tab when a resize removes the overflow menu', async () => {
    useLocale().setLocale('zh-Hans')
    const tabs = ['one', 'two', 'three', 'four', 'five', 'six'].map(makeTab)
    const wrapper = mount(TabBar, {
      attachTo: document.body,
      props: { tabs, activeTabId: 'four' },
    })
    Object.defineProperty(wrapper.get('.tab-bar').element, 'clientWidth', { configurable: true, value: 500 })
    window.dispatchEvent(new Event('resize'))
    await wrapper.vm.$nextTick()

    await wrapper.get('.tab-overflow-trigger').trigger('click')
    await vi.waitFor(() => expect(document.activeElement?.getAttribute('role')).toBe('menuitem'))

    Object.defineProperty(wrapper.get('.tab-bar').element, 'clientWidth', { configurable: true, value: 1_200 })
    window.dispatchEvent(new Event('resize'))
    await vi.waitFor(() => expect(wrapper.find('.tab-overflow-trigger').exists()).toBe(false))
    expect(document.activeElement).toBe(wrapper.get('[data-tab-id="four"]').element)
    wrapper.unmount()
  })

  it('only moves tabs into the menu once the minimum readable tab width no longer fits', async () => {
    useLocale().setLocale('zh-Hans')
    const wrapper = mount(TabBar, {
      attachTo: document.body,
      props: { tabs: ['one', 'two', 'three', 'four', 'five', 'six'].map(makeTab), activeTabId: 'three' },
    })
    Object.defineProperty(wrapper.get('.tab-bar').element, 'clientWidth', { configurable: true, value: 1_200 })
    window.dispatchEvent(new Event('resize'))
    await wrapper.vm.$nextTick()

    expect(wrapper.findAll('[role="tab"]')).toHaveLength(6)
    expect(wrapper.find('.tab-overflow-trigger').exists()).toBe(false)

    Object.defineProperty(wrapper.get('.tab-bar').element, 'clientWidth', { configurable: true, value: 1_007 })
    window.dispatchEvent(new Event('resize'))
    await wrapper.vm.$nextTick()

    expect(wrapper.findAll('[role="tab"]')).toHaveLength(5)
    expect(wrapper.get('.tab-overflow-trigger').attributes('aria-label')).toBe('更多文档（1 个）')
    wrapper.unmount()
  })

  it('shows localized, visible success feedback after copying code', async () => {
    const locale = useLocale()
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
    const writeText = vi.fn(async () => undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    locale.setLocale('en')

    try {
      const wrapper = mount(ReadingMode, {
        attachTo: document.body,
        props: {
          document: {
            html: '<pre><code class="language-text">sample</code></pre>',
            outline: [], resources: [], links: [], diagrams: [], diagnostics: [],
            stats: { inputBytes: 1, astNodes: 1, headingCount: 0, diagramCount: 0 },
          },
          title: 'document.md', documentId: 'document-1', active: true,
          initialScrollRatio: 0, remoteImageAuthorized: false,
        },
      })

      await vi.waitFor(() => expect(wrapper.find('.code-copy-button').exists()).toBe(true))
      const copyButton = wrapper.get('button[aria-label="Copy code"]')
      await copyButton.trigger('click')
      await vi.waitFor(() => expect(wrapper.get('.code-copy-feedback').text()).toBe('Code copied'))
      expect(copyButton.attributes('aria-label')).toBe('Code copied')
      expect(copyButton.find('[data-icon="check"]').exists()).toBe(true)
      expect(writeText).toHaveBeenCalledWith('sample')

      writeText.mockRejectedValueOnce(new Error('clipboard unavailable'))
      await copyButton.trigger('click')
      await vi.waitFor(() => expect(wrapper.get('.reader-status').text()).toContain('Unable to copy code'))
      expect(copyButton.attributes('aria-label')).toBe('Copy code')
      expect(copyButton.attributes('data-copied')).toBeUndefined()
      expect(copyButton.find('[data-icon="copy"]').exists()).toBe(true)
      expect(wrapper.find('.code-copy-feedback').exists()).toBe(false)

      await copyButton.trigger('click')
      await vi.waitFor(() => expect(wrapper.get('.code-copy-feedback').text()).toBe('Code copied'))
      expect(wrapper.find('.reader-status').exists()).toBe(false)
      wrapper.unmount()
    } finally {
      locale.setLocale('zh-Hans')
      if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard)
      else delete (navigator as Navigator & { clipboard?: Clipboard }).clipboard
    }
  })
})
