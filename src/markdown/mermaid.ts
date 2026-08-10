const MAX_DIAGRAM_BYTES = 16 * 1024
const MAX_DIAGRAM_LINES = 300
const MAX_LINE_LENGTH = 1_000
const MAX_DIAGRAM_TOKENS = 5_000
const MAX_DIAGRAM_EDGES = 200

export type MermaidGateResult =
  | { allowed: true }
  | { allowed: false; code: 'MERMAID_UNSAFE' | 'MERMAID_LIMIT' }

export function gateMermaidSource(source: string): MermaidGateResult {
  const bytes = new TextEncoder().encode(source).byteLength
  const lines = source.split(/\r?\n/)
  const tokens = source.trim() ? source.trim().split(/\s+/) : []
  const edges = source.match(/(?:-->|---|-.->|==>|--)/g)?.length ?? 0
  if (
    bytes > MAX_DIAGRAM_BYTES ||
    lines.length > MAX_DIAGRAM_LINES ||
    lines.some((line) => line.length > MAX_LINE_LENGTH) ||
    tokens.length > MAX_DIAGRAM_TOKENS ||
    edges > MAX_DIAGRAM_EDGES
  ) {
    return { allowed: false, code: 'MERMAID_LIMIT' }
  }
  const lower = source.toLocaleLowerCase()
  if (
    lower.includes('javascript:') ||
    lower.includes('https://') ||
    lower.includes('http://') ||
    lower.includes('file://') ||
    lower.includes('click ') && lower.includes('callback')
  ) {
    return { allowed: false, code: 'MERMAID_UNSAFE' }
  }
  return { allowed: true }
}

const SAFE_SVG_ELEMENTS = new Set([
  'svg', 'g', 'defs', 'marker', 'path', 'rect', 'circle', 'ellipse', 'line', 'polyline', 'polygon',
  'text', 'tspan', 'style', 'title', 'desc', 'clippath', 'lineargradient', 'radialgradient',
  'stop', 'mask', 'filter', 'fegaussianblur', 'fecolormatrix', 'feoffset', 'feblend', 'feflood',
  'fecomposite', 'fedropshadow', 'pattern', 'symbol',
])

const SAFE_SVG_ATTRIBUTES = new Set([
  'id', 'class', 'xmlns', 'version', 'width', 'height', 'viewbox', 'preserveaspectratio', 'x', 'y',
  'x1', 'y1', 'x2', 'y2', 'cx', 'cy', 'r', 'rx', 'ry', 'd', 'points', 'transform', 'fill', 'fill-opacity',
  'fill-rule', 'clip-rule', 'stroke', 'stroke-opacity', 'stroke-width', 'stroke-linecap', 'stroke-linejoin',
  'stroke-miterlimit', 'stroke-dasharray', 'stroke-dashoffset', 'opacity', 'display', 'visibility',
  'overflow', 'clip', 'clip-path', 'mask', 'filter', 'marker-start', 'marker-mid', 'marker-end',
  'markerheight', 'markerwidth', 'markerunits', 'refx', 'refy', 'orient',
  'text-anchor', 'dominant-baseline', 'alignment-baseline', 'dx', 'dy', 'font-family', 'font-size',
  'font-style', 'font-weight', 'letter-spacing', 'text-decoration', 'shape-rendering', 'vector-effect',
  'style', 'offset', 'stop-color', 'stop-opacity', 'gradientunits', 'gradienttransform', 'patternunits',
  'patterncontentunits', 'result', 'in', 'in2', 'mode', 'stddeviation', 'values', 'color', 'flood-color', 'flood-opacity', 'pointer-events',
  'role', 'focusable', 'name', 'aria-label', 'aria-hidden', 'aria-roledescription',
])

const SAFE_CSS_PROPERTIES = new Set([
  'alignment-baseline', 'background', 'background-color', 'baseline-shift', 'clip', 'clip-path',
  'color', 'dominant-baseline', 'display', 'fill', 'fill-opacity', 'fill-rule', 'filter', 'flood-color',
  'flood-opacity', 'font-family', 'font-size', 'font-style', 'font-weight', 'letter-spacing', 'marker-end',
  'marker-mid', 'marker-start', 'mask', 'opacity', 'overflow', 'pointer-events', 'shape-rendering',
  'stop-color', 'stop-opacity', 'stroke', 'stroke-dasharray', 'stroke-dashoffset', 'stroke-linecap',
  'stroke-linejoin', 'stroke-miterlimit', 'stroke-opacity', 'stroke-width', 'text-anchor', 'text-decoration',
  'text-transform', 'transform', 'vector-effect', 'visibility', 'white-space', 'max-width', 'min-width',
  'max-height', 'min-height', 'width', 'height',
])

