import {
  MAX_EDITOR_SESSION_BYTES,
  MAX_EDITOR_SESSION_GLOBAL_BYTES,
  MAX_EDITOR_SOURCE_BYTES,
  EditorSession,
  createEditorSession,
  type CreateEditorSessionOptions,
} from './editorSession'

export interface OpenEditorSessionOptions {
  documentId: string
  source: string
  contextEpoch?: number
  persistedRevision?: number
}

export interface EditorSessionManagerOptions {
  maxDocumentBytes?: number
  maxGlobalBytes?: number
}

function sourceCharge(source: string) {
  return Math.max(new TextEncoder().encode(source).byteLength, source.length * 2)
}

/**
 * Keeps CodeMirror state out of Vue's deep reactivity while enforcing the
 * shared budget that the reader tab limit intentionally does not guarantee.
 */
export class EditorSessionManager {
  private readonly sessions = new Map<string, EditorSession>()
  private readonly maxDocumentBytes: number
  private readonly maxGlobalBytes: number

  constructor(options: EditorSessionManagerOptions = {}) {
    this.maxDocumentBytes = options.maxDocumentBytes ?? MAX_EDITOR_SESSION_BYTES
    this.maxGlobalBytes = options.maxGlobalBytes ?? MAX_EDITOR_SESSION_GLOBAL_BYTES
  }

  get(documentId: string) {
    return this.sessions.get(documentId)
  }

  open(options: OpenEditorSessionOptions) {
    const existing = this.sessions.get(options.documentId)
    if (existing) return existing

    if (new TextEncoder().encode(options.source).byteLength > MAX_EDITOR_SOURCE_BYTES) return undefined
    if (sourceCharge(options.source) > this.maxDocumentBytes) return undefined
    if (this.totalSessionCharge() + sourceCharge(options.source) > this.maxGlobalBytes) return undefined

    const sessionOptions: CreateEditorSessionOptions = {
      ...options,
      maxDocumentBytes: this.maxDocumentBytes,
      maxGlobalBytes: this.maxGlobalBytes,
      getOtherSessionCharge: () => this.totalSessionChargeExcept(options.documentId),
    }
    const session = createEditorSession(sessionOptions)
    this.sessions.set(options.documentId, session)
    return session
  }

  close(documentId: string) {
    return this.sessions.delete(documentId)
  }

  totalSessionCharge() {
    let total = 0
    for (const session of this.sessions.values()) total += session.snapshot.sessionCharge
    return total
  }

  private totalSessionChargeExcept(documentId: string) {
    let total = 0
    for (const [id, session] of this.sessions) {
      if (id !== documentId) total += session.snapshot.sessionCharge
    }
    return total
  }
}
