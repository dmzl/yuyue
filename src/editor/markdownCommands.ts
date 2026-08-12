import { EditorSelection } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'

export const markdownCommandIds = [
  'heading', 'bold', 'italic', 'strikethrough', 'bullet-list', 'ordered-list',
  'task-list', 'quote', 'link', 'inline-code', 'code-block',
] as const

export type MarkdownCommandId = typeof markdownCommandIds[number]

function wrapSelection(view: EditorView, prefix: string, suffix: string, placeholder: string) {
  const state = view.state
  view.dispatch(state.changeByRange((range) => {
    const selected = state.sliceDoc(range.from, range.to)
    const content = selected || placeholder
    const from = range.from
    const insertion = `${prefix}${content}${suffix}`
    return {
      changes: { from, to: range.to, insert: insertion },
      range: EditorSelection.range(from + prefix.length, from + prefix.length + content.length),
      userEvent: 'input.command',
    }
  }))
}

function prefixLines(view: EditorView, prefix: string) {
  const state = view.state
  view.dispatch(state.changeByRange((range) => {
    const firstLine = state.doc.lineAt(range.from)
    const lastLine = state.doc.lineAt(range.to)
    const source = state.sliceDoc(firstLine.from, lastLine.to)
    const insertion = source.split('\n').map((line) => `${prefix}${line}`).join('\n')
    const startOffset = range.from - firstLine.from
    const endOffset = range.to - firstLine.from
    const lineCountBeforeStart = source.slice(0, startOffset).split('\n').length - 1
    const lineCountBeforeEnd = source.slice(0, endOffset).split('\n').length - 1
    return {
      changes: { from: firstLine.from, to: lastLine.to, insert: insertion },
      range: EditorSelection.range(
        range.anchor + prefix.length * (lineCountBeforeStart + 1),
        range.head + prefix.length * (lineCountBeforeEnd + 1),
      ),
      userEvent: 'input.command',
    }
  }))
}

function insertCodeBlock(view: EditorView, placeholder: string) {
  wrapSelection(view, '```markdown\n', '\n```', placeholder)
}

export function applyMarkdownCommand(view: EditorView, command: MarkdownCommandId, placeholder: string) {
  switch (command) {
    case 'heading':
      prefixLines(view, '# ')
      return
    case 'bold':
      wrapSelection(view, '**', '**', placeholder)
      return
    case 'italic':
      wrapSelection(view, '*', '*', placeholder)
      return
    case 'strikethrough':
      wrapSelection(view, '~~', '~~', placeholder)
      return
    case 'bullet-list':
      prefixLines(view, '- ')
      return
    case 'ordered-list':
      prefixLines(view, '1. ')
      return
    case 'task-list':
      prefixLines(view, '- [ ] ')
      return
    case 'quote':
      prefixLines(view, '> ')
      return
    case 'link':
      wrapSelection(view, '[', '](https://)', placeholder)
      return
    case 'inline-code':
      wrapSelection(view, '`', '`', placeholder)
      return
    case 'code-block':
      insertCodeBlock(view, placeholder)
      return
  }
}
