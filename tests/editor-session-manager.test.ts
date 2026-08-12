import { describe, expect, it } from 'vitest'
import { EditorSessionManager } from '../src/editor/editorSessionManager'

describe('EditorSessionManager', () => {
  it('admits a new session only while the shared editor budget has room, then releases it on close', () => {
    const manager = new EditorSessionManager({ maxGlobalBytes: 30 })

    const first = manager.open({ documentId: 'a', source: '12345678' })
    const rejected = manager.open({ documentId: 'b', source: '12345678' })

    expect(first).toBeDefined()
    expect(rejected).toBeUndefined()
    expect(manager.get('a')).toBe(first)

    manager.close('a')
    expect(manager.open({ documentId: 'b', source: '12345678' })).toBeDefined()
  })
})
