import mermaid from 'mermaid'
import { gateMermaidSource } from './markdown/mermaid'

type RenderMessage = {
  type?: string
  nonce?: string
  requestId?: string
  rendererEpoch?: string
  source?: string
}

mermaid.initialize({
  startOnLoad: false,
  securityLevel: 'strict',
  htmlLabels: false,
  theme: 'default',
  flowchart: { htmlLabels: false, useMaxWidth: true },
})

const capability = new URL(window.location.href).searchParams.get('capability')
if (capability) window.parent.postMessage({ type: 'ready', nonce: capability }, '*')

window.addEventListener('message', async (event: MessageEvent<RenderMessage>) => {
  if (event.data?.type !== 'render') return
  if (event.source !== window.parent) return
  const { nonce, requestId, rendererEpoch, source } = event.data
  if (!nonce || nonce !== capability || !requestId || !rendererEpoch || typeof source !== 'string') return
  const reply = (message: Record<string, unknown>) => window.parent.postMessage({ nonce, requestId, rendererEpoch, ...message }, '*')
  const gate = gateMermaidSource(source)
  if (!gate.allowed) {
    reply({ type: 'failed', code: gate.code })
    return
  }
  try {
    const rendered = await mermaid.render(`mdreader-${requestId.replace(/[^a-zA-Z0-9_-]/g, '')}`, source)
    if (new TextEncoder().encode(rendered.svg).byteLength > 2 * 1024 * 1024) {
      reply({ type: 'failed', code: 'MERMAID_LIMIT' })
      return
    }
    reply({ type: 'rendered', svg: rendered.svg })
  } catch {
    reply({ type: 'failed', code: 'MERMAID_RENDER_FAILED' })
  }
})

export {}
