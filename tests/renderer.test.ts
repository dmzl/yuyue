import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { renderMarkdown } from '../src/markdown/renderer'

describe('RenderDocument markdown pipeline', () => {
  it('returns structured output for common AI Markdown without unsafe resource attributes', async () => {
    const document = await renderMarkdown([
      '# Product brief',
      '## Plan',
      '### Details',
      '#### Notes',
      '',
      '- [x] Ship it',
      '',
      '```typescript',
      'const ready = true',
      '```',
      '',
      '| Name | Status |',
      '| --- | --- |',
      '| Reader | ready |',
      '',
      'A footnote[^1] and inline math $x^2$.',
      '',
      '[^1]: Verified locally.',
      '',
      '![local](assets/diagram.svg "Local diagram")',
      '![remote](https://cdn.example.test/remote.png)',
      '',
      '```mermaid',
      'flowchart LR',
      '  A --> B',
      '```',
      '',
      '<script>alert("no")</script>',
    ].join('\n'))

    expect(document.html).toContain('<table>')
    expect(document.html).toContain('type="checkbox"')
    expect(document.html).toContain('class="katex"')
    expect(document.html).toContain('language-typescript')
    expect(document.html).toContain('id="product-brief"')
    expect(document.html).not.toContain('<script>')
    expect(document.html).not.toContain('src="assets/diagram.svg"')
    expect(document.html).not.toContain('src="https://cdn.example.test/remote.png"')
    expect(document.html).toContain('&#x3C;script>')

    expect(document.outline.map((item) => item.level)).toEqual([1, 2, 3, 4])
    expect(new Set(document.outline.map((item) => item.id)).size).toBe(4)
    expect(document.resources).toEqual([
      expect.objectContaining({ kind: 'local', source: 'assets/diagram.svg' }),
      expect.objectContaining({ kind: 'https', source: 'https://cdn.example.test/remote.png' }),
    ])
    expect(document.diagrams).toEqual([
      expect.objectContaining({ source: 'flowchart LR\n  A --> B' }),
    ])
    expect(document.stats.headingCount).toBe(4)
    expect(document.stats.diagramCount).toBe(1)
  })

  it('keeps external links inert until the desktop shell explicitly opens them', async () => {
    const document = await renderMarkdown(
      '[release](https://example.test/release) [section](#plan) [file](file:///tmp/secret)',
    )

    expect(document.html).not.toContain('href="https://example.test/release"')
    expect(document.html).toContain('data-md-link-id=')
    expect(document.html).toContain('href="#plan"')
    expect(document.html).not.toContain('file:///tmp/secret')
    expect(document.links).toEqual([
      expect.objectContaining({ kind: 'external', url: 'https://example.test/release' }),
      expect.objectContaining({ kind: 'internal', url: '#plan' }),
      expect.objectContaining({ kind: 'blocked' }),
    ])
  })

  it('preserves the typed GitHub Alert class after sanitization', async () => {
    const source = readFileSync(resolve(process.cwd(), 'tests/fixtures/ai-markdown.md'), 'utf8')
    const document = await renderMarkdown(source)

    expect(document.html).toContain('markdown-alert-tip')
    expect(document.html).toContain('data-alert-kind="tip"')
    expect(document.diagrams).toHaveLength(1)
  })

  it('keeps the security corpus literal and side-channel typed', async () => {
    const source = readFileSync(resolve(process.cwd(), 'tests/fixtures/security.md'), 'utf8')
    const document = await renderMarkdown(source)

    expect(document.html).not.toContain('<script>')
    expect(document.html).toContain('&#x3C;script>')
    expect(document.resources.map((resource) => resource.kind)).toEqual(['https', 'blocked'])
    expect(document.links.map((link) => link.kind)).toEqual(['external', 'blocked'])
    expect(document.diagrams[0]?.source).toContain('click A')
  })

  it('rejects input beyond the Markdown size budget with a stable diagnostic', async () => {
    const document = await renderMarkdown('x'.repeat(10 * 1024 * 1024 + 1))

    expect(document.html).toBe('')
    expect(document.diagnostics).toEqual([
      expect.objectContaining({ code: 'MARKDOWN_TOO_LARGE', severity: 'error' }),
    ])
  })

  it('rejects an oversized AST before running expensive transforms', async () => {
    const document = await renderMarkdown('# item\n'.repeat(100_001))

    expect(document.html).toBe('')
    expect(document.diagnostics).toEqual([
      expect.objectContaining({ code: 'MARKDOWN_AST_LIMIT', severity: 'error' }),
    ])
  }, 20_000)

  it('reports the Mermaid document budget without dropping the rest of the document', async () => {
    const source = Array.from({ length: 51 }, (_, index) => [
      `## Diagram ${index}`,
      '',
      '```mermaid',
      'flowchart LR',
      '  A --> B',
      '```',
    ].join('\n')).join('\n\n')
    const document = await renderMarkdown(source)

    expect(document.diagnostics).toEqual([
      expect.objectContaining({ code: 'MERMAID_LIMIT', severity: 'error' }),
    ])
    expect(document.diagrams).toHaveLength(51)
    expect(document.diagrams[50]).toEqual(expect.objectContaining({ error: 'MERMAID_LIMIT' }))
    expect(document.html).toContain('Diagram 0')
  })
})
