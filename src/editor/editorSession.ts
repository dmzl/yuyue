import { Compartment, EditorState, StateEffect, Text, Transaction, type Extension, type TransactionSpec } from '@codemirror/state'
import { history } from '@codemirror/commands'

export const MAX_SNAPSHOT_SLICE_CODE_UNITS = 16 * 1024
export const MAX_ENCODED_SNAPSHOT_CHUNK_BYTES = 48 * 1024
export const MAX_RAW_SNAPSHOT_CHUNK_BYTES = 64 * 1024
export const MAX_EDITOR_SOURCE_BYTES = 10 * 1024 * 1024
export const MAX_EDITOR_SESSION_BYTES = 48 * 1024 * 1024
export const MAX_EDITOR_SESSION_GLOBAL_BYTES = 96 * 1024 * 1024
const NORMAL_TRANSACTION_HEADROOM_BYTES = 1024 * 1024
const EVENT_OVERHEAD_BYTES = 16 * 1024

export type EditorWriteStatus = 'saved' | 'pending' | 'writing' | 'failed' | 'conflict' | 'recovery-required'
export interface RecoveryRecordSummary {
  recordId: string
  ordinal: number
  action: 'saveCurrentBufferCopy' | 'revealPreservedItem'
  actionToken: string
  actionCompleted?: boolean
  ackToken?: string
  acknowledged?: boolean
  savedGeneration?: number
}

export interface EditorSessionSnapshot {
  documentId: string
  contextEpoch: number
  persistedRevision: number
  editGeneration: number
  persistedGeneration: number
  previewGeneration: number
  writeStatus: EditorWriteStatus
  sourceUtf8Bytes: number
  sourceHighWaterCharge: number
  historyCharge: number
  sessionCharge: number
  lastError?: string
  conflictToken?: string
  conflictAtRevision?: number
  inFlightCommitId?: string
  inFlightGeneration?: number
  recoveryEventId?: string
  recoveryRecords?: RecoveryRecordSummary[]
}

export interface CreateEditorSessionOptions {
  documentId: string
  source: string
  contextEpoch?: number
  persistedRevision?: number
  maxDocumentBytes?: number
  maxGlobalBytes?: number
  getOtherSessionCharge?: () => number
}

type Charge = {
  nextUtf8Bytes: number
  nextSourceCharge: number
  eventCharge: number
}

function isHighSurrogate(value: number) {
  return value >= 0xd800 && value <= 0xdbff
}

function isLowSurrogate(value: number) {
  return value >= 0xdc00 && value <= 0xdfff
}

function safeSnapshotEnd(text: Text, from: number, maximumEnd: number) {
  if (maximumEnd >= text.length || maximumEnd <= from) return maximumEnd
  const before = text.sliceString(maximumEnd - 1, maximumEnd).charCodeAt(0)
  const after = text.sliceString(maximumEnd, maximumEnd + 1).charCodeAt(0)
  return isHighSurrogate(before) && isLowSurrogate(after) ? maximumEnd - 1 : maximumEnd
}

/**
 * Iterates immutable CodeMirror text in a bounded window. This is the only
 * primitive the save uploader uses; it deliberately never calls Text#toString
 * or relies on Text#iterRange yield size.
 */
export function forEachBoundedSnapshotSlice(text: Text, visit: (slice: string) => void) {
  let from = 0
  while (from < text.length) {
    const tentativeEnd = Math.min(text.length, from + MAX_SNAPSHOT_SLICE_CODE_UNITS)
    const end = safeSnapshotEnd(text, from, tentativeEnd)
    if (end <= from) throw new Error('EDITOR_SNAPSHOT_BOUNDARY_INVALID')
    const slice = text.sliceString(from, end)
    visit(slice)
    from = end
  }
}

/** Sends each bounded UTF-8 body to the caller, one at a time. */
export function forEachBoundedSnapshotChunk(text: Text, visit: (chunk: Uint8Array) => void) {
  const encoder = new TextEncoder()
  forEachBoundedSnapshotSlice(text, (slice) => {
    const chunk = encoder.encode(slice)
    if (chunk.byteLength > MAX_ENCODED_SNAPSHOT_CHUNK_BYTES || chunk.byteLength > MAX_RAW_SNAPSHOT_CHUNK_BYTES) {
      throw new Error('EDITOR_SNAPSHOT_CHUNK_TOO_LARGE')
    }
    visit(chunk)
  })
}

