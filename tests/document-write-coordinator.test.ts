import { describe, expect, it } from 'vitest'
import {
  DocumentWriteCoordinator,
  type Invoke,
} from '../src/editor/documentWriteCoordinator'
import { createEditorSession } from '../src/editor/editorSession'

describe('DocumentWriteCoordinator', () => {
  it('streams an immutable CodeMirror snapshot as bounded raw chunks and only marks the matching generation saved', async () => {
    const calls: Array<{ command: string; args?: Record<string, unknown> | Uint8Array; options?: { headers: HeadersInit } }> = []
    const chunks: Uint8Array[] = []
    const invoke: Invoke = async (command, args, options) => {
      calls.push({ command, args, options })
      if (command === 'reserve_document_write') {
        return { uploadId: 'upload-a', token: 'token-a', expiresInMs: 15_000, contextEpoch: 0 } as never
      }
      if (command === 'append_document_write_chunk') {
        expect(args).toBeInstanceOf(Uint8Array)
        expect(options?.headers).toMatchObject({
          'x-yuyue-upload-id': 'upload-a',
          'x-yuyue-upload-token': 'token-a',
        })
        chunks.push(args as Uint8Array)
        return undefined as never
      }
      if (command === 'finalize_document_write') {
        return {
          kind: 'saved',
          documentId: 'doc-a',
          contextEpoch: 0,
          sourceRevision: 3,
          commitId: 'commit-a',
          writeGeneration: 1,
        } as never
      }
      return undefined as never
    }
    const session = createEditorSession({ documentId: 'doc-a', source: '初始 😀' })
    session.dispatch({ changes: { from: session.state.doc.length, insert: ' + 修改' }, userEvent: 'input.type' })
    const coordinator = new DocumentWriteCoordinator({ invoke, createCommitId: () => 'commit-a' })

    await coordinator.flush(session)

    expect(calls.map((call) => call.command)).toEqual([
      'reserve_document_write',
      'begin_document_write',
      'append_document_write_chunk',
      'finalize_document_write',
    ])
    expect(calls[0].args).toMatchObject({
      request: { contentUtf8Bytes: expect.any(Number), writeGeneration: 1 },
    })
    expect(calls[1].args).toEqual({ request: { uploadId: 'upload-a', token: 'token-a' } })
    expect(calls[3].args).toEqual({ request: { uploadId: 'upload-a', token: 'token-a' } })
    expect(new TextDecoder().decode(chunks[0])).toBe('初始 😀 + 修改')
    expect(session.snapshot.writeStatus).toBe('saved')
    expect(session.snapshot.persistedRevision).toBe(3)
  })

  it('does not let an old failed upload clear newer local edits', async () => {
    let releaseFailure: (() => void) | undefined
    let reachedAppend: (() => void) | undefined
    const appendStarted = new Promise<void>((resolve) => { reachedAppend = resolve })
    const invoke: Invoke = async (command) => {
      if (command === 'reserve_document_write') {
        return { uploadId: 'upload-b', token: 'token-b', expiresInMs: 15_000, contextEpoch: 0 } as never
      }
      if (command === 'append_document_write_chunk') {
        reachedAppend?.()
        await new Promise<void>((resolve) => { releaseFailure = resolve })
        throw new Error('DOCUMENT_WRITE_FAILED')
      }
      return undefined as never
    }
    const session = createEditorSession({ documentId: 'doc-b', source: 'first' })
    session.dispatch({ changes: { from: 5, insert: ' one' }, userEvent: 'input.type' })
    const coordinator = new DocumentWriteCoordinator({ invoke, createCommitId: () => 'commit-b' })
    const pending = coordinator.flush(session)
    await appendStarted
    session.dispatch({ changes: { from: session.state.doc.length, insert: ' two' }, userEvent: 'input.type' })
    releaseFailure?.()
    await pending

    expect(session.snapshot.editGeneration).toBe(2)
    expect(session.snapshot.writeStatus).toBe('pending')
  })

  it('streams Save As without requiring a dirty edit and advances only through contextRebound', async () => {
    const calls: Array<{ command: string; args?: Record<string, unknown> | Uint8Array }> = []
    const invoke: Invoke = async (command, args) => {
      calls.push({ command, args })
      if (command === 'reserve_document_write') {
        expect(args).toMatchObject({
          request: { saveAsToken: 'save-as-token', contextEpoch: 0 },
        })
        return { uploadId: 'upload-save-as', token: 'upload-token', expiresInMs: 15_000, contextEpoch: 0 } as never
      }
      if (command === 'finalize_document_write') {
        return {
          kind: 'contextRebound', documentId: 'doc-save-as', contextEpoch: 1, sourceRevision: 1,
          commitId: 'commit-save-as', writeGeneration: 0, fileName: 'copy.md',
        } as never
      }
      return undefined as never
    }
    const session = createEditorSession({ documentId: 'doc-save-as', source: '# copy' })
    const coordinator = new DocumentWriteCoordinator({ invoke, createCommitId: () => 'commit-save-as' })

    await coordinator.flush(session, { saveAsToken: 'save-as-token' })

    expect(calls.at(-1)?.command).toBe('finalize_document_write')
    expect(session.snapshot.contextEpoch).toBe(1)
    expect(session.snapshot.writeStatus).toBe('saved')
  })

  it('refreshes an expired overwrite token without hiding conflict actions', async () => {
    const invoke: Invoke = async (command) => {
      if (command === 'reserve_document_write') throw new Error('DOCUMENT_WRITE_CONFLICT')
      if (command === 'refresh_document_conflict') {
        return { observedRevision: 4, conflictToken: 'fresh-token' } as never
      }
      return undefined as never
    }
    const session = createEditorSession({ documentId: 'doc-conflict', source: 'disk' })
    session.dispatch({ changes: { from: 4, insert: ' edit' }, userEvent: 'input.type' })
    session.markConflict(3, 'expired-token')
    const coordinator = new DocumentWriteCoordinator({ invoke, createCommitId: () => 'retry-commit' })

    await coordinator.flush(session, { conflictToken: 'expired-token' })

    expect(session.snapshot.writeStatus).toBe('conflict')
    expect(session.snapshot.conflictAtRevision).toBe(4)
    expect(session.snapshot.conflictToken).toBe('fresh-token')
  })

  it('waits for an in-flight autosave before consuming a Save As preparation', async () => {
    let releaseFirst: (() => void) | undefined
    let firstAppendReached: (() => void) | undefined
    const firstAppend = new Promise<void>((resolve) => { firstAppendReached = resolve })
    let reservations = 0
    const invoke: Invoke = async (command, args) => {
      if (command === 'reserve_document_write') {
        reservations += 1
        if (reservations === 2) expect(args).toMatchObject({ request: { saveAsToken: 'prepared-target' } })
        return { uploadId: `upload-${reservations}`, token: `token-${reservations}`, expiresInMs: 15_000, contextEpoch: 0 } as never
      }
      if (command === 'append_document_write_chunk' && reservations === 1) {
        firstAppendReached?.()
        await new Promise<void>((resolve) => { releaseFirst = resolve })
      }
      if (command === 'finalize_document_write') {
        return reservations === 1
          ? { kind: 'saved', documentId: 'doc-race', contextEpoch: 0, sourceRevision: 1, commitId: 'commit-1', writeGeneration: 1 } as never
          : { kind: 'contextRebound', documentId: 'doc-race', contextEpoch: 1, sourceRevision: 2, commitId: 'commit-2', writeGeneration: 1, fileName: 'copy.md' } as never
      }
      return undefined as never
    }
    const session = createEditorSession({ documentId: 'doc-race', source: 'start' })
    session.dispatch({ changes: { from: 5, insert: ' edit' }, userEvent: 'input.type' })
    let commit = 0
    const coordinator = new DocumentWriteCoordinator({ invoke, createCommitId: () => `commit-${++commit}` })
    const autosave = coordinator.flush(session)
    await firstAppend
    const saveAs = coordinator.flush(session, { saveAsToken: 'prepared-target' })
    releaseFirst?.()
    await Promise.all([autosave, saveAs])

    expect(reservations).toBe(2)
    expect(session.snapshot.contextEpoch).toBe(1)
  })

  it('uploads the current recovery generation instead of the failed write snapshot', async () => {
    const chunks: Uint8Array[] = []
    const invoke: Invoke = async (command, args) => {
      if (command === 'prepare_recovery_save_to') {
        expect(args).toMatchObject({ request: { writeGeneration: 2, recordId: 'record-current' } })
        return { uploadId: 'recovery-upload', token: 'recovery-token', expiresInMs: 15_000, contextEpoch: 0 } as never
      }
      if (command === 'append_document_write_chunk') chunks.push(args as Uint8Array)
      if (command === 'finalize_document_write') {
        return { kind: 'recoveryCopySaved', recordId: 'record-current', recoveryEventId: 'event-current', ackToken: 'ack-current', writeGeneration: 2 } as never
      }
      return undefined as never
    }
    const session = createEditorSession({ documentId: 'doc-recovery-current', source: 'g0' })
    session.dispatch({ changes: { from: 2, insert: '-g1' }, userEvent: 'input.type' })
    session.markRecoveryRequired('event-current', [
      { recordId: 'record-current', ordinal: 1, action: 'saveCurrentBufferCopy', actionToken: 'action-current' },
    ])
    session.dispatch({ changes: { from: session.state.doc.length, insert: '-g2' }, userEvent: 'input.type' })
    const coordinator = new DocumentWriteCoordinator({ invoke, createCommitId: () => 'recovery-commit' })

    const completed = await coordinator.saveRecoveryCopy(session, 'event-current', 'record-current', 'action-current')

    expect(new TextDecoder().decode(chunks[0])).toBe('g0-g1-g2')
    expect(completed).toEqual({ recordId: 'record-current', ackToken: 'ack-current', savedGeneration: 2 })
  })
})
