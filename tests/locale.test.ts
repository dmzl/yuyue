// @vitest-environment jsdom

import { afterEach, describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ReaderSettings from '../src/components/ReaderSettings.vue'
import { resolveInitialLocale, useLocale } from '../src/composables/useLocale'

describe('interface locale', () => {
  afterEach(() => {
    const { setLocale } = useLocale()
    setLocale('zh-Hans')
    localStorage.removeItem('mdreader-locale')
  })

  it('maps a first launch to the supported system locale and otherwise falls back to Simplified Chinese', () => {
    expect(resolveInitialLocale(null, 'en-US')).toBe('en')
    expect(resolveInitialLocale(null, 'ja-JP')).toBe('ja')
    expect(resolveInitialLocale(null, 'zh-TW')).toBe('zh-Hant')
    expect(resolveInitialLocale(null, 'zh-HK')).toBe('zh-Hant')
    expect(resolveInitialLocale(null, 'fr-FR')).toBe('zh-Hans')
  })

  it('uses a saved choice before the system locale and keeps selections local', () => {
    expect(resolveInitialLocale('ja', 'en-US')).toBe('ja')
    expect(resolveInitialLocale('unsupported', 'en-US')).toBe('en')

    const { currentLocale, setLocale, t } = useLocale()
    setLocale('ja')
    expect(currentLocale.value).toBe('ja')
    expect(localStorage.getItem('mdreader-locale')).toBe('ja')
    expect(t('language')).toBe('言語')
  })

  it('updates the settings language selector immediately', async () => {
    const { setLocale } = useLocale()
    setLocale('en')
    const wrapper = mount(ReaderSettings, {
      props: {
        open: true,
        fontScale: 1,
        contentWidth: 'comfortable',
        hasRemoteImages: false,
        remoteImageAuthorized: false,
      },
    })

    const select = wrapper.get('select[aria-label="Language"]')
    expect(select.element.value).toBe('en')
    await select.setValue('zh-Hant')
    expect(wrapper.get('span.settings-label').text()).toBe('文字大小')
    expect(localStorage.getItem('mdreader-locale')).toBe('zh-Hant')
    wrapper.unmount()
  })
})
