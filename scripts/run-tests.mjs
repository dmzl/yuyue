import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'

const directory = mkdtempSync(join(tmpdir(), 'mdreader-vitest-'))
const outputFile = join(directory, 'results.json')
const command = process.platform === 'win32' ? 'npx.cmd' : 'npx'
const result = spawnSync(command, ['vitest', 'run', '--reporter=json', '--outputFile', outputFile], { stdio: 'inherit' })
if (result.status !== 0) process.exit(result.status ?? 1)

try {
  const report = JSON.parse(readFileSync(outputFile, 'utf8'))
  if (!Number.isInteger(report.numTotalTests) || report.numTotalTests < 1) {
    throw new Error('Vitest reported zero executed tests')
  }
  console.log(`Executed ${report.numTotalTests} frontend tests in ${report.numTotalTestSuites} suites.`)
} finally {
  rmSync(directory, { recursive: true, force: true })
}
