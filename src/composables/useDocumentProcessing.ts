import { computed, shallowRef } from 'vue'

export interface DocumentImportStartedEvent {
  operationId: string
  sequence: number
  fileName: string
}

export type DocumentProcessingPhase =
  | 'importing'
  | 'rendering'
  | 'awaiting-paint'
  | 'completed'
  | 'failed'
  | 'cancelled'

export interface DocumentProcessingOperation extends DocumentImportStartedEvent {
  phase: DocumentProcessingPhase
  activationIntent: number
  documentId?: string
  generation?: number
}

const TERMINAL_PHASES = new Set<DocumentProcessingPhase>(['completed', 'failed', 'cancelled'])
const MAX_RETAINED_OPERATIONS = 64

export function useDocumentProcessing(
  onTimeout: (operation: DocumentProcessingOperation, wasLatest: boolean) => void,
  deadlineMs = 15_000,
) {
  const operations = new Map<string, DocumentProcessingOperation>()
  const timers = new Map<string, ReturnType<typeof setTimeout>>()
  const version = shallowRef(0)
  let latestOperationId = ''
  let latestSequence = -1

  function changed() {
    version.value += 1
  }

  function clearTimer(operationId: string) {
    const timer = timers.get(operationId)
    if (timer) clearTimeout(timer)
    timers.delete(operationId)
  }

  function finalize(operationId: string, phase: Extract<DocumentProcessingPhase, 'completed' | 'failed' | 'cancelled'>) {
    const operation = operations.get(operationId)
    if (!operation || TERMINAL_PHASES.has(operation.phase)) return false
    operation.phase = phase
    clearTimer(operationId)
    changed()
    return true
  }

  function pruneTerminalOperations() {
    if (operations.size < MAX_RETAINED_OPERATIONS) return
    for (const [operationId, operation] of operations) {
      if (operations.size < MAX_RETAINED_OPERATIONS) break
      if (operationId === latestOperationId || !TERMINAL_PHASES.has(operation.phase)) continue
      operations.delete(operationId)
    }
  }

  function start(event: DocumentImportStartedEvent, activationIntent: number) {
    const existing = operations.get(event.operationId)
    if (existing) return existing
    pruneTerminalOperations()
    const operation: DocumentProcessingOperation = {
      ...event,
      phase: 'importing',
      activationIntent,
    }
    operations.set(event.operationId, operation)
    if (event.sequence > latestSequence) {
      latestSequence = event.sequence
      latestOperationId = event.operationId
    }
    timers.set(event.operationId, setTimeout(() => {
      const wasLatest = event.operationId === latestOperationId
      if (!finalize(event.operationId, 'failed')) return
      onTimeout(operation, wasLatest)
    }, deadlineMs))
    changed()
    return operation
  }

  function ensure(event: DocumentImportStartedEvent, activationIntent: number) {
    return operations.get(event.operationId) ?? start(event, activationIntent)
  }

  function beginRender(operationId: string, documentId: string, generation: number) {
    const operation = operations.get(operationId)
    if (!operation || TERMINAL_PHASES.has(operation.phase)) return false
    operation.documentId = documentId
    operation.generation = generation
    operation.phase = 'rendering'
    changed()
    return true
  }

  function awaitPaint(operationId: string, documentId: string, generation: number) {
    const operation = operations.get(operationId)
    if (!matches(operation, documentId, generation)) return false
    operation.phase = 'awaiting-paint'
    changed()
    return true
  }

  function completePaint(operationId: string, documentId: string, generation: number) {
    const operation = operations.get(operationId)
    if (!matches(operation, documentId, generation) || operation.phase !== 'awaiting-paint') return false
    return finalize(operationId, 'completed')
  }

  function finishBackground(operationId: string, documentId: string, generation: number) {
    const operation = operations.get(operationId)
    if (!matches(operation, documentId, generation)) return false
    return finalize(operationId, 'completed')
  }

  function matches(
    operation: DocumentProcessingOperation | undefined,
    documentId: string,
    generation: number,
  ): operation is DocumentProcessingOperation {
    return Boolean(
      operation &&
      !TERMINAL_PHASES.has(operation.phase) &&
      operation.documentId === documentId &&
      operation.generation === generation,
    )
  }

  function fail(operationId: string) {
    return finalize(operationId, 'failed')
  }

  function cancel(operationId: string) {
    return finalize(operationId, 'cancelled')
  }

  function cancelDocument(documentId: string) {
    for (const operation of operations.values()) {
      if (operation.documentId === documentId) cancel(operation.operationId)
    }
  }

  function shouldActivate(operationId: string, activationIntent: number) {
    const operation = operations.get(operationId)
    return Boolean(
      operation &&
      !TERMINAL_PHASES.has(operation.phase) &&
      operation.sequence === latestSequence &&
      operation.activationIntent === activationIntent,
    )
  }

  function get(operationId: string) {
    return operations.get(operationId)
  }

  const visibleOperation = computed(() => {
    void version.value
    const operation = operations.get(latestOperationId)
    return operation && !TERMINAL_PHASES.has(operation.phase) ? operation : null
  })

  function dispose() {
    for (const timer of timers.values()) clearTimeout(timer)
    timers.clear()
    operations.clear()
    changed()
  }

  return {
    visibleOperation,
    start,
    ensure,
    beginRender,
    awaitPaint,
    completePaint,
    finishBackground,
    fail,
    cancel,
    cancelDocument,
    shouldActivate,
    get,
    dispose,
  }
}
