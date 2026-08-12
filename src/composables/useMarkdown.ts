import { shallowRef } from 'vue'
import type { RenderDocument, RenderOptions } from '../markdown/renderer'
import { RenderWorkerSupervisor, type RenderResult } from '../markdown/workerClient'

const workerSupervisor = new RenderWorkerSupervisor()

export function useMarkdown() {
  const document = shallowRef<RenderDocument | null>(null)

  async function render(source: string, documentKey: string, priority = 0, options: RenderOptions = {}): Promise<RenderResult> {
    let result: RenderResult
    if (typeof Worker === 'undefined') {
      const { renderMarkdown } = await import('../markdown/renderer')
      const rendered = await renderMarkdown(source, options)
      result = {
        document: rendered,
        serializedBytes: new TextEncoder().encode(JSON.stringify(rendered)).byteLength,
      }
    } else {
      result = await workerSupervisor.render(source, documentKey, priority, options)
    }
    document.value = result.document
    return result
  }

  function prepare() {
    return typeof Worker !== 'undefined' && workerSupervisor.prepare()
  }

  function cancel(documentKey: string) {
    workerSupervisor.cancel(documentKey)
  }

  function dispose() {
    workerSupervisor.dispose()
  }

  return { document, render, prepare, cancel, dispose }
}
