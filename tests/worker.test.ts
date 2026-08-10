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

  postMessage(message: RenderMessage) {
    this.sent.push(message)
  }

  terminate() {}

  respond(response: unknown) {
    this.onmessage?.({ data: response } as MessageEvent)
  }
}

describe('RenderWorkerSupervisor', () => {
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
    const rendered = { html: '<h1>new</h1>' } as RenderDocument
    worker.respond({
      type: 'rendered',
      requestId: secondRequestId,
      documentKey: 'document-1',
      document: rendered,
    })

    await expect(second).resolves.toBe(rendered)
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
      document: {} as RenderDocument,
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
      document: {} as RenderDocument,
    })
    await expect(first).resolves.toBeDefined()

    const recovered = supervisor.render('# recovered', 'document-2')
    expect(worker.sent).toHaveLength(2)
    const recoveredRequestId = worker.sent[1]?.requestId
    worker.respond({
      type: 'rendered',
      requestId: recoveredRequestId,
      documentKey: 'document-2',
      document: {} as RenderDocument,
    })
    await expect(recovered).resolves.toBeDefined()
    supervisor.dispose()
  })
})
