import type { MessageKey } from '../composables/useLocale'

type Translate = (key: MessageKey, values?: Record<string, string | number>) => string

export function createCodeMirrorPhrases(t: Translate): Record<string, string> {
  return {
    Find: t('editorSearchFind'),
    Replace: t('editorSearchReplace'),
    next: t('editorSearchNext'),
    previous: t('editorSearchPrevious'),
    all: t('editorSearchAll'),
    'match case': t('editorSearchMatchCase'),
    regexp: t('editorSearchRegexp'),
    'by word': t('editorSearchByWord'),
    replace: t('editorSearchReplaceOne'),
    'replace all': t('editorSearchReplaceAll'),
    close: t('editorSearchClose'),
    'current match': t('editorSearchCurrentMatch'),
    'on line': t('editorSearchOnLine'),
    'replaced match on line $': t('editorSearchReplacedMatchOnLine'),
    'replaced $ matches': t('editorSearchReplacedMatches'),
    'Go to line': t('editorGoToLine'),
    go: t('editorGo'),
    'Control character': t('editorControlCharacter'),
    'Selection deleted': t('editorSelectionDeleted'),
  }
}
