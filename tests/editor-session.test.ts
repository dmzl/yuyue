import { EditorSelection, Text, Transaction } from '@codemirror/state'
import { redo, undo } from '@codemirror/commands'
import { describe, expect, it } from 'vitest'
import {
  MAX_EDITOR_SOURCE_BYTES,
  MAX_ENCODED_SNAPSHOT_CHUNK_BYTES,
  MAX_SNAPSHOT_SLICE_CODE_UNITS,
  createEditorSession,
  forEachBoundedSnapshotChunk,
  readEditorText,
} from '../src/editor/editorSession'

describe('local Markdown editor session', () => {
  it('rejects edits that would cross the 10 MiB source boundary', () => {
    const source = 'a'.repeat(MAX_EDITOR_SOURCE_BYTES)
    const session = createEditorSession({ documentId: 'document-source-limit', source })
    const transaction = session.dispatch({ changes: { from: source.length, insert: 'b' }, userEvent: 'input.type' })

    expect(transaction.docChanged).toBe(false)
    expect(session.snapshot.sourceUtf8Bytes).toBe(MAX_EDITOR_SOURCE_BYTES)
  })
  it('does not let a late saved or failed write clear a newer conflict', () => {
    const session = createEditorSession({ documentId: 'document-conflict', source: 'safe', contextEpoch: 7 })
    session.dispatch({ changes: { from: 4, insert: ' edit' }, userEvent: 'input.type' })
    expect(session.markWriteStarted('commit-old', 1)).toBe(true)
    session.markConflict(2, 'conflict-token')

    expect(session.markWriteSaved('commit-old', 1, 3, 7)).toBe(false)
    expect(session.markWriteFailed('DOCUMENT_WRITE_FAILED', 'commit-old', 1, 7)).toBe(false)
    expect(session.snapshot.writeStatus).toBe('conflict')
    expect(session.snapshot.conflictToken).toBe('conflict-token')
    expect(session.snapshot.persistedGeneration).toBe(0)
  })

  it('keeps recovery absorbing until every record is acknowledged and a Save As rebind succeeds', () => {
    const session = createEditorSession({ documentId: 'document-recovery', source: 'safe', contextEpoch: 3 })
    session.markRecoveryRequired('event-a', [
      { recordId: 'record-a', ordinal: 1, action: 'saveCurrentBufferCopy', actionToken: 'action-a' },
      { recordId: 'record-b', ordinal: 2, action: 'revealPreservedItem', actionToken: 'action-b' },
    ])
    session.markConflict(4, 'late-conflict')
    expect(session.markWriteStarted('blocked', 0, undefined, true)).toBe(false)
    session.markRecoveryActionCompleted('record-a', 'ack-a', 0)
    session.markRecoveryAcknowledged('record-a')
    session.markRecoveryActionCompleted('record-b', 'ack-b')
    session.markRecoveryAcknowledged('record-b')

    expect(session.markWriteStarted('save-as', 0, undefined, true)).toBe(true)
    expect(session.snapshot.recoveryRecords?.[0].savedGeneration).toBe(0)
    expect(session.snapshot.writeStatus).toBe('recovery-required')
    expect(session.markContextRebound('save-as', 0, 3, 4, 5)).toBe(true)
    expect(session.snapshot.writeStatus).toBe('saved')
    expect(session.snapshot.recoveryRecords).toBeUndefined()
  })

  it('keeps completed recovery actions operable when a recovery event is extended', () => {
    const session = createEditorSession({ documentId: 'document-recovery-extension', source: 'safe', contextEpoch: 3 })
    session.markRecoveryRequired('event-a', [
      {
        recordId: 'record-current',
        ordinal: 1,
        action: 'saveCurrentBufferCopy',
        actionToken: '',
        actionCompleted: true,
        ackToken: 'ack-current',
        savedGeneration: 0,
        acknowledged: false,
      },
      {
        recordId: 'record-preserved',
        ordinal: 2,
        action: 'revealPreservedItem',
        actionToken: '',
        actionCompleted: true,
        acknowledged: true,
      },
      {
        recordId: 'record-new',
        ordinal: 3,
        action: 'revealPreservedItem',
        actionToken: 'action-new',
        actionCompleted: false,
        acknowledged: false,
      },
    ])

    expect(session.snapshot.recoveryRecords?.[0]).toMatchObject({
      actionCompleted: true,
      ackToken: 'ack-current',
      savedGeneration: 0,
    })
    expect(session.snapshot.recoveryRecords?.[1].acknowledged).toBe(true)
    expect(session.snapshot.recoveryRecords?.[2]).toMatchObject({
      actionCompleted: false,
      actionToken: 'action-new',
    })
  })

  it('keeps selection-only moves out of content history and its byte ledger', () => {
    const session = createEditorSession({ documentId: 'document-a', source: 'first line\nsecond line' })
    const selection = session.state.update({
      selection: EditorSelection.cursor(5),
      userEvent: 'select.pointer',
    })

    expect(selection.docChanged).toBe(false)
    expect(selection.annotation(Transaction.addToHistory)).toBe(false)
    session.accept(selection)
    expect(session.snapshot.historyCharge).toBe(0)

    session.dispatch({ changes: { from: 5, insert: '!' }, userEvent: 'input.type' })
    expect(session.snapshot.historyCharge).toBeGreaterThan(0)

    const view = {
      get state() { return session.state },
      dispatch(transaction: Transaction) { session.accept(transaction) },
    }
    expect(undo(view)).toBe(true)
    expect(readEditorText(session.state.doc)).toBe('first line\nsecond line')
    expect(redo(view)).toBe(true)
    expect(readEditorText(session.state.doc)).toBe('first! line\nsecond line')
  })

  it('rejects document changes that try to bypass the frozen history admission path', () => {
    const session = createEditorSession({ documentId: 'document-b', source: 'safe' })
    const transaction = session.state.update({
      changes: { from: 4, insert: '!' },
      annotations: Transaction.addToHistory.of(false),
      userEvent: 'input.type',
    })

    expect(transaction.docChanged).toBe(false)
    session.accept(transaction)
    expect(readEditorText(session.state.doc)).toBe('safe')
    expect(session.snapshot.historyCharge).toBe(0)
  })

  it('walks immutable snapshots in bounded UTF-16 and UTF-8 chunks without splitting surrogate pairs', () => {
    const source = `${'a'.repeat(MAX_SNAPSHOT_SLICE_CODE_UNITS - 1)}😀${'中文'.repeat(MAX_SNAPSHOT_SLICE_CODE_UNITS / 2)}`
    const chunks: string[] = []

    forEachBoundedSnapshotChunk(Text.of(source.split('\n')), (chunk) => chunks.push(new TextDecoder().decode(chunk)))

    expect(chunks.join('')).toBe(source)
    expect(chunks).toHaveLength(3)
    expect(chunks.every((chunk) => chunk.length <= MAX_SNAPSHOT_SLICE_CODE_UNITS)).toBe(true)
    expect(chunks.every((chunk) => new TextEncoder().encode(chunk).byteLength <= MAX_ENCODED_SNAPSHOT_CHUNK_BYTES)).toBe(true)
    expect(chunks.every((chunk) => !(/[\uD800-\uDBFF]$/.test(chunk)))).toBe(true)
  })
})
