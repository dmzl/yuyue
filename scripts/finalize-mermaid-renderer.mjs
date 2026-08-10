import { createHash } from 'node:crypto'
import { readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const outputRoot = resolve('dist')
const htmlPath = resolve(outputRoot, 'mermaid-renderer.html')
const templatePath = resolve('mermaid-renderer.html')
const template = readFileSync(templatePath, 'utf8')
const bundles = readdirSync(resolve(outputRoot, 'assets')).filter((file) => /^mermaid-renderer-[^/]+\.js$/.test(file))
if (bundles.length !== 1) throw new Error('Expected exactly one Mermaid renderer bundle')
const scriptPath = resolve(outputRoot, 'assets', bundles[0])
const scriptSource = readFileSync(scriptPath, 'utf8').replaceAll('</script', '<\\/script')
const digest = createHash('sha256').update(scriptSource).digest('base64')
const integrity = `sha256-${digest}`
const updated = template
  .replace('__MERMAID_RENDERER_SCRIPT__', () => scriptSource)
  .replace("script-src 'self'", `script-src 'self' '${integrity}'`)
writeFileSync(htmlPath, updated)
