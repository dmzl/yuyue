import { invoke } from '@tauri-apps/api/core'
import {
  forEachBoundedSnapshotChunkAsync,
  utf8ByteLength,
  type EditorSession,
} from './editorSession'

export const DEFAULT_AUTOSAVE_DELAY_MS = 1_000

export interface WriteReservation {
  uploadId: string
  token: string
  expiresInMs: number
  contextEpoch: number
}

export type DocumentWriteOutcome =
  | {
    kind: 'saved'
    documentId: string
    contextEpoch: number
    sourceRevision: number
    commitId: string
    writeGeneration: number
  }
  | {
    kind: 'conflict'
    documentId: string
    contextEpoch: number
    observedRevision: number
    conflictToken: string
  }
  | {
    kind: 'recoveryRequired'
    documentId: string
    contextEpoch: number
    sourceRevision: number
    code: string
    recoveryEventId: string
    recoveryRecords: import('./editorSession').RecoveryRecordSummary[]
  }
  | {
    kind: 'contextRebound'
    documentId: string
    contextEpoch: number
    sourceRevision: number
    commitId: string
    writeGeneration: number
    fileName: string
  }
  | {
    kind: 'recoveryCopySaved'
    recordId: string
    recoveryEventId: string
    ackToken: string
    writeGeneration: number
  }

export type Invoke = <T>(command: string, args?: Record<string, unknown> | Uint8Array, options?: { headers: HeadersInit }) => Promise<T>

export interface DocumentWriteCoordinatorOptions {
  autosaveDelayMs?: number
  invoke?: Invoke
  onSessionStateChange?: () => void
  createCommitId?: () => string
}

export interface FlushOptions {
  conflictToken?: string
  saveAsToken?: string
}

function errorCode(error: unknown) {
  if (typeof error === 'string' && error) return error
  if (error instanceof Error && error.message) return error.message
  if (typeof error === 'object' && error && 'code' in error && typeof error.code === 'string') return error.code
  return 'DOCUMENT_WRITE_FAILED'
}