/**
 * Async counterpart for the raw IPC uploader. Each request is awaited before
 * the next immutable snapshot window is encoded, so neither the whole string
 * nor an unbounded queued byte array is materialized in the renderer process.
 */
export async function forEachBoundedSnapshotChunkAsync(
  text: Text,
  visit: (chunk: Uint8Array) => Promise<void>,
) {
  const encoder = new TextEncoder()
  let from = 0
  while (from < text.length) {
    const tentativeEnd = Math.min(text.length, from + MAX_SNAPSHOT_SLICE_CODE_UNITS)
    const end = safeSnapshotEnd(text, from, tentativeEnd)
    if (end <= from) throw new Error('EDITOR_SNAPSHOT_BOUNDARY_INVALID')
    const chunk = encoder.encode(text.sliceString(from, end))
    if (chunk.byteLength > MAX_ENCODED_SNAPSHOT_CHUNK_BYTES || chunk.byteLength > MAX_RAW_SNAPSHOT_CHUNK_BYTES) {
      throw new Error('EDITOR_SNAPSHOT_CHUNK_TOO_LARGE')
    }
    await visit(chunk)
    from = end
  }
}

export function utf8ByteLength(text: Text) {
  let total = 0
  forEachBoundedSnapshotChunk(text, (chunk) => {
    total += chunk.byteLength
  })
  return total
}

/**
 * Produces a short-lived string only at a rendering or reading boundary. Save
 * code must use forEachBoundedSnapshotChunk instead.
 */
export function readEditorText(text: Text) {
  const slices: string[] = []
  forEachBoundedSnapshotSlice(text, (slice) => slices.push(slice))
  return slices.join('')
}

function sourceCharge(utf8Bytes: number, utf16CodeUnits: number) {
  return Math.max(utf8Bytes, utf16CodeUnits * 2)
}

function isHistoryTraversal(transaction: Transaction) {
  return transaction.isUserEvent('undo') || transaction.isUserEvent('redo')
}

function countChangedRanges(transaction: Transaction) {
  let count = 0
  transaction.changes.iterChanges(() => { count += 1 }, true)
  return count
}

function transactionCharge(transaction: Transaction, currentUtf8Bytes: number): Charge {
  let nextUtf8Bytes = currentUtf8Bytes
  let insertedUtf16Bytes = 0
  let replacedUtf16Bytes = 0

  transaction.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    const replaced = transaction.startState.doc.slice(fromA, toA)
    nextUtf8Bytes -= utf8ByteLength(replaced)
    nextUtf8Bytes += utf8ByteLength(inserted)
    insertedUtf16Bytes += inserted.length * 2
    replacedUtf16Bytes += (toA - fromA) * 2
  }, true)

  const changedRangeCount = countChangedRanges(transaction)
  const startSelectionRangeCount = transaction.startState.selection.ranges.length
  const newSelectionRangeCount = transaction.newSelection.ranges.length
  const invertedEffectCount = 0
  const eventCharge = 8 * (insertedUtf16Bytes + replacedUtf16Bytes)
    + EVENT_OVERHEAD_BYTES * (changedRangeCount + startSelectionRangeCount + newSelectionRangeCount + invertedEffectCount + 1)

  return {
    nextUtf8Bytes,
    nextSourceCharge: sourceCharge(nextUtf8Bytes, transaction.newDoc.length),
    eventCharge,
  }
}

export class EditorSession {
  private readonly maxDocumentBytes: number
  private readonly maxGlobalBytes: number
  private readonly getOtherSessionCharge: () => number
  private readonly extensions: Extension[]
  private readonly editorCompartment = new Compartment()
  private editorConfigured = false
  private currentEditorExtensions?: Extension
  private _state: EditorState
  private sourceUtf8Bytes: number
  private sourceHighWaterCharge: number
  private historyCharge = 0
  private editGeneration = 0
  private persistedGeneration = 0
  private previewGeneration = 0
  private writeStatus: EditorWriteStatus = 'saved'
  private lastError?: string
  private conflictToken?: string
  private conflictAtRevision?: number
  private inFlightCommitId?: string
  private inFlightGeneration?: number
  private recoveryEventId?: string
  private recoveryRecords?: RecoveryRecordSummary[]

