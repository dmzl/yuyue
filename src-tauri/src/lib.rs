use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{Arc, LazyLock, Mutex},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{ImageFormat, ImageReader, Limits};
use notify::{event::ModifyKind, EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{
    http,
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    AppHandle, Emitter, Manager, State, WebviewWindow,
};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

#[cfg(target_os = "macos")]
use objc2_foundation::{NSCopying, NSString};

const MAX_DOCUMENT_INPUT_BYTES: usize = 10 * 1024 * 1024;
const MAX_OPEN_DOCUMENTS: usize = 32;
const MAX_IMPORT_OPERATIONS: usize = 32;
const MAX_ACTIVE_IMPORTS: usize = 4;
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_SVG_BYTES: usize = 5 * 1024 * 1024;
const MAX_SVG_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_DOCUMENT_RESOURCES: usize = 32;
const MAX_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_TOTAL_RESOURCES: usize = 128;
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const PRINT_MARGIN_TOP_POINTS: f64 = 51.0;
const PRINT_MARGIN_HORIZONTAL_POINTS: f64 = 45.0;
const PRINT_MARGIN_BOTTOM_POINTS: f64 = 56.7;

static FILE_WATCHERS: LazyLock<Mutex<HashMap<String, notify::RecommendedWatcher>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// Parsing every local SVG used to scan the complete system font catalog. A document
// with several diagrams therefore repeated a large, synchronous initialization on
// the import path. The immutable database is safe to share across parses: usvg only
// clones it when a caller explicitly asks to mutate it.
static SVG_FONT_DATABASE: LazyLock<Arc<fontdb::Database>> = LazyLock::new(|| {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    Arc::new(database)
});

// SVG rasterization can allocate and encode several megabytes per diagram. Keep
// that work bounded when two documents begin their deferred image enhancement at
// nearly the same time; it runs on a blocking worker, never on the UI path.
static SVG_RASTERIZATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentPayload {
    document_id: String,
    file_name: String,
    content: String,
    source_revision: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ResolvedImageSource {
    url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileChangedEvent {
    document_id: String,
    source_revision: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentErrorEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    document_id: Option<String>,
    code: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentOpenedEvent {
    operation_id: String,
    sequence: u64,
    document: DocumentPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentImportErrorEvent {
    operation_id: String,
    sequence: u64,
    code: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentImportOperation {
    operation_id: String,
    sequence: u64,
    file_name: String,
}

impl DocumentImportOperation {
    #[cfg(test)]
    fn for_test(operation_id: &str, sequence: u64, file_name: &str) -> Self {
        Self {
            operation_id: operation_id.to_string(),
            sequence,
            file_name: file_name.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingImport {
    kind: String,
    operation_id: String,
    sequence: u64,
    file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    document: Option<DocumentPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

impl PendingImport {
    fn opened(operation: DocumentImportOperation, document: DocumentPayload) -> Self {
        Self {
            kind: "opened".to_string(),
            operation_id: operation.operation_id,
            sequence: operation.sequence,
            file_name: operation.file_name,
            document: Some(document),
            code: None,
        }
    }

    fn error(operation: DocumentImportOperation, code: String) -> Self {
        Self {
            kind: "error".to_string(),
            operation_id: operation.operation_id,
            sequence: operation.sequence,
            file_name: operation.file_name,
            document: None,
            code: Some(code),
        }
    }

    fn document_id(&self) -> Option<&str> {
        self.document
            .as_ref()
            .map(|document| document.document_id.as_str())
    }

    fn operation(&self) -> DocumentImportOperation {
        DocumentImportOperation {
            operation_id: self.operation_id.clone(),
            sequence: self.sequence,
            file_name: self.file_name.clone(),
        }
    }
}

#[derive(Debug)]
struct ImportJob {
    operation: DocumentImportOperation,
    path: PathBuf,
    started_published: bool,
}

#[cfg(test)]
impl ImportJob {
    fn for_test(sequence: u64) -> Self {
        Self {
            operation: DocumentImportOperation {
                operation_id: format!("operation-{sequence}"),
                sequence,
                file_name: format!("document-{sequence}.md"),
            },
            path: PathBuf::from(format!("document-{sequence}.md")),
            started_published: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct PrintErrorEvent {
    code: String,
}

#[derive(Debug)]
struct ResourceSnapshot {
    resource_id: String,
    bytes: Vec<u8>,
    mime: String,
}

#[derive(Debug)]
struct DocumentContext {
    path: PathBuf,
    root: PathBuf,
    revision: u64,
    resources: HashMap<String, ResourceSnapshot>,
    resource_bytes: usize,
    creator_operation_id: String,
    published: bool,
    claims: HashSet<String>,
    pending_owner_count: usize,
}

#[derive(Debug)]
struct PreparedDocument {
    path: PathBuf,
    root: PathBuf,
    file_name: String,
    content: String,
}

#[derive(Debug)]
struct PreparedImage {
    bytes: Vec<u8>,
    mime: &'static str,
}

#[derive(Debug)]
struct LocalImageResolution {
    identity: String,
    root: PathBuf,
    cached_url: Option<String>,
}

#[derive(Debug, Default)]
struct DocumentRegistry {
    documents: HashMap<String, DocumentContext>,
    path_index: HashMap<PathBuf, String>,
    pending_import: Option<PendingImport>,
    frontend_ready: bool,
    total_resources: usize,
    total_bytes: usize,
    import_queue: VecDeque<ImportJob>,
    active_imports: usize,
    import_reserved_bytes: usize,
    import_sequence: u64,
}

impl DocumentRegistry {
    fn begin_import(&mut self, path: &Path) -> (DocumentImportOperation, bool) {
        self.import_sequence = self.import_sequence.saturating_add(1);
        (
            DocumentImportOperation {
                operation_id: Uuid::new_v4().simple().to_string(),
                sequence: self.import_sequence,
                file_name: safe_file_label(path),
            },
            self.frontend_ready,
        )
    }

    fn enqueue_import(&mut self, job: ImportJob) -> Result<(), String> {
        if self.import_queue.len() + self.active_imports >= MAX_IMPORT_OPERATIONS {
            return Err("DOCUMENT_TAB_LIMIT".to_string());
        }
        self.import_queue.push_back(job);
        Ok(())
    }

    fn start_next_import(&mut self) -> Option<ImportJob> {
        if self.active_imports >= MAX_ACTIVE_IMPORTS {
            return None;
        }
        let job = self.import_queue.pop_front()?;
        self.active_imports += 1;
        self.import_reserved_bytes += MAX_DOCUMENT_INPUT_BYTES;
        Some(job)
    }

    fn finish_import_slot(&mut self) {
        self.active_imports = self.active_imports.saturating_sub(1);
        self.import_reserved_bytes = self
            .import_reserved_bytes
            .saturating_sub(MAX_DOCUMENT_INPUT_BYTES);
    }

    #[cfg(test)]
    fn open_document(&mut self, path: PathBuf) -> Result<DocumentPayload, String> {
        let operation_id = format!("sync-{}", Uuid::new_v4().simple());
        let payload = self.commit_prepared(prepare_document(path)?, &operation_id)?;
        self.publish_claim(&payload.document_id, &operation_id);
        Ok(payload)
    }

    fn commit_prepared(
        &mut self,
        prepared: PreparedDocument,
        operation_id: &str,
    ) -> Result<DocumentPayload, String> {
        let PreparedDocument {
            path,
            root,
            file_name,
            content,
        } = prepared;
        if let Some(existing_id) = self.path_index.get(&path).cloned() {
            let context = self
                .documents
                .get_mut(&existing_id)
                .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
            context.claims.insert(operation_id.to_string());
            return Ok(DocumentPayload {
                document_id: existing_id,
                file_name,
                content,
                source_revision: context.revision,
            });
        }

        if self.documents.len() >= MAX_OPEN_DOCUMENTS {
            return Err("DOCUMENT_TAB_LIMIT".to_string());
        }

        let document_id = Uuid::new_v4().simple().to_string();
        let payload = DocumentPayload {
            document_id: document_id.clone(),
            file_name: file_name.clone(),
            content,
            source_revision: 0,
        };
        self.path_index.insert(path.clone(), document_id.clone());
        self.documents.insert(
            document_id.clone(),
            DocumentContext {
                path,
                root,
                revision: 0,
                resources: HashMap::new(),
                resource_bytes: 0,
                creator_operation_id: operation_id.to_string(),
                published: false,
                claims: HashSet::from([operation_id.to_string()]),
                pending_owner_count: 0,
            },
        );
        Ok(payload)
    }

    fn publish_claim(&mut self, document_id: &str, operation_id: &str) {
        if let Some(context) = self.documents.get_mut(document_id) {
            context.published = true;
            context.claims.remove(operation_id);
        }
        self.reap_unpublished(document_id);
    }

    fn release_claim(&mut self, document_id: &str, operation_id: &str) {
        if let Some(context) = self.documents.get_mut(document_id) {
            context.claims.remove(operation_id);
        }
        self.reap_unpublished(document_id);
    }

    fn reap_unpublished(&mut self, document_id: &str) {
        let should_reap = self.documents.get(document_id).is_some_and(|context| {
            !context.creator_operation_id.is_empty()
                && !context.published
                && context.claims.is_empty()
                && context.pending_owner_count == 0
        });
        if should_reap {
            self.close_document(document_id);
        }
    }

    fn queue_pending_import(&mut self, pending: PendingImport) {
        let new_document_id = pending.document_id().map(str::to_string);
        let operation_id = pending.operation_id.clone();
        if let Some(document_id) = new_document_id.as_deref() {
            if let Some(context) = self.documents.get_mut(document_id) {
                context.pending_owner_count += 1;
            }
        }
        let previous = self.pending_import.replace(pending);
        if let Some(document_id) = previous.as_ref().and_then(PendingImport::document_id) {
            self.release_pending_owner(document_id);
        }
        if let Some(document_id) = new_document_id.as_deref() {
            self.release_claim(document_id, &operation_id);
        }
    }

    fn release_pending_owner(&mut self, document_id: &str) {
        if let Some(context) = self.documents.get_mut(document_id) {
            context.pending_owner_count = context.pending_owner_count.saturating_sub(1);
        }
        self.reap_unpublished(document_id);
    }

    fn take_pending_import(&mut self) -> Option<PendingImport> {
        self.frontend_ready = true;
        let pending = self.pending_import.take()?;
        if let Some(document_id) = pending.document_id() {
            if let Some(context) = self.documents.get_mut(document_id) {
                context.published = true;
            }
            self.release_pending_owner(document_id);
        }
        Some(pending)
    }

    fn close_document(&mut self, document_id: &str) {
        if let Some(context) = self.documents.remove(document_id) {
            self.path_index.remove(&context.path);
            self.total_resources = self.total_resources.saturating_sub(context.resources.len());
            self.total_bytes = self.total_bytes.saturating_sub(context.resource_bytes);
        }
    }

    fn suggested_pdf_file_name_for_document(&self, document_id: &str) -> Option<String> {
        self.documents
            .get(document_id)
            .and_then(|context| context.path.file_name())
            .and_then(|file_name| file_name.to_str())
            .map(suggested_pdf_file_name)
    }

    fn reload_document(
        &mut self,
        document_id: &str,
        observed_revision: Option<u64>,
    ) -> Result<DocumentPayload, String> {
        let (path, file_name) = self
            .documents
            .get(document_id)
            .map(|context| {
                (
                    context.path.clone(),
                    context
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("document.md")
                        .to_string(),
                )
            })
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        validate_canonical_document_path(&path)?;
        let content = read_document_content(&path)?;
        let (revision, released_resources, released_bytes) = {
            let context = self
                .documents
                .get_mut(document_id)
                .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
            let released_resources = context.resources.len();
            let released_bytes = context.resource_bytes;
            context.resources.clear();
            context.resource_bytes = 0;
            context.revision = observed_revision
                .map(|revision| context.revision.max(revision))
                .unwrap_or_else(|| context.revision.saturating_add(1));
            (context.revision, released_resources, released_bytes)
        };
        self.total_resources = self.total_resources.saturating_sub(released_resources);
        self.total_bytes = self.total_bytes.saturating_sub(released_bytes);
        Ok(DocumentPayload {
            document_id: document_id.to_string(),
            file_name,
            content,
            source_revision: revision,
        })
    }

    fn mark_file_changed(&mut self, document_id: &str) -> Result<u64, String> {
        let context = self
            .documents
            .get_mut(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        context.revision = context.revision.saturating_add(1);
        Ok(context.revision)
    }

    #[cfg(test)]
    fn resolve_image(
        &mut self,
        document_id: &str,
        source: &str,
        allow_remote: bool,
    ) -> Result<ResolvedImageSource, String> {
        match classify_image_source(source) {
            ImageSourceClass::Https(url) => {
                if allow_remote {
                    Ok(ResolvedImageSource { url })
                } else {
                    Err("IMAGE_REMOTE_BLOCKED".to_string())
                }
            }
            ImageSourceClass::Data(url) => {
                let (extension, bytes) = decode_data_image(&url)?;
                self.snapshot_image_bytes(document_id, url, extension, bytes)
            }
            ImageSourceClass::Rejected(error) => Err(error),
            ImageSourceClass::Local(source) => self.snapshot_local_image(document_id, &source),
        }
    }

    #[cfg(test)]
    fn snapshot_local_image(
        &mut self,
        document_id: &str,
        source: &str,
    ) -> Result<ResolvedImageSource, String> {
        let resolution = self.local_image_resolution(document_id, source)?;
        if let Some(url) = resolution.cached_url {
            return Ok(ResolvedImageSource { url });
        }

        let prepared = prepare_local_image(&resolution.root, &resolution.identity)?;
        self.store_prepared_image(document_id, resolution.identity, prepared)
    }

    fn local_image_resolution(
        &self,
        document_id: &str,
        source: &str,
    ) -> Result<LocalImageResolution, String> {
        let identity = normalize_relative_source(source)?;
        self.documents
            .get(document_id)
            .map(|context| LocalImageResolution {
                cached_url: context
                    .resources
                    .get(&identity)
                    .map(|snapshot| image_protocol_url(document_id, &snapshot.resource_id)),
                identity,
                root: context.root.clone(),
            })
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())
    }

    fn store_prepared_image(
        &mut self,
        document_id: &str,
        identity: String,
        prepared: PreparedImage,
    ) -> Result<ResolvedImageSource, String> {
        let PreparedImage { bytes, mime } = prepared;
        let context = self
            .documents
            .get_mut(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if let Some(snapshot) = context.resources.get(&identity) {
            return Ok(ResolvedImageSource {
                url: image_protocol_url(document_id, &snapshot.resource_id),
            });
        }

        if context.resources.len() >= MAX_DOCUMENT_RESOURCES
            || context.resource_bytes.saturating_add(bytes.len()) > MAX_DOCUMENT_BYTES
            || self.total_resources >= MAX_TOTAL_RESOURCES
            || self.total_bytes.saturating_add(bytes.len()) > MAX_TOTAL_BYTES
        {
            return Err("IMAGE_RESOURCE_LIMIT".to_string());
        }

        let resource_id = Uuid::new_v4().simple().to_string();
        let byte_count = bytes.len();
        context.resource_bytes += byte_count;
        context.resources.insert(
            identity,
            ResourceSnapshot {
                resource_id: resource_id.clone(),
                bytes,
                mime: mime.to_string(),
            },
        );
        self.total_resources += 1;
        self.total_bytes += byte_count;
        Ok(ResolvedImageSource {
            url: image_protocol_url(document_id, &resource_id),
        })
    }

    #[cfg(test)]
    fn snapshot_image_bytes(
        &mut self,
        document_id: &str,
        identity: String,
        extension: &str,
        bytes: Vec<u8>,
    ) -> Result<ResolvedImageSource, String> {
        let (bytes, mime) = prepare_image_bytes(extension, &bytes)?;
        self.store_prepared_image(document_id, identity, PreparedImage { bytes, mime })
    }

    fn protocol_snapshot(&self, document_id: &str, resource_id: &str) -> Option<&ResourceSnapshot> {
        self.documents
            .get(document_id)?
            .resources
            .values()
            .find(|snapshot| snapshot.resource_id == resource_id)
    }
}

fn safe_file_label(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let normalized = raw
        .chars()
        .filter(|character| {
            !matches!(
                *character,
                '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
            )
        })
        .map(|character| if character.is_control() { ' ' } else { character })
        .collect::<String>();
    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let bounded = collapsed.chars().take(120).collect::<String>();
    if bounded.is_empty() {
        "document.md".to_string()
    } else {
        bounded
    }
}

fn prepare_document(path: PathBuf) -> Result<PreparedDocument, String> {
    let input_metadata =
        fs::symlink_metadata(&path).map_err(|_| "DOCUMENT_OPEN_FAILED".to_string())?;
    if input_metadata.file_type().is_symlink() {
        return Err("DOCUMENT_IDENTITY_CHANGED".to_string());
    }
    if !input_metadata.is_file() {
        return Err("DOCUMENT_NOT_A_FILE".to_string());
    }
    let path = fs::canonicalize(path).map_err(|_| "DOCUMENT_OPEN_FAILED".to_string())?;
    if !is_supported_file(&path) {
        return Err("DOCUMENT_UNSUPPORTED_FORMAT".to_string());
    }
    validate_canonical_document_path(&path)?;
    let content = read_document_content(&path)?;
    validate_canonical_document_path(&path)?;
    let root = path
        .parent()
        .ok_or_else(|| "DOCUMENT_OPEN_FAILED".to_string())?
        .to_path_buf();
    let file_name = safe_file_label(&path);
    Ok(PreparedDocument {
        path,
        root,
        file_name,
        content,
    })
}

fn read_document_content(path: &Path) -> Result<String, String> {
    let mut file = open_document_no_follow(path)?;
    let mut bytes = Vec::with_capacity(MAX_DOCUMENT_INPUT_BYTES.min(64 * 1024));
    file.by_ref()
        .take((MAX_DOCUMENT_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "DOCUMENT_READ_FAILED".to_string())?;
    if bytes.len() > MAX_DOCUMENT_INPUT_BYTES {
        return Err("DOCUMENT_TOO_LARGE".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "DOCUMENT_NOT_UTF8".to_string())
}

fn open_document_no_follow(path: &Path) -> Result<File, String> {
    let root = path
        .parent()
        .ok_or_else(|| "DOCUMENT_READ_FAILED".to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "DOCUMENT_READ_FAILED".to_string())?;
    open_image_no_follow(root, file_name).map_err(|_| "DOCUMENT_READ_FAILED".to_string())
}

fn validate_canonical_document_path(path: &Path) -> Result<(), String> {
    let canonical = fs::canonicalize(path).map_err(|_| "DOCUMENT_READ_FAILED".to_string())?;
    if canonical != path {
        return Err("DOCUMENT_IDENTITY_CHANGED".to_string());
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| "DOCUMENT_READ_FAILED".to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("DOCUMENT_IDENTITY_CHANGED".to_string());
    }
    if !metadata.is_file() {
        return Err("DOCUMENT_NOT_A_FILE".to_string());
    }
    if metadata.len() > MAX_DOCUMENT_INPUT_BYTES as u64 {
        return Err("DOCUMENT_TOO_LARGE".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImageSourceClass {
    Local(String),
    Https(String),
    Data(String),
    Rejected(String),
}

fn classify_image_source(source: &str) -> ImageSourceClass {
    let lowercase = source.to_ascii_lowercase();
    if lowercase.starts_with("https://")
        && source.len() > "https://".len()
        && !source.chars().any(char::is_whitespace)
    {
        ImageSourceClass::Https(source.to_string())
    } else if lowercase.starts_with("data:") {
        ImageSourceClass::Data(source.to_string())
    } else if source.starts_with('/')
        || source.starts_with('\\')
        || source.starts_with("//")
        || has_explicit_scheme(source)
    {
        ImageSourceClass::Rejected("IMAGE_SOURCE_SCHEME".to_string())
    } else {
        ImageSourceClass::Local(source.to_string())
    }
}

fn decode_data_image(source: &str) -> Result<(&'static str, Vec<u8>), String> {
    let payload = source
        .strip_prefix("data:")
        .ok_or_else(|| "IMAGE_DATA_INVALID".to_string())?;
    let (metadata, data) = payload
        .split_once(',')
        .ok_or_else(|| "IMAGE_DATA_INVALID".to_string())?;
    let metadata = metadata.to_ascii_lowercase();
    let extension = match metadata.as_str() {
        "image/png;base64" => "png",
        "image/jpeg;base64" => "jpeg",
        "image/gif;base64" => "gif",
        "image/webp;base64" => "webp",
        "image/svg+xml;base64" => "svg",
        _ => return Err("IMAGE_DATA_INVALID".to_string()),
    };
    let max_encoded_bytes = (MAX_IMAGE_BYTES.saturating_add(2) / 3).saturating_mul(4);
    if data.is_empty() || data.len() > max_encoded_bytes {
        return Err("IMAGE_DATA_INVALID".to_string());
    }
    let bytes = BASE64
        .decode(data)
        .map_err(|_| "IMAGE_DATA_INVALID".to_string())?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("IMAGE_DATA_INVALID".to_string());
    }
    Ok((extension, bytes))
}

#[cfg(test)]
fn validate_data_image(source: &str) -> Result<(), String> {
    let (extension, bytes) = decode_data_image(source)?;
    prepare_image_bytes(extension, &bytes).map(|_| ())
}

fn normalize_relative_source(source: &str) -> Result<String, String> {
    if source.is_empty() {
        return Err("IMAGE_SOURCE_EMPTY".to_string());
    }
    let decoded = percent_decode(source)?;
    if decoded.is_empty()
        || decoded.contains('\0')
        || decoded.starts_with('/')
        || decoded.starts_with("//")
        || is_windows_absolute(&decoded)
    {
        return Err("IMAGE_PATH_INVALID".to_string());
    }
    if decoded.contains('\\') || has_explicit_scheme(&decoded) {
        return Err("IMAGE_SOURCE_SCHEME".to_string());
    }
    let mut components = Vec::new();
    for component in decoded.split('/') {
        match component {
            "" => return Err("IMAGE_PATH_INVALID".to_string()),
            "." => {}
            ".." => return Err("IMAGE_PATH_TRAVERSAL".to_string()),
            value => components.push(value),
        }
    }
    if components.is_empty() {
        return Err("IMAGE_PATH_INVALID".to_string());
    }
    Ok(components.join("/"))
}

fn percent_decode(source: &str) -> Result<String, String> {
    let bytes = source.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err("IMAGE_PATH_INVALID".to_string());
        }
        let high = hex_value(bytes[index + 1]).ok_or_else(|| "IMAGE_PATH_INVALID".to_string())?;
        let low = hex_value(bytes[index + 2]).ok_or_else(|| "IMAGE_PATH_INVALID".to_string())?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| "IMAGE_PATH_INVALID".to_string())
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
fn is_windows_absolute(source: &str) -> bool {
    let bytes = source.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || source.starts_with("\\\\")
}
fn has_explicit_scheme(source: &str) -> bool {
    let Some(colon) = source.find(':') else {
        return false;
    };
    let before = &source[..colon];
    !before.is_empty()
        && before.bytes().enumerate().all(|(index, value)| {
            (index == 0 && value.is_ascii_alphabetic())
                || (index > 0 && (value.is_ascii_alphanumeric() || b"+.-".contains(&value)))
        })
}
fn is_supported_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("md" | "markdown" | "txt")
    )
}
fn is_protocol_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}
fn parse_protocol_path(path: &str) -> Option<(String, String)> {
    let mut segments = path.strip_prefix('/')?.split('/');
    let document_id = segments.next()?;
    let resource_id = segments.next()?;
    (segments.next().is_none()
        && !path.contains('%')
        && !path.contains(['?', '#', '\\'])
        && is_protocol_identifier(document_id)
        && is_protocol_identifier(resource_id))
    .then(|| (document_id.to_string(), resource_id.to_string()))
}
fn image_protocol_url(document_id: &str, resource_id: &str) -> String {
    format!("mdreader-image://localhost/{document_id}/{resource_id}")
}

#[cfg(unix)]
fn open_image_no_follow(root: &Path, source: &str) -> Result<File, String> {
    use std::{ffi::CString, os::unix::io::FromRawFd};
    let root = CString::new(root.as_os_str().as_encoded_bytes())
        .map_err(|_| "IMAGE_PATH_INVALID".to_string())?;
    let mut directory_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if directory_fd < 0 {
        return Err("IMAGE_OPEN_FAILED".to_string());
    }
    let components: Vec<_> = Path::new(source).components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            unsafe {
                libc::close(directory_fd);
            };
            return Err("IMAGE_PATH_INVALID".to_string());
        };
        let name = CString::new(component.as_encoded_bytes())
            .map_err(|_| "IMAGE_PATH_INVALID".to_string())?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if index + 1 == components.len() {
                0
            } else {
                libc::O_DIRECTORY
            };
        let next_fd = unsafe { libc::openat(directory_fd, name.as_ptr(), flags) };
        unsafe {
            libc::close(directory_fd);
        }
        if next_fd < 0 {
            return Err("IMAGE_OPEN_FAILED".to_string());
        }
        directory_fd = next_fd;
    }
    Ok(unsafe { File::from_raw_fd(directory_fd) })
}

#[cfg(not(unix))]
fn open_image_no_follow(_: &Path, _: &str) -> Result<File, String> {
    Err("IMAGE_PLATFORM_UNSUPPORTED".to_string())
}

fn prepare_local_image(root: &Path, identity: &str) -> Result<PreparedImage, String> {
    let extension = Path::new(identity)
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "IMAGE_UNSUPPORTED_FORMAT".to_string())?
        .to_string();
    let mut file = open_image_no_follow(root, identity)?;
    let metadata = file
        .metadata()
        .map_err(|_| "IMAGE_OPEN_FAILED".to_string())?;
    if !metadata.is_file() {
        return Err("IMAGE_NOT_A_FILE".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_IMAGE_BYTES as u64) as usize + 1);
    file.by_ref()
        .take((MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "IMAGE_READ_FAILED".to_string())?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("IMAGE_TOO_LARGE".to_string());
    }
    let (bytes, mime) = prepare_image_bytes(&extension, &bytes)?;
    Ok(PreparedImage { bytes, mime })
}

fn prepare_data_image(source: &str) -> Result<(String, PreparedImage), String> {
    let (extension, bytes) = decode_data_image(source)?;
    let (bytes, mime) = prepare_image_bytes(extension, &bytes)?;
    Ok((source.to_string(), PreparedImage { bytes, mime }))
}

#[cfg(test)]
fn validate_image_bytes(extension: &str, bytes: &[u8]) -> Result<&'static str, String> {
    prepare_image_bytes(extension, bytes).map(|(_, mime)| mime)
}

fn prepare_image_bytes(extension: &str, bytes: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    match extension.to_ascii_lowercase().as_str() {
        "png" if decode_raster(bytes, ImageFormat::Png) => Ok((bytes.to_vec(), "image/png")),
        "jpg" | "jpeg" if decode_raster(bytes, ImageFormat::Jpeg) => {
            Ok((bytes.to_vec(), "image/jpeg"))
        }
        "gif" if decode_raster(bytes, ImageFormat::Gif) => Ok((bytes.to_vec(), "image/gif")),
        "webp" if decode_raster(bytes, ImageFormat::WebP) => Ok((bytes.to_vec(), "image/webp")),
        "svg" => rasterize_safe_svg(bytes).map(|png| (png, "image/png")),
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Err("IMAGE_TYPE_MISMATCH".to_string()),
        _ => Err("IMAGE_UNSUPPORTED_FORMAT".to_string()),
    }
}

fn decode_raster(bytes: &[u8], format: ImageFormat) -> bool {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    reader.limits(limits);
    reader.decode().is_ok()
}

#[cfg(test)]
fn validate_svg(bytes: &[u8]) -> bool {
    rasterize_safe_svg(bytes).is_ok()
}

fn rasterize_safe_svg(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.is_empty() || bytes.len() > MAX_SVG_BYTES || !is_safe_svg_markup(bytes) {
        return Err("SVG_UNSAFE_CONTENT".to_string());
    }
    let _rasterization = SVG_RASTERIZATION_LOCK
        .lock()
        .map_err(|_| "SVG_RENDER_FAILED".to_string())?;
    let options = safe_svg_options();
    let tree =
        usvg::Tree::from_data(bytes, &options).map_err(|_| "SVG_UNSAFE_CONTENT".to_string())?;
    let size = tree.size().to_int_size();
    let pixels = u64::from(size.width()).saturating_mul(u64::from(size.height()));
    if size.width() == 0
        || size.height() == 0
        || size.width() > MAX_IMAGE_DIMENSION
        || size.height() > MAX_IMAGE_DIMENSION
        || pixels > MAX_SVG_PIXELS
    {
        return Err("SVG_LIMIT".to_string());
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| "SVG_LIMIT".to_string())?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );
    pixmap
        .encode_png()
        .map_err(|_| "SVG_RENDER_FAILED".to_string())
}

fn safe_svg_options() -> usvg::Options<'static> {
    let mut options = usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    options.fontdb = Arc::clone(&SVG_FONT_DATABASE);
    options
}

fn is_local_svg_fragment(value: &str) -> bool {
    let target = value.trim();
    let target = target
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            target
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(target);
    let Some(fragment) = target.strip_prefix('#') else {
        return false;
    };
    !fragment.is_empty()
        && fragment
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '.' | ':' | '-'))
}

fn contains_unsafe_svg_url(source: &str) -> bool {
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find("url") {
        let start = cursor + relative_start;
        let mut open = start + 3;
        while source
            .as_bytes()
            .get(open)
            .is_some_and(|value| value.is_ascii_whitespace())
        {
            open += 1;
        }
        if source.as_bytes().get(open) != Some(&b'(') {
            cursor = start + 3;
            continue;
        }
        let Some(relative_end) = source[open + 1..].find(')') else {
            return true;
        };
        let end = open + 1 + relative_end;
        if !is_local_svg_fragment(&source[open + 1..end]) {
            return true;
        }
        cursor = end + 1;
    }
    false
}

fn is_safe_svg_markup(bytes: &[u8]) -> bool {
    let source = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    let forbidden = [
        "<script",
        "<foreignobject",
        "<image",
        "<feimage",
        "<iframe",
        "<object",
        "<embed",
        "<use",
        "xlink:href",
        "href=\"http:",
        "href=\"https:",
        "href=\"file:",
        "href='http:",
        "href='https:",
        "href='file:",
    ];
    source.matches("<svg").count() == 1
        && !forbidden.iter().any(|value| source.contains(value))
        && !contains_unsafe_svg_url(&source)
}

fn serve_image(registry: &DocumentRegistry, path: &str) -> http::Response<Vec<u8>> {
    let Some((document_id, resource_id)) = parse_protocol_path(path) else {
        return empty_response(http::StatusCode::BAD_REQUEST);
    };
    let Some(snapshot) = registry.protocol_snapshot(&document_id, &resource_id) else {
        return empty_response(http::StatusCode::NOT_FOUND);
    };
    http::Response::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, &snapshot.mime)
        .header(http::header::CACHE_CONTROL, "no-store")
        .body(snapshot.bytes.clone())
        .unwrap_or_else(|_| empty_response(http::StatusCode::INTERNAL_SERVER_ERROR))
}
fn empty_response(status: http::StatusCode) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CACHE_CONTROL, "no-store")
        .body(Vec::new())
        .expect("response builder")
}

fn emit_import_terminal(app: &AppHandle, pending: PendingImport, started_published: bool) {
    let operation = pending.operation();
    if !started_published {
        let _ = app.emit("document-import-started", operation.clone());
    }
    if let Some(document) = pending.document {
        let document_id = document.document_id.clone();
        let opened = DocumentOpenedEvent {
            operation_id: operation.operation_id.clone(),
            sequence: operation.sequence,
            document,
        };
        if app.emit("document-opened", opened).is_ok() {
            app.state::<Mutex<DocumentRegistry>>()
                .lock()
                .expect("document registry")
                .publish_claim(&document_id, &operation.operation_id);
        } else {
            app.state::<Mutex<DocumentRegistry>>()
                .lock()
                .expect("document registry")
                .release_claim(&document_id, &operation.operation_id);
            let _ = app.emit(
                "document-import-error",
                DocumentImportErrorEvent {
                    operation_id: operation.operation_id,
                    sequence: operation.sequence,
                    code: "DOCUMENT_OPEN_FAILED".to_string(),
                },
            );
        }
    } else {
        let _ = app.emit(
            "document-import-error",
            DocumentImportErrorEvent {
                operation_id: operation.operation_id,
                sequence: operation.sequence,
                code: pending
                    .code
                    .unwrap_or_else(|| "DOCUMENT_OPEN_FAILED".to_string()),
            },
        );
    }
}

fn publish_import_result(
    app: &AppHandle,
    operation: DocumentImportOperation,
    started_published: bool,
    prepared: Result<PreparedDocument, String>,
) {
    let (pending, frontend_ready) = {
        let registry_state = app.state::<Mutex<DocumentRegistry>>();
        let mut registry = registry_state.lock().expect("document registry");
        let pending = match prepared {
            Ok(prepared) => match registry.commit_prepared(prepared, &operation.operation_id) {
                Ok(document) => PendingImport::opened(operation, document),
                Err(code) => PendingImport::error(operation, code),
            },
            Err(code) => PendingImport::error(operation, code),
        };
        if !registry.frontend_ready {
            registry.queue_pending_import(pending);
            return;
        }
        (pending, true)
    };
    if frontend_ready {
        emit_import_terminal(app, pending, started_published);
    }
}

fn drive_import_queue(app: &AppHandle) {
    loop {
        let job = app
            .state::<Mutex<DocumentRegistry>>()
            .lock()
            .expect("document registry")
            .start_next_import();
        let Some(job) = job else {
            return;
        };
        let ImportJob {
            operation,
            path,
            started_published,
        } = job;
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let prepared =
                match tauri::async_runtime::spawn_blocking(move || prepare_document(path)).await {
                    Ok(result) => result,
                    Err(_) => Err("DOCUMENT_OPEN_FAILED".to_string()),
                };
            publish_import_result(&app_handle, operation, started_published, prepared);
            app_handle
                .state::<Mutex<DocumentRegistry>>()
                .lock()
                .expect("document registry")
                .finish_import_slot();
            drive_import_queue(&app_handle);
        });
    }
}

fn publish_document(app: &AppHandle, path: PathBuf) {
    let (operation, frontend_ready) = app
        .state::<Mutex<DocumentRegistry>>()
        .lock()
        .expect("document registry")
        .begin_import(&path);
    let started_published = frontend_ready
        && app
            .emit("document-import-started", operation.clone())
            .is_ok();
    let job = ImportJob {
        operation: operation.clone(),
        path,
        started_published,
    };
    let admission = app
        .state::<Mutex<DocumentRegistry>>()
        .lock()
        .expect("document registry")
        .enqueue_import(job);
    if let Err(code) = admission {
        publish_import_result(app, operation, started_published, Err(code));
        return;
    }
    drive_import_queue(app);
}

#[tauri::command]
fn open_document_dialog(window: WebviewWindow) {
    let app = window.app_handle().clone();
    window
        .dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown", "txt"])
        .pick_file(move |file| {
            if let Some(file) = file {
                if let Ok(path) = file.into_path() {
                    publish_document(&app, path);
                }
            }
        });
}

fn suggested_pdf_file_name(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("document");
    format!("{stem}.pdf")
}

#[cfg(target_os = "macos")]
fn open_native_print_dialog(
    window: &WebviewWindow,
    suggested_file_name: String,
) -> tauri::Result<()> {
    window.with_webview(move |webview| unsafe {
        let view: &objc2_web_kit::WKWebView = &*webview.inner().cast();
        let print_info = objc2_app_kit::NSPrintInfo::sharedPrintInfo().copy();
        print_info.setTopMargin(PRINT_MARGIN_TOP_POINTS);
        print_info.setRightMargin(PRINT_MARGIN_HORIZONTAL_POINTS);
        print_info.setBottomMargin(PRINT_MARGIN_BOTTOM_POINTS);
        print_info.setLeftMargin(PRINT_MARGIN_HORIZONTAL_POINTS);

        let print_operation = view.printOperationWithPrintInfo(&print_info);
        let job_title = NSString::from_str(&suggested_file_name);
        print_operation.setJobTitle(Some(&job_title));
        print_operation.setCanSpawnSeparateThread(true);

        let native_window: &objc2_app_kit::NSWindow = &*webview.ns_window().cast();
        print_operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            native_window,
            None,
            None,
            std::ptr::null_mut(),
        );
    })
}

