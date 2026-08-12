import type { RenderDocument, RenderOptions } from './renderer'
import RenderWorker from './worker?worker'

type RenderResponse =
  | { type: 'rendered'; requestId: string; documentKey: string; document: unknown; serializedBytes: unknown }
  | { type: 'failed'; requestId: string; documentKey: string; code: string }

export interface RenderResult {
  document: RenderDocument
  serializedBytes: number
}

type PendingRender = {
  documentKey: string
  source: string
  sourceBytes: number
  priority: number
  sequence: number
  options: RenderOptions
  resolve: (result: RenderResult) => void
  reject: (error: RenderWorkerError) => void
}

const MAX_PENDING_SOURCE_BYTES = 64 * 1024 * 1024
const MAX_RENDER_RESULT_BYTES = 128 * 1024 * 1024
const MAX_RENDER_COLLECTION_ITEMS = 200_000

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isBoundedString(value: unknown): value is string {
  return typeof value === 'string' && value.length <= MAX_RENDER_RESULT_BYTES
}

function isBoundedArray(value: unknown): value is unknown[] {
  return Array.isArray(value) && value.length <= MAX_RENDER_COLLECTION_ITEMS
}

function isNonNegativeSafeInteger(value: unknown, maximum = MAX_RENDER_RESULT_BYTES): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value <= maximum
}

function isSourcePosition(value: unknown) {
  if (value === undefined) return true
  if (!isRecord(value)) return false
  const start = value.start
  const end = value.end
  return isNonNegativeSafeInteger(start) && isNonNegativeSafeInteger(end) && start <= end
}

function lineCount(source: string) {
  let lines = 1
  for (let index = 0; index < source.length; index += 1) {
    const code = source.charCodeAt(index)
    if (code === 10) {
      lines += 1
    } else if (code === 13) {
      lines += 1
      if (source.charCodeAt(index + 1) === 10) index += 1
    }
  }
  return lines
}

function isRenderDocument(value: unknown, pending?: PendingRender): value is RenderDocument {
  if (!isRecord(value) ||
    !isBoundedString(value.html) ||
    !isBoundedArray(value.outline) ||
    !isBoundedArray(value.resources) ||
    !isBoundedArray(value.links) ||
    !isBoundedArray(value.diagrams) ||
    !isBoundedArray(value.diagnostics) ||
    !isBoundedArray(value.sourceBlocks) ||
    !isBoundedString(value.renderLeaseId) ||
    !isNonNegativeSafeInteger(value.contextEpoch) ||
    !isNonNegativeSafeInteger(value.renderGeneration) ||
    !isRecord(value.stats)) return false

  const outlineValid = value.outline.every((item) => {
    if (!isRecord(item)) return false
    const level = item.level
    return isBoundedString(item.id) &&
      isBoundedString(item.text) &&
      isNonNegativeSafeInteger(level, 6) &&
      level >= 1 &&
      isSourcePosition(item.sourcePosition)
  })
  const resourcesValid = value.resources.every((item) => isRecord(item) &&
    isBoundedString(item.id) &&
    (item.kind === 'local' || item.kind === 'https' || item.kind === 'data' || item.kind === 'blocked') &&
    isBoundedString(item.source) &&
    isBoundedString(item.alt) &&
    (item.title === undefined || isBoundedString(item.title)))
  const linksValid = value.links.every((item) => isRecord(item) &&
    isBoundedString(item.id) &&
    (item.kind === 'internal' || item.kind === 'external' || item.kind === 'blocked') &&
    isBoundedString(item.url) &&
    isBoundedString(item.text))
  const diagramsValid = value.diagrams.every((item) => isRecord(item) &&
    isBoundedString(item.id) &&
    isBoundedString(item.source) &&
    (item.error === undefined || item.error === 'MERMAID_LIMIT'))
  const diagnosticsValid = value.diagnostics.every((item) => isRecord(item) &&
    isBoundedString(item.code) &&
    (item.severity === 'info' || item.severity === 'warning' || item.severity === 'error') &&
    isBoundedString(item.message) &&
    isSourcePosition(item.sourcePosition))
  const stats = value.stats
  const statsValid = isNonNegativeSafeInteger(stats.inputBytes) &&
    isNonNegativeSafeInteger(stats.astNodes, MAX_RENDER_COLLECTION_ITEMS) &&
    stats.headingCount === value.outline.length &&
    stats.diagramCount === value.diagrams.length

  const totalSourceLines = pending ? lineCount(pending.source) : Number.MAX_SAFE_INTEGER
  const sourceBlockIds = new Set<string>()
  const sourceBlocksValid = value.sourceBlocks.every((item) => {
    if (!isRecord(item) || typeof item.id !== 'string' || !/^sb-[a-f0-9]{8}-[0-9]{1,6}$/.test(item.id)) return false
    if (item.kind !== undefined && item.kind !== 'heading' && item.kind !== 'block') return false
    if (!isNonNegativeSafeInteger(item.startLine) || item.startLine < 1
      || !isNonNegativeSafeInteger(item.endLine) || item.endLine < item.startLine
      || item.endLine > totalSourceLines || sourceBlockIds.has(item.id)) return false
    sourceBlockIds.add(item.id)
    return true
  })
  const domSet = new Set<string>()
  const sourceBlockPattern = /\sdata-md-source-block-id="([^"]+)"/g
  let sourceBlockDomValid = true
  let domMatch: RegExpExecArray | null
  while ((domMatch = sourceBlockPattern.exec(value.html)) !== null) {
    const id = domMatch[1]
    if (domSet.has(id)) {
      sourceBlockDomValid = false
      break
    }
    domSet.add(id)
  }
  sourceBlockDomValid = sourceBlockDomValid
    && domSet.size === sourceBlockIds.size
    && [...domSet].every((id) => sourceBlockIds.has(id))

  const renderContextValid = /^lease-[a-z0-9-]{1,120}$/i.test(value.renderLeaseId)
    && (!pending || pending.options.contextEpoch === undefined || value.contextEpoch === pending.options.contextEpoch)
    && (!pending || pending.options.renderGeneration === undefined || value.renderGeneration === pending.options.renderGeneration)
    && (!pending || pending.options.renderLeaseId === undefined || value.renderLeaseId === pending.options.renderLeaseId)

  return outlineValid && resourcesValid && linksValid && diagramsValid && diagnosticsValid
    && statsValid && sourceBlocksValid && sourceBlockDomValid && renderContextValid
}