  readonly documentId: string
  contextEpoch: number
  persistedRevision: number

  constructor(options: CreateEditorSessionOptions) {
    this.documentId = options.documentId
    this.contextEpoch = options.contextEpoch ?? 0
    this.persistedRevision = options.persistedRevision ?? 0
    this.maxDocumentBytes = options.maxDocumentBytes ?? MAX_EDITOR_SESSION_BYTES
    this.maxGlobalBytes = options.maxGlobalBytes ?? MAX_EDITOR_SESSION_GLOBAL_BYTES
    this.getOtherSessionCharge = options.getOtherSessionCharge ?? (() => 0)
    this._state = EditorState.create({ doc: options.source })
    this.sourceUtf8Bytes = utf8ByteLength(this._state.doc)
    this.sourceHighWaterCharge = sourceCharge(this.sourceUtf8Bytes, this._state.doc.length)
    this.extensions = [
      history({ minDepth: Number.MAX_SAFE_INTEGER }),
      EditorState.transactionFilter.of((transaction) => this.admit(transaction)),
      EditorState.transactionExtender.of((transaction) => {
        if (!transaction.docChanged && !transaction.startState.selection.eq(transaction.newSelection)) {
          return { annotations: Transaction.addToHistory.of(false) }
        }
        return null
      }),
    ]
    this._state = EditorState.create({ doc: options.source, extensions: this.extensions })
  }

  get state() {
    return this._state
  }

  /**
   * View and language extensions are configured only after the user enters
   * edit mode. Keeping them outside the initial state lets the app lazy-load
   * the CodeMirror view bundle instead of paying for it at reader launch.
   */
  configureEditor(extensions: Extension) {
    this.currentEditorExtensions = extensions
    if (!this.editorConfigured) {
      this.editorConfigured = true
      this.accept(this._state.update({ effects: StateEffect.appendConfig.of(this.editorCompartment.of(extensions)) }))
      return true
    }
    this.accept(this._state.update({ effects: this.editorCompartment.reconfigure(extensions) }))
    return false
  }

  get snapshot(): EditorSessionSnapshot {
    return {
      documentId: this.documentId,
      contextEpoch: this.contextEpoch,
      persistedRevision: this.persistedRevision,
      editGeneration: this.editGeneration,
      persistedGeneration: this.persistedGeneration,
      previewGeneration: this.previewGeneration,
      writeStatus: this.writeStatus,
      sourceUtf8Bytes: this.sourceUtf8Bytes,
      sourceHighWaterCharge: this.sourceHighWaterCharge,
      historyCharge: this.historyCharge,
      sessionCharge: this.sourceHighWaterCharge + this.historyCharge,
      ...(this.lastError ? { lastError: this.lastError } : {}),
      ...(this.conflictToken ? { conflictToken: this.conflictToken } : {}),
      ...(this.conflictAtRevision !== undefined ? { conflictAtRevision: this.conflictAtRevision } : {}),
      ...(this.inFlightCommitId ? { inFlightCommitId: this.inFlightCommitId } : {}),
      ...(this.inFlightGeneration !== undefined ? { inFlightGeneration: this.inFlightGeneration } : {}),
      ...(this.recoveryEventId ? { recoveryEventId: this.recoveryEventId } : {}),
      ...(this.recoveryRecords ? { recoveryRecords: this.recoveryRecords.map((record) => ({ ...record })) } : {}),
    }
  }

  dispatch(spec: TransactionSpec) {
    const transaction = this._state.update(spec)
    this.accept(transaction)
    return transaction
  }

  accept(transaction: Transaction) {
    if (transaction.startState !== this._state) {
      throw new Error('EDITOR_TRANSACTION_STALE')
    }

    if (transaction.docChanged) {
      const charge = transactionCharge(transaction, this.sourceUtf8Bytes)
      if (charge.nextUtf8Bytes < 0) throw new Error('EDITOR_BYTE_ACCOUNTING_INVALID')
      if (isHistoryTraversal(transaction)) {
        this.sourceUtf8Bytes = charge.nextUtf8Bytes
      } else {
        if (transaction.annotation(Transaction.addToHistory) === false) {
          throw new Error('EDITOR_HISTORY_BYPASS_REJECTED')
        }
        this.assertAdmittedCharge(charge, transaction)
        this.sourceUtf8Bytes = charge.nextUtf8Bytes
        this.sourceHighWaterCharge = Math.max(this.sourceHighWaterCharge, charge.nextSourceCharge)
        this.historyCharge += charge.eventCharge
      }
      this.editGeneration += 1
      if (this.writeStatus !== 'recovery-required') this.writeStatus = 'pending'
    }

    this._state = transaction.state
    return this._state
  }