#[cfg(not(target_os = "macos"))]
fn open_native_print_dialog(
    window: &WebviewWindow,
    _suggested_file_name: String,
) -> tauri::Result<()> {
    window.print()
}

fn emit_print_error(app: &AppHandle, code: &str) {
    let _ = app.emit(
        "print-error",
        PrintErrorEvent {
            code: code.to_string(),
        },
    );
}

fn open_print_dialog(app: &AppHandle, document_id: &str) {
    let suggested_file_name = {
        let registry = app.state::<Mutex<DocumentRegistry>>();
        let registry = registry.lock().expect("document registry");
        registry.suggested_pdf_file_name_for_document(document_id)
    };
    let Some(suggested_file_name) = suggested_file_name else {
        emit_print_error(app, "PRINT_DOCUMENT_MISSING");
        return;
    };

    let printed = app
        .get_webview_window("main")
        .map(|window| open_native_print_dialog(&window, suggested_file_name).is_ok())
        .unwrap_or(false);
    if !printed {
        emit_print_error(app, "PRINT_DIALOG_FAILED");
    }
}

fn request_print_export(app: &AppHandle) {
    let has_open_document = {
        let registry = app.state::<Mutex<DocumentRegistry>>();
        let has_open_document = !registry
            .lock()
            .expect("document registry")
            .documents
            .is_empty();
        has_open_document
    };
    if has_open_document {
        let _ = app.emit("export-current-pdf", ());
    } else {
        emit_print_error(app, "PRINT_DOCUMENT_MISSING");
    }
}

