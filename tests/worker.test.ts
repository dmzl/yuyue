import { describe, expect, it } from 'vitest'
import type { RenderDocument } from '../src/markdown/renderer'
import { RenderWorkerSupervisor } from '../src/markdown/workerClient'

type RenderMessage = {
  type: 'render'
  requestId: string
  documentKey: string
  source: string
}

class FakeWorker {
  onmessage: ((event: MessageEvent) => void) | null = null
  onerror: (() => void) | null = null
  sent: RenderMessage[] = []
  terminated = 0

  postMessage(message: RenderMessage) {
    this.sent.push(message)
  }

  terminate() {
    this.terminated += 1
  }

  respond(response: unknown) {
    this.onmessage?.({ data: response } as MessageEvent)
  }
}

function renderedDocument(overrides: Partial<RenderDocument> = {}): RenderDocument {
  return {
    html: '<h1>rendered</h1>',
    outline: [],
    resources: [],
    links: [],
    diagrams: [],
    diagnostics: [],
    stats: { inputBytes: 1, astNodes: 1, headingCount: 0, diagramCount: 0 },
    sourceBlocks: [],
    renderLeaseId: 'lease-test-1',
    contextEpoch: 0,
    renderGeneration: 0,
    ...overrides,
  }
}

describe('RenderWorkerSupervisor', () => {
  it('prepares one reusable worker without creating a render job', () => {
    const worker = new FakeWorker()
    let creations = 0
    const supervisor = new RenderWorkerSupervisor(() => {
      creations += 1
      return worker as unknown as Worker
    })

    expect(supervisor.prepare()).toBe(true)
    expect(supervisor.prepare()).toBe(true)
    expect(creations).toBe(1)
    expect(worker.sent).toHaveLength(0)
    supervisor.dispose()
  })

  it('allows a real render to retry after a failed warmup', async () => {
    const worker = new FakeWorker()
    let creations = 0
    const supervisor = new RenderWorkerSupervisor(() => {
      creations += 1
      if (creations === 1) throw new Error('warmup failed')
      return worker as unknown as Worker
    })

    expect(supervisor.prepare()).toBe(false)
    const pending = supervisor.render('# retry', 'document-retry')
    worker.respond({
      type: 'rendered',
      requestId: worker.sent[0]?.requestId,
      documentKey: 'document-retry',
      document: renderedDocument({ html: '<h1>retry</h1>' }),
      serializedBytes: 64,
    })

    await expect(pending).resolves.toMatchObject({ serializedBytes: 64 })
    expect(creations).toBe(2)
    supervisor.dispose()
  })

  it('returns the worker-computed serialized result size atomically with the document', async () => {
    const worker = new FakeWorker()
    const supervisor = new RenderWorkerSupervisor(() => worker as unknown as Worker)
    const pending = supervisor.render('# sized', 'document-sized')
    const document = renderedDocument({ html: '<h1>sized</h1>' })

    worker.respond({
      type: 'rendered',
      requestId: worker.sent[0]?.requestId,
      documentKey: 'document-sized',
      document,
      serializedBytes: 321,
    })

    await expect(pending).resolves.toEqual({ document, serializedBytes: 321 })
    supervisor.dispose()
  })

  it('rejects rendered responses with an invalid serialized size', async () => {
    const worker = new FakeWorker()
    const supervisor = new RenderWorkerSupervisor(() => worker as unknown as Worker)
    const pending = supervisor.render('# invalid size', 'document-invalid-size')

    worker.respond({
      type: 'rendered',
      requestId: worker.sent[0]?.requestId,
      documentKey: 'document-invalid-size',
      document: renderedDocument({ html: '<h1>invalid</h1>' }),
      serializedBytes: Number.NaN,
    })

    await expect(pending).rejects.toMatchObject({ code: 'RENDER_PROTOCOL_ERROR' })
    supervisor.dispose()
  })

  it('accepts source block bounds across mixed Markdown newline styles', async () => {
    const worker = new FakeWorker()
    const supervisor = new RenderWorkerSupervisor(() => worker as unknown as Worker)
    const pending = supervisor.render('# one\r\ntwo\rthree\nfour', 'document-mixed-newlines')
    const document = renderedDocument({
      html: '<p data-md-source-block-id="sb-deadbeef-1">four</p>',
      sourceBlocks: [{ id: 'sb-deadbeef-1', kind: 'block', startLine: 1, endLine: 4 }],
    })

    worker.respond({
      type: 'rendered',
      requestId: worker.sent[0]?.requestId,
      documentKey: 'document-mixed-newlines',
      document,
      serializedBytes: 128,
    })

    await expect(pending).resolves.toEqual({ document, serializedBytes: 128 })
    supervisor.dispose()
  })

  it('rejects duplicate source block ids in rendered HTML', async () => {
    const worker = new FakeWorker()
    const supervisor = new RenderWorkerSupervisor(() => worker as unknown as Worker)
    const pending = supervisor.render('# duplicate', 'document-duplicate-dom-id')

    worker.respond({
      type: 'rendered',
      requestId: worker.sent[0]?.requestId,
      documentKey: 'document-duplicate-dom-id',
      document: renderedDocument({
        html: '<p data-md-source-block-id="sb-deadbeef-1">a</p><p data-md-source-block-id="sb-deadbeef-1">b</p>',
        sourceBlocks: [{ id: 'sb-deadbeef-1', kind: 'block', startLine: 1, endLine: 1 }],
      }),
      serializedBytes: 128,
    })

    await expect(pending).rejects.toMatchObject({ code: 'RENDER_PROTOCOL_ERROR' })
    supervisor.dispose()
  })

  it('releases the queue when a superseded in-flight response arrives', async () => {
    const worker = new FakeWorker()
    const supervisor = new RenderWorkerSupervisor(() => worker as unknown as Worker)

    const first = supervisor.render('# old', 'document-1')
    const firstRequestId = worker.sent[0]?.requestId
    expect(firstRequestId).toBeDefined()

    const second = supervisor.render('# new', 'document-1')
    await expect(first).rejects.toMatchObject({ code: 'RENDER_STALE' })
    expect(worker.sent).toHaveLength(1)

    worker.respond({
      type: 'failed',
      requestId: firstRequestId,
      documentKey: 'document-1',
      code: 'RENDER_STALE',
    })

    expect(worker.sent).toHaveLength(2)
    const secondRequestId = worker.sent[1]?.requestId
    expect(secondRequestId).toBeDefined()
    const rendered = renderedDocument({ html: '<h1>new</h1>' })
    worker.respond({
      type: 'rendered',
      requestId: secondRequestId,
      documentKey: 'document-1',
      document: rendered,
      serializedBytes: 128,
    })

    await expect(second).resolves.toEqual({ document: rendered, serializedBytes: 128 })
    supervisor.dispose()
  })

  it('rejects a worker response that is addressed to another document', async () => {
    const worker = new FakeWorker()
    const supervisor = new RenderWorkerSupervisor(() => worker as unknown as Worker)

    const render = supervisor.render('# current', 'document-1')
    const requestId = worker.sent[0]?.requestId
    expect(requestId).toBeDefined()

    worker.respond({
      type: 'rendered',
      requestId,
      documentKey: 'document-2',
      document: renderedDocument(),
      serializedBytes: 1,
    })

    await expect(render).rejects.toMatchObject({ code: 'RENDER_PROTOCOL_ERROR' })
    supervisor.dispose()
  })

  it('bounds queued source bytes and releases the budget after completion', async () => {
    const worker = new FakeWorker()
    const supervisor = new RenderWorkerSupervisor(() => worker as unknown as Worker, 64)
    const oversizedSource = 'x'.repeat(64)

    const first = supervisor.render(oversizedSource, 'document-1')
    expect(worker.sent).toHaveLength(1)
    const rejected = supervisor.render('# queued', 'document-2')
    await expect(rejected).rejects.toMatchObject({ code: 'RENDER_QUEUE_LIMIT' })

    const firstRequestId = worker.sent[0]?.requestId
    worker.respond({
      type: 'rendered',
      requestId: firstRequestId,
      documentKey: 'document-1',
      document: renderedDocument(),
      serializedBytes: 1,
    })
    await expect(first).resolves.toBeDefined()

    const recovered = supervisor.render('# recovered', 'document-2')
    expect(worker.sent).toHaveLength(2)
    const recoveredRequestId = worker.sent[1]?.requestId
    worker.respond({
      type: 'rendered',
      requestId: recoveredRequestId,
      documentKey: 'document-2',
      document: renderedDocument(),
      serializedBytes: 1,
    })
    await expect(recovered).resolves.toBeDefined()
    supervisor.dispose()
  })

  it('rejects malformed rendered documents before they enter the tab cache', async () => {
    const malformedDocuments: unknown[] = [
      undefined,
      { html: '<h1>missing arrays</h1>' },
      { ...renderedDocument(), outline: {} },
      {
        ...renderedDocument(),
        resources: [{ id: 'resource-1', kind: 'local', source: 42, alt: '' }],
      },
    ]

    for (const document of malformedDocuments) {
      const worker = new FakeWorker()
      const supervisor = new RenderWorkerSupervisor(() => worker as unknown as Worker)
      const pending = supervisor.render('# malformed', 'document-malformed')

      worker.respond({
        type: 'rendered',
        requestId: worker.sent[0]?.requestId,
        documentKey: 'document-malformed',
        document,
        serializedBytes: 64,
      })

      await expect(pending).rejects.toMatchObject({ code: 'RENDER_PROTOCOL_ERROR' })
      expect(worker.terminated).toBe(1)
      supervisor.dispose()
    }
  })
})