function defaultCommitId() {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return crypto.randomUUID()
  return `commit-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
}

/**
 * Owns only short-lived save transport state. EditorSession remains the source
 * of truth, so an immutable CodeMirror snapshot can finish writing while the
 * user continues typing into a newer generation.
 */
export class DocumentWriteCoordinator {
  private readonly autosaveDelayMs: number
  private readonly call: Invoke
  private readonly onSessionStateChange: () => void
  private readonly createCommitId: () => string
  private timer?: ReturnType<typeof setTimeout>
  private inFlight?: Promise<void>
  private recoveryActive = false
  private disposed = false

  constructor(options: DocumentWriteCoordinatorOptions = {}) {
    this.autosaveDelayMs = options.autosaveDelayMs ?? DEFAULT_AUTOSAVE_DELAY_MS
    this.call = options.invoke ?? invoke as Invoke
    this.onSessionStateChange = options.onSessionStateChange ?? (() => {})
    this.createCommitId = options.createCommitId ?? defaultCommitId
  }

  schedule(session: EditorSession) {
    if (this.disposed || !this.canStart(session)) return
    if (this.timer) clearTimeout(this.timer)
    this.timer = setTimeout(() => {
      this.timer = undefined
      void this.flush(session)
    }, this.autosaveDelayMs)
  }

  async flush(session: EditorSession, options: FlushOptions = {}): Promise<void> {
    if (this.disposed || !this.canStart(session, options)) return
    if (this.inFlight) {
      await this.inFlight
      if (options.saveAsToken && !this.disposed) return this.flush(session, options)
      return
    }
    const work = this.writeCurrentSnapshot(session, options)
    this.inFlight = work
    try {
      await work
    } finally {
      if (this.inFlight === work) this.inFlight = undefined
      if (!this.disposed && session.snapshot.writeStatus === 'pending') this.schedule(session)
    }
  }

  dispose() {
    this.disposed = true
    if (this.timer) clearTimeout(this.timer)
    this.timer = undefined
  }

  async saveRecoveryCopy(
    session: EditorSession,
    recoveryEventId: string,
    recordId: string,
    actionToken: string,
  ): Promise<{ recordId: string; ackToken: string; savedGeneration: number } | null> {
    if (this.disposed || this.recoveryActive) return null
    this.recoveryActive = true
    const snapshot = session.state.doc
    const writeGeneration = session.snapshot.editGeneration
    const commitId = this.createCommitId()
    let reservation: WriteReservation | null = null
    try {
      if (this.inFlight) await this.inFlight
      reservation = await this.call<WriteReservation | null>('prepare_recovery_save_to', {
        request: {
          documentId: session.documentId,
          contextEpoch: session.contextEpoch,
          recoveryEventId,
          recordId,
          actionToken,
          commitId,
          writeGeneration,
          contentUtf8Bytes: utf8ByteLength(snapshot),
        },
      })
      if (!reservation) return null
      if (reservation.contextEpoch !== session.contextEpoch) throw new Error('DOCUMENT_WRITE_IDENTITY_CHANGED')
      await this.call<void>('begin_document_write', {
        request: { uploadId: reservation.uploadId, token: reservation.token },
      })
      let sequence = 0
      await forEachBoundedSnapshotChunkAsync(snapshot, async (chunk) => {
        await this.call<void>('append_document_write_chunk', chunk, {
          headers: {
            'x-yuyue-upload-id': reservation!.uploadId,
            'x-yuyue-upload-token': reservation!.token,
            'x-yuyue-upload-sequence': String(sequence),
            'x-yuyue-upload-length': String(chunk.byteLength),
          },
        })
        sequence += 1
      })
      const outcome = await this.call<DocumentWriteOutcome>('finalize_document_write', {
        request: { uploadId: reservation.uploadId, token: reservation.token },
      })
      if (outcome.kind === 'recoveryRequired') {
        session.markRecoveryRequired(outcome.recoveryEventId, outcome.recoveryRecords)
        return null
      }
      if (outcome.kind !== 'recoveryCopySaved'
        || outcome.recordId !== recordId
        || outcome.recoveryEventId !== recoveryEventId
        || outcome.writeGeneration !== writeGeneration) {
        throw new Error('RECOVERY_SAVE_FAILED')
      }
      return { recordId, ackToken: outcome.ackToken, savedGeneration: writeGeneration }
    } finally {
      this.recoveryActive = false
      if (reservation) {
        void this.call<void>('cancel_document_write', {
          request: { uploadId: reservation.uploadId, token: reservation.token },
        }).catch(() => {})
      }
      this.onSessionStateChange()
    }
  }

  private canStart(session: EditorSession, options: FlushOptions = {}) {
    const snapshot = session.snapshot
    return (snapshot.writeStatus !== 'conflict' || options.conflictToken === snapshot.conflictToken || Boolean(options.saveAsToken))
      && (snapshot.writeStatus !== 'recovery-required'
        || Boolean(options.saveAsToken) && Boolean(snapshot.recoveryRecords?.length) && snapshot.recoveryRecords!.every((record) => record.acknowledged))
      && (snapshot.editGeneration > snapshot.persistedGeneration || Boolean(options.saveAsToken))
  }

  private async writeCurrentSnapshot(session: EditorSession, options: FlushOptions) {
    const snapshot = session.state.doc
    const writeGeneration = session.snapshot.editGeneration
    const commitId = this.createCommitId()
    const contentUtf8Bytes = utf8ByteLength(snapshot)
    let reservation: WriteReservation | undefined

    if (!session.markWriteStarted(commitId, writeGeneration, options.conflictToken, Boolean(options.saveAsToken))) return
    this.onSessionStateChange()
    try {
      reservation = await this.call<WriteReservation>('reserve_document_write', {
        request: {
          documentId: session.documentId,
          contextEpoch: session.contextEpoch,
          commitId,
          writeGeneration,
          contentUtf8Bytes,
          ...(options.conflictToken ? { conflictToken: options.conflictToken } : {}),
          ...(options.saveAsToken ? { saveAsToken: options.saveAsToken } : {}),
        },
      })
      if (reservation.contextEpoch !== session.contextEpoch) throw new Error('DOCUMENT_WRITE_IDENTITY_CHANGED')
      await this.call<void>('begin_document_write', {
        request: { uploadId: reservation.uploadId, token: reservation.token },
      })
      let sequence = 0
      await forEachBoundedSnapshotChunkAsync(snapshot, async (chunk) => {
        await this.call<void>('append_document_write_chunk', chunk, {
          headers: {
            'x-yuyue-upload-id': reservation!.uploadId,
            'x-yuyue-upload-token': reservation!.token,
            'x-yuyue-upload-sequence': String(sequence),
            'x-yuyue-upload-length': String(chunk.byteLength),
          },
        })
        sequence += 1
      })
      const outcome = await this.call<DocumentWriteOutcome>('finalize_document_write', {
        request: { uploadId: reservation.uploadId, token: reservation.token },
      })
      if (outcome.kind === 'saved') {
        session.markWriteSaved(outcome.commitId, outcome.writeGeneration, outcome.sourceRevision, outcome.contextEpoch)
      } else if (outcome.kind === 'contextRebound') {
        session.markContextRebound(
          outcome.commitId,
          outcome.writeGeneration,
          reservation.contextEpoch,
          outcome.contextEpoch,
          outcome.sourceRevision,
        )
      } else if (outcome.kind === 'conflict') {
        session.markConflict(outcome.observedRevision, outcome.conflictToken)
      } else if (outcome.kind === 'recoveryRequired') {
        session.markRecoveryRequired(outcome.recoveryEventId, outcome.recoveryRecords)
      } else {
        throw new Error('DOCUMENT_WRITE_PROTOCOL')
      }
    } catch (error) {
      const code = errorCode(error)
      if (code === 'DOCUMENT_WRITE_CONFLICT') {
        try {
          const refreshed = await this.call<{ observedRevision: number; conflictToken: string }>('refresh_document_conflict', {
            documentId: session.documentId,
            contextEpoch: reservation?.contextEpoch ?? session.contextEpoch,
          })
          session.markConflict(refreshed.observedRevision, refreshed.conflictToken)
        } catch {
          session.markConflict(session.snapshot.persistedRevision, session.snapshot.conflictToken)
        }
      } else {
        session.markWriteFailed(code, commitId, writeGeneration, reservation?.contextEpoch ?? session.contextEpoch)
      }
      if (reservation) {
        void this.call<void>('cancel_document_write', {
          request: { uploadId: reservation.uploadId, token: reservation.token },
        }).catch(() => {})
      }
    } finally {
      this.onSessionStateChange()
    }
  }
}
