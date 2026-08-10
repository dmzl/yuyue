import { describe, expect, it, vi } from 'vitest'
import { useDocumentProcessing } from '../src/composables/useDocumentProcessing'

describe('document processing operations', () => {
  it('keeps the newest import visible when older operations finish out of order', () => {
    const processing = useDocumentProcessing(vi.fn())
    processing.start({ operationId: 'older', sequence: 1, fileName: 'older.md' }, 0)
    processing.start({ operationId: 'newer', sequence: 2, fileName: 'newer.md' }, 0)

    expect(processing.visibleOperation.value?.operationId).toBe('newer')
    expect(processing.shouldActivate('older', 0)).toBe(false)

    processing.beginRender('older', 'document-older', 1)
    processing.finishBackground('older', 'document-older', 1)

    expect(processing.visibleOperation.value?.operationId).toBe('newer')
    expect(processing.visibleOperation.value?.phase).toBe('importing')
    processing.dispose()
  })

  it('accepts only the paint acknowledgement bound to the current document generation', () => {
    const processing = useDocumentProcessing(vi.fn())
    processing.start({ operationId: 'paint', sequence: 3, fileName: 'paint.md' }, 4)
    processing.beginRender('paint', 'document-paint', 7)
    processing.awaitPaint('paint', 'document-paint', 7)

    expect(processing.completePaint('paint', 'document-paint', 6)).toBe(false)
    expect(processing.visibleOperation.value?.phase).toBe('awaiting-paint')
    expect(processing.completePaint('paint', 'document-paint', 7)).toBe(true)
    expect(processing.visibleOperation.value).toBeNull()
    processing.dispose()
  })

  it('does not activate a completed import after the user switches tabs', () => {
    const processing = useDocumentProcessing(vi.fn())
    processing.start({ operationId: 'manual-intent', sequence: 4, fileName: 'intent.md' }, 2)

    expect(processing.shouldActivate('manual-intent', 2)).toBe(true)
    expect(processing.shouldActivate('manual-intent', 3)).toBe(false)
    processing.dispose()
  })

  it('bounds retained terminal operations without dropping the latest operation', () => {
    const processing = useDocumentProcessing(vi.fn())

    for (let sequence = 0; sequence < 64; sequence += 1) {
      const operationId = `terminal-${sequence}`
      processing.start({ operationId, sequence, fileName: `${sequence}.md` }, 0)
      processing.cancel(operationId)
    }

    processing.start({ operationId: 'current', sequence: 64, fileName: 'current.md' }, 0)

    expect(processing.get('terminal-0')).toBeUndefined()
    expect(processing.get('terminal-63')).toBeDefined()
    expect(processing.visibleOperation.value?.operationId).toBe('current')
    processing.dispose()
  })

  it('keeps close, render failure, duplicate messages, and stale paint acknowledgements isolated', () => {
    const processing = useDocumentProcessing(vi.fn())
    processing.start({ operationId: 'replace', sequence: 7, fileName: 'replace.md' }, 0)
    expect(processing.ensure({ operationId: 'replace', sequence: 7, fileName: 'replace.md' }, 3).activationIntent).toBe(0)
    expect(processing.beginRender('replace', 'document-replace', 2)).toBe(true)
    expect(processing.awaitPaint('replace', 'document-replace', 2)).toBe(true)
    expect(processing.beginRender('replace', 'document-replace', 3)).toBe(true)
    expect(processing.awaitPaint('replace', 'document-replace', 3)).toBe(true)

    expect(processing.completePaint('replace', 'document-replace', 2)).toBe(false)
    expect(processing.fail('replace')).toBe(true)
    expect(processing.fail('replace')).toBe(false)
    expect(processing.completePaint('replace', 'document-replace', 3)).toBe(false)

    processing.start({ operationId: 'closed', sequence: 8, fileName: 'closed.md' }, 0)
    processing.beginRender('closed', 'document-closed', 1)
    processing.cancelDocument('document-closed')
    expect(processing.get('closed')?.phase).toBe('cancelled')
    expect(processing.finishBackground('closed', 'document-closed', 1)).toBe(false)
    processing.dispose()
  })
})
