import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { resolve } from 'node:path'

const root = resolve('.')
const requiredFiles = [
  'LICENSE',
  'README.md',
  'SECURITY.md',
  'CONTRIBUTING.md',
  'CODE_OF_CONDUCT.md',
  'package-lock.json',
  'src-tauri/Cargo.lock',
  'docs/release/VERIFICATION.md',
  'docs/release/UNSIGNED_MACOS.md',
  'scripts/build-unsigned-dmg.sh',
  '.github/workflows/ci.yml',
  '.github/workflows/release.yml',
]
for (const file of requiredFiles) {
  if (!existsSync(resolve(root, file))) throw new Error(`Missing release file: ${file}`)
}

const packageJson = JSON.parse(readFileSync(resolve(root, 'package.json'), 'utf8'))
for (const script of ['test:ci', 'lint', 'audit', 'release:check']) {
  if (!packageJson.scripts?.[script]) throw new Error(`Missing package script: ${script}`)
}
const readme = readFileSync(resolve(root, 'README.md'), 'utf8')
for (const phrase of ['unsigned', 'SHA-256', 'Gatekeeper']) {
  if (!readme.includes(phrase)) throw new Error(`README missing release guidance: ${phrase}`)
}
const designSource = readFileSync(resolve(root, 'docs/features/open-source-v0.1/TECH_DESIGN.md'), 'utf8')
const canonicalDesign = designSource.replace(/^Design digest：.*$/m, 'Design digest：`canonical-sha256: <digest>`')
const designDigest = createHash('sha256').update(canonicalDesign).digest('hex')
if (!designSource.includes(`canonical-sha256: ${designDigest}`)) throw new Error('TECH_DESIGN canonical digest is stale')
const implementationSpec = readFileSync(resolve(root, 'docs/features/open-source-v0.1/IMPLEMENTATION_SPEC.md'), 'utf8')
if (!implementationSpec.includes(`canonical digest \`${designDigest}\``)) throw new Error('IMPLEMENTATION_SPEC source digest is stale')
const ciWorkflow = readFileSync(resolve(root, '.github/workflows/ci.yml'), 'utf8')
for (const phrase of ['npm ci', 'npm run test:ci', 'npm run audit', 'cargo test --locked']) {
  if (!ciWorkflow.includes(phrase)) throw new Error(`CI workflow missing required check: ${phrase}`)
}
const releaseWorkflow = readFileSync(resolve(root, '.github/workflows/release.yml'), 'utf8')
for (const phrase of [
  "tags:",
  "'v*.*.*'",
  '--no-sign',
  'shasum -a 256',
  'actions/attest-build-provenance@v2',
  'softprops/action-gh-release@v2',
  'build --bundles app --ci --no-sign',
  'build-unsigned-dmg.sh',
]) {
  if (!releaseWorkflow.includes(phrase)) throw new Error(`Release workflow missing required step: ${phrase}`)
}
const dmgScript = readFileSync(resolve(root, 'scripts/build-unsigned-dmg.sh'), 'utf8')
for (const phrase of ['mkdir -p', 'lipo -archs', 'hdiutil create', '-fs HFS+', '-format UDZO', '-srcfolder']) {
  if (!dmgScript.includes(phrase)) throw new Error(`Unsigned DMG script missing required step: ${phrase}`)
}
const rendererHtml = readFileSync(resolve(root, 'dist/mermaid-renderer.html'), 'utf8')
if (!rendererHtml.includes("script-src 'self' 'sha256-") || rendererHtml.includes('__MERMAID_RENDERER_SCRIPT__')) {
  throw new Error('Mermaid renderer inline integrity/CSP finalization is missing')
}
const rendererBundles = readdirSync(resolve(root, 'dist/assets')).filter((file) => /^mermaid-renderer-[^/]+\.js$/.test(file))
if (rendererBundles.length !== 1) throw new Error('Expected exactly one Mermaid renderer bundle')
const rendererBundle = readFileSync(resolve(root, 'dist/assets', rendererBundles[0]), 'utf8')
if (/\bimport\s*\(/.test(rendererBundle)) throw new Error('Mermaid renderer still contains a dynamic import')
console.log('Release structure, docs, renderer integrity, and executed-test scripts are present.')