function isRenderResponse(value: unknown): value is RenderResponse {
  if (!isRecord(value) ||
    (value.type !== 'rendered' && value.type !== 'failed') ||
    typeof value.requestId !== 'string' ||
    typeof value.documentKey !== 'string') return false
  if (value.type === 'rendered') return 'document' in value && 'serializedBytes' in value
  return typeof value.code === 'string'
}

export class RenderWorkerError extends Error {
  constructor(public readonly code: string) {
    super(code)
    this.name = 'RenderWorkerError'
  }
}

export class RenderWorkerSupervisor {
  private worker: Worker | null = null
  private epoch = 0
  private sequence = 0
  private pending = new Map<string, PendingRender>()
  private latestByDocument = new Map<string, string>()
  private queue: string[] = []
  private inFlight: string | null = null
  private pendingSourceBytes = 0
  private executionTimeouts = new Map<string, ReturnType<typeof setTimeout>>()

  constructor(
    private readonly workerFactory: () => Worker = () => new RenderWorker(),
    private readonly maxPendingSourceBytes = MAX_PENDING_SOURCE_BYTES,
  ) {}

  prepare(): boolean {
    try {
      this.ensureWorker()
      return true
    } catch {
      this.worker = null
      return false
    }
  }

  render(source: string, documentKey: string, priority = 0, options: RenderOptions = {}): Promise<RenderResult> {
    const sourceBytes = new TextEncoder().encode(source).byteLength
    const previousRequestId = this.latestByDocument.get(documentKey)
    const previous = previousRequestId ? this.pending.get(previousRequestId) : undefined
    const previousBytes = previous?.sourceBytes ?? 0
    if (this.pendingSourceBytes - previousBytes + sourceBytes > this.maxPendingSourceBytes) {
      return Promise.reject(new RenderWorkerError('RENDER_QUEUE_LIMIT'))
    }

    let worker: Worker
    try {
      worker = this.ensureWorker()
    } catch {
      this.restartWorker()
      return Promise.reject(new RenderWorkerError('RENDER_CRASH'))
    }
    const requestId = `${this.epoch}-${++this.sequence}`
    if (previousRequestId) {
      this.rejectPending(previousRequestId, new RenderWorkerError('RENDER_STALE'), {
        preserveInFlightTimeout: this.inFlight === previousRequestId,
      })
    }
    this.latestByDocument.set(documentKey, requestId)

    return new Promise<RenderResult>((resolve, reject) => {
      this.pending.set(requestId, {
        documentKey,
        source,
        sourceBytes,
        priority,
        sequence: this.sequence,
        options,
        resolve,
        reject,
      })
      this.pendingSourceBytes += sourceBytes
      this.queue.push(requestId)
      this.pump(worker)
    })
  }

  dispose() {
    this.restartWorker()
  }

  cancel(documentKey: string) {
    const requestId = this.latestByDocument.get(documentKey)
    if (requestId) {
      this.rejectPending(requestId, new RenderWorkerError('RENDER_CANCELLED'), {
        preserveInFlightTimeout: this.inFlight === requestId,
      })
      this.pump(this.worker)
    }
  }