  markPreviewRequested() {
    this.previewGeneration += 1
    return this.previewGeneration
  }

  markWriteStarted(commitId: string, writeGeneration: number, conflictToken?: string, allowConflict = false) {
    const recoveryRebind = this.writeStatus === 'recovery-required'
      && allowConflict
      && Boolean(this.recoveryRecords?.length)
      && this.recoveryRecords!.every((record) => record.acknowledged)
    if (this.writeStatus === 'recovery-required' && !recoveryRebind) return false
    if (this.writeStatus === 'conflict' && !allowConflict && (!conflictToken || conflictToken !== this.conflictToken)) return false
    this.inFlightCommitId = commitId
    this.inFlightGeneration = writeGeneration
    if (!recoveryRebind) this.writeStatus = 'writing'
    this.lastError = undefined
    return true
  }

  private matchesActiveWrite(commitId: string, writeGeneration: number, contextEpoch: number) {
    return this.contextEpoch === contextEpoch
      && this.inFlightCommitId === commitId
      && this.inFlightGeneration === writeGeneration
      && this.writeStatus !== 'conflict'
      && this.writeStatus !== 'recovery-required'
  }

  markWriteSaved(commitId: string, writeGeneration: number, sourceRevision: number, contextEpoch: number) {
    if (!this.matchesActiveWrite(commitId, writeGeneration, contextEpoch)) return false
    this.persistedRevision = Math.max(this.persistedRevision, sourceRevision)
    this.persistedGeneration = Math.max(this.persistedGeneration, writeGeneration)
    this.inFlightCommitId = undefined
    this.inFlightGeneration = undefined
    this.writeStatus = this.persistedGeneration === this.editGeneration ? 'saved' : 'pending'
    return true
  }

  markContextRebound(commitId: string, writeGeneration: number, previousContextEpoch: number, contextEpoch: number, sourceRevision: number) {
    const recoveryRebind = this.writeStatus === 'recovery-required'
      && this.contextEpoch === previousContextEpoch
      && this.inFlightCommitId === commitId
      && this.inFlightGeneration === writeGeneration
      && Boolean(this.recoveryRecords?.length)
      && this.recoveryRecords!.every((record) => record.acknowledged)
    if (contextEpoch <= previousContextEpoch || (!recoveryRebind && !this.matchesActiveWrite(commitId, writeGeneration, previousContextEpoch))) return false
    this.persistedRevision = Math.max(this.persistedRevision, sourceRevision)
    this.persistedGeneration = Math.max(this.persistedGeneration, writeGeneration)
    this.inFlightCommitId = undefined
    this.inFlightGeneration = undefined
    this.contextEpoch = contextEpoch
    this.conflictToken = undefined
    this.conflictAtRevision = undefined
    this.recoveryEventId = undefined
    this.recoveryRecords = undefined
    this.writeStatus = this.persistedGeneration === this.editGeneration ? 'saved' : 'pending'
    return true
  }

  markWriteFailed(code: string, commitId: string, writeGeneration: number, contextEpoch: number) {
    if (!this.matchesActiveWrite(commitId, writeGeneration, contextEpoch)) return false
    this.inFlightCommitId = undefined
    this.inFlightGeneration = undefined
    if (writeGeneration < this.editGeneration) {
      this.writeStatus = 'pending'
      return true
    }
    this.lastError = code
    this.writeStatus = 'failed'
    return true
  }

  markConflict(sourceRevision: number, token?: string) {
    if (this.writeStatus === 'recovery-required') return
    this.persistedRevision = Math.max(this.persistedRevision, sourceRevision)
    this.conflictAtRevision = sourceRevision
    this.conflictToken = token
    this.inFlightCommitId = undefined
    this.inFlightGeneration = undefined
    this.writeStatus = 'conflict'
  }