#[tauri::command]
async fn export_png_dialog(
    app: AppHandle,
    svg: String,
    suggested_file_name: String,
) -> Result<(), String> {
    let file_path = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("PNG", &["png"])
            .set_file_name(suggested_file_name)
            .blocking_save_file()
    })
    .await
    .map_err(|_| "EXPORT_DIALOG_FAILED".to_string())?;

    let Some(file_path) = file_path else {
        return Ok(());
    };

    let path = file_path
        .into_path()
        .map_err(|_| "EXPORT_PATH_INVALID".to_string())?;

    let png = rasterize_safe_svg(svg.as_bytes()).map_err(|code| {
        if code == "SVG_LIMIT" {
            "EXPORT_RENDER_LIMIT"
        } else {
            "EXPORT_RENDER_FAILED"
        }
    })?;
    fs::write(path, png).map_err(|_| "EXPORT_WRITE_FAILED".to_string())?;
    Ok(())
}

#[tauri::command]
fn take_pending_import(registry: State<'_, Mutex<DocumentRegistry>>) -> Option<PendingImport> {
    registry
        .lock()
        .expect("document registry")
        .take_pending_import()
}

#[tauri::command]
fn export_current_pdf(app: AppHandle, document_id: String) {
    open_print_dialog(&app, &document_id)
}

