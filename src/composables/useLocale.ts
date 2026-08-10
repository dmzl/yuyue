import { shallowRef } from 'vue'

export const LOCALES = ['zh-Hans', 'en', 'zh-Hant', 'ja'] as const
export type Locale = typeof LOCALES[number]

const LOCALE_KEY = 'mdreader-locale'

const messages = {
  'zh-Hans': {
    appName: '屿阅',
    open: '打开', openMarkdownFile: '打开 Markdown 文件', readingSettings: '阅读设置', closeNotification: '关闭提示',
    importingDocument: '正在导入文档…', renderingDocument: '正在渲染文档…', welcomeTitle: '把 Markdown 读成文档', welcomeDescription: '打开本地文件，专注于内容。远程资源默认保持阻断。', welcomeHint: '支持 .md / .markdown / .txt · 也可以将文件拖入窗口', dropToOpen: '释放以打开 Markdown 文件',
    documentOpenFailed: '无法打开文档', unsupportedFormat: '不支持的文档格式', documentTooLarge: '文档超过 10 MiB 限制', documentReadFailed: '文档读取失败，已保留上一次内容', documentIdentityChanged: '文件身份已变化，请重新打开文档', documentNotUtf8: '文档不是有效的 UTF-8 文本', documentTabLimit: '打开的文档已达到 32 个上限', documentWatchFailed: '文件变化监听未启动', markdownAstLimit: '文档结构超过安全限制', markdownRenderLimit: '文档渲染结果超过安全限制', markdownRenderFailed: '文档渲染失败', renderTimeout: '文档渲染超时，其他标签仍可继续使用', renderCrash: '文档渲染进程已恢复，请重试', renderQueueLimit: '同时处理的文档超过安全容量，请稍后重试', renderProtocolError: '文档渲染通信异常，请重试', renderCancelled: '文档渲染已取消', documentProcessingFailed: '文档处理失败', openDialogFailed: '打开文件对话框失败', printDocumentMissing: '请先打开 Markdown 文档，再导出 PDF', printDialogFailed: '无法打开系统打印面板', appInitializeFailed: '应用初始化失败',
    themeLight: '浅色', themeDark: '深色', themeSystem: '跟随系统', themeFollowingSystem: '跟随系统（当前为{theme}）', currentThemeAction: '当前：{current}；点击切换到{next}', themeChanged: '已切换到{theme}模式',
    language: '语言', readingAppearance: '阅读外观', fontSize: '文字大小', decreaseFont: '缩小文字', increaseFont: '放大文字', resetFont: '恢复默认文字大小', switchComfortableWidth: '切换为舒适宽度', switchFullWidth: '切换为完整宽度', remoteImages: '远程图片', remoteImagesAllowed: '当前标签已允许远程图片', loadRemoteImages: '加载此文档远程图片', loadRemoteImagesHint: '仅为当前标签临时加载 HTTPS 图片', closeReadingSettings: '关闭阅读设置',
    searchDocument: '搜索文档', searchResults: '搜索结果', noResults: '无结果', previousMatch: '上一个匹配', nextMatch: '下一个匹配', closeSearch: '关闭搜索',
    openDocuments: '打开的文档', switchTo: '切换到 {name}', documentHasError: '此文档有错误', needsAttention: '需检查', closeDocument: '关闭文档', documentTabActions: '文档标签操作', moreDocuments: '更多文档（{count} 个）', hiddenDocuments: '其他打开的文档', close: '关闭', closeOthers: '关闭其他', closeLeft: '关闭左侧', closeRight: '关闭右侧', closeAll: '关闭全部',
    outline: '目录', headingNavigation: '标题导航', retryDocument: '重试此文档', retry: '重试', outlineLocationMissing: '未找到该目录位置', code: '代码', codeLanguage: '语言：{language}', copyCode: '复制代码', copied: '已复制', codeCopied: '代码已复制', copyCodeFailed: '复制代码失败', imageUnableToLoad: '图片无法加载', imageSourceBlocked: '图片来源已阻断', remoteImageBlocked: '远程图片已阻断', remoteImageBlockedWithAlt: '远程图片已阻断：{alt}', loadForCurrentTab: '仅为当前标签加载', openImagePreview: '打开图片预览', openImagePreviewWithAlt: '打开图片预览：{alt}',
    mermaidDiagram: 'Mermaid 图表', mermaidLimit: 'Mermaid 图表超过安全限制', mermaidBlocked: 'Mermaid 图表内容已阻断', mermaidRenderFailed: 'Mermaid 图表渲染失败', mermaidUnsafeOutput: 'Mermaid 输出未通过安全检查', openMermaidDiagram: '打开 Mermaid 图表：{title}', mermaidTimeout: 'Mermaid 图表渲染超时', mermaidDeferred: '图表较多，已暂停后续 Mermaid 渲染。', continueRendering: '继续渲染', continueMermaidRendering: '继续渲染剩余 Mermaid 图表', externalLinkFailed: '外部链接无法打开', linkTypeBlocked: '该链接类型已阻断',
    imagePreview: '大图预览', imagePreviewTools: '大图预览工具栏', zoomOut: '缩小预览', zoomIn: '放大预览', zoomStatus: '当前缩放 {percent}%，点击恢复适应窗口', resetZoom: '恢复适应窗口', exportPng: '导出 PNG', pngExported: 'PNG 已导出', pngExportLimit: 'PNG 导出超过安全限制', pngExportFailed: 'PNG 导出失败', closeImagePreview: '关闭大图',
  },
  en: {
    appName: '屿阅',
    open: 'Open', openMarkdownFile: 'Open Markdown file', readingSettings: 'Reading settings', closeNotification: 'Dismiss notification',
    importingDocument: 'Importing document…', renderingDocument: 'Rendering document…', welcomeTitle: 'Read Markdown as a document', welcomeDescription: 'Open a local file and focus on its content. Remote resources stay blocked by default.', welcomeHint: 'Supports .md / .markdown / .txt · You can also drop a file into this window', dropToOpen: 'Drop to open a Markdown file',
    documentOpenFailed: 'Unable to open document', unsupportedFormat: 'Unsupported document format', documentTooLarge: 'Document exceeds the 10 MiB limit', documentReadFailed: 'Unable to read document; previous content is retained', documentIdentityChanged: 'File identity changed. Please open the document again.', documentNotUtf8: 'Document is not valid UTF-8 text', documentTabLimit: 'The 32-document limit has been reached', documentWatchFailed: 'File-change monitoring did not start', markdownAstLimit: 'Document structure exceeds the safety limit', markdownRenderLimit: 'Rendered document exceeds the safety limit', markdownRenderFailed: 'Unable to render document', renderTimeout: 'Document rendering timed out; other tabs remain available', renderCrash: 'Document renderer recovered. Please try again.', renderQueueLimit: 'Too many documents are being processed. Please try again later.', renderProtocolError: 'Document rendering communication failed. Please try again.', renderCancelled: 'Document rendering was cancelled', documentProcessingFailed: 'Document processing failed', openDialogFailed: 'Unable to open the file picker', printDocumentMissing: 'Open a Markdown document before exporting a PDF', printDialogFailed: 'Unable to open the system print panel', appInitializeFailed: 'App initialization failed',
    themeLight: 'Light', themeDark: 'Dark', themeSystem: 'System', themeFollowingSystem: 'System ({theme} now)', currentThemeAction: 'Current: {current}; click to switch to {next}', themeChanged: 'Switched to {theme} mode',
    language: 'Language', readingAppearance: 'Reading appearance', fontSize: 'Text size', decreaseFont: 'Decrease text size', increaseFont: 'Increase text size', resetFont: 'Reset text size', switchComfortableWidth: 'Use comfortable width', switchFullWidth: 'Use full width', remoteImages: 'Remote images', remoteImagesAllowed: 'Remote images are allowed for this tab', loadRemoteImages: 'Load remote images for this document', loadRemoteImagesHint: 'Temporarily load HTTPS images for this tab only', closeReadingSettings: 'Close reading settings',
    searchDocument: 'Search document', searchResults: 'Search results', noResults: 'No results', previousMatch: 'Previous match', nextMatch: 'Next match', closeSearch: 'Close search',
    openDocuments: 'Open documents', switchTo: 'Switch to {name}', documentHasError: 'This document has an error', needsAttention: 'Needs attention', closeDocument: 'Close document', documentTabActions: 'Document tab actions', moreDocuments: 'More documents ({count})', hiddenDocuments: 'Other open documents', close: 'Close', closeOthers: 'Close others', closeLeft: 'Close tabs to the left', closeRight: 'Close tabs to the right', closeAll: 'Close all',
    outline: 'Outline', headingNavigation: 'Heading navigation', retryDocument: 'Retry this document', retry: 'Retry', outlineLocationMissing: 'Outline location was not found', code: 'Code', codeLanguage: 'Language: {language}', copyCode: 'Copy code', copied: 'Copied', codeCopied: 'Code copied', copyCodeFailed: 'Unable to copy code', imageUnableToLoad: 'Unable to load image', imageSourceBlocked: 'Image source is blocked', remoteImageBlocked: 'Remote image is blocked', remoteImageBlockedWithAlt: 'Remote image is blocked: {alt}', loadForCurrentTab: 'Load for this tab only', openImagePreview: 'Open image preview', openImagePreviewWithAlt: 'Open image preview: {alt}',
    mermaidDiagram: 'Mermaid diagram', mermaidLimit: 'Mermaid diagram exceeds the safety limit', mermaidBlocked: 'Mermaid diagram content is blocked', mermaidRenderFailed: 'Unable to render Mermaid diagram', mermaidUnsafeOutput: 'Mermaid output did not pass the safety check', openMermaidDiagram: 'Open Mermaid diagram: {title}', mermaidTimeout: 'Mermaid diagram rendering timed out', mermaidDeferred: 'There are many diagrams. Further Mermaid rendering is paused.', continueRendering: 'Continue rendering', continueMermaidRendering: 'Continue rendering remaining Mermaid diagrams', externalLinkFailed: 'Unable to open external link', linkTypeBlocked: 'This link type is blocked',
    imagePreview: 'Large image preview', imagePreviewTools: 'Large image preview tools', zoomOut: 'Zoom out', zoomIn: 'Zoom in', zoomStatus: 'Current zoom {percent}%; click to fit window', resetZoom: 'Fit window', exportPng: 'Export PNG', pngExported: 'PNG exported', pngExportLimit: 'PNG export exceeds the safety limit', pngExportFailed: 'PNG export failed', closeImagePreview: 'Close image preview',
  },
  'zh-Hant': {
    appName: '屿阅',
    open: '開啟', openMarkdownFile: '開啟 Markdown 檔案', readingSettings: '閱讀設定', closeNotification: '關閉提示',
    importingDocument: '正在匯入文件…', renderingDocument: '正在轉譯文件…', welcomeTitle: '把 Markdown 讀成文件', welcomeDescription: '開啟本機檔案，專注於內容。遠端資源預設保持封鎖。', welcomeHint: '支援 .md / .markdown / .txt · 也可以將檔案拖入視窗', dropToOpen: '放開以開啟 Markdown 檔案',
    documentOpenFailed: '無法開啟文件', unsupportedFormat: '不支援的文件格式', documentTooLarge: '文件超過 10 MiB 限制', documentReadFailed: '文件讀取失敗，已保留上一次內容', documentIdentityChanged: '檔案身分已變更，請重新開啟文件', documentNotUtf8: '文件不是有效的 UTF-8 文字', documentTabLimit: '開啟的文件已達 32 個上限', documentWatchFailed: '檔案變更監聽未啟動', markdownAstLimit: '文件結構超過安全限制', markdownRenderLimit: '文件轉譯結果超過安全限制', markdownRenderFailed: '文件轉譯失敗', renderTimeout: '文件轉譯逾時，其他分頁仍可繼續使用', renderCrash: '文件轉譯程序已恢復，請重試', renderQueueLimit: '同時處理的文件超過安全容量，請稍後重試', renderProtocolError: '文件轉譯通訊異常，請重試', renderCancelled: '文件轉譯已取消', documentProcessingFailed: '文件處理失敗', openDialogFailed: '開啟檔案對話框失敗', printDocumentMissing: '請先開啟 Markdown 文件，再匯出 PDF', printDialogFailed: '無法開啟系統列印面板', appInitializeFailed: '應用程式初始化失敗',
    themeLight: '淺色', themeDark: '深色', themeSystem: '跟隨系統', themeFollowingSystem: '跟隨系統（目前為{theme}）', currentThemeAction: '目前：{current}；點擊切換至{next}', themeChanged: '已切換至{theme}模式',
    language: '語言', readingAppearance: '閱讀外觀', fontSize: '文字大小', decreaseFont: '縮小文字', increaseFont: '放大文字', resetFont: '回復預設文字大小', switchComfortableWidth: '切換為舒適寬度', switchFullWidth: '切換為完整寬度', remoteImages: '遠端圖片', remoteImagesAllowed: '目前分頁已允許遠端圖片', loadRemoteImages: '載入此文件遠端圖片', loadRemoteImagesHint: '僅為目前分頁暫時載入 HTTPS 圖片', closeReadingSettings: '關閉閱讀設定',
    searchDocument: '搜尋文件', searchResults: '搜尋結果', noResults: '沒有結果', previousMatch: '上一個符合項目', nextMatch: '下一個符合項目', closeSearch: '關閉搜尋',
    openDocuments: '已開啟的文件', switchTo: '切換至 {name}', documentHasError: '此文件有錯誤', needsAttention: '需檢查', closeDocument: '關閉文件', documentTabActions: '文件分頁動作', moreDocuments: '更多文件（{count} 個）', hiddenDocuments: '其他已開啟的文件', close: '關閉', closeOthers: '關閉其他', closeLeft: '關閉左側', closeRight: '關閉右側', closeAll: '關閉全部',
    outline: '目錄', headingNavigation: '標題導覽', retryDocument: '重試此文件', retry: '重試', outlineLocationMissing: '找不到該目錄位置', code: '程式碼', codeLanguage: '語言：{language}', copyCode: '複製程式碼', copied: '已複製', codeCopied: '程式碼已複製', copyCodeFailed: '複製程式碼失敗', imageUnableToLoad: '圖片無法載入', imageSourceBlocked: '圖片來源已封鎖', remoteImageBlocked: '遠端圖片已封鎖', remoteImageBlockedWithAlt: '遠端圖片已封鎖：{alt}', loadForCurrentTab: '僅為目前分頁載入', openImagePreview: '開啟圖片預覽', openImagePreviewWithAlt: '開啟圖片預覽：{alt}',
    mermaidDiagram: 'Mermaid 圖表', mermaidLimit: 'Mermaid 圖表超過安全限制', mermaidBlocked: 'Mermaid 圖表內容已封鎖', mermaidRenderFailed: 'Mermaid 圖表轉譯失敗', mermaidUnsafeOutput: 'Mermaid 輸出未通過安全檢查', openMermaidDiagram: '開啟 Mermaid 圖表：{title}', mermaidTimeout: 'Mermaid 圖表轉譯逾時', mermaidDeferred: '圖表較多，已暫停後續 Mermaid 轉譯。', continueRendering: '繼續轉譯', continueMermaidRendering: '繼續轉譯剩餘 Mermaid 圖表', externalLinkFailed: '無法開啟外部連結', linkTypeBlocked: '此連結類型已封鎖',
    imagePreview: '大圖預覽', imagePreviewTools: '大圖預覽工具列', zoomOut: '縮小預覽', zoomIn: '放大預覽', zoomStatus: '目前縮放 {percent}%，點擊回復適應視窗', resetZoom: '適應視窗', exportPng: '匯出 PNG', pngExported: 'PNG 已匯出', pngExportLimit: 'PNG 匯出超過安全限制', pngExportFailed: 'PNG 匯出失敗', closeImagePreview: '關閉大圖',
  },
  ja: {
    appName: '屿阅',
    open: '開く', openMarkdownFile: 'Markdown ファイルを開く', readingSettings: '閲覧設定', closeNotification: '通知を閉じる',
    importingDocument: '文書を読み込み中…', renderingDocument: '文書を描画中…', welcomeTitle: 'Markdown を文書として読む', welcomeDescription: 'ローカルファイルを開いて内容に集中できます。リモートリソースは既定でブロックされます。', welcomeHint: '.md / .markdown / .txt に対応 · ファイルをウィンドウにドロップすることもできます', dropToOpen: 'ドロップして Markdown ファイルを開く',
    documentOpenFailed: '文書を開けません', unsupportedFormat: 'サポートされていない文書形式です', documentTooLarge: '文書が 10 MiB の上限を超えています', documentReadFailed: '文書を読み取れません。前回の内容は保持されています', documentIdentityChanged: 'ファイルの識別情報が変わりました。もう一度開いてください', documentNotUtf8: '文書は有効な UTF-8 テキストではありません', documentTabLimit: '開ける文書は 32 件までです', documentWatchFailed: 'ファイル変更の監視を開始できませんでした', markdownAstLimit: '文書構造が安全上限を超えています', markdownRenderLimit: '文書の描画結果が安全上限を超えています', markdownRenderFailed: '文書を描画できません', renderTimeout: '文書の描画がタイムアウトしました。他のタブは引き続き使用できます', renderCrash: '文書レンダラーが復旧しました。もう一度お試しください', renderQueueLimit: '同時に処理する文書が安全容量を超えています。しばらくしてからお試しください', renderProtocolError: '文書描画の通信エラーです。もう一度お試しください', renderCancelled: '文書の描画は取り消されました', documentProcessingFailed: '文書の処理に失敗しました', openDialogFailed: 'ファイル選択画面を開けません', printDocumentMissing: 'PDF を書き出す前に Markdown 文書を開いてください', printDialogFailed: 'システムの印刷パネルを開けません', appInitializeFailed: 'アプリの初期化に失敗しました',
    themeLight: 'ライト', themeDark: 'ダーク', themeSystem: 'システムに従う', themeFollowingSystem: 'システムに従う（現在：{theme}）', currentThemeAction: '現在：{current}。クリックして{next}に切り替え', themeChanged: '{theme}モードに切り替えました',
    language: '言語', readingAppearance: '閲覧表示', fontSize: '文字サイズ', decreaseFont: '文字を小さくする', increaseFont: '文字を大きくする', resetFont: '文字サイズをリセット', switchComfortableWidth: '読みやすい幅に切り替え', switchFullWidth: '全幅に切り替え', remoteImages: 'リモート画像', remoteImagesAllowed: 'このタブではリモート画像が許可されています', loadRemoteImages: 'この文書のリモート画像を読み込む', loadRemoteImagesHint: 'このタブでのみ HTTPS 画像を一時的に読み込む', closeReadingSettings: '閲覧設定を閉じる',
    searchDocument: '文書を検索', searchResults: '検索結果', noResults: '結果なし', previousMatch: '前の一致', nextMatch: '次の一致', closeSearch: '検索を閉じる',
    openDocuments: '開いている文書', switchTo: '{name} に切り替える', documentHasError: 'この文書にはエラーがあります', needsAttention: '要確認', closeDocument: '文書を閉じる', documentTabActions: '文書タブの操作', moreDocuments: '他の文書（{count} 件）', hiddenDocuments: '他の開いている文書', close: '閉じる', closeOthers: '他を閉じる', closeLeft: '左側を閉じる', closeRight: '右側を閉じる', closeAll: 'すべて閉じる',
    outline: '目次', headingNavigation: '見出しナビゲーション', retryDocument: 'この文書を再試行', retry: '再試行', outlineLocationMissing: '目次の位置が見つかりません', code: 'コード', codeLanguage: '言語：{language}', copyCode: 'コードをコピー', copied: 'コピー済み', codeCopied: 'コードをコピーしました', copyCodeFailed: 'コードをコピーできません', imageUnableToLoad: '画像を読み込めません', imageSourceBlocked: '画像ソースはブロックされています', remoteImageBlocked: 'リモート画像はブロックされています', remoteImageBlockedWithAlt: 'リモート画像はブロックされています：{alt}', loadForCurrentTab: 'このタブだけで読み込む', openImagePreview: '画像プレビューを開く', openImagePreviewWithAlt: '画像プレビューを開く：{alt}',
    mermaidDiagram: 'Mermaid 図', mermaidLimit: 'Mermaid 図が安全上限を超えています', mermaidBlocked: 'Mermaid 図の内容はブロックされています', mermaidRenderFailed: 'Mermaid 図を描画できません', mermaidUnsafeOutput: 'Mermaid の出力は安全検査に通りませんでした', openMermaidDiagram: 'Mermaid 図を開く：{title}', mermaidTimeout: 'Mermaid 図の描画がタイムアウトしました', mermaidDeferred: '図が多いため、以降の Mermaid 描画を一時停止しました。', continueRendering: '描画を続ける', continueMermaidRendering: '残りの Mermaid 図を描画する', externalLinkFailed: '外部リンクを開けません', linkTypeBlocked: 'このリンク形式はブロックされています',
    imagePreview: '大きな画像プレビュー', imagePreviewTools: '大きな画像プレビューのツール', zoomOut: '縮小', zoomIn: '拡大', zoomStatus: '現在の倍率 {percent}%。クリックしてウィンドウに合わせる', resetZoom: 'ウィンドウに合わせる', exportPng: 'PNG を書き出す', pngExported: 'PNG を書き出しました', pngExportLimit: 'PNG の書き出しが安全上限を超えています', pngExportFailed: 'PNG の書き出しに失敗しました', closeImagePreview: '大きな画像プレビューを閉じる',
  },
} as const