const SAFE_CSS_VALUE = /^[a-z0-9#%().,/'"_+\-*!\s]+$/i
const LOCAL_CSS_URL = /url\s*\(\s*#[a-z0-9_.:-]+\s*\)/gi

function isDangerousCssValue(value: string) {
  const lower = value.toLocaleLowerCase()
  return (
    value.includes('\\') ||
    value.includes('/*') ||
    value.includes('*/') ||
    lower.includes('@') ||
    lower.includes('expression') ||
    lower.includes('behavior') ||
    lower.includes('-moz-binding') ||
    lower.includes('javascript:') ||
    lower.includes('vbscript:') ||
    lower.includes('data:') ||
    /(?:image-set|url)\s*\(/i.test(value.replace(LOCAL_CSS_URL, ''))
  )
}

function sanitizeCssDeclarations(source: string): string | null {
  const declarations = source.split(';').map((item) => item.trim()).filter(Boolean)
  if (declarations.length === 0) return /^[;\s]*$/.test(source) ? '' : null
  const safeDeclarations: string[] = []
  for (const declaration of declarations) {
    const separator = declaration.indexOf(':')
    if (separator <= 0) return null
    const property = declaration.slice(0, separator).trim().toLocaleLowerCase()
    const value = declaration.slice(separator + 1).trim()
    if (isDangerousCssValue(value)) return null
    if (!SAFE_CSS_VALUE.test(value)) return null
    if (SAFE_CSS_PROPERTIES.has(property)) safeDeclarations.push(`${property}:${value}`)
  }
  return safeDeclarations.join(';')
}

function isSafeCssSelector(selector: string) {
  const value = selector.trim()
  if (!value || /[\\@{}]/.test(value) || /(?:^|[\s>+,(])(?:html|body|head)(?:$|[\s>+.#:[(])/i.test(value)) return false
  return /^[\w#.$:\-\s>+*,()[\]="']+$/i.test(value)
}

function sanitizeCssBlock(source: string): string | null {
  if (source.includes('/*') || source.includes('*/')) return null
  if (source.includes('\\')) return null
  if (source.trim() === '') return ''
  const safeRules: string[] = []
  let cursor = 0
  while (cursor < source.length) {
    while (/\s/.test(source[cursor] ?? '')) cursor += 1
    if (cursor >= source.length) break
    const selectorStart = cursor
    const blockStart = source.indexOf('{', cursor)
    if (blockStart < 0) return null
    const selector = source.slice(selectorStart, blockStart).trim()
    let depth = 1
    cursor = blockStart + 1
    const declarationsStart = cursor
    while (cursor < source.length && depth > 0) {
      if (source[cursor] === '{') depth += 1
      if (source[cursor] === '}') depth -= 1
      cursor += 1
    }
    if (depth !== 0) return null
    const declarations = source.slice(declarationsStart, cursor - 1)
    if (selector.startsWith('@')) continue
    if (!isSafeCssSelector(selector)) return null
    const safeDeclarations = sanitizeCssDeclarations(declarations)
    if (safeDeclarations === null) return null
    if (safeDeclarations) safeRules.push(`${selector}{${safeDeclarations}}`)
  }
  return safeRules.join('')
}

function isSafeSvgAttribute(name: string, value: string) {
  const lowerName = name.toLocaleLowerCase()
  const lowerValue = value.toLocaleLowerCase()
  if (lowerName.startsWith('on') || lowerName === 'href' || lowerName === 'xlink:href' || lowerName.startsWith('xmlns:')) return false
  if (lowerName === 'xml:space') return value === 'preserve'
  if (!SAFE_SVG_ATTRIBUTES.has(lowerName) && !lowerName.startsWith('data-') && !lowerName.startsWith('aria-')) return false
  if (lowerName === 'xmlns' && value !== 'http://www.w3.org/2000/svg') return false
  if (lowerName === 'style') return sanitizeCssDeclarations(value) !== null
  if (['clip-path', 'fill', 'filter', 'marker-end', 'marker-mid', 'marker-start', 'mask', 'stroke'].includes(lowerName)) {
    return !isDangerousCssValue(value) && SAFE_CSS_VALUE.test(value)
  }
  return !lowerValue.includes('javascript:') && !lowerValue.includes('vbscript:') && !lowerValue.includes('data:text/html')
}

export function sanitizeMermaidSvg(svg: string): string | null {
  if (new TextEncoder().encode(svg).byteLength > 2 * 1024 * 1024) return null
  if (typeof DOMParser === 'undefined' || typeof XMLSerializer === 'undefined') return null
  const parsed = new DOMParser().parseFromString(svg, 'image/svg+xml')
  if (parsed.querySelector('parsererror')) return null
  const root = parsed.documentElement
  if (root.tagName.toLocaleLowerCase() !== 'svg') return null
  const elements = [root, ...Array.from(root.querySelectorAll('*'))]
  for (const element of elements) {
    if (!SAFE_SVG_ELEMENTS.has(element.tagName.toLocaleLowerCase())) return null
    for (const attribute of Array.from(element.attributes)) {
      if (!isSafeSvgAttribute(attribute.name, attribute.value)) return null
      if (attribute.name.toLocaleLowerCase() === 'style') {
        const style = sanitizeCssDeclarations(attribute.value)
        if (style === null) return null
        if (style) element.setAttribute('style', style)
        else element.removeAttribute('style')
      }
    }
    if (element.tagName.toLocaleLowerCase() === 'style') {
      const style = sanitizeCssBlock(element.textContent ?? '')
      if (style === null) return null
      if (style) element.textContent = style
      else element.remove()
    }
  }
  return new XMLSerializer().serializeToString(root)
}