  markRecoveryRequired(recoveryEventId?: string, recoveryRecords?: RecoveryRecordSummary[]) {
    this.writeStatus = 'recovery-required'
    this.recoveryEventId = recoveryEventId
    this.recoveryRecords = recoveryRecords?.map((record) => ({ ...record }))
    this.conflictToken = undefined
    this.inFlightCommitId = undefined
    this.inFlightGeneration = undefined
  }

  markRecoveryActionCompleted(recordId: string, ackToken: string, savedGeneration?: number) {
    const record = this.recoveryRecords?.find((candidate) => candidate.recordId === recordId)
    if (!record || this.writeStatus !== 'recovery-required') return false
    record.actionCompleted = true
    record.actionToken = ''
    record.ackToken = ackToken
    if (savedGeneration !== undefined) record.savedGeneration = savedGeneration
    return true
  }

  markRecoveryActionToken(recordId: string, actionToken: string) {
    const record = this.recoveryRecords?.find((candidate) => candidate.recordId === recordId)
    if (!record || this.writeStatus !== 'recovery-required' || record.ackToken || record.acknowledged) return false
    record.actionToken = actionToken
    return true
  }

  markRecoveryAcknowledged(recordId: string) {
    const record = this.recoveryRecords?.find((candidate) => candidate.recordId === recordId)
    if (!record || this.writeStatus !== 'recovery-required') return false
    record.actionCompleted = true
    record.acknowledged = true
    record.ackToken = undefined
    return true
  }

  replaceFromDisk(source: string, sourceRevision: number, nextContextEpoch = this.contextEpoch) {
    this.contextEpoch = nextContextEpoch
    this.persistedRevision = Math.max(this.persistedRevision, sourceRevision)
    this._state = EditorState.create({
      doc: source,
      extensions: this.currentEditorExtensions
        ? [...this.extensions, this.editorCompartment.of(this.currentEditorExtensions)]
        : this.extensions,
    })
    this.sourceUtf8Bytes = utf8ByteLength(this._state.doc)
    this.sourceHighWaterCharge = sourceCharge(this.sourceUtf8Bytes, this._state.doc.length)
    this.historyCharge = 0
    this.editGeneration = 0
    this.persistedGeneration = 0
    this.previewGeneration += 1
    this.writeStatus = 'saved'
    this.lastError = undefined
    this.conflictToken = undefined
    this.conflictAtRevision = undefined
    this.inFlightCommitId = undefined
    this.inFlightGeneration = undefined
    this.recoveryEventId = undefined
    this.recoveryRecords = undefined
  }

  private admit(transaction: Transaction): Transaction | readonly TransactionSpec[] {
    if (!transaction.docChanged) return transaction
    if (transaction.annotation(Transaction.addToHistory) === false) return []
    if (transaction.effects.length > 0 && !isHistoryTraversal(transaction)) return []
    if (isHistoryTraversal(transaction)) return transaction
    const charge = transactionCharge(transaction, this.sourceUtf8Bytes)
    if (!this.isChargeAdmitted(charge, transaction)) return []
    return transaction
  }

  private isChargeAdmitted(charge: Charge, transaction: Transaction) {
    const prospectiveSessionCharge = Math.max(this.sourceHighWaterCharge, charge.nextSourceCharge)
      + this.historyCharge
      + charge.eventCharge
    const transactionLimit = MAX_EDITOR_SESSION_BYTES - NORMAL_TRANSACTION_HEADROOM_BYTES
    const isComposition = transaction.isUserEvent('input.type.compose')
    const allowedDocumentCharge = isComposition ? this.maxDocumentBytes : Math.min(this.maxDocumentBytes, transactionLimit)
    return prospectiveSessionCharge <= allowedDocumentCharge
      && charge.nextUtf8Bytes <= MAX_EDITOR_SOURCE_BYTES
      && this.getOtherSessionCharge() + prospectiveSessionCharge <= this.maxGlobalBytes
  }

  private assertAdmittedCharge(charge: Charge, transaction: Transaction) {
    if (!this.isChargeAdmitted(charge, transaction)) throw new Error('EDITOR_SESSION_CAPACITY_EXCEEDED')
  }
}

export function createEditorSession(options: CreateEditorSessionOptions) {
  return new EditorSession(options)
}
