// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import mermaid from 'mermaid'
import { gateMermaidSource, sanitizeMermaidSvg } from '../src/markdown/mermaid'

describe('Mermaid safety gate', () => {
  it('allows bounded local graph syntax', () => {
    expect(gateMermaidSource('flowchart LR\n  A --> B')).toEqual({ allowed: true })
  })

  it('rejects network and callback content', () => {
    expect(gateMermaidSource('flowchart LR\n  click A callback "https://example.test"')).toEqual({
      allowed: false,
      code: 'MERMAID_UNSAFE',
    })
  })

  it('rejects oversized diagrams', () => {
    expect(gateMermaidSource(`flowchart LR\n${'A --> B\n'.repeat(301)}`)).toEqual({
      allowed: false,
      code: 'MERMAID_LIMIT',
    })
  })

  it('only returns SVG that has no active or URL-bearing nodes', () => {
    expect(sanitizeMermaidSvg('<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0" /></svg>')).toContain('<svg')
    expect(sanitizeMermaidSvg('<svg><defs><marker id="arrow" /></defs><path marker-end="url(#arrow)" /></svg>')).toContain('marker-end')
    expect(sanitizeMermaidSvg('<svg><defs><filter id="shadow"><feDropShadow dx="2" dy="2" stdDeviation="1" flood-color="#333" markerUnits="userSpaceOnUse" refX="5" refY="4.5" markerWidth="8" markerHeight="8" orient="auto" /></filter></defs></svg>')).toContain('feDropShadow')
    expect(sanitizeMermaidSvg('<svg><path style="fill:url(https://example.test/a.svg)" /></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><script>window.alert(1)</script></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><image href="https://example.test/a.png" /></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><animate attributeName="x" /></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><unknown-node /></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><style>@import url(https://example.test/a.css)</style></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><style>body{display:none}</style></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><style>.node{background-image:u\\72l(https://example.test/a.png)}</style></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><style>.node{background-image:u/**/rl(https://example.test/a.png)}</style></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><style>.node{background-image:-webkit-image-set("https://example.test/a.png" 1x)}</style></svg>')).toBeNull()
    expect(sanitizeMermaidSvg('<svg><style>.node{fill:#fff;stroke-width:1.5}</style></svg>')).toContain('<style>')
  })

  it('accepts Mermaid sequence output attributes used by the E-OP1 corpus', () => {
    const svg = [
      '<svg xmlns="http://www.w3.org/2000/svg" role="graphics-document document">',
      '<symbol id="sequence-symbol"><path d="M0 0" /></symbol>',
      '<rect name="来源业务系统" clip-rule="evenodd" x="0" y="0" width="10" height="10" />',
      '</svg>',
    ].join('')

    expect(sanitizeMermaidSvg(svg)).toContain('<symbol')
  })

  it('keeps valid Mermaid text output while removing nonessential animation CSS', () => {
    const svg = [
      '<svg xmlns="http://www.w3.org/2000/svg" width="100%" style="max-width: 116px;" role="graphics-document document" aria-roledescription="flowchart-v2">',
      '<style>#diagram{font-family:"trebuchet ms",verdana,arial,sans-serif;font-size:16px;fill:#333;}@keyframes dash{to{stroke-dashoffset:0;}}#diagram .node{fill:#ECECFF;stroke:#9370DB;stroke-width:1px;animation:dash 50s linear infinite;}</style>',
      '<g class="node"><text text-anchor="middle"><tspan font-style="normal" font-weight="normal">结果接入与语义标准化</tspan></text></g>',
      '</svg>',
    ].join('')

    const sanitized = sanitizeMermaidSvg(svg)

    expect(sanitized).toContain('结果接入与语义标准化')
    expect(sanitized).toContain('font-family')
    expect(sanitized).not.toContain('@keyframes')
    expect(sanitized).not.toContain('animation:')
  })

  it('accepts the actual E-OP1 Mermaid outputs', async () => {
    Object.defineProperty(SVGElement.prototype, 'getBBox', {
      configurable: true,
      value: () => ({ x: 0, y: 0, width: 100, height: 40 }),
    })
    Object.defineProperty(SVGElement.prototype, 'getComputedTextLength', {
      configurable: true,
      value: () => 100,
    })
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      htmlLabels: false,
      theme: 'default',
      flowchart: { htmlLabels: false, useMaxWidth: true },
    })
    const rendered = await mermaid.render('eop1-flowchart', [
      'flowchart LR',
      'A["外围权威结果"] --> B["结果接入与语义标准化"]',
      'B --> C["每日分析与内部验证"]',
      'C --> D["问题与决策服务"]',
    ].join('\n'))

    expect(sanitizeMermaidSvg(rendered.svg)).not.toBeNull()

    const sequence = await mermaid.render('eop1-sequence', [
      'sequenceDiagram',
      'autonumber',
      'participant S as 来源业务系统',
      'participant R as 结果接入与语义',
      'participant A as 智能分析与决策中枢',
      'participant P as 问题与决策服务',
      'S->>R: 提供订单、规划、OTB、品类和锁定结果',
      'R->>A: 返回标准结果与必要明细能力',
      'alt 可信问题 + 可靠方案',
      'A->>P: 提交问题正式化及方案引用',
      'else 未形成可信问题',
      'A-->>R: 保存分析或数据异常，不创建业务问题',
      'end',
      'Note over P,R: Action 结果不关闭正式问题',
    ].join('\n'))

    expect(sanitizeMermaidSvg(sequence.svg)).not.toBeNull()
  })
})
