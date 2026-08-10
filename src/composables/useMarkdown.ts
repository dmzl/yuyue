import { shallowRef } from 'vue'
import { renderMarkdown, type RenderDocument } from '../markdown/renderer'
import { RenderWorkerSupervisor } from '../markdown/workerClient'

const workerSupervisor = new RenderWorkerSupervisor()

export function useMarkdown() {
  const document = shallowRef<RenderDocument | null>(null)

  async function render(source: string, documentKey: string, priority = 0): Promise<RenderDocument> {
    let rendered: RenderDocument
    if (typeof Worker === 'undefined') {
      rendered = await renderMarkdown(source)
    } else {
      rendered = await workerSupervisor.render(source, documentKey, priority)
    }
    document.value = rendered
    return rendered
  }

  function cancel(documentKey: string) {
    workerSupervisor.cancel(documentKey)
  }

  return { document, render, cancel }
}