#[tauri::command]
fn reload_document(
    document_id: String,
    source_revision: Option<u64>,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<DocumentPayload, String> {
    registry
        .lock()
        .expect("document registry")
        .reload_document(&document_id, source_revision)
}

#[tauri::command]
async fn resolve_image_source(
    app: AppHandle,
    document_id: String,
    source: String,
    allow_remote: bool,
) -> Result<ResolvedImageSource, String> {
    match classify_image_source(&source) {
        ImageSourceClass::Https(url) => {
            if allow_remote {
                Ok(ResolvedImageSource { url })
            } else {
                Err("IMAGE_REMOTE_BLOCKED".to_string())
            }
        }
        ImageSourceClass::Rejected(error) => Err(error),
        ImageSourceClass::Local(source) => {
            let resolution = app
                .state::<Mutex<DocumentRegistry>>()
                .lock()
                .expect("document registry")
                .local_image_resolution(&document_id, &source)?;
            if let Some(url) = resolution.cached_url {
                return Ok(ResolvedImageSource { url });
            }

            let LocalImageResolution { identity, root, .. } = resolution;
            let preparation_identity = identity.clone();
            let prepared = tauri::async_runtime::spawn_blocking(move || {
                prepare_local_image(&root, &preparation_identity)
            })
            .await
            .map_err(|_| "IMAGE_READ_FAILED".to_string())??;
            app.state::<Mutex<DocumentRegistry>>()
                .lock()
                .expect("document registry")
                .store_prepared_image(&document_id, identity, prepared)
        }
        ImageSourceClass::Data(source) => {
            let (identity, prepared) =
                tauri::async_runtime::spawn_blocking(move || prepare_data_image(&source))
                    .await
                    .map_err(|_| "IMAGE_DATA_INVALID".to_string())??;
            app.state::<Mutex<DocumentRegistry>>()
                .lock()
                .expect("document registry")
                .store_prepared_image(&document_id, identity, prepared)
        }
    }
}

fn normalize_external_url(url: &str) -> Result<String, String> {
    let value = url.trim();
    let lowercase = value.to_ascii_lowercase();
    let is_http = lowercase.starts_with("http://") || lowercase.starts_with("https://");
    let authority = value.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    let host = authority.split(['/', '?', '#']).next().unwrap_or("");
    if !is_http || host.is_empty() || value.chars().any(char::is_whitespace) || value.contains('\\')
    {
        return Err("EXTERNAL_URL_REJECTED".to_string());
    }
    Ok(value.to_string())
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    let url = normalize_external_url(&url)?;
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg(url)
            .status()
            .map_err(|_| "EXTERNAL_OPEN_FAILED".to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err("EXTERNAL_OPEN_FAILED".to_string())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("EXTERNAL_OPENER_UNSUPPORTED".to_string())
    }
}

#[tauri::command]
fn close_document(document_id: String, registry: State<'_, Mutex<DocumentRegistry>>) {
    stop_watching_file(&document_id);
    registry
        .lock()
        .expect("document registry")
        .close_document(&document_id);
}

fn start_watching_file(app: &AppHandle, document_id: String, path: PathBuf) -> Result<(), String> {
    let app = app.clone();
    let target_path = path.clone();
    let watcher_document_id = document_id.clone();
    let mut watcher = notify::recommended_watcher(move |event: Result<notify::Event, _>| {
        if event.is_err() {
            let _ = app.emit(
                "document-error",
                DocumentErrorEvent {
                    document_id: Some(watcher_document_id.clone()),
                    code: "DOCUMENT_WATCH_FAILED".to_string(),
                },
            );
            return;
        }
        if let Ok(event) = event {
            let relevant = event.paths.iter().any(|candidate| {
                candidate == &target_path
                    || candidate.file_name().is_some()
                        && candidate.file_name() == target_path.file_name()
            });
            if relevant
                && matches!(
                    event.kind,
                    EventKind::Modify(ModifyKind::Data(_))
                        | EventKind::Modify(ModifyKind::Name(_))
                        | EventKind::Create(_)
                        | EventKind::Remove(_)
                )
            {
                let source_revision = app
                    .state::<Mutex<DocumentRegistry>>()
                    .lock()
                    .ok()
                    .and_then(|mut registry| registry.mark_file_changed(&watcher_document_id).ok());
                if let Some(source_revision) = source_revision {
                    let _ = app.emit(
                        "file-changed",
                        FileChangedEvent {
                            document_id: watcher_document_id.clone(),
                            source_revision,
                        },
                    );
                }
            }
        }
    })
    .map_err(|_| "DOCUMENT_WATCH_FAILED".to_string())?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "DOCUMENT_WATCH_FAILED".to_string())?;
    watcher
        .watch(parent, RecursiveMode::NonRecursive)
        .map_err(|_| "DOCUMENT_WATCH_FAILED".to_string())?;
    FILE_WATCHERS
        .lock()
        .map_err(|_| "DOCUMENT_WATCH_FAILED".to_string())?
        .insert(document_id, watcher);
    Ok(())
}

