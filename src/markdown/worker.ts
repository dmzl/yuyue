import { renderMarkdown, type RenderDocument } from './renderer'

type RenderRequest = {
  type: 'render'
  requestId: string
  documentKey: string
  source: string
}

type RenderResponse =
  | { type: 'rendered'; requestId: string; documentKey: string; document: RenderDocument; serializedBytes: number }
  | { type: 'failed'; requestId: string; documentKey: string; code: string }

type WorkerScope = {
  onmessage: ((event: MessageEvent<RenderRequest>) => void) | null
  postMessage: (message: RenderResponse) => void
}

const workerScope = globalThis as unknown as WorkerScope
const latestRequestByDocument = new Map<string, string>()

function postStaleResponse(requestId: string, documentKey: string) {
  workerScope.postMessage({
    type: 'failed',
    requestId,
    documentKey,
    code: 'RENDER_STALE',
  })
}

workerScope.onmessage = async ({ data }) => {
  if (data.type !== 'render') return
  latestRequestByDocument.set(data.documentKey, data.requestId)
  try {
    const document = await renderMarkdown(data.source)
    if (latestRequestByDocument.get(data.documentKey) !== data.requestId) {
      postStaleResponse(data.requestId, data.documentKey)
      return
    }
    workerScope.postMessage({
      type: 'rendered',
      requestId: data.requestId,
      documentKey: data.documentKey,
      document,
      serializedBytes: new TextEncoder().encode(JSON.stringify(document)).byteLength,
    })
  } catch {
    if (latestRequestByDocument.get(data.documentKey) !== data.requestId) {
      postStaleResponse(data.requestId, data.documentKey)
      return
    }
    workerScope.postMessage({
      type: 'failed',
      requestId: data.requestId,
      documentKey: data.documentKey,
      code: 'MARKDOWN_RENDER_FAILED',
    })
  }
}

export {}