  private ensureWorker(): Worker {
    if (this.worker) return this.worker
    const epoch = this.epoch
    const worker = this.workerFactory()
    worker.onmessage = (event: MessageEvent<unknown>) => {
      if (epoch !== this.epoch) return
      const response = event.data
      if (!isRenderResponse(response)) {
        this.restartWorker(new RenderWorkerError('RENDER_PROTOCOL_ERROR'))
        return
      }
      const pending = this.pending.get(response.requestId)
      this.clearExecutionTimeout(response.requestId)
      if (!pending) {
        // A newer request for the same document may have superseded an
        // in-flight request. The worker still sends the old response, and
        // that response is the hand-off point at which the queue can move on.
        if (this.inFlight === response.requestId) {
          this.inFlight = null
          this.pump(worker)
        }
        return
      }
      if (response.documentKey !== pending.documentKey) {
        this.rejectPending(response.requestId, new RenderWorkerError('RENDER_PROTOCOL_ERROR'))
        if (this.inFlight === response.requestId) this.inFlight = null
        this.restartWorker()
        return
      }
      let rendered: RenderResult | undefined
      if (response.type === 'rendered') {
        if (!isNonNegativeSafeInteger(response.serializedBytes) || !isRenderDocument(response.document, pending)) {
          this.rejectPending(response.requestId, new RenderWorkerError('RENDER_PROTOCOL_ERROR'))
          if (this.inFlight === response.requestId) this.inFlight = null
          this.restartWorker()
          return
        }
        rendered = { document: response.document, serializedBytes: response.serializedBytes }
      }
      this.pending.delete(response.requestId)
      this.pendingSourceBytes = Math.max(0, this.pendingSourceBytes - pending.sourceBytes)
      if (this.inFlight === response.requestId) this.inFlight = null
      if (this.latestByDocument.get(pending.documentKey) === response.requestId) {
        this.latestByDocument.delete(pending.documentKey)
      }
      if (response.type === 'rendered') {
        pending.resolve(rendered as RenderResult)
      } else {
        pending.reject(new RenderWorkerError(response.code))
      }
      this.pump(worker)
    }
    worker.onerror = () => {
      if (epoch !== this.epoch) return
      this.rejectAll(new RenderWorkerError('RENDER_CRASH'))
      this.restartWorker()
    }
    this.worker = worker
    return worker
  }

  private rejectPending(
    requestId: string,
    error: RenderWorkerError,
    options: { preserveInFlightTimeout?: boolean } = {},
  ) {
    const pending = this.pending.get(requestId)
    if (!pending) return
    if (!options.preserveInFlightTimeout) this.clearExecutionTimeout(requestId)
    this.pending.delete(requestId)
    this.pendingSourceBytes = Math.max(0, this.pendingSourceBytes - pending.sourceBytes)
    this.queue = this.queue.filter((queuedRequestId) => queuedRequestId !== requestId)
    if (this.latestByDocument.get(pending.documentKey) === requestId) {
      this.latestByDocument.delete(pending.documentKey)
    }
    pending.reject(error)
  }

  private rejectAll(error: RenderWorkerError) {
    for (const [requestId] of this.pending) this.rejectPending(requestId, error)
    for (const requestId of this.executionTimeouts.keys()) this.clearExecutionTimeout(requestId)
    this.latestByDocument.clear()
    this.queue = []
    this.inFlight = null
    this.pendingSourceBytes = 0
  }

  private clearExecutionTimeout(requestId: string) {
    const timeout = this.executionTimeouts.get(requestId)
    if (!timeout) return
    clearTimeout(timeout)
    this.executionTimeouts.delete(requestId)
  }

  private armExecutionTimeout(requestId: string) {
    this.clearExecutionTimeout(requestId)
    const timeout = setTimeout(() => {
      this.clearExecutionTimeout(requestId)
      this.rejectPending(requestId, new RenderWorkerError('RENDER_TIMEOUT'))
      if (this.inFlight === requestId) this.inFlight = null
      this.restartWorker()
    }, 15_000)
    this.executionTimeouts.set(requestId, timeout)
  }

  private restartWorker(error = new RenderWorkerError('RENDER_WORKER_RESTARTED')) {
    this.epoch += 1
    this.rejectAll(error)
    this.worker?.terminate()
    this.worker = null
  }

  private pump(worker: Worker | null) {
    if (!worker || this.inFlight) return
    let selectedIndex = -1
    let selected: PendingRender | undefined
    for (let index = 0; index < this.queue.length; index += 1) {
      const pending = this.pending.get(this.queue[index])
      if (!pending) continue
      if (!selected || pending.priority > selected.priority || pending.priority === selected.priority && pending.sequence < selected.sequence) {
        selectedIndex = index
        selected = pending
      }
    }
    if (!selected || selectedIndex < 0) {
      this.queue = []
      return
    }
    const [requestId] = this.queue.splice(selectedIndex, 1)
    this.inFlight = requestId
    try {
      worker.postMessage({
        type: 'render',
        requestId,
        documentKey: selected.documentKey,
        source: selected.source,
        ...selected.options,
      })
      this.armExecutionTimeout(requestId)
    } catch {
      this.inFlight = null
      this.rejectPending(requestId, new RenderWorkerError('RENDER_CRASH'))
      this.restartWorker()
    }
  }
}