fn stop_watching_file(document_id: &str) {
    FILE_WATCHERS
        .lock()
        .expect("file watchers")
        .remove(document_id);
}

#[tauri::command]
fn start_document_watch(
    app: AppHandle,
    document_id: String,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    let path = registry
        .lock()
        .expect("document registry")
        .documents
        .get(&document_id)
        .map(|context| context.path.clone())
        .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
    start_watching_file(&app, document_id, path)
}

#[tauri::command]
fn stop_document_watch() {
    FILE_WATCHERS.lock().expect("file watchers").clear();
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod local_image_tests {
    use super::*;

    #[test]
    fn import_coordinator_bounds_queue_active_work_and_source_reservations() {
        let mut registry = DocumentRegistry::default();
        for sequence in 0..MAX_IMPORT_OPERATIONS {
            registry
                .enqueue_import(ImportJob::for_test(sequence as u64))
                .expect("operation within queue bound");
        }
        assert!(matches!(
            registry.enqueue_import(ImportJob::for_test(MAX_IMPORT_OPERATIONS as u64)),
            Err(error) if error == "DOCUMENT_TAB_LIMIT"
        ));

        let active = (0..MAX_ACTIVE_IMPORTS)
            .map(|_| registry.start_next_import().expect("active import slot"))
            .collect::<Vec<_>>();
        assert!(registry.start_next_import().is_none());
        assert_eq!(registry.active_imports, MAX_ACTIVE_IMPORTS);
        assert_eq!(
            registry.import_reserved_bytes,
            MAX_ACTIVE_IMPORTS * MAX_DOCUMENT_INPUT_BYTES
        );

        for _job in active {
            registry.finish_import_slot();
        }
        assert_eq!(registry.active_imports, 0);
        assert_eq!(registry.import_reserved_bytes, 0);
    }

    #[test]
    fn import_coordinator_releases_failed_slots_before_admitting_the_next_job() {
        let mut registry = DocumentRegistry::default();
        let active = (0..MAX_ACTIVE_IMPORTS)
            .map(|sequence| {
                registry
                    .enqueue_import(ImportJob::for_test(sequence as u64))
                    .expect("initial job is admitted");
                registry.start_next_import().expect("initial job starts")
            })
            .collect::<Vec<_>>();

        for (index, code) in [
            "DOCUMENT_OPEN_FAILED",
            "DOCUMENT_TAB_LIMIT",
            "DOCUMENT_OPEN_FAILED",
            "DOCUMENT_OPEN_FAILED",
        ]
        .iter()
        .enumerate()
        {
            let failed = &active[index];
            let terminal = PendingImport::error(failed.operation.clone(), (*code).to_string());
            assert_eq!(terminal.kind, "error");
            assert_eq!(terminal.code.as_deref(), Some(*code));

            let replacement_sequence = (MAX_ACTIVE_IMPORTS + index) as u64;
            registry
                .enqueue_import(ImportJob::for_test(replacement_sequence))
                .expect("replacement waits for the freed slot");
            registry.finish_import_slot();
            let replacement = registry
                .start_next_import()
                .expect("failed work releases a slot for the next job");
            assert_eq!(replacement.operation.sequence, replacement_sequence);
            assert_eq!(registry.active_imports, MAX_ACTIVE_IMPORTS);
            assert_eq!(
                registry.import_reserved_bytes,
                MAX_ACTIVE_IMPORTS * MAX_DOCUMENT_INPUT_BYTES
            );
        }

        for _ in 0..MAX_ACTIVE_IMPORTS {
            registry.finish_import_slot();
        }
        assert_eq!(registry.active_imports, 0);
        assert_eq!(registry.import_reserved_bytes, 0);
    }

    #[test]
    fn taking_a_pending_error_marks_the_frontend_ready_without_retaining_a_context() {
        let mut registry = DocumentRegistry::default();
        registry.queue_pending_import(PendingImport::error(
            DocumentImportOperation::for_test("failed-startup", 1, "failed.md"),
            "DOCUMENT_OPEN_FAILED".to_string(),
        ));

        let pending = registry
            .take_pending_import()
            .expect("pending error is returned once");
        assert!(registry.frontend_ready);
        assert_eq!(pending.kind, "error");
        assert_eq!(pending.code.as_deref(), Some("DOCUMENT_OPEN_FAILED"));
        assert!(pending.document.is_none());
        assert!(registry.pending_import.is_none());
        assert!(registry.documents.is_empty());
    }

    #[test]
    fn import_file_labels_never_expose_directories_or_control_characters() {
        let path = PathBuf::from("/private/customer/\u{202e} report\n\t.md");
        let label = safe_file_label(&path);
        assert_eq!(label, "report .md");
        assert!(!label.contains("private"));
        assert!(label.chars().count() <= 120);
    }

    #[test]
    fn import_and_watcher_errors_keep_distinct_public_event_schemas() {
        let import = serde_json::to_value(DocumentImportErrorEvent {
            operation_id: "operation".to_string(),
            sequence: 7,
            code: "DOCUMENT_OPEN_FAILED".to_string(),
        })
        .expect("serialize import error");
        let watcher = serde_json::to_value(DocumentErrorEvent {
            document_id: Some("document".to_string()),
            code: "DOCUMENT_READ_FAILED".to_string(),
        })
        .expect("serialize watcher error");

        assert_eq!(
            import.get("operationId").and_then(|value| value.as_str()),
            Some("operation")
        );
        assert!(import.get("documentId").is_none());
        assert_eq!(
            watcher.get("documentId").and_then(|value| value.as_str()),
            Some("document")
        );
        assert!(watcher.get("operationId").is_none());
    }

    #[test]
    fn rejects_escaped_and_nonlocal_paths() {
        assert_eq!(
            normalize_relative_source("../outside.png"),
            Err("IMAGE_PATH_TRAVERSAL".to_string())
        );
        assert_eq!(
            normalize_relative_source("assets/%2e%2e/outside.png"),
            Err("IMAGE_PATH_TRAVERSAL".to_string())
        );
        assert!(matches!(
            classify_image_source("file:///tmp/outside.png"),
            ImageSourceClass::Rejected(_)
        ));
    }
    #[test]
    fn accepts_only_decodable_allowlisted_data_images() {
        assert!(validate_data_image(
            "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4="
        )
        .is_ok());
        assert!(validate_data_image("data:text/html;base64,PGgxPg==").is_err());
    }

    #[test]
    fn snapshots_data_svg_as_png_without_reprocessing_the_rasterized_bytes() {
        let mut registry = DocumentRegistry::default();
        registry.documents.insert(
            "doc".to_string(),
            DocumentContext {
                path: PathBuf::new(),
                root: PathBuf::new(),
                revision: 0,
                resources: HashMap::new(),
                resource_bytes: 0,
                creator_operation_id: "test".to_string(),
                published: true,
                claims: HashSet::new(),
                pending_owner_count: 0,
            },
        );
        let source = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxIiBoZWlnaHQ9IjEiPjxyZWN0IHdpZHRoPSIxIiBoZWlnaHQ9IjEiLz48L3N2Zz4=";
        let resolved = registry
            .resolve_image("doc", source, false)
            .expect("data SVG should be snapshotted");
        let path = resolved
            .url
            .strip_prefix("mdreader-image://localhost")
            .expect("protocol path");
        let served = serve_image(&registry, path);
        assert_eq!(served.status(), http::StatusCode::OK);
        assert_eq!(served.headers()[http::header::CONTENT_TYPE], "image/png");
        assert!(served.body().starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn remote_images_are_blocked_until_the_document_grants_live_tab_access() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-remote-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document = directory.join("document.md");
        fs::write(&document, "![remote](https://example.test/image.png)").expect("test markdown");

        let mut registry = DocumentRegistry::default();
        let payload = registry.open_document(document).expect("open document");
        assert!(matches!(
            registry.resolve_image(&payload.document_id, "https://example.test/image.png", false),
            Err(error) if error == "IMAGE_REMOTE_BLOCKED"
        ));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn unsafe_svg_content_is_not_accepted_as_a_renderable_image() {
        assert!(!validate_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#
        ));
        assert!(!validate_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://evil.test/a.png" /></svg>"#
        ));
        assert!(!validate_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><feImage href="/private/secret.png" /></svg>"#
        ));
        assert!(!validate_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><svg><path d="M0 0" /></svg></svg>"#
        ));
        assert!(!validate_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><path style="fill:url(https://evil.test/a.svg)" /></svg>"#
        ));
    }

    #[test]
    fn accepts_svg_with_local_fragment_paint_references() {
        assert!(validate_svg(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20"><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="4" markerHeight="4" orient="auto"><path d="M0 0 L10 5 L0 10 Z" /></marker><filter id="shadow"><feDropShadow dx="0" dy="1" stdDeviation="1" /></filter></defs><path d="M1 10 L18 10" marker-end="url(#arrow)" filter="url(#shadow)" /></svg>"##
        ));
    }

    #[test]
    fn safe_svg_rasterization_reuses_one_available_system_font_database() {
        let first = safe_svg_options();
        let second = safe_svg_options();
        assert!(first.fontdb.faces().next().is_some());
        assert!(Arc::ptr_eq(&first.fontdb, &second.fontdb));
    }

    #[test]
    fn rejects_documents_over_the_input_size_budget_before_reading_them() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-large-document-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document = directory.join("large.md");
        fs::write(&document, vec![b'x'; 10 * 1024 * 1024 + 1]).expect("large markdown");

        let mut registry = DocumentRegistry::default();
        assert!(matches!(
            registry.open_document(document),
            Err(error) if error == "DOCUMENT_TOO_LARGE"
        ));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_documents_even_when_the_target_is_supported() {
        use std::os::unix::fs::symlink;
        let directory =
            std::env::temp_dir().join(format!("mdreader-supported-symlink-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let target = directory.join("target.md");
        let link = directory.join("linked.md");
        fs::write(&target, "# target").expect("markdown target");
        symlink(&target, &link).expect("markdown symlink");

        let mut registry = DocumentRegistry::default();
        assert!(matches!(
            registry.open_document(link),
            Err(error) if error == "DOCUMENT_IDENTITY_CHANGED"
        ));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn rejects_svg_dimensions_above_the_edge_limit() {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="1"><path d="M0 0" /></svg>"#,
            16_385
        );
        assert!(!validate_svg(svg.as_bytes()));
    }

    #[test]
    fn opening_the_same_canonical_path_reuses_its_document_identity() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-dedup-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document = directory.join("document.md");
        fs::write(&document, "# same").expect("test markdown");

        let mut registry = DocumentRegistry::default();
        let first = registry
            .open_document(document.clone())
            .expect("first open");
        let second = registry.open_document(document).expect("second open");
        assert_eq!(first.document_id, second.document_id);
        assert_eq!(registry.documents.len(), 1);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn the_last_failed_claim_reaps_an_unpublished_reused_context() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-claim-reap-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document = directory.join("document.md");
        fs::write(&document, "# claims").expect("test markdown");

        let mut registry = DocumentRegistry::default();
        let created = registry
            .commit_prepared(
                prepare_document(document.clone()).expect("prepare creator"),
                "creator",
            )
            .expect("create claim");
        let reused = registry
            .commit_prepared(prepare_document(document).expect("prepare reuse"), "reuser")
            .expect("reuse claim");
        assert_eq!(created.document_id, reused.document_id);

        registry.release_claim(&created.document_id, "creator");
        assert!(registry.documents.contains_key(&created.document_id));
        registry.release_claim(&reused.document_id, "reuser");
        assert!(!registry.documents.contains_key(&created.document_id));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn replacing_pending_with_a_reused_claim_keeps_the_shared_context_alive() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-pending-reuse-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document = directory.join("document.md");
        fs::write(&document, "# pending reuse").expect("test markdown");
        let mut registry = DocumentRegistry::default();

        let creator_operation = DocumentImportOperation::for_test("creator", 1, "document.md");
        let created = registry
            .commit_prepared(
                prepare_document(document.clone()).expect("prepare creator"),
                &creator_operation.operation_id,
            )
            .expect("create claim");
        registry.queue_pending_import(PendingImport::opened(creator_operation, created.clone()));

        let reuse_operation = DocumentImportOperation::for_test("reuser", 2, "document.md");
        let reused = registry
            .commit_prepared(
                prepare_document(document).expect("prepare reuse"),
                &reuse_operation.operation_id,
            )
            .expect("reuse claim");
        registry.queue_pending_import(PendingImport::opened(reuse_operation, reused));

        assert!(registry.documents.contains_key(&created.document_id));
        assert_eq!(
            registry
                .pending_import
                .as_ref()
                .and_then(PendingImport::document_id),
            Some(created.document_id.as_str())
        );
        registry.take_pending_import();
        assert!(registry
            .documents
            .get(&created.document_id)
            .is_some_and(|context| context.published));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn enforces_the_open_document_limit() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-tab-limit-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let mut registry = DocumentRegistry::default();
        let first_path = directory.join("document-0.md");
        let mut first_document_id = String::new();
        for index in 0..MAX_OPEN_DOCUMENTS {
            let path = directory.join(format!("document-{index}.md"));
            fs::write(&path, "# document").expect("test markdown");
            let opened = registry.open_document(path).expect("within tab limit");
            if index == 0 {
                first_document_id = opened.document_id;
            }
        }
        let duplicate = registry
            .open_document(first_path)
            .expect("duplicate remains valid at unique-context capacity");
        assert_eq!(duplicate.document_id, first_document_id);
        let rejected = directory.join("document-over-limit.md");
        fs::write(&rejected, "# document").expect("test markdown");
        assert!(matches!(
            registry.open_document(rejected),
            Err(error) if error == "DOCUMENT_TAB_LIMIT"
        ));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn concurrent_duplicate_commits_reuse_one_of_thirty_one_contexts() {
        let directory = std::env::temp_dir().join(format!(
            "mdreader-concurrent-duplicate-test-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("test directory");
        let mut registry = DocumentRegistry::default();
        let duplicate_path = directory.join("document-0.md");
        let mut original_id = String::new();
        for index in 0..31 {
            let path = directory.join(format!("document-{index}.md"));
            fs::write(&path, "# document").expect("test markdown");
            let opened = registry.open_document(path).expect("within tab limit");
            if index == 0 {
                original_id = opened.document_id;
            }
        }

        let first_prepared = prepare_document(duplicate_path.clone()).expect("first prepare");
        let second_prepared = prepare_document(duplicate_path).expect("second prepare");
        let first = registry
            .commit_prepared(first_prepared, "duplicate-one")
            .expect("first duplicate claim");
        let second = registry
            .commit_prepared(second_prepared, "duplicate-two")
            .expect("second duplicate claim");
        assert_eq!(first.document_id, original_id);
        assert_eq!(second.document_id, original_id);
        assert_eq!(registry.documents.len(), 31);
        registry.publish_claim(&first.document_id, "duplicate-one");
        registry.publish_claim(&second.document_id, "duplicate-two");
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn file_change_revision_is_monotonic_and_reload_uses_observed_revision() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-revision-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        fs::write(&document_path, "# first").expect("test markdown");

        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path.clone())
            .expect("open document");
        fs::write(&document_path, "# second").expect("updated markdown");
        let changed_revision = registry
            .mark_file_changed(&document.document_id)
            .expect("mark file changed");
        let reloaded = registry
            .reload_document(&document.document_id, Some(changed_revision))
            .expect("reload document");
        assert_eq!(reloaded.source_revision, changed_revision);
        assert_eq!(reloaded.content, "# second");
        let manually_reloaded = registry
            .reload_document(&document.document_id, None)
            .expect("manual reload document");
        assert_eq!(manually_reloaded.source_revision, changed_revision + 1);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn external_opener_accepts_only_http_and_https_urls() {
        assert_eq!(
            normalize_external_url("https://example.test/docs?q=1"),
            Ok("https://example.test/docs?q=1".to_string())
        );
        assert_eq!(
            normalize_external_url("http://example.test"),
            Ok("http://example.test".to_string())
        );
        for rejected in [
            "javascript:alert(1)",
            "file:///tmp/secret",
            "mailto:a@b.test",
            "https://",
        ] {
            assert!(normalize_external_url(rejected).is_err(), "{rejected}");
        }
    }
    #[test]
    fn parses_only_well_formed_protocol_paths() {
        assert_eq!(
            parse_protocol_path("/doc/resource"),
            Some(("doc".to_string(), "resource".to_string()))
        );
        assert_eq!(parse_protocol_path("/doc/%2e%2e"), None);
    }
    #[test]
    fn validates_complete_image_contents_not_just_magic() {
        let png = b"\x89PNG\r\n\x1a\nvalid";
        assert_eq!(
            validate_image_bytes("png", png),
            Err("IMAGE_TYPE_MISMATCH".to_string())
        );
        assert_eq!(
            validate_image_bytes("jpg", png),
            Err("IMAGE_TYPE_MISMATCH".to_string())
        );
    }
    #[test]
    fn closes_resources_idempotently() {
        let mut registry = DocumentRegistry::default();
        registry.documents.insert(
            "doc".to_string(),
            DocumentContext {
                path: PathBuf::new(),
                root: PathBuf::new(),
                revision: 0,
                resources: HashMap::new(),
                resource_bytes: 0,
                creator_operation_id: "test".to_string(),
                published: true,
                claims: HashSet::new(),
                pending_owner_count: 0,
            },
        );
        registry.close_document("doc");
        registry.close_document("doc");
        assert!(registry.documents.is_empty());
    }

    #[test]
    fn derives_the_pdf_file_name_from_the_original_markdown_file_name() {
        let mut registry = DocumentRegistry::default();
        registry.documents.insert(
            "current".to_string(),
            DocumentContext {
                path: PathBuf::from("季度.汇报.markdown"),
                root: PathBuf::new(),
                revision: 0,
                resources: HashMap::new(),
                resource_bytes: 0,
                creator_operation_id: "test".to_string(),
                published: true,
                claims: HashSet::new(),
                pending_owner_count: 0,
            },
        );
        assert_eq!(
            registry
                .suggested_pdf_file_name_for_document("current")
                .as_deref(),
            Some("季度.汇报.pdf")
        );
        assert_eq!(
            registry.suggested_pdf_file_name_for_document("missing"),
            None
        );
        assert_eq!(suggested_pdf_file_name("notes.md"), "notes.pdf");
    }
    #[test]
    fn a_replaced_pending_payload_is_discarded_with_its_context() {
        let mut registry = DocumentRegistry::default();
        for id in ["first", "second"] {
            registry.documents.insert(
                id.to_string(),
                DocumentContext {
                    path: PathBuf::new(),
                    root: PathBuf::new(),
                    revision: 0,
                    resources: HashMap::new(),
                    resource_bytes: 0,
                    creator_operation_id: id.to_string(),
                    published: false,
                    claims: HashSet::new(),
                    pending_owner_count: 0,
                },
            );
        }
        registry.queue_pending_import(PendingImport::opened(
            DocumentImportOperation::for_test("first", 1, "first.md"),
            DocumentPayload {
                document_id: "first".to_string(),
                file_name: "first.md".to_string(),
                content: String::new(),
                source_revision: 0,
            },
        ));
        registry.queue_pending_import(PendingImport::opened(
            DocumentImportOperation::for_test("second", 2, "second.md"),
            DocumentPayload {
                document_id: "second".to_string(),
                file_name: "second.md".to_string(),
                content: String::new(),
                source_revision: 0,
            },
        ));
        assert!(!registry.documents.contains_key("first"));
        assert_eq!(
            registry
                .pending_import
                .as_ref()
                .and_then(PendingImport::document_id),
            Some("second")
        );
    }
    #[test]
    fn snapshots_a_local_image_and_revokes_its_protocol_url_on_close() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-image-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        let image_path = directory.join("image.svg");
        fs::write(&document_path, "![local](image.svg)").expect("test markdown");
        fs::write(
            &image_path,
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        )
        .expect("test image");

        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path)
            .expect("open document");
        let resolved = registry
            .resolve_image(&document.document_id, "image.svg", false)
            .expect("snapshot image");
        assert!(
            matches!(registry.resolve_image(&document.document_id, "../image.svg", false), Err(error) if error == "IMAGE_PATH_TRAVERSAL")
        );
        let path = resolved
            .url
            .strip_prefix("mdreader-image://localhost")
            .expect("protocol path");
        let served = serve_image(&registry, path);
        assert_eq!(served.status(), http::StatusCode::OK);
        assert_eq!(served.headers()[http::header::CACHE_CONTROL], "no-store");
        assert_eq!(served.headers()[http::header::CONTENT_TYPE], "image/png");
        assert!(served.body().starts_with(b"\x89PNG\r\n\x1a\n"));

        registry.close_document(&document.document_id);
        assert_eq!(
            serve_image(&registry, path).status(),
            http::StatusCode::NOT_FOUND
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn prepares_a_local_image_before_relocking_the_registry_to_commit_it() {
        let directory = std::env::temp_dir().join(format!(
            "mdreader-image-preparation-test-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        let image_path = directory.join("image.svg");
        fs::write(&document_path, "![local](image.svg)").expect("test markdown");
        fs::write(
            &image_path,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><rect width=\"1\" height=\"1\" /></svg>",
        )
        .expect("test image");

        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path)
            .expect("open document");
        let resolution = registry
            .local_image_resolution(&document.document_id, "image.svg")
            .expect("resolve local image");
        assert!(resolution.cached_url.is_none());

        let prepared = prepare_local_image(&resolution.root, &resolution.identity)
            .expect("prepare image without the registry");
        assert_eq!(registry.total_resources, 0);
        let resolved = registry
            .store_prepared_image(&document.document_id, resolution.identity, prepared)
            .expect("commit prepared image");

        assert!(resolved.url.starts_with("mdreader-image://localhost/"));
        assert_eq!(registry.total_resources, 1);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn reload_releases_resource_snapshots_from_the_previous_revision() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-reload-resource-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        let image_path = directory.join("image.svg");
        fs::write(&document_path, "![local](image.svg)").expect("test markdown");
        fs::write(
            &image_path,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><rect width=\"1\" height=\"1\" /></svg>",
        )
        .expect("test image");

        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path.clone())
            .expect("open document");
        let resolved = registry
            .resolve_image(&document.document_id, "image.svg", false)
            .expect("snapshot image");
        let protocol_path = resolved
            .url
            .strip_prefix("mdreader-image://localhost")
            .expect("protocol path")
            .to_string();
        assert_eq!(registry.total_resources, 1);

        fs::write(&document_path, "# refreshed").expect("updated markdown");
        registry
            .reload_document(&document.document_id, None)
            .expect("reload document");

        assert_eq!(registry.total_resources, 0);
        assert_eq!(registry.total_bytes, 0);
        assert_eq!(
            serve_image(&registry, &protocol_path).status(),
            http::StatusCode::NOT_FOUND
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_documents_and_images() {
        use std::os::unix::fs::symlink;
        let directory =
            std::env::temp_dir().join(format!("mdreader-symlink-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let real_text = directory.join("real.unsupported");
        let outside_image = directory.join("outside.svg");
        let document_path = directory.join("document.md");
        fs::write(&real_text, "not markdown by extension").expect("test text");
        fs::write(
            &outside_image,
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        )
        .expect("test image");
        symlink(&real_text, directory.join("pretend.md")).expect("document symlink");
        fs::write(&document_path, "![local](linked.svg)").expect("test markdown");
        symlink(&outside_image, directory.join("linked.svg")).expect("image symlink");

        let mut registry = DocumentRegistry::default();
        assert!(
            matches!(registry.open_document(directory.join("pretend.md")), Err(error) if error == "DOCUMENT_IDENTITY_CHANGED")
        );
        let document = registry
            .open_document(document_path)
            .expect("open document");
        assert!(
            matches!(registry.resolve_image(&document.document_id, "linked.svg", false), Err(error) if error == "IMAGE_OPEN_FAILED")
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_atomic_document_replacement_with_a_symlink() {
        use std::os::unix::fs::symlink;
        let directory =
            std::env::temp_dir().join(format!("mdreader-replaced-document-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        let outside_path = directory.join("outside.md");
        fs::write(&document_path, "# original").expect("original markdown");
        fs::write(&outside_path, "# outside").expect("outside markdown");

        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path.clone())
            .expect("open document");
        fs::remove_file(&document_path).expect("remove original");
        symlink(&outside_path, &document_path).expect("replace with symlink");

        assert!(matches!(
            registry.reload_document(&document.document_id, None),
            Err(error) if error == "DOCUMENT_IDENTITY_CHANGED"
        ));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn enforces_resource_counts_and_releases_them_when_a_document_closes() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-quota-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        for index in 0..=MAX_DOCUMENT_RESOURCES {
            fs::write(
                directory.join(format!("image-{index}.svg")),
                "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
            )
            .expect("test image");
        }
        let mut registry = DocumentRegistry::default();
        let mut documents = Vec::new();
        for index in 0..5 {
            let path = directory.join(format!("document-{index}.md"));
            fs::write(&path, "# quota").expect("test markdown");
            documents.push(registry.open_document(path).expect("open document"));
        }
        for document in documents.iter().take(4) {
            for image in 0..MAX_DOCUMENT_RESOURCES {
                registry
                    .resolve_image(&document.document_id, &format!("image-{image}.svg"), false)
                    .expect("within quota");
            }
            assert!(
                matches!(registry.resolve_image(&document.document_id, "image-32.svg", false), Err(error) if error == "IMAGE_RESOURCE_LIMIT")
            );
        }
        assert_eq!(registry.total_resources, MAX_TOTAL_RESOURCES);
        assert!(
            matches!(registry.resolve_image(&documents[4].document_id, "image-0.svg", false), Err(error) if error == "IMAGE_RESOURCE_LIMIT")
        );
        registry.close_document(&documents[0].document_id);
        assert!(registry
            .resolve_image(&documents[4].document_id, "image-0.svg", false)
            .is_ok());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn menu_labels_follow_the_supported_interface_locale() {
        assert_eq!(menu_labels("en").open_document, "Open…");
        assert_eq!(menu_labels("zh-Hant").export_pdf, "匯出為 PDF…");
        assert_eq!(menu_labels("ja").file, "ファイル");
        assert_eq!(menu_labels("unsupported").open_document, "打开…");
    }
}

struct MenuLabels {
    file: &'static str,
    edit: &'static str,
    view: &'static str,
    window: &'static str,
    open_document: &'static str,
    export_pdf: &'static str,
}

fn menu_labels(locale: &str) -> MenuLabels {
    match locale {
        "en" => MenuLabels {
            file: "File",
            edit: "Edit",
            view: "View",
            window: "Window",
            open_document: "Open…",
            export_pdf: "Export as PDF…",
        },
        "zh-Hant" => MenuLabels {
            file: "檔案",
            edit: "編輯",
            view: "顯示",
            window: "視窗",
            open_document: "開啟…",
            export_pdf: "匯出為 PDF…",
        },
        "ja" => MenuLabels {
            file: "ファイル",
            edit: "編集",
            view: "表示",
            window: "ウィンドウ",
            open_document: "開く…",
            export_pdf: "PDFとして書き出す…",
        },
        _ => MenuLabels {
            file: "文件",
            edit: "编辑",
            view: "视图",
            window: "窗口",
            open_document: "打开…",
            export_pdf: "导出为 PDF…",
        },
    }
}

fn setup_app_menu(app: &AppHandle, locale: &str) -> tauri::Result<()> {
    let labels = menu_labels(locale);
    let open_document = MenuItemBuilder::with_id("open-document", labels.open_document)
        .accelerator("CmdOrCtrl+O")
        .build(app)?;
    let export_current_pdf = MenuItemBuilder::with_id("export-current-pdf", labels.export_pdf)
        .accelerator("CmdOrCtrl+P")
        .build(app)?;

    let application_menu = SubmenuBuilder::new(app, "屿阅")
        .about(None)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file_menu = SubmenuBuilder::new(app, labels.file)
        .item(&open_document)
        .item(&export_current_pdf)
        .separator()
        .close_window()
        .build()?;
    let edit_menu = SubmenuBuilder::new(app, labels.edit)
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view_menu = SubmenuBuilder::new(app, labels.view).fullscreen().build()?;
    let window_menu = SubmenuBuilder::new(app, labels.window).minimize().build()?;
    let menu = MenuBuilder::new(app)
        .item(&application_menu)
        .item(&file_menu)
        .item(&edit_menu)
        .item(&view_menu)
        .item(&window_menu)
        .build()?;

    app.set_menu(menu)?;
    Ok(())
}

#[tauri::command]
fn set_menu_locale(app: AppHandle, locale: String) -> Result<(), String> {
    setup_app_menu(&app, &locale).map_err(|_| "MENU_UPDATE_FAILED".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(DocumentRegistry::default()))
        .register_uri_scheme_protocol("mdreader-image", |context, request| {
            let registry = context.app_handle().state::<Mutex<DocumentRegistry>>();
            let response = serve_image(
                &registry.lock().expect("document registry"),
                request.uri().path(),
            );
            response
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _| {
            if let Some(path) = argv
                .into_iter()
                .map(PathBuf::from)
                .find(|path| is_supported_file(path))
            {
                publish_document(app, path);
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            setup_app_menu(app.handle(), "zh-Hans")?;
            if let Some(path) = std::env::args()
                .skip(1)
                .map(PathBuf::from)
                .find(|path| is_supported_file(path))
            {
                publish_document(app.handle(), path);
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == "open-document" {
                if let Some(window) = app.get_webview_window("main") {
                    open_document_dialog(window);
                }
            } else if event.id() == "export-current-pdf" {
                request_print_export(app);
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                if let Some(path) = paths.iter().find(|path| is_supported_file(path)) {
                    publish_document(window.app_handle(), path.clone());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_document_dialog,
            export_current_pdf,
            set_menu_locale,
            export_png_dialog,
            open_external_url,
            take_pending_import,
            reload_document,
            resolve_image_source,
            close_document,
            start_document_watch,
            stop_document_watch
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Opened { urls } = event {
                if let Some(path) = urls
                    .into_iter()
                    .find_map(|url| url.to_file_path().ok())
                    .filter(|path| is_supported_file(path))
                {
                    publish_document(app, path);
                }
            }
        });
}
