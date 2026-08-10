import type { RenderDocument } from './renderer'
import RenderWorker from './worker?worker'

type RenderResponse =
  | { type: 'rendered'; requestId: string; documentKey: string; document: RenderDocument }
  | { type: 'failed'; requestId: string; documentKey: string; code: string }

type PendingRender = {
  documentKey: string
  source: string
  sourceBytes: number
  priority: number
  sequence: number
  resolve: (document: RenderDocument) => void
  reject: (error: RenderWorkerError) => void
}

const MAX_PENDING_SOURCE_BYTES = 64 * 1024 * 1024

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

  render(source: string, documentKey: string, priority = 0): Promise<RenderDocument> {
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

    return new Promise<RenderDocument>((resolve, reject) => {
      this.pending.set(requestId, {
        documentKey,
        source,
        sourceBytes,
        priority,
        sequence: this.sequence,
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
    worker.onmessage = (event: MessageEvent<RenderResponse>) => {
      if (epoch !== this.epoch) return
      const response = event.data
      if (
        !response ||
        typeof response !== 'object' ||
        (response.type !== 'rendered' && response.type !== 'failed') ||
        typeof response.requestId !== 'string' ||
        typeof response.documentKey !== 'string'
      ) {
        this.restartWorker()
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
      this.pending.delete(response.requestId)
      this.pendingSourceBytes = Math.max(0, this.pendingSourceBytes - pending.sourceBytes)
      if (this.inFlight === response.requestId) this.inFlight = null
      if (this.latestByDocument.get(pending.documentKey) === response.requestId) {
        this.latestByDocument.delete(pending.documentKey)
      }
      if (response.type === 'rendered') {
        pending.resolve(response.document)
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

  private restartWorker() {
    this.epoch += 1
    this.rejectAll(new RenderWorkerError('RENDER_WORKER_RESTARTED'))
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
      })
      this.armExecutionTimeout(requestId)
    } catch {
      this.inFlight = null
      this.rejectPending(requestId, new RenderWorkerError('RENDER_CRASH'))
      this.restartWorker()
    }
  }
}