export type MessageKey = keyof typeof messages['zh-Hans']

function isLocale(value: string | null | undefined): value is Locale {
  return typeof value === 'string' && (LOCALES as readonly string[]).includes(value)
}

export function resolveInitialLocale(storedLocale: string | null | undefined, systemLocale: string | null | undefined): Locale {
  if (isLocale(storedLocale)) return storedLocale
  const normalized = systemLocale?.toLowerCase() ?? ''
  if (normalized.startsWith('zh-tw') || normalized.startsWith('zh-hk') || normalized.startsWith('zh-mo')) return 'zh-Hant'
  if (normalized.startsWith('en')) return 'en'
  if (normalized.startsWith('ja')) return 'ja'
  return 'zh-Hans'
}

function storedLocale() {
  if (typeof localStorage === 'undefined') return null
  return localStorage.getItem(LOCALE_KEY)
}

const currentLocale = shallowRef<Locale>(resolveInitialLocale(storedLocale(), typeof navigator === 'undefined' ? null : navigator.language))

export function useLocale() {
  function setLocale(locale: Locale) {
    currentLocale.value = locale
    if (typeof localStorage !== 'undefined') localStorage.setItem(LOCALE_KEY, locale)
  }

  function t(key: MessageKey, values: Record<string, string | number> = {}) {
    return messages[currentLocale.value][key].replace(/\{(\w+)\}/g, (placeholder, name) => String(values[name] ?? placeholder))
  }

  return {
    currentLocale,
    localeOptions: [
      { value: 'zh-Hans' as const, label: '简体中文' },
      { value: 'en' as const, label: 'English' },
      { value: 'zh-Hant' as const, label: '繁體中文' },
      { value: 'ja' as const, label: '日本語' },
    ],
    setLocale,
    t,
  }
}
