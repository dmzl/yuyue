use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, LazyLock, Mutex,
    },
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{ImageFormat, ImageReader, Limits};
use notify::{event::ModifyKind, EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{
    http,
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    AppHandle, Emitter, Manager, State, WebviewWindow,
};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

#[cfg(all(target_os = "macos", not(test)))]
use block2::StackBlock;

#[cfg(target_os = "macos")]
use objc2_foundation::{NSCopying, NSString};

#[cfg(all(target_os = "macos", not(test)))]
use objc2::AnyThread;

#[cfg(all(target_os = "macos", not(test)))]
use objc2_foundation::{NSError, NSFileCoordinator, NSFileCoordinatorWritingOptions, NSURL};

#[cfg(unix)]
use std::os::unix::{
    ffi::OsStrExt,
    fs::MetadataExt,
    io::{AsRawFd, FromRawFd},
};

#[cfg(unix)]
use std::path::Component;

#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
};

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
const MAX_WRITE_RAW_CHUNK_BYTES: usize = 64 * 1024;
const MAX_WRITE_UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_WRITE_IO_BYTES: usize = 48 * 1024 * 1024;
const MAX_CLIPBOARD_INGRESS_BYTES: usize = 24 * 1024 * 1024;
const MAX_METADATA_VERIFY_BYTES: usize = 16 * 1024 * 1024;
const MAX_SELF_WRITE_LEDGER: usize = 64;
const MAX_IMAGE_SOURCE_GRANTS: usize = 64;
const WRITE_UPLOAD_TTL: Duration = Duration::from_secs(15);

#[cfg(target_os = "macos")]
const RENAME_NOFOLLOW_ANY: libc::c_uint = 0x0000_0010;
#[cfg(target_os = "macos")]
const RENAME_RESOLVE_BENEATH: libc::c_uint = 0x0000_0020;
const PRINT_MARGIN_TOP_POINTS: f64 = 51.0;
const PRINT_MARGIN_HORIZONTAL_POINTS: f64 = 45.0;
const PRINT_MARGIN_BOTTOM_POINTS: f64 = 56.7;

static FILE_WATCHERS: LazyLock<Mutex<HashMap<String, notify::RecommendedWatcher>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
struct ProvisionalWatcher {
    watcher: notify::RecommendedWatcher,
    activation: Arc<Mutex<Option<(AppHandle, String)>>>,
    dirty: Arc<AtomicBool>,
}
static PROVISIONAL_WATCHERS: LazyLock<Mutex<HashMap<String, ProvisionalWatcher>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Default)]
struct WatchReadGate {
    active: usize,
    reserved_bytes: usize,
    scheduled_documents: HashSet<String>,
    pending_documents: HashSet<String>,
}
static WATCH_READ_GATE: LazyLock<(Mutex<WatchReadGate>, Condvar)> =
    LazyLock::new(|| (Mutex::new(WatchReadGate::default()), Condvar::new()));
const APP_EXIT_ATTEMPT_TTL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
struct AppExitAttempt {
    id: String,
    expires_at: Instant,
}

#[derive(Debug, Default)]
struct AppExitAttemptGate {
    current: Option<AppExitAttempt>,
}

impl AppExitAttemptGate {
    fn begin(&mut self, now: Instant) -> Result<AppExitAttempt, String> {
        if self
            .current
            .as_ref()
            .is_some_and(|attempt| attempt.expires_at > now)
        {
            return Err("APP_EXIT_ALREADY_PENDING".to_string());
        }
        let attempt = AppExitAttempt {
            id: Uuid::new_v4().simple().to_string(),
            expires_at: now + APP_EXIT_ATTEMPT_TTL,
        };
        self.current = Some(attempt.clone());
        Ok(attempt)
    }

    fn allows_current_attempt(&mut self, now: Instant) -> bool {
        self.current
            .as_ref()
            .is_some_and(|attempt| attempt.expires_at > now)
    }

    fn expire_if_matches(&mut self, attempt_id: &str) -> bool {
        if self
            .current
            .as_ref()
            .is_some_and(|attempt| attempt.id == attempt_id)
        {
            self.current = None;
            return true;
        }
        false
    }
}

static APP_EXIT_ATTEMPT: LazyLock<Mutex<AppExitAttemptGate>> =
    LazyLock::new(|| Mutex::new(AppExitAttemptGate::default()));

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

type CopiedImage = (PathBuf, String, FileIdentity, String, String);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FileFingerprint {
    identity: FileIdentity,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    sha256: [u8; 32],
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FileMetadataSignature {
    uid: u32,
    gid: u32,
    mode: u32,
    created_seconds: i64,
    created_nanoseconds: i64,
    acl: Vec<u8>,
    xattrs: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Debug, Clone)]
struct SelfWriteIntent {
    context_epoch: u64,
    commit_id: String,
    before: FileFingerprint,
    after: FileFingerprint,
    source_revision: u64,
    created_at: Instant,
}

#[derive(Debug, Clone)]
struct ConflictToken {
    token: String,
    context_epoch: u64,
    revision: u64,
    fingerprint: FileFingerprint,
    expires_at: Instant,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReserveDocumentWriteRequest {
    document_id: String,
    context_epoch: u64,
    commit_id: String,
    write_generation: u64,
    content_utf8_bytes: usize,
    #[serde(default)]
    conflict_token: Option<String>,
    #[serde(default)]
    save_as_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareRecoverySaveRequest {
    document_id: String,
    context_epoch: u64,
    recovery_event_id: String,
    record_id: String,
    action_token: String,
    commit_id: String,
    write_generation: u64,
    content_utf8_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WriteUploadReservation {
    upload_id: String,
    token: String,
    expires_in_ms: u64,
    context_epoch: u64,
}

#[derive(Debug, Clone)]
struct SaveAsPreparation {
    token: String,
    window_label: String,
    document_id: String,
    old_context_epoch: u64,
    next_context_epoch: u64,
    target_path: PathBuf,
    replacing_existing: bool,
    expected_target_fingerprint: Option<FileFingerprint>,
    expected_target_metadata: Option<FileMetadataSignature>,
    expires_at: Instant,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveAsPrepared {
    token: String,
    next_context_epoch: u64,
    file_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryRecordSummary {
    record_id: String,
    ordinal: usize,
    action: String,
    action_token: String,
    action_completed: bool,
    acknowledged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    ack_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    saved_generation: Option<u64>,
}

#[derive(Debug, Clone)]
struct RecoveryRecord {
    recovery_event_id: String,
    window_label: String,
    document_id: String,
    context_epoch: u64,
    ordinal: usize,
    path: PathBuf,
    action: String,
    action_token: String,
    action_completed: bool,
    acknowledged: bool,
    ack_token: Option<String>,
    saved_generation: Option<u64>,
    expires_at: Instant,
}

fn recovery_record_summary(record_id: &str, record: &RecoveryRecord) -> RecoveryRecordSummary {
    RecoveryRecordSummary {
        record_id: record_id.to_string(),
        ordinal: record.ordinal,
        action: record.action.clone(),
        action_token: record.action_token.clone(),
        action_completed: record.action_completed,
        acknowledged: record.acknowledged,
        ack_token: record.ack_token.clone(),
        saved_generation: record.saved_generation,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryActionCompleted {
    record_id: String,
    ack_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    saved_generation: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryActionRefreshed {
    action_token: String,
}

#[derive(Debug, Serialize)]
struct RecoveryAcknowledged {
    resolved: bool,
}

#[derive(Debug)]
struct ImageInsertRollback {
    token: String,
    window_label: String,
    document_id: String,
    context_epoch: u64,
    root: PathBuf,
    name: String,
    identity: FileIdentity,
    expires_at: Instant,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageInsertResult {
    markdown_url: String,
    alt: String,
    rollback_token: String,
}

#[derive(Debug)]
struct ImageSourceGrant {
    token: String,
    window_label: String,
    path: PathBuf,
    expires_at: Instant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageSourceGrantEvent {
    token: String,
}

#[derive(Debug)]
struct ClipboardImageUpload {
    upload_id: String,
    token: String,
    window_label: String,
    document_id: String,
    context_epoch: u64,
    extension: String,
    source_stem: String,
    expected_bytes: usize,
    bytes_written: usize,
    next_sequence: u64,
    staging: StagingFile,
    expires_at: Instant,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReserveClipboardImageRequest {
    document_id: String,
    context_epoch: u64,
    extension: String,
    source_stem: String,
    content_bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentEditEligibility {
    eligible: bool,
    context_epoch: u64,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BeginDocumentWriteRequest {
    upload_id: String,
    token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinalizeDocumentWriteRequest {
    upload_id: String,
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
enum DocumentWriteOutcome {
    Saved {
        document_id: String,
        context_epoch: u64,
        source_revision: u64,
        commit_id: String,
        write_generation: u64,
    },
    Conflict {
        document_id: String,
        context_epoch: u64,
        observed_revision: u64,
        conflict_token: String,
    },
    RecoveryRequired {
        document_id: String,
        context_epoch: u64,
        source_revision: u64,
        code: String,
        recovery_event_id: String,
        recovery_records: Vec<RecoveryRecordSummary>,
    },
    ContextRebound {
        document_id: String,
        context_epoch: u64,
        source_revision: u64,
        commit_id: String,
        write_generation: u64,
        file_name: String,
    },
    RecoveryCopySaved {
        record_id: String,
        recovery_event_id: String,
        ack_token: String,
        write_generation: u64,
    },
}

#[derive(Debug)]
struct StagingFile {
    root: PathBuf,
    name: String,
    file: File,
    identity: FileIdentity,
}

#[derive(Debug)]
struct WriteUpload {
    upload_id: String,
    token: String,
    window_label: String,
    document_id: String,
    context_epoch: u64,
    expected_revision: u64,
    expected_fingerprint: FileFingerprint,
    expected_metadata: Option<FileMetadataSignature>,
    commit_id: String,
    write_generation: u64,
    expected_bytes: usize,
    next_sequence: u64,
    bytes_written: usize,
    expires_at: Instant,
    target: WriteTarget,
    staging: Option<StagingFile>,
    hasher: IncrementalSha256,
}

#[derive(Debug, Clone)]
enum WriteTarget {
    Original {
        root: PathBuf,
        name: String,
    },
    SaveAs {
        preparation_token: String,
        root: PathBuf,
        name: String,
        next_context_epoch: u64,
        replacing_existing: bool,
        expected_target_fingerprint: Option<FileFingerprint>,
        expected_target_metadata: Option<FileMetadataSignature>,
    },
    RecoveryCopy {
        recovery_event_id: String,
        record_id: String,
        root: PathBuf,
        name: String,
        replacing_existing: bool,
        expected_target_fingerprint: Option<FileFingerprint>,
        expected_target_metadata: Option<FileMetadataSignature>,
    },
}

#[derive(Debug, Default)]
struct WriteCoordinator {
    uploads: HashMap<String, WriteUpload>,
    active_by_document: HashMap<String, String>,
    active_upload: Option<String>,
    reserved_bytes: usize,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Debug)]
struct CommonCryptoSha256Context {
    count: [u32; 2],
    hash: [u32; 8],
    wbuf: [u32; 16],
}

#[cfg(target_os = "macos")]
// CommonCrypto symbols are exported by libSystem on current macOS SDKs; it is
// not a linkable framework even though the public headers live in a framework-
// shaped include path.
#[link(name = "System")]
unsafe extern "C" {
    fn CC_SHA256_Init(context: *mut CommonCryptoSha256Context) -> i32;
    fn CC_SHA256_Update(
        context: *mut CommonCryptoSha256Context,
        data: *const std::ffi::c_void,
        length: u32,
    ) -> i32;
    fn CC_SHA256_Final(output: *mut u8, context: *mut CommonCryptoSha256Context) -> i32;
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct IncrementalSha256 {
    context: CommonCryptoSha256Context,
}

#[cfg(target_os = "macos")]
impl IncrementalSha256 {
    fn new() -> Result<Self, String> {
        let mut context = CommonCryptoSha256Context {
            count: [0; 2],
            hash: [0; 8],
            wbuf: [0; 16],
        };
        if unsafe { CC_SHA256_Init(&mut context) } != 1 {
            return Err("DOCUMENT_WRITE_FAILED".to_string());
        }
        Ok(Self { context })
    }

    fn update(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > u32::MAX as usize {
            return Err("DOCUMENT_WRITE_TOO_LARGE".to_string());
        }
        if unsafe { CC_SHA256_Update(&mut self.context, bytes.as_ptr().cast(), bytes.len() as u32) }
            != 1
        {
            return Err("DOCUMENT_WRITE_FAILED".to_string());
        }
        Ok(())
    }

    fn finalize(mut self) -> Result<[u8; 32], String> {
        let mut digest = [0; 32];
        if unsafe { CC_SHA256_Final(digest.as_mut_ptr(), &mut self.context) } != 1 {
            return Err("DOCUMENT_WRITE_FAILED".to_string());
        }
        Ok(digest)
    }
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
struct IncrementalSha256;

#[cfg(not(target_os = "macos"))]
impl IncrementalSha256 {
    fn new() -> Result<Self, String> {
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }

    fn update(&mut self, _: &[u8]) -> Result<(), String> {
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }

    fn finalize(self) -> Result<[u8; 32], String> {
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    conflict_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConflictRefresh {
    observed_revision: u64,
    conflict_token: String,
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
    leases: HashSet<String>,
}

#[derive(Debug)]
struct DocumentContext {
    path: PathBuf,
    root: PathBuf,
    context_epoch: u64,
    revision: u64,
    fingerprint: FileFingerprint,
    metadata: Option<FileMetadataSignature>,
    self_write_ledger: VecDeque<SelfWriteIntent>,
    conflict_token: Option<ConflictToken>,
    resources: HashMap<String, ResourceSnapshot>,
    released_render_leases: HashSet<String>,
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
    fingerprint: FileFingerprint,
    metadata: Option<FileMetadataSignature>,
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
    write_coordinator: WriteCoordinator,
    save_as_preparations: HashMap<String, SaveAsPreparation>,
    save_as_path_reservations: HashMap<PathBuf, String>,
    image_insert_rollbacks: HashMap<String, ImageInsertRollback>,
    image_source_grants: HashMap<String, ImageSourceGrant>,
    clipboard_image_uploads: HashMap<String, ClipboardImageUpload>,
    finalizing_documents: HashSet<String>,
    recovery_records: HashMap<String, RecoveryRecord>,
}

impl DocumentRegistry {
    fn register_image_source_grant(&mut self, grant: ImageSourceGrant) -> Result<(), String> {
        self.image_source_grants
            .retain(|_, candidate| candidate.expires_at > Instant::now());
        if self.image_source_grants.len() >= MAX_IMAGE_SOURCE_GRANTS {
            return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
        }
        self.image_source_grants.insert(grant.token.clone(), grant);
        Ok(())
    }

    fn recovery_outcome(
        &mut self,
        upload: &WriteUpload,
        preserved: Vec<PathBuf>,
    ) -> DocumentWriteOutcome {
        let recovery_event_id = Uuid::new_v4().simple().to_string();
        let mut summaries = Vec::new();
        let record_id = Uuid::new_v4().simple().to_string();
        let action_token = Uuid::new_v4().simple().to_string();
        summaries.push(RecoveryRecordSummary {
            record_id: record_id.clone(),
            ordinal: 1,
            action: "saveCurrentBufferCopy".to_string(),
            action_token: action_token.clone(),
            action_completed: false,
            acknowledged: false,
            ack_token: None,
            saved_generation: None,
        });
        self.recovery_records.insert(
            record_id,
            RecoveryRecord {
                recovery_event_id: recovery_event_id.clone(),
                window_label: upload.window_label.clone(),
                document_id: upload.document_id.clone(),
                context_epoch: upload.context_epoch,
                ordinal: 1,
                path: PathBuf::new(),
                action: "saveCurrentBufferCopy".to_string(),
                action_token,
                action_completed: false,
                acknowledged: false,
                ack_token: None,
                saved_generation: None,
                expires_at: Instant::now() + Duration::from_secs(120),
            },
        );
        for (index, path) in preserved.into_iter().enumerate() {
            let record_id = Uuid::new_v4().simple().to_string();
            let action_token = Uuid::new_v4().simple().to_string();
            summaries.push(RecoveryRecordSummary {
                record_id: record_id.clone(),
                ordinal: index + 2,
                action: "revealPreservedItem".to_string(),
                action_token: action_token.clone(),
                action_completed: false,
                acknowledged: false,
                ack_token: None,
                saved_generation: None,
            });
            self.recovery_records.insert(
                record_id.clone(),
                RecoveryRecord {
                    recovery_event_id: recovery_event_id.clone(),
                    window_label: upload.window_label.clone(),
                    document_id: upload.document_id.clone(),
                    context_epoch: upload.context_epoch,
                    ordinal: index + 2,
                    path,
                    action: "revealPreservedItem".to_string(),
                    action_token,
                    action_completed: false,
                    acknowledged: false,
                    ack_token: None,
                    saved_generation: None,
                    expires_at: Instant::now() + Duration::from_secs(120),
                },
            );
        }
        DocumentWriteOutcome::RecoveryRequired {
            document_id: upload.document_id.clone(),
            context_epoch: upload.context_epoch,
            source_revision: upload.expected_revision,
            code: "DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string(),
            recovery_event_id,
            recovery_records: summaries,
        }
    }

    fn extend_recovery_outcome(
        &mut self,
        upload: &WriteUpload,
        recovery_event_id: &str,
        preserved: Vec<PathBuf>,
    ) -> DocumentWriteOutcome {
        let next_ordinal = self
            .recovery_records
            .values()
            .filter(|record| record.recovery_event_id == recovery_event_id)
            .map(|record| record.ordinal)
            .max()
            .unwrap_or(0)
            + 1;
        for (offset, path) in preserved.into_iter().enumerate() {
            let record_id = Uuid::new_v4().simple().to_string();
            self.recovery_records.insert(
                record_id,
                RecoveryRecord {
                    recovery_event_id: recovery_event_id.to_string(),
                    window_label: upload.window_label.clone(),
                    document_id: upload.document_id.clone(),
                    context_epoch: upload.context_epoch,
                    ordinal: next_ordinal + offset,
                    path,
                    action: "revealPreservedItem".to_string(),
                    action_token: Uuid::new_v4().simple().to_string(),
                    action_completed: false,
                    acknowledged: false,
                    ack_token: None,
                    saved_generation: None,
                    expires_at: Instant::now() + Duration::from_secs(120),
                },
            );
        }
        let mut records = self
            .recovery_records
            .iter()
            .filter(|(_, record)| record.recovery_event_id == recovery_event_id)
            .map(|(record_id, record)| recovery_record_summary(record_id, record))
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.ordinal);
        DocumentWriteOutcome::RecoveryRequired {
            document_id: upload.document_id.clone(),
            context_epoch: upload.context_epoch,
            source_revision: upload.expected_revision,
            code: "DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string(),
            recovery_event_id: recovery_event_id.to_string(),
            recovery_records: records,
        }
    }

    fn refresh_document_conflict(
        &mut self,
        document_id: &str,
        context_epoch: u64,
    ) -> Result<ConflictRefresh, String> {
        let context = self
            .documents
            .get_mut(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.context_epoch != context_epoch || context.conflict_token.is_none() {
            return Err("DOCUMENT_WRITE_CONFLICT".to_string());
        }
        let observed = fingerprint_path(&context.path)?;
        if observed != context.fingerprint {
            context.revision = context.revision.saturating_add(1);
            context.fingerprint = observed.clone();
        }
        let token = Uuid::new_v4().simple().to_string();
        context.conflict_token = Some(ConflictToken {
            token: token.clone(),
            context_epoch,
            revision: context.revision,
            fingerprint: observed,
            expires_at: Instant::now() + WRITE_UPLOAD_TTL,
        });
        Ok(ConflictRefresh {
            observed_revision: context.revision,
            conflict_token: token,
        })
    }

    fn reap_expired_clipboard_images(&mut self) {
        let now = Instant::now();
        let expired = self
            .clipboard_image_uploads
            .iter()
            .filter_map(|(id, upload)| (upload.expires_at <= now).then_some(id.clone()))
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(upload) = self.clipboard_image_uploads.remove(&id) {
                let _ = upload.staging.remove_exact();
                if self.write_coordinator.active_upload.as_deref() == Some(id.as_str()) {
                    self.write_coordinator.active_upload = None;
                }
                self.write_coordinator.reserved_bytes = self
                    .write_coordinator
                    .reserved_bytes
                    .saturating_sub(MAX_CLIPBOARD_INGRESS_BYTES);
            }
        }
    }

    fn reserve_clipboard_image(
        &mut self,
        window_label: String,
        request: ReserveClipboardImageRequest,
    ) -> Result<WriteUploadReservation, String> {
        self.reap_expired_clipboard_images();
        if request.content_bytes == 0
            || request.content_bytes > MAX_IMAGE_BYTES
            || !matches!(
                request.extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp"
            )
            || self.write_coordinator.active_upload.is_some()
        {
            return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
        }
        let context = self
            .documents
            .get(&request.document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.context_epoch != request.context_epoch {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        let upload_id = Uuid::new_v4().simple().to_string();
        let token = Uuid::new_v4().simple().to_string();
        let staging = create_staging_file(&context.root)?;
        self.clipboard_image_uploads.insert(
            upload_id.clone(),
            ClipboardImageUpload {
                upload_id: upload_id.clone(),
                token: token.clone(),
                window_label,
                document_id: request.document_id,
                context_epoch: request.context_epoch,
                extension: request.extension.to_ascii_lowercase(),
                source_stem: request
                    .source_stem
                    .chars()
                    .filter(|value| !value.is_control())
                    .take(80)
                    .collect::<String>(),
                expected_bytes: request.content_bytes,
                bytes_written: 0,
                next_sequence: 0,
                staging,
                expires_at: Instant::now() + WRITE_UPLOAD_TTL,
            },
        );
        self.write_coordinator.active_upload = Some(upload_id.clone());
        self.write_coordinator.reserved_bytes = self
            .write_coordinator
            .reserved_bytes
            .saturating_add(MAX_CLIPBOARD_INGRESS_BYTES);
        Ok(WriteUploadReservation {
            upload_id,
            token,
            expires_in_ms: WRITE_UPLOAD_TTL.as_millis() as u64,
            context_epoch: request.context_epoch,
        })
    }

    fn append_clipboard_image_chunk(
        &mut self,
        window_label: &str,
        upload_id: &str,
        token: &str,
        sequence: u64,
        claimed_length: usize,
        bytes: &[u8],
    ) -> Result<(), String> {
        if bytes.len() > MAX_WRITE_RAW_CHUNK_BYTES || bytes.len() != claimed_length {
            return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
        }
        let upload = self
            .clipboard_image_uploads
            .get_mut(upload_id)
            .ok_or_else(|| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
        if upload.token != token
            || upload.window_label != window_label
            || upload.expires_at <= Instant::now()
            || upload.next_sequence != sequence
            || upload.bytes_written.saturating_add(bytes.len()) > upload.expected_bytes
        {
            return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
        }
        upload
            .staging
            .file
            .write_all(bytes)
            .map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
        upload.bytes_written += bytes.len();
        upload.next_sequence += 1;
        Ok(())
    }

    fn take_clipboard_image_for_finalize(
        &mut self,
        window_label: &str,
        request: BeginDocumentWriteRequest,
    ) -> Result<(ClipboardImageUpload, PathBuf), String> {
        let upload = self
            .clipboard_image_uploads
            .remove(&request.upload_id)
            .ok_or_else(|| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
        self.write_coordinator.active_upload = None;
        self.write_coordinator.reserved_bytes = self
            .write_coordinator
            .reserved_bytes
            .saturating_sub(MAX_CLIPBOARD_INGRESS_BYTES);
        if upload.upload_id != request.upload_id
            || upload.token != request.token
            || upload.window_label != window_label
            || upload.expires_at <= Instant::now()
            || upload.bytes_written != upload.expected_bytes
        {
            let _ = upload.staging.remove_exact();
            return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
        }
        let document_path = self
            .documents
            .get(&upload.document_id)
            .filter(|context| context.context_epoch == upload.context_epoch)
            .map(|context| context.path.clone())
            .ok_or_else(|| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?;
        Ok((upload, document_path))
    }

    fn cancel_clipboard_image(
        &mut self,
        window_label: &str,
        request: BeginDocumentWriteRequest,
    ) -> Result<(), String> {
        let upload = self
            .clipboard_image_uploads
            .get(&request.upload_id)
            .ok_or_else(|| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
        if upload.token != request.token || upload.window_label != window_label {
            return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
        }
        let upload = self
            .clipboard_image_uploads
            .remove(&request.upload_id)
            .expect("validated upload");
        if self.write_coordinator.active_upload.as_deref() == Some(request.upload_id.as_str()) {
            self.write_coordinator.active_upload = None;
        }
        self.write_coordinator.reserved_bytes = self
            .write_coordinator
            .reserved_bytes
            .saturating_sub(MAX_CLIPBOARD_INGRESS_BYTES);
        upload.staging.remove_exact()
    }

    fn register_image_insert(
        &mut self,
        window_label: String,
        document_id: String,
        context_epoch: u64,
        copied: CopiedImage,
    ) -> Result<ImageInsertResult, String> {
        let (root, name, identity, markdown_url, alt) = copied;
        let context_valid = self
            .documents
            .get(&document_id)
            .is_some_and(|context| context.context_epoch == context_epoch);
        if !context_valid {
            let _ = remove_exact_name(&root, &name, &identity);
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        let token = Uuid::new_v4().simple().to_string();
        self.image_insert_rollbacks.insert(
            token.clone(),
            ImageInsertRollback {
                token: token.clone(),
                window_label,
                document_id,
                context_epoch,
                root,
                name,
                identity,
                expires_at: Instant::now() + Duration::from_secs(120),
            },
        );
        Ok(ImageInsertResult {
            markdown_url,
            alt,
            rollback_token: token,
        })
    }

    fn register_save_as_preparation(
        &mut self,
        window_label: String,
        document_id: &str,
        target_path: PathBuf,
    ) -> Result<SaveAsPrepared, String> {
        self.reap_expired_save_as_preparations();
        if self
            .recovery_records
            .values()
            .any(|record| record.document_id == document_id && !record.acknowledged)
        {
            return Err("DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string());
        }
        let context = self
            .documents
            .get(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if target_path == context.path {
            return Err("DOCUMENT_SAVE_AS_SAME_TARGET".to_string());
        }
        if self.path_index.contains_key(&target_path)
            || self.save_as_path_reservations.contains_key(&target_path)
        {
            return Err("DOCUMENT_TARGET_ALREADY_OPEN".to_string());
        }
        let replacing_existing = target_path.exists();
        let (expected_target_fingerprint, expected_target_metadata) = if replacing_existing {
            let metadata = fs::symlink_metadata(&target_path)
                .map_err(|_| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err("DOCUMENT_SAVE_AS_FAILED".to_string());
            }
            let mut file = open_document_no_follow(&target_path)?;
            (
                Some(fingerprint_file(&mut file)?),
                Some(metadata_signature(&file)?),
            )
        } else {
            (None, None)
        };
        let token = Uuid::new_v4().simple().to_string();
        let preparation = SaveAsPreparation {
            token: token.clone(),
            window_label,
            document_id: document_id.to_string(),
            old_context_epoch: context.context_epoch,
            next_context_epoch: context.context_epoch.saturating_add(1),
            target_path: target_path.clone(),
            replacing_existing,
            expected_target_fingerprint,
            expected_target_metadata,
            expires_at: Instant::now() + Duration::from_secs(120),
        };
        self.save_as_path_reservations
            .insert(target_path.clone(), token.clone());
        self.save_as_preparations.insert(token.clone(), preparation);
        Ok(SaveAsPrepared {
            token,
            next_context_epoch: context.context_epoch.saturating_add(1),
            file_name: target_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| "DOCUMENT_SAVE_AS_FAILED".to_string())?
                .to_string(),
        })
    }

    fn cancel_save_as_preparation(&mut self, token: &str) {
        if let Some(preparation) = self.save_as_preparations.remove(token) {
            self.save_as_path_reservations
                .remove(&preparation.target_path);
        }
        cancel_provisional_watcher(token);
    }

    fn reap_expired_save_as_preparations(&mut self) {
        let now = Instant::now();
        let expired = self
            .save_as_preparations
            .iter()
            .filter_map(|(token, preparation)| {
                (preparation.expires_at <= now).then_some(token.clone())
            })
            .collect::<Vec<_>>();
        for token in expired {
            self.cancel_save_as_preparation(&token);
        }
    }

    fn recovery_record_for_action(
        &self,
        window_label: &str,
        record_id: &str,
        event_id: &str,
        action_token: &str,
    ) -> Result<RecoveryRecord, String> {
        let record = self
            .recovery_records
            .get(record_id)
            .ok_or_else(|| "RECOVERY_ACTION_INVALID".to_string())?;
        if record.window_label != window_label
            || record.recovery_event_id != event_id
            || record.action_token != action_token
            || record.action_completed
            || record.acknowledged
            || record.expires_at <= Instant::now()
        {
            return Err("RECOVERY_ACTION_INVALID".to_string());
        }
        Ok(record.clone())
    }

    fn complete_recovery_action(
        &mut self,
        record_id: &str,
        saved_generation: Option<u64>,
    ) -> Result<RecoveryActionCompleted, String> {
        let record = self
            .recovery_records
            .get_mut(record_id)
            .ok_or_else(|| "RECOVERY_ACTION_INVALID".to_string())?;
        if record.action_completed || record.acknowledged {
            return Err("RECOVERY_ACTION_INVALID".to_string());
        }
        record.action_completed = true;
        record.action_token.clear();
        record.saved_generation = saved_generation;
        let ack_token = Uuid::new_v4().simple().to_string();
        record.ack_token = Some(ack_token.clone());
        record.expires_at = Instant::now() + WRITE_UPLOAD_TTL;
        Ok(RecoveryActionCompleted {
            record_id: record_id.to_string(),
            ack_token,
            saved_generation,
        })
    }

    fn acknowledge_recovery(
        &mut self,
        window_label: &str,
        record_id: &str,
        event_id: &str,
        ack_token: &str,
    ) -> Result<RecoveryAcknowledged, String> {
        let (document_id, context_epoch) = {
            let record = self
                .recovery_records
                .get_mut(record_id)
                .ok_or_else(|| "RECOVERY_ACK_INVALID".to_string())?;
            if record.window_label != window_label
                || record.recovery_event_id != event_id
                || !record.action_completed
                || record.acknowledged
                || record.ack_token.as_deref() != Some(ack_token)
                || record.expires_at <= Instant::now()
            {
                return Err("RECOVERY_ACK_INVALID".to_string());
            }
            record.acknowledged = true;
            record.ack_token = None;
            (record.document_id.clone(), record.context_epoch)
        };
        let resolved = !self.recovery_records.values().any(|record| {
            record.document_id == document_id
                && record.context_epoch == context_epoch
                && record.recovery_event_id == event_id
                && !record.acknowledged
        });
        Ok(RecoveryAcknowledged { resolved })
    }

    fn refresh_recovery_ack(
        &mut self,
        window_label: &str,
        record_id: &str,
        event_id: &str,
    ) -> Result<RecoveryActionCompleted, String> {
        let record = self
            .recovery_records
            .get_mut(record_id)
            .ok_or_else(|| "RECOVERY_ACK_INVALID".to_string())?;
        if record.window_label != window_label
            || record.recovery_event_id != event_id
            || !record.action_completed
            || record.acknowledged
        {
            return Err("RECOVERY_ACK_INVALID".to_string());
        }
        let ack_token = Uuid::new_v4().simple().to_string();
        record.ack_token = Some(ack_token.clone());
        record.expires_at = Instant::now() + WRITE_UPLOAD_TTL;
        Ok(RecoveryActionCompleted {
            record_id: record_id.to_string(),
            ack_token,
            saved_generation: record.saved_generation,
        })
    }

    fn refresh_recovery_action(
        &mut self,
        window_label: &str,
        record_id: &str,
        event_id: &str,
    ) -> Result<RecoveryActionRefreshed, String> {
        let record = self
            .recovery_records
            .get_mut(record_id)
            .ok_or_else(|| "RECOVERY_ACTION_INVALID".to_string())?;
        if record.window_label != window_label
            || record.recovery_event_id != event_id
            || record.action_completed
            || record.acknowledged
        {
            return Err("RECOVERY_ACTION_INVALID".to_string());
        }
        record.action_token = Uuid::new_v4().simple().to_string();
        record.expires_at = Instant::now() + Duration::from_secs(120);
        Ok(RecoveryActionRefreshed {
            action_token: record.action_token.clone(),
        })
    }

    fn document_edit_eligibility(
        &self,
        document_id: &str,
    ) -> Result<DocumentEditEligibility, String> {
        let context = self
            .documents
            .get(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        let reason = (|| {
            let file = open_child_no_follow(
                &context.root,
                context
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?,
                libc::O_RDWR,
            )
            .map_err(|_| "DOCUMENT_WRITE_READ_ONLY".to_string())?;
            let metadata = file
                .metadata()
                .map_err(|_| "DOCUMENT_WRITE_READ_ONLY".to_string())?;
            #[cfg(unix)]
            if metadata.nlink() != 1 {
                return Err("DOCUMENT_WRITE_HARDLINK_UNSUPPORTED".to_string());
            }
            if !metadata.is_file() {
                return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
            }
            metadata_signature(&file)?;
            supports_atomic_swap(&context.root)?;
            Ok::<(), String>(())
        })()
        .err();
        Ok(DocumentEditEligibility {
            eligible: reason.is_none(),
            context_epoch: context.context_epoch,
            reason,
        })
    }

    fn reserve_document_write(
        &mut self,
        window_label: String,
        request: ReserveDocumentWriteRequest,
    ) -> Result<WriteUploadReservation, String> {
        self.reap_expired_write_uploads();
        self.reap_expired_clipboard_images();
        self.reap_expired_save_as_preparations();
        if request.document_id.is_empty()
            || request.commit_id.is_empty()
            || request.content_utf8_bytes > MAX_DOCUMENT_INPUT_BYTES
        {
            return Err("DOCUMENT_WRITE_TOO_LARGE".to_string());
        }
        let has_recovery = self
            .recovery_records
            .values()
            .any(|record| record.document_id == request.document_id);
        let unresolved_recovery = self
            .recovery_records
            .values()
            .any(|record| record.document_id == request.document_id && !record.acknowledged);
        if has_recovery && (request.save_as_token.is_none() || unresolved_recovery) {
            return Err("DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string());
        }
        if self
            .write_coordinator
            .active_by_document
            .contains_key(&request.document_id)
            || self.write_coordinator.active_upload.is_some()
        {
            return Err("DOCUMENT_WRITE_BUSY".to_string());
        }
        if self
            .write_coordinator
            .reserved_bytes
            .saturating_add(MAX_WRITE_UPLOAD_BYTES)
            > MAX_WRITE_IO_BYTES
        {
            return Err("DOCUMENT_WRITE_BUSY".to_string());
        }
        let context = self
            .documents
            .get(&request.document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.context_epoch != request.context_epoch {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        if let Some(token) = request.conflict_token.as_deref() {
            let Some(current) = context.conflict_token.as_ref() else {
                return Err("DOCUMENT_WRITE_CONFLICT".to_string());
            };
            if current.token != token
                || current.context_epoch != context.context_epoch
                || current.revision != context.revision
                || current.fingerprint != context.fingerprint
                || current.expires_at <= Instant::now()
            {
                return Err("DOCUMENT_WRITE_CONFLICT".to_string());
            }
        } else if context.conflict_token.is_some() && request.save_as_token.is_none() {
            return Err("DOCUMENT_WRITE_CONFLICT".to_string());
        }

        let save_as = if let Some(token) = request.save_as_token.as_deref() {
            let preparation = self
                .save_as_preparations
                .remove(token)
                .ok_or_else(|| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
            self.save_as_path_reservations
                .remove(&preparation.target_path);
            if preparation.token != token
                || preparation.window_label != window_label
                || preparation.document_id != request.document_id
                || preparation.old_context_epoch != context.context_epoch
                || preparation.expires_at <= Instant::now()
            {
                return Err("DOCUMENT_SAVE_AS_FAILED".to_string());
            }
            Some(preparation)
        } else {
            None
        };

        let upload_id = Uuid::new_v4().simple().to_string();
        let token = Uuid::new_v4().simple().to_string();
        let expires_at = Instant::now() + WRITE_UPLOAD_TTL;
        let upload = WriteUpload {
            upload_id: upload_id.clone(),
            token: token.clone(),
            window_label,
            document_id: request.document_id.clone(),
            context_epoch: context.context_epoch,
            expected_revision: context.revision,
            expected_fingerprint: context.fingerprint.clone(),
            expected_metadata: context.metadata.clone(),
            commit_id: request.commit_id,
            write_generation: request.write_generation,
            expected_bytes: request.content_utf8_bytes,
            next_sequence: 0,
            bytes_written: 0,
            expires_at,
            target: if let Some(preparation) = save_as {
                WriteTarget::SaveAs {
                    preparation_token: preparation.token,
                    root: preparation
                        .target_path
                        .parent()
                        .ok_or_else(|| "DOCUMENT_SAVE_AS_FAILED".to_string())?
                        .to_path_buf(),
                    name: preparation
                        .target_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| "DOCUMENT_SAVE_AS_FAILED".to_string())?
                        .to_string(),
                    next_context_epoch: preparation.next_context_epoch,
                    replacing_existing: preparation.replacing_existing,
                    expected_target_fingerprint: preparation.expected_target_fingerprint,
                    expected_target_metadata: preparation.expected_target_metadata,
                }
            } else {
                WriteTarget::Original {
                    root: context.root.clone(),
                    name: context
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?
                        .to_string(),
                }
            },
            staging: None,
            hasher: IncrementalSha256::new()?,
        };
        self.write_coordinator
            .active_by_document
            .insert(request.document_id, upload_id.clone());
        self.write_coordinator.active_upload = Some(upload_id.clone());
        self.write_coordinator.reserved_bytes += MAX_WRITE_UPLOAD_BYTES;
        self.write_coordinator
            .uploads
            .insert(upload_id.clone(), upload);
        Ok(WriteUploadReservation {
            upload_id,
            token,
            expires_in_ms: WRITE_UPLOAD_TTL.as_millis() as u64,
            context_epoch: request.context_epoch,
        })
    }

    fn reserve_recovery_copy(
        &mut self,
        window_label: String,
        request: PrepareRecoverySaveRequest,
        target_path: PathBuf,
    ) -> Result<WriteUploadReservation, String> {
        self.reap_expired_write_uploads();
        if request.content_utf8_bytes > MAX_DOCUMENT_INPUT_BYTES {
            return Err("DOCUMENT_WRITE_TOO_LARGE".to_string());
        }
        let record = self.recovery_record_for_action(
            &window_label,
            &request.record_id,
            &request.recovery_event_id,
            &request.action_token,
        )?;
        if record.action != "saveCurrentBufferCopy"
            || record.document_id != request.document_id
            || record.context_epoch != request.context_epoch
        {
            return Err("RECOVERY_ACTION_INVALID".to_string());
        }
        if self
            .write_coordinator
            .active_by_document
            .contains_key(&request.document_id)
            || self.write_coordinator.active_upload.is_some()
            || self
                .write_coordinator
                .reserved_bytes
                .saturating_add(MAX_WRITE_UPLOAD_BYTES)
                > MAX_WRITE_IO_BYTES
        {
            return Err("DOCUMENT_WRITE_BUSY".to_string());
        }
        let context = self
            .documents
            .get(&request.document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.context_epoch != request.context_epoch {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        if target_path == context.path
            || self.path_index.contains_key(&target_path)
            || self.save_as_path_reservations.contains_key(&target_path)
        {
            return Err("RECOVERY_SAVE_FAILED".to_string());
        }
        let root = target_path
            .parent()
            .ok_or_else(|| "RECOVERY_SAVE_FAILED".to_string())?
            .to_path_buf();
        let name = target_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "RECOVERY_SAVE_FAILED".to_string())?
            .to_string();
        let replacing_existing = target_path.exists();
        let (expected_target_fingerprint, expected_target_metadata) = if replacing_existing {
            let metadata = fs::symlink_metadata(&target_path)
                .map_err(|_| "RECOVERY_SAVE_FAILED".to_string())?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err("RECOVERY_SAVE_FAILED".to_string());
            }
            let mut file = open_document_no_follow(&target_path)?;
            (
                Some(fingerprint_file(&mut file)?),
                Some(metadata_signature(&file)?),
            )
        } else {
            (None, None)
        };
        let upload_id = Uuid::new_v4().simple().to_string();
        let token = Uuid::new_v4().simple().to_string();
        let upload = WriteUpload {
            upload_id: upload_id.clone(),
            token: token.clone(),
            window_label,
            document_id: request.document_id.clone(),
            context_epoch: request.context_epoch,
            expected_revision: context.revision,
            expected_fingerprint: context.fingerprint.clone(),
            expected_metadata: context.metadata.clone(),
            commit_id: request.commit_id,
            write_generation: request.write_generation,
            expected_bytes: request.content_utf8_bytes,
            next_sequence: 0,
            bytes_written: 0,
            expires_at: Instant::now() + WRITE_UPLOAD_TTL,
            target: WriteTarget::RecoveryCopy {
                recovery_event_id: request.recovery_event_id,
                record_id: request.record_id,
                root,
                name,
                replacing_existing,
                expected_target_fingerprint,
                expected_target_metadata,
            },
            staging: None,
            hasher: IncrementalSha256::new()?,
        };
        self.write_coordinator
            .active_by_document
            .insert(request.document_id, upload_id.clone());
        self.write_coordinator.active_upload = Some(upload_id.clone());
        self.write_coordinator.reserved_bytes += MAX_WRITE_UPLOAD_BYTES;
        self.write_coordinator
            .uploads
            .insert(upload_id.clone(), upload);
        Ok(WriteUploadReservation {
            upload_id,
            token,
            expires_in_ms: WRITE_UPLOAD_TTL.as_millis() as u64,
            context_epoch: request.context_epoch,
        })
    }

    fn begin_document_write(
        &mut self,
        window_label: &str,
        request: BeginDocumentWriteRequest,
    ) -> Result<(), String> {
        self.reap_expired_write_uploads();
        let upload = self.write_upload_mut(&request.upload_id, &request.token, window_label)?;
        if upload.staging.is_some() {
            return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
        }
        let root = match &upload.target {
            WriteTarget::Original { root, .. }
            | WriteTarget::SaveAs { root, .. }
            | WriteTarget::RecoveryCopy { root, .. } => root,
        };
        let staging = create_staging_file(root)?;
        upload.staging = Some(staging);
        Ok(())
    }

    fn append_document_write_chunk(
        &mut self,
        window_label: &str,
        upload_id: &str,
        token: &str,
        sequence: u64,
        claimed_length: usize,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.reap_expired_write_uploads();
        if bytes.len() > MAX_WRITE_RAW_CHUNK_BYTES || bytes.len() != claimed_length {
            self.cancel_document_write(upload_id);
            return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
        }
        let upload = self.write_upload_mut(upload_id, token, window_label)?;
        if upload.staging.is_none()
            || upload.next_sequence != sequence
            || upload.bytes_written.saturating_add(bytes.len()) > upload.expected_bytes
        {
            return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
        }
        let staging = upload.staging.as_mut().expect("checked staging");
        staging
            .file
            .write_all(bytes)
            .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
        upload.hasher.update(bytes)?;
        upload.bytes_written += bytes.len();
        upload.next_sequence += 1;
        Ok(())
    }

    fn take_document_write_for_finalize(
        &mut self,
        window_label: &str,
        request: &FinalizeDocumentWriteRequest,
    ) -> Result<WriteUpload, String> {
        self.reap_expired_write_uploads();
        let upload = self.write_upload(&request.upload_id, &request.token, window_label)?;
        if upload.staging.is_none() || upload.bytes_written != upload.expected_bytes {
            return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
        }
        let upload = self
            .write_coordinator
            .uploads
            .remove(&request.upload_id)
            .expect("upload was just verified");
        self.write_coordinator
            .active_by_document
            .remove(&upload.document_id);
        Ok(upload)
    }

    #[cfg(test)]
    fn finish_document_write(
        &mut self,
        upload: WriteUpload,
    ) -> Result<DocumentWriteOutcome, String> {
        let upload_id = upload.upload_id.clone();
        let result = self.finish_document_write_inner(upload);
        self.release_document_write_permit(&upload_id);
        result
    }

    #[cfg(test)]
    fn finish_document_write_inner(
        &mut self,
        mut upload: WriteUpload,
    ) -> Result<DocumentWriteOutcome, String> {
        let document_id = upload.document_id.clone();
        let Some(context) = self.documents.get(&document_id) else {
            return Err("DOCUMENT_NOT_FOUND".to_string());
        };
        let recovery_copy = matches!(&upload.target, WriteTarget::RecoveryCopy { .. });
        if context.context_epoch != upload.context_epoch
            || !recovery_copy
                && (context.revision != upload.expected_revision
                    || context.fingerprint != upload.expected_fingerprint)
        {
            return self.document_write_conflict(&document_id, upload.context_epoch);
        }
        let outcome = commit_document_upload(&mut upload)?;
        self.apply_document_write_commit(upload, outcome)
    }

    fn apply_document_write_commit(
        &mut self,
        upload: WriteUpload,
        outcome: CommitDocumentWrite,
    ) -> Result<DocumentWriteOutcome, String> {
        let document_id = upload.document_id.clone();
        if let WriteTarget::RecoveryCopy {
            recovery_event_id,
            record_id,
            ..
        } = &upload.target
        {
            match outcome {
                CommitDocumentWrite::Saved { .. } => {
                    let completed =
                        self.complete_recovery_action(record_id, Some(upload.write_generation))?;
                    return Ok(DocumentWriteOutcome::RecoveryCopySaved {
                        record_id: record_id.clone(),
                        recovery_event_id: recovery_event_id.clone(),
                        ack_token: completed.ack_token,
                        write_generation: upload.write_generation,
                    });
                }
                CommitDocumentWrite::Conflict => {
                    return Err("RECOVERY_SAVE_CONFLICT".to_string());
                }
                CommitDocumentWrite::RecoveryRequired { preserved } => {
                    return Ok(self.extend_recovery_outcome(&upload, recovery_event_id, preserved));
                }
            }
        }
        if let WriteTarget::SaveAs {
            root,
            name,
            next_context_epoch,
            expected_target_fingerprint,
            ..
        } = &upload.target
        {
            let (fingerprint, metadata) = match outcome {
                CommitDocumentWrite::Saved {
                    fingerprint,
                    metadata,
                } => (fingerprint, metadata),
                CommitDocumentWrite::Conflict => {
                    return Err("DOCUMENT_SAVE_AS_CONFLICT".to_string())
                }
                CommitDocumentWrite::RecoveryRequired { preserved } => {
                    return Ok(self.recovery_outcome(&upload, preserved));
                }
            };
            let target_path = root.join(name);
            let context = self
                .documents
                .get_mut(&document_id)
                .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
            let old_path = context.path.clone();
            context.path = target_path.clone();
            context.root = root.clone();
            context.context_epoch = *next_context_epoch;
            context.revision = context.revision.saturating_add(1);
            context.fingerprint = fingerprint.clone();
            context.metadata = Some(metadata);
            context.conflict_token = None;
            context.self_write_ledger.clear();
            context.self_write_ledger.push_back(SelfWriteIntent {
                context_epoch: *next_context_epoch,
                commit_id: upload.commit_id.clone(),
                before: expected_target_fingerprint.clone().unwrap_or_default(),
                after: fingerprint.clone(),
                source_revision: context.revision,
                created_at: Instant::now(),
            });
            self.recovery_records.retain(|_, record| {
                record.document_id != document_id || record.context_epoch != upload.context_epoch
            });
            self.path_index.remove(&old_path);
            self.path_index.insert(target_path, document_id.clone());
            return Ok(DocumentWriteOutcome::ContextRebound {
                document_id,
                context_epoch: context.context_epoch,
                source_revision: context.revision,
                commit_id: upload.commit_id,
                write_generation: upload.write_generation,
                file_name: name.clone(),
            });
        }
        let WriteTarget::Original { root, name } = &upload.target else {
            unreachable!()
        };
        let _ = (root, name);
        match outcome {
            CommitDocumentWrite::Saved {
                fingerprint,
                metadata,
            } => {
                let context = self
                    .documents
                    .get_mut(&document_id)
                    .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
                context.revision = context.revision.saturating_add(1);
                context.fingerprint = fingerprint.clone();
                context.metadata = Some(metadata);
                context.conflict_token = None;
                context.self_write_ledger.push_back(SelfWriteIntent {
                    context_epoch: context.context_epoch,
                    commit_id: upload.commit_id.clone(),
                    before: upload.expected_fingerprint,
                    after: fingerprint,
                    source_revision: context.revision,
                    created_at: Instant::now(),
                });
                while context.self_write_ledger.len() > MAX_SELF_WRITE_LEDGER {
                    context.self_write_ledger.pop_front();
                }
                Ok(DocumentWriteOutcome::Saved {
                    document_id,
                    context_epoch: context.context_epoch,
                    source_revision: context.revision,
                    commit_id: upload.commit_id,
                    write_generation: upload.write_generation,
                })
            }
            CommitDocumentWrite::Conflict => {
                self.document_write_conflict(&document_id, upload.context_epoch)
            }
            CommitDocumentWrite::RecoveryRequired { preserved } => {
                Ok(self.recovery_outcome(&upload, preserved))
            }
        }
    }

    fn document_write_conflict(
        &mut self,
        document_id: &str,
        expected_epoch: u64,
    ) -> Result<DocumentWriteOutcome, String> {
        let context = self
            .documents
            .get_mut(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.context_epoch != expected_epoch {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        let observed = fingerprint_path(&context.path)?;
        context.revision = context.revision.saturating_add(1);
        context.fingerprint = observed.clone();
        let token = Uuid::new_v4().simple().to_string();
        context.conflict_token = Some(ConflictToken {
            token: token.clone(),
            context_epoch: context.context_epoch,
            revision: context.revision,
            fingerprint: observed,
            expires_at: Instant::now() + WRITE_UPLOAD_TTL,
        });
        Ok(DocumentWriteOutcome::Conflict {
            document_id: document_id.to_string(),
            context_epoch: context.context_epoch,
            observed_revision: context.revision,
            conflict_token: token,
        })
    }

    fn write_upload<'a>(
        &'a self,
        upload_id: &str,
        token: &str,
        window_label: &str,
    ) -> Result<&'a WriteUpload, String> {
        let upload = self
            .write_coordinator
            .uploads
            .get(upload_id)
            .ok_or_else(|| "DOCUMENT_WRITE_PROTOCOL".to_string())?;
        if upload.token != token
            || upload.window_label != window_label
            || upload.expires_at <= Instant::now()
        {
            return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
        }
        Ok(upload)
    }

    fn write_upload_mut<'a>(
        &'a mut self,
        upload_id: &str,
        token: &str,
        window_label: &str,
    ) -> Result<&'a mut WriteUpload, String> {
        let upload = self
            .write_coordinator
            .uploads
            .get_mut(upload_id)
            .ok_or_else(|| "DOCUMENT_WRITE_PROTOCOL".to_string())?;
        if upload.token != token
            || upload.window_label != window_label
            || upload.expires_at <= Instant::now()
        {
            return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
        }
        Ok(upload)
    }

    fn cancel_document_write(&mut self, upload_id: &str) {
        if let Some(upload) = self.write_coordinator.uploads.remove(upload_id) {
            if let WriteTarget::SaveAs {
                preparation_token, ..
            } = &upload.target
            {
                cancel_provisional_watcher(preparation_token);
            }
            self.write_coordinator
                .active_by_document
                .remove(&upload.document_id);
            drop(upload);
        }
        self.release_document_write_permit(upload_id);
    }

    fn release_document_write_permit(&mut self, upload_id: &str) {
        if self.write_coordinator.active_upload.as_deref() == Some(upload_id) {
            self.write_coordinator.active_upload = None;
            self.write_coordinator.reserved_bytes = self
                .write_coordinator
                .reserved_bytes
                .saturating_sub(MAX_WRITE_UPLOAD_BYTES);
        }
    }

    fn reap_expired_write_uploads(&mut self) {
        let expired = self
            .write_coordinator
            .uploads
            .iter()
            .filter_map(|(id, upload)| (upload.expires_at <= Instant::now()).then_some(id.clone()))
            .collect::<Vec<_>>();
        for upload_id in expired {
            self.cancel_document_write(&upload_id);
        }
    }

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
            fingerprint,
            metadata,
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
                context_epoch: 0,
                revision: 0,
                fingerprint,
                metadata,
                self_write_ledger: VecDeque::new(),
                conflict_token: None,
                resources: HashMap::new(),
                released_render_leases: HashSet::new(),
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
        if let Some(upload_id) = self
            .write_coordinator
            .active_by_document
            .get(document_id)
            .cloned()
        {
            self.cancel_document_write(&upload_id);
        }
        if let Some(context) = self.documents.remove(document_id) {
            self.path_index.remove(&context.path);
            self.total_resources = self.total_resources.saturating_sub(context.resources.len());
            self.total_bytes = self.total_bytes.saturating_sub(context.resource_bytes);
        }
        let save_as_tokens = self
            .save_as_preparations
            .iter()
            .filter_map(|(token, preparation)| {
                (preparation.document_id == document_id).then_some(token.clone())
            })
            .collect::<Vec<_>>();
        for token in save_as_tokens {
            self.cancel_save_as_preparation(&token);
            cancel_provisional_watcher(&token);
        }
        self.image_insert_rollbacks
            .retain(|_, record| record.document_id != document_id);
        self.clipboard_image_uploads
            .retain(|_, upload| upload.document_id != document_id);
        self.recovery_records
            .retain(|_, record| record.document_id != document_id);
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
        let fingerprint = fingerprint_path(&path)?;
        let metadata = open_document_no_follow(&path)
            .and_then(|file| metadata_signature(&file))
            .ok();
        let revision = {
            let context = self
                .documents
                .get_mut(document_id)
                .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
            context.fingerprint = fingerprint;
            context.metadata = metadata;
            context.conflict_token = None;
            context.revision = observed_revision
                .map(|revision| context.revision.max(revision))
                .unwrap_or_else(|| context.revision.saturating_add(1));
            context.revision
        };
        Ok(DocumentPayload {
            document_id: document_id.to_string(),
            file_name,
            content,
            source_revision: revision,
        })
    }

    fn watcher_read_target(&self, document_id: &str) -> Result<(PathBuf, u64), String> {
        self.documents
            .get(document_id)
            .map(|context| (context.path.clone(), context.context_epoch))
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())
    }

    fn apply_file_changed(
        &mut self,
        document_id: &str,
        context_epoch: u64,
        observed: FileFingerprint,
        metadata: Option<FileMetadataSignature>,
    ) -> Result<Option<(u64, String)>, String> {
        let context = self
            .documents
            .get_mut(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.context_epoch != context_epoch {
            return Ok(None);
        }
        let now = Instant::now();
        context
            .self_write_ledger
            .retain(|intent| now.duration_since(intent.created_at) <= Duration::from_secs(30));
        if let Some(position) = context.self_write_ledger.iter().position(|intent| {
            intent.context_epoch == context.context_epoch
                && !intent.commit_id.is_empty()
                && intent.before != observed
                && intent.after == observed
                && intent.source_revision <= context.revision
        }) {
            context.self_write_ledger.remove(position);
            return Ok(None);
        }
        if observed == context.fingerprint {
            return Ok(None);
        }
        context.revision = context.revision.saturating_add(1);
        context.fingerprint = observed.clone();
        context.metadata = metadata;
        let token = Uuid::new_v4().simple().to_string();
        context.conflict_token = Some(ConflictToken {
            token: token.clone(),
            context_epoch: context.context_epoch,
            revision: context.revision,
            fingerprint: observed,
            expires_at: now + WRITE_UPLOAD_TTL,
        });
        Ok(Some((context.revision, token)))
    }

    #[cfg(test)]
    fn mark_file_changed(&mut self, document_id: &str) -> Result<Option<(u64, String)>, String> {
        let (path, context_epoch) = self.watcher_read_target(document_id)?;
        let observed = fingerprint_path(&path)?;
        let metadata = open_document_no_follow(&path)
            .and_then(|file| metadata_signature(&file))
            .ok();
        self.apply_file_changed(document_id, context_epoch, observed, metadata)
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
        let resolution = self.local_image_resolution(document_id, source, "lease-legacy")?;
        if let Some(url) = resolution.cached_url {
            return Ok(ResolvedImageSource { url });
        }

        let prepared = prepare_local_image(&resolution.root, &resolution.identity)?;
        self.store_prepared_image(document_id, resolution.identity, prepared, "lease-legacy")
    }

    fn local_image_resolution(
        &mut self,
        document_id: &str,
        source: &str,
        render_lease_id: &str,
    ) -> Result<LocalImageResolution, String> {
        let identity = normalize_relative_source(source)?;
        let context = self
            .documents
            .get_mut(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.released_render_leases.contains(render_lease_id) {
            return Err("RENDER_LEASE_RELEASED".to_string());
        }
        let cached_url = context.resources.get_mut(&identity).map(|snapshot| {
            snapshot.leases.insert(render_lease_id.to_string());
            image_protocol_url(document_id, &snapshot.resource_id)
        });
        Ok(LocalImageResolution {
            cached_url,
            identity,
            root: context.root.clone(),
        })
    }

    fn store_prepared_image(
        &mut self,
        document_id: &str,
        identity: String,
        prepared: PreparedImage,
        render_lease_id: &str,
    ) -> Result<ResolvedImageSource, String> {
        let PreparedImage { bytes, mime } = prepared;
        let context = self
            .documents
            .get_mut(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.released_render_leases.contains(render_lease_id) {
            return Err("RENDER_LEASE_RELEASED".to_string());
        }
        if let Some(snapshot) = context.resources.get_mut(&identity) {
            snapshot.leases.insert(render_lease_id.to_string());
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
                leases: HashSet::from([render_lease_id.to_string()]),
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
        self.store_prepared_image(
            document_id,
            identity,
            PreparedImage { bytes, mime },
            "lease-legacy",
        )
    }

    fn protocol_snapshot(&self, document_id: &str, resource_id: &str) -> Option<&ResourceSnapshot> {
        self.documents
            .get(document_id)?
            .resources
            .values()
            .find(|snapshot| snapshot.resource_id == resource_id)
    }

    fn release_render_lease(
        &mut self,
        document_id: &str,
        render_lease_id: &str,
    ) -> Result<(), String> {
        if !render_lease_id.starts_with("lease-") || render_lease_id.len() > 128 {
            return Err("RENDER_PROTOCOL_ERROR".to_string());
        }
        let context = self
            .documents
            .get_mut(document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        context
            .released_render_leases
            .insert(render_lease_id.to_string());
        let mut released_resources = 0usize;
        let mut released_bytes = 0usize;
        context.resources.retain(|_, snapshot| {
            snapshot.leases.remove(render_lease_id);
            if snapshot.leases.is_empty() {
                released_resources += 1;
                released_bytes += snapshot.bytes.len();
                false
            } else {
                true
            }
        });
        context.resource_bytes = context.resource_bytes.saturating_sub(released_bytes);
        self.total_resources = self.total_resources.saturating_sub(released_resources);
        self.total_bytes = self.total_bytes.saturating_sub(released_bytes);
        Ok(())
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
    let fingerprint = fingerprint_path(&path)?;
    let metadata = open_document_no_follow(&path)
        .and_then(|file| metadata_signature(&file))
        .ok();
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
        fingerprint,
        metadata,
    })
}

fn read_document_content(path: &Path) -> Result<String, String> {
    let mut file = open_document_no_follow(path)?;
    let mut bytes = Vec::with_capacity(MAX_DOCUMENT_INPUT_BYTES.min(64 * 1024));
    Read::by_ref(&mut file)
        .take((MAX_DOCUMENT_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "DOCUMENT_READ_FAILED".to_string())?;
    if bytes.len() > MAX_DOCUMENT_INPUT_BYTES {
        return Err("DOCUMENT_TOO_LARGE".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "DOCUMENT_NOT_UTF8".to_string())
}

fn fingerprint_path(path: &Path) -> Result<FileFingerprint, String> {
    let mut file = open_document_no_follow(path)?;
    fingerprint_file(&mut file)
}

fn fingerprint_file(file: &mut File) -> Result<FileFingerprint, String> {
    #[cfg(unix)]
    {
        let before = file
            .metadata()
            .map_err(|_| "DOCUMENT_READ_FAILED".to_string())?;
        if !before.is_file() || before.len() > MAX_DOCUMENT_INPUT_BYTES as u64 {
            return Err("DOCUMENT_TOO_LARGE".to_string());
        }
        let mut hasher = IncrementalSha256::new()?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| "DOCUMENT_READ_FAILED".to_string())?;
        let mut buffer = [0_u8; MAX_WRITE_RAW_CHUNK_BYTES];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|_| "DOCUMENT_READ_FAILED".to_string())?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read])?;
        }
        let after = file
            .metadata()
            .map_err(|_| "DOCUMENT_READ_FAILED".to_string())?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return Err("DOCUMENT_IDENTITY_CHANGED".to_string());
        }
        Ok(FileFingerprint {
            identity: FileIdentity {
                device: before.dev(),
                inode: before.ino(),
            },
            length: before.len(),
            modified_seconds: before.mtime(),
            modified_nanoseconds: before.mtime_nsec(),
            sha256: hasher.finalize()?,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn acl_get_fd(fd: libc::c_int) -> *mut std::ffi::c_void;
    fn acl_to_text(acl: *mut std::ffi::c_void, length: *mut libc::ssize_t) -> *mut libc::c_char;
    fn acl_free(object: *mut std::ffi::c_void) -> libc::c_int;
}

#[cfg(target_os = "macos")]
fn metadata_signature(file: &File) -> Result<FileMetadataSignature, String> {
    use std::ffi::{CStr, CString};

    let fd = file.as_raw_fd();
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
    }
    let acl = unsafe { acl_get_fd(fd) };
    // APFS commonly reports ENOENT for a regular file without an extended ACL.
    // That is a known, preservable state (empty ACL), unlike a denied or
    // unsupported lookup, which remains fail-closed.
    let acl_bytes = if acl.is_null() {
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            Vec::new()
        } else {
            return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
        }
    } else {
        let mut acl_length: libc::ssize_t = 0;
        let acl_text = unsafe { acl_to_text(acl, &mut acl_length) };
        if acl_text.is_null() || acl_length < 0 || acl_length as usize > MAX_METADATA_VERIFY_BYTES {
            unsafe { acl_free(acl) };
            return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
        }
        let acl_bytes =
            unsafe { std::slice::from_raw_parts(acl_text.cast::<u8>(), acl_length as usize) }
                .to_vec();
        unsafe {
            acl_free(acl_text.cast());
            acl_free(acl);
        }
        acl_bytes
    };

    let names_length = unsafe { libc::flistxattr(fd, std::ptr::null_mut(), 0, 0) };
    if names_length < 0 || names_length as usize > MAX_METADATA_VERIFY_BYTES {
        return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
    }
    let mut names = vec![0_u8; names_length as usize];
    if names_length > 0
        && unsafe {
            libc::flistxattr(
                fd,
                names.as_mut_ptr().cast::<libc::c_char>(),
                names.len(),
                0,
            )
        } != names_length
    {
        return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
    }
    let mut xattrs = Vec::new();
    let mut total = acl_bytes.len().saturating_add(names.len());
    for name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name =
            CString::new(name).map_err(|_| "DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string())?;
        let value_length =
            unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
        if value_length < 0
            || total.saturating_add(value_length as usize) > MAX_METADATA_VERIFY_BYTES
        {
            return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
        }
        let mut value = vec![0_u8; value_length as usize];
        if value_length > 0
            && unsafe {
                libc::fgetxattr(
                    fd,
                    name.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                    0,
                    0,
                )
            } != value_length
        {
            return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
        }
        total = total.saturating_add(value.len());
        xattrs.push((
            CStr::from_bytes_with_nul(name.as_bytes_with_nul())
                .expect("CString")
                .to_bytes()
                .to_vec(),
            value,
        ));
    }
    xattrs.sort();
    Ok(FileMetadataSignature {
        uid: stat.st_uid,
        gid: stat.st_gid,
        mode: u32::from(stat.st_mode),
        created_seconds: stat.st_birthtime,
        created_nanoseconds: stat.st_birthtime_nsec,
        acl: acl_bytes,
        xattrs,
    })
}

#[cfg(not(target_os = "macos"))]
fn metadata_signature(_: &File) -> Result<FileMetadataSignature, String> {
    Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
}

#[derive(Debug)]
enum CommitDocumentWrite {
    Saved {
        fingerprint: FileFingerprint,
        metadata: FileMetadataSignature,
    },
    Conflict,
    RecoveryRequired {
        preserved: Vec<PathBuf>,
    },
}

fn recovery_preserving(paths: impl IntoIterator<Item = PathBuf>) -> CommitDocumentWrite {
    CommitDocumentWrite::RecoveryRequired {
        preserved: paths.into_iter().collect(),
    }
}

fn commit_document_upload(upload: &mut WriteUpload) -> Result<CommitDocumentWrite, String> {
    let staging = upload
        .staging
        .take()
        .ok_or_else(|| "DOCUMENT_WRITE_PROTOCOL".to_string())?;
    let hasher = std::mem::replace(&mut upload.hasher, IncrementalSha256::new()?);
    let digest = hasher.finalize()?;
    match &upload.target {
        WriteTarget::SaveAs {
            root,
            name,
            replacing_existing,
            expected_target_fingerprint,
            expected_target_metadata,
            ..
        } => {
            if *replacing_existing {
                commit_staged_document_write(
                    root,
                    name,
                    staging,
                    StagedWriteExpectations {
                        fingerprint: expected_target_fingerprint
                            .clone()
                            .ok_or_else(|| "DOCUMENT_SAVE_AS_FAILED".to_string())?,
                        metadata: expected_target_metadata
                            .clone()
                            .ok_or_else(|| "DOCUMENT_SAVE_AS_FAILED".to_string())?,
                        bytes: upload.expected_bytes,
                        digest,
                        replacing: true,
                    },
                )
            } else {
                commit_staged_document_create_new(
                    root,
                    name,
                    staging,
                    upload.expected_bytes,
                    digest,
                )
            }
        }
        WriteTarget::RecoveryCopy {
            root,
            name,
            replacing_existing,
            expected_target_fingerprint,
            expected_target_metadata,
            ..
        } => {
            if *replacing_existing {
                commit_staged_document_write(
                    root,
                    name,
                    staging,
                    StagedWriteExpectations {
                        fingerprint: expected_target_fingerprint
                            .clone()
                            .ok_or_else(|| "RECOVERY_SAVE_FAILED".to_string())?,
                        metadata: expected_target_metadata
                            .clone()
                            .ok_or_else(|| "RECOVERY_SAVE_FAILED".to_string())?,
                        bytes: upload.expected_bytes,
                        digest,
                        replacing: true,
                    },
                )
            } else {
                commit_staged_document_create_new(
                    root,
                    name,
                    staging,
                    upload.expected_bytes,
                    digest,
                )
            }
        }
        WriteTarget::Original { root, name } => commit_staged_document_write(
            root,
            name,
            staging,
            StagedWriteExpectations {
                fingerprint: upload.expected_fingerprint.clone(),
                metadata: upload
                    .expected_metadata
                    .clone()
                    .ok_or_else(|| "DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string())?,
                bytes: upload.expected_bytes,
                digest,
                replacing: false,
            },
        ),
    }
}

fn create_staging_file(root: &Path) -> Result<StagingFile, String> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        let directory = open_directory_no_follow(root)?;
        let name = format!(".yuyue-upload-{}.tmp", Uuid::new_v4().simple());
        let c_name =
            CString::new(name.as_bytes()).map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err("DOCUMENT_WRITE_FAILED".to_string());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file
            .metadata()
            .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
        if !metadata.is_file() {
            return Err("DOCUMENT_WRITE_FAILED".to_string());
        }
        Ok(StagingFile {
            root: root.to_path_buf(),
            name,
            identity: FileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            file,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }
}

impl StagingFile {
    fn verify_identity(&self) -> Result<(), String> {
        let metadata = self
            .file
            .metadata()
            .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
        #[cfg(unix)]
        if metadata.dev() != self.identity.device || metadata.ino() != self.identity.inode {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        if !metadata.is_file() {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        Ok(())
    }

    fn remove_exact(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            let directory = open_directory_no_follow(&self.root)?;
            let current = file_identity_at(directory.as_raw_fd(), &self.name)?;
            if current != self.identity {
                return Err("DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string());
            }
            let name = std::ffi::CString::new(self.name.as_bytes())
                .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err("DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
        }
    }
}

fn open_directory_no_follow(root: &Path) -> Result<File, String> {
    #[cfg(unix)]
    {
        let root = std::ffi::CString::new(root.as_os_str().as_bytes())
            .map_err(|_| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?;
        let fd = unsafe {
            libc::open(
                root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }
}

fn file_identity_at(directory_fd: libc::c_int, name: &str) -> Result<FileIdentity, String> {
    #[cfg(unix)]
    {
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?;
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe {
            libc::fstatat(
                directory_fd,
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        if (stat.st_mode & libc::S_IFMT) != libc::S_IFREG {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        Ok(FileIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (directory_fd, name);
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }
}

fn open_child_no_follow(root: &Path, name: &str, flags: libc::c_int) -> Result<File, String> {
    #[cfg(unix)]
    {
        let directory = open_directory_no_follow(root)?;
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(not(unix))]
    {
        let _ = (root, name, flags);
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }
}

fn verify_staging_upload(
    staging: &mut StagingFile,
    expected_bytes: usize,
    expected_digest: [u8; 32],
) -> Result<(), String> {
    staging.verify_identity()?;
    staging
        .file
        .sync_all()
        .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    staging
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    let mut hasher = IncrementalSha256::new()?;
    let mut total = 0_usize;
    let mut utf8_tail = Vec::new();
    let mut buffer = [0_u8; MAX_WRITE_RAW_CHUNK_BYTES];
    loop {
        let read = staging
            .file
            .read(&mut buffer)
            .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        if total > expected_bytes || total > MAX_DOCUMENT_INPUT_BYTES {
            return Err("DOCUMENT_WRITE_TOO_LARGE".to_string());
        }
        hasher.update(&buffer[..read])?;
        utf8_tail.extend_from_slice(&buffer[..read]);
        match std::str::from_utf8(&utf8_tail) {
            Ok(_) => utf8_tail.clear(),
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                utf8_tail.drain(..valid);
                if utf8_tail.len() > 3 {
                    return Err("DOCUMENT_NOT_UTF8".to_string());
                }
            }
            Err(_) => return Err("DOCUMENT_NOT_UTF8".to_string()),
        }
    }
    if total != expected_bytes || !utf8_tail.is_empty() || hasher.finalize()? != expected_digest {
        return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
    }
    staging
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    staging.verify_identity()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn supports_atomic_swap(root: &Path) -> Result<(), String> {
    let root = std::ffi::CString::new(root.as_os_str().as_bytes())
        .map_err(|_| "DOCUMENT_WRITE_ATOMIC_SWAP_UNSUPPORTED".to_string())?;
    let mut attributes = libc::attrlist {
        bitmapcount: 5,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_CAPABILITIES,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    #[repr(C)]
    struct CapabilityBuffer {
        length: u32,
        capabilities: libc::vol_capabilities_attr_t,
    }
    let mut buffer = CapabilityBuffer {
        length: 0,
        capabilities: libc::vol_capabilities_attr_t {
            capabilities: [0; 4],
            valid: [0; 4],
        },
    };
    if unsafe {
        libc::getattrlist(
            root.as_ptr(),
            (&mut attributes as *mut libc::attrlist).cast(),
            (&mut buffer as *mut CapabilityBuffer).cast(),
            std::mem::size_of::<CapabilityBuffer>(),
            0,
        )
    } != 0
    {
        return Err("DOCUMENT_WRITE_ATOMIC_SWAP_UNSUPPORTED".to_string());
    }
    let field = libc::VOL_CAPABILITIES_INTERFACES;
    let capability = libc::VOL_CAP_INT_RENAME_SWAP;
    if buffer.capabilities.valid[field] & capability == 0
        || buffer.capabilities.capabilities[field] & capability == 0
    {
        return Err("DOCUMENT_WRITE_ATOMIC_SWAP_UNSUPPORTED".to_string());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn supports_atomic_swap(_: &Path) -> Result<(), String> {
    Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
}

#[cfg(all(target_os = "macos", not(test)))]
fn coordinate_document_write<T>(
    path: &Path,
    replacing: bool,
    body: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    objc2::rc::autoreleasepool(|_| {
        let path = path
            .to_str()
            .ok_or_else(|| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let coordinator =
            NSFileCoordinator::initWithFilePresenter(NSFileCoordinator::alloc(), None);
        let result = std::cell::RefCell::new(None);
        let body = std::cell::RefCell::new(Some(body));
        let accessor = StackBlock::new(|coordinated_url: std::ptr::NonNull<NSURL>| {
            let coordinated_path = unsafe { coordinated_url.as_ref() }
                .path()
                .map(|value| PathBuf::from(value.to_string()))
                .ok_or_else(|| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
            let outcome = body
                .borrow_mut()
                .take()
                .ok_or_else(|| "DOCUMENT_WRITE_FAILED".to_string())
                .and_then(|body| coordinated_path.and_then(|path| body(&path)));
            *result.borrow_mut() = Some(outcome);
        });
        let mut error: Option<objc2::rc::Retained<NSError>> = None;
        coordinator.coordinateWritingItemAtURL_options_error_byAccessor(
            &url,
            if replacing {
                NSFileCoordinatorWritingOptions::ForReplacing
            } else {
                NSFileCoordinatorWritingOptions::empty()
            },
            Some(&mut error),
            &accessor,
        );
        if error.is_some() {
            return Err("DOCUMENT_WRITE_FAILED".to_string());
        }
        result
            .into_inner()
            .unwrap_or_else(|| Err("DOCUMENT_WRITE_FAILED".to_string()))
    })
}

// The macOS unit-test runner is an isolated command-line process, where
// NSFileCoordinator refuses all accesses before an accessor is reached. Keep
// the transaction test deterministic here; installed-app acceptance exercises
// the real coordinator path before this feature can ship.
#[cfg(all(target_os = "macos", test))]
fn coordinate_document_write<T>(
    path: &Path,
    _: bool,
    body: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    body(path)
}

#[cfg(not(target_os = "macos"))]
fn coordinate_document_write<T>(
    _: &Path,
    _: bool,
    _: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
}

struct StagedWriteExpectations {
    fingerprint: FileFingerprint,
    metadata: FileMetadataSignature,
    bytes: usize,
    digest: [u8; 32],
    replacing: bool,
}

fn commit_staged_document_write(
    root: &Path,
    name: &str,
    mut uploaded: StagingFile,
    expected: StagedWriteExpectations,
) -> Result<CommitDocumentWrite, String> {
    // The upload file is only a raw, private ingress buffer. The candidate
    // which may replace user content is deliberately created later, inside
    // NSFileCoordinator's accessor.
    verify_staging_upload(&mut uploaded, expected.bytes, expected.digest)?;
    let target = root.join(name);
    let mut result = coordinate_document_write(&target, expected.replacing, |coordinated_path| {
        let coordinated_root = coordinated_path
            .parent()
            .ok_or_else(|| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?;
        let coordinated_name = coordinated_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?;
        if coordinated_root != root || coordinated_name != name {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        commit_staged_document_write_in_accessor(
            root,
            name,
            &mut uploaded,
            expected.fingerprint.clone(),
            expected.metadata.clone(),
            expected.bytes,
            expected.digest,
        )
    });
    if let Ok(CommitDocumentWrite::RecoveryRequired { preserved }) = &mut result {
        preserved.insert(0, uploaded.root.join(&uploaded.name));
        return result;
    }
    // This upload has never been visible as the target. It can be removed only
    // after confirming the exact inode we created still occupies its name.
    if uploaded.remove_exact().is_err() {
        return Ok(recovery_preserving([uploaded.root.join(&uploaded.name)]));
    }
    result
}

#[cfg(target_os = "macos")]
fn commit_staged_document_write_in_accessor(
    root: &Path,
    name: &str,
    uploaded: &mut StagingFile,
    expected: FileFingerprint,
    expected_metadata: FileMetadataSignature,
    expected_bytes: usize,
    expected_digest: [u8; 32],
) -> Result<CommitDocumentWrite, String> {
    supports_atomic_swap(root)?;
    let mut original = open_child_no_follow(root, name, libc::O_RDONLY)?;
    let original_metadata = original
        .metadata()
        .map_err(|_| "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string())?;
    #[cfg(unix)]
    if !original_metadata.is_file() || original_metadata.nlink() != 1 {
        return Err(if original_metadata.nlink() != 1 {
            "DOCUMENT_WRITE_HARDLINK_UNSUPPORTED".to_string()
        } else {
            "DOCUMENT_WRITE_IDENTITY_CHANGED".to_string()
        });
    }
    let observed = fingerprint_file(&mut original)?;
    if observed != expected {
        return Ok(CommitDocumentWrite::Conflict);
    }
    let observed_metadata = metadata_signature(&original)?;
    if observed_metadata != expected_metadata {
        return Ok(CommitDocumentWrite::Conflict);
    }
    let mut candidate = create_staging_file(root)?;
    let result = (|| {
        copy_uploaded_content(uploaded, &mut candidate, expected_bytes)?;
        verify_staging_upload(&mut candidate, expected_bytes, expected_digest)?;
        if unsafe {
            libc::fcopyfile(
                original.as_raw_fd(),
                candidate.file.as_raw_fd(),
                std::ptr::null_mut(),
                libc::COPYFILE_METADATA,
            )
        } != 0
        {
            return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
        }
        restore_creation_time(&candidate.file, &expected_metadata)?;
        let candidate_metadata = metadata_signature(&candidate.file)?;
        if candidate_metadata != expected_metadata {
            return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
        }
        full_sync(&candidate.file)?;
        let candidate_fingerprint = fingerprint_file(&mut candidate.file)?;
        if candidate_fingerprint.length != expected_bytes as u64
            || candidate_fingerprint.sha256 != expected_digest
            || candidate_fingerprint.identity != candidate.identity
        {
            return Err("DOCUMENT_WRITE_FAILED".to_string());
        }

        // Re-read the name just before swapping: the fd still identifies the
        // original inode, but pathname identity is the conflict boundary.
        let current_target = fingerprint_path(&root.join(name))?;
        if current_target != expected {
            return Ok(CommitDocumentWrite::Conflict);
        }
        atomic_swap(root, &candidate.name, name)?;

        let swapped_target = fingerprint_path(&root.join(name));
        let displaced = fingerprint_path(&root.join(&candidate.name));
        let (Ok(swapped_target), Ok(displaced)) = (swapped_target, displaced) else {
            return Ok(recovery_preserving([
                root.join(name),
                root.join(&candidate.name),
            ]));
        };
        if swapped_target != candidate_fingerprint {
            return Ok(recovery_preserving([
                root.join(name),
                root.join(&candidate.name),
            ]));
        }
        if displaced == expected {
            if remove_exact_name(root, &candidate.name, &expected.identity).is_err() {
                return Ok(recovery_preserving([
                    root.join(name),
                    root.join(&candidate.name),
                ]));
            }
            return Ok(CommitDocumentWrite::Saved {
                fingerprint: candidate_fingerprint,
                metadata: candidate_metadata,
            });
        }

        // A writer raced the swap. Only swap back while both observed names still
        // match the exact inodes we inspected; otherwise keep every version.
        let before_rollback_target = fingerprint_path(&root.join(name));
        let before_rollback_displaced = fingerprint_path(&root.join(&candidate.name));
        if before_rollback_target.as_ref() != Ok(&candidate_fingerprint)
            || before_rollback_displaced.as_ref() != Ok(&displaced)
        {
            return Ok(recovery_preserving([
                root.join(name),
                root.join(&candidate.name),
            ]));
        }
        if atomic_swap(root, &candidate.name, name).is_err() {
            return Ok(recovery_preserving([
                root.join(name),
                root.join(&candidate.name),
            ]));
        }
        let restored_target = fingerprint_path(&root.join(name));
        let restored_candidate = fingerprint_path(&root.join(&candidate.name));
        if restored_target.as_ref() != Ok(&displaced)
            || restored_candidate.as_ref() != Ok(&candidate_fingerprint)
        {
            return Ok(recovery_preserving([
                root.join(name),
                root.join(&candidate.name),
            ]));
        }
        if remove_exact_name(root, &candidate.name, &candidate_fingerprint.identity).is_err() {
            return Ok(recovery_preserving([
                root.join(name),
                root.join(&candidate.name),
            ]));
        }
        Ok(CommitDocumentWrite::Conflict)
    })();

    // Every error and normal conflict before the swap leaves the candidate at
    // its private path. Delete it only after identity verification. A recovery
    // result may describe an already-observed post-swap state, so preserve it.
    if matches!(&result, Err(_) | Ok(CommitDocumentWrite::Conflict))
        && candidate.remove_exact().is_err()
    {
        return Ok(recovery_preserving([
            root.join(name),
            root.join(&candidate.name),
        ]));
    }
    result
}

#[cfg(not(target_os = "macos"))]
fn commit_staged_document_write_in_accessor(
    _: &Path,
    _: &str,
    _: &mut StagingFile,
    _: FileFingerprint,
    _: FileMetadataSignature,
    _: usize,
    _: [u8; 32],
) -> Result<CommitDocumentWrite, String> {
    Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
}

#[cfg(target_os = "macos")]
fn restore_creation_time(file: &File, metadata: &FileMetadataSignature) -> Result<(), String> {
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_CRTIME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut creation_time = libc::timespec {
        tv_sec: metadata.created_seconds,
        tv_nsec: metadata.created_nanoseconds,
    };
    if unsafe {
        libc::fsetattrlist(
            file.as_raw_fd(),
            (&mut attributes as *mut libc::attrlist).cast(),
            (&mut creation_time as *mut libc::timespec).cast(),
            std::mem::size_of::<libc::timespec>(),
            0,
        )
    } != 0
    {
        return Err("DOCUMENT_WRITE_METADATA_UNSUPPORTED".to_string());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn restore_creation_time(_: &File, _: &FileMetadataSignature) -> Result<(), String> {
    Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
}

fn copy_uploaded_content(
    uploaded: &mut StagingFile,
    candidate: &mut StagingFile,
    expected_bytes: usize,
) -> Result<(), String> {
    uploaded.verify_identity()?;
    candidate.verify_identity()?;
    uploaded
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    candidate
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    let mut written = 0_usize;
    let mut buffer = [0_u8; MAX_WRITE_RAW_CHUNK_BYTES];
    loop {
        let read = uploaded
            .file
            .read(&mut buffer)
            .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
        if read == 0 {
            break;
        }
        written = written.saturating_add(read);
        if written > expected_bytes {
            return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
        }
        candidate
            .file
            .write_all(&buffer[..read])
            .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    }
    if written != expected_bytes {
        return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
    }
    candidate.verify_identity()
}

#[cfg(target_os = "macos")]
fn full_sync(file: &File) -> Result<(), String> {
    file.sync_all()
        .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } != 0 {
        return Err("DOCUMENT_WRITE_FAILED".to_string());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn full_sync(_: &File) -> Result<(), String> {
    Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
}

#[cfg(target_os = "macos")]
fn commit_staged_document_create_new(
    root: &Path,
    name: &str,
    mut staging: StagingFile,
    expected_bytes: usize,
    expected_digest: [u8; 32],
) -> Result<CommitDocumentWrite, String> {
    verify_staging_upload(&mut staging, expected_bytes, expected_digest)?;
    full_sync(&staging.file)?;
    let directory = open_directory_no_follow(root)?;
    let source = std::ffi::CString::new(staging.name.as_bytes())
        .map_err(|_| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
    let target = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
    let flags = libc::RENAME_EXCL | RENAME_NOFOLLOW_ANY | RENAME_RESOLVE_BENEATH;
    if unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            target.as_ptr(),
            flags,
        )
    } != 0
    {
        return Err("DOCUMENT_SAVE_AS_FAILED".to_string());
    }
    let target_path = root.join(name);
    let verified = (|| {
        let mut file = open_child_no_follow(root, name, libc::O_RDONLY)?;
        let fingerprint = fingerprint_file(&mut file)?;
        let metadata = metadata_signature(&file)?;
        Ok::<_, String>((fingerprint, metadata))
    })();
    match verified {
        Ok((fingerprint, metadata)) => Ok(CommitDocumentWrite::Saved {
            fingerprint,
            metadata,
        }),
        Err(_) => Ok(recovery_preserving([target_path])),
    }
}

#[cfg(not(target_os = "macos"))]
fn commit_staged_document_create_new(
    _: &Path,
    _: &str,
    _: StagingFile,
    _: usize,
    _: [u8; 32],
) -> Result<CommitDocumentWrite, String> {
    Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
}

#[cfg(target_os = "macos")]
fn atomic_swap(root: &Path, left: &str, right: &str) -> Result<(), String> {
    let directory = open_directory_no_follow(root)?;
    let left =
        std::ffi::CString::new(left.as_bytes()).map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    let right = std::ffi::CString::new(right.as_bytes())
        .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string())?;
    let flags = libc::RENAME_SWAP | RENAME_NOFOLLOW_ANY | RENAME_RESOLVE_BENEATH;
    if unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            left.as_ptr(),
            directory.as_raw_fd(),
            right.as_ptr(),
            flags,
        )
    } != 0
    {
        return Err("DOCUMENT_WRITE_ATOMIC_SWAP_UNSUPPORTED".to_string());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn atomic_swap(_: &Path, _: &str, _: &str) -> Result<(), String> {
    Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
}

fn remove_exact_name(root: &Path, name: &str, expected: &FileIdentity) -> Result<(), String> {
    #[cfg(unix)]
    {
        let directory = open_directory_no_follow(root)?;
        if file_identity_at(directory.as_raw_fd(), name)? != *expected {
            return Err("DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string());
        }
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| "DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string())?;
        if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err("DOCUMENT_WRITE_RECOVERY_REQUIRED".to_string());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (root, name, expected);
        Err("DOCUMENT_WRITE_PLATFORM_UNSUPPORTED".to_string())
    }
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
fn is_supported_insert_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    ) && fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
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
    Read::by_ref(&mut file)
        .take((MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "IMAGE_READ_FAILED".to_string())?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("IMAGE_TOO_LARGE".to_string());
    }
    let (bytes, mime) = prepare_image_bytes(&extension, &bytes)?;
    Ok(PreparedImage { bytes, mime })
}

fn percent_encode_markdown_path(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                (*byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn copy_inserted_image(document_path: &Path, selected: &Path) -> Result<CopiedImage, String> {
    let source_metadata =
        fs::symlink_metadata(selected).map_err(|_| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
        return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
    }
    let extension = selected
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "IMAGE_INSERT_UNSUPPORTED_FORMAT".to_string())?;
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return Err("IMAGE_INSERT_UNSUPPORTED_FORMAT".to_string());
    }
    let source_root = fs::canonicalize(
        selected
            .parent()
            .ok_or_else(|| "IMAGE_INSERT_SOURCE_INVALID".to_string())?,
    )
    .map_err(|_| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
    let source_name = selected
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
    let prepared = prepare_local_image(&source_root, source_name).map_err(|code| {
        if code == "IMAGE_UNSUPPORTED_FORMAT" || code == "IMAGE_TYPE_MISMATCH" {
            "IMAGE_INSERT_UNSUPPORTED_FORMAT".to_string()
        } else {
            "IMAGE_INSERT_SOURCE_INVALID".to_string()
        }
    })?;
    let source_stem = selected
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    store_prepared_insert_image(document_path, source_stem, &extension, prepared)
}

#[cfg(unix)]
fn store_prepared_insert_image(
    document_path: &Path,
    source_stem: &str,
    extension: &str,
    prepared: PreparedImage,
) -> Result<CopiedImage, String> {
    let document_root = document_path
        .parent()
        .ok_or_else(|| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
    let stem = document_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
    let assets_name = format!("{stem}.assets");
    let assets_root = document_root.join(&assets_name);
    match fs::symlink_metadata(&assets_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("IMAGE_INSERT_WRITE_FAILED".to_string())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&assets_root).map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
        }
        Err(_) => return Err("IMAGE_INSERT_WRITE_FAILED".to_string()),
    }
    let directory = open_directory_no_follow(&assets_root)?;
    let alt = source_stem.to_string();
    for suffix in 0..10_000u32 {
        let name = if suffix == 0 {
            format!("{source_stem}.{extension}")
        } else {
            format!("{source_stem}-{suffix}.{extension}")
        };
        let c_name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err("IMAGE_INSERT_WRITE_FAILED".to_string());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        if file.write_all(&prepared.bytes).is_err() || full_sync(&file).is_err() {
            drop(file);
            let _ = unsafe { libc::unlinkat(directory.as_raw_fd(), c_name.as_ptr(), 0) };
            return Err("IMAGE_INSERT_WRITE_FAILED".to_string());
        }
        let metadata = file
            .metadata()
            .map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
        let identity = FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        let markdown_url = percent_encode_markdown_path(&format!("{assets_name}/{name}"));
        return Ok((assets_root, name, identity, markdown_url, alt));
    }
    Err("IMAGE_INSERT_WRITE_FAILED".to_string())
}

#[cfg(not(unix))]
fn store_prepared_insert_image(
    _: &Path,
    _: &str,
    _: &str,
    _: PreparedImage,
) -> Result<CopiedImage, String> {
    Err("IMAGE_INSERT_WRITE_FAILED".to_string())
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
fn document_edit_eligibility(
    document_id: String,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<DocumentEditEligibility, String> {
    registry
        .lock()
        .expect("document registry")
        .document_edit_eligibility(&document_id)
}

#[tauri::command]
async fn prepare_save_as(
    window: WebviewWindow,
    app: AppHandle,
    document_id: String,
) -> Result<Option<SaveAsPrepared>, String> {
    let suggested = {
        let registry = app.state::<Mutex<DocumentRegistry>>();
        let registry = registry.lock().expect("document registry");
        registry
            .documents
            .get(&document_id)
            .and_then(|context| context.path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("document.md")
            .to_string()
    };
    let selected = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || {
            app.dialog()
                .file()
                .add_filter("Markdown", &["md", "markdown", "txt"])
                .set_file_name(suggested)
                .blocking_save_file()
        }
    })
    .await
    .map_err(|_| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected = selected
        .into_path()
        .map_err(|_| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
    if !is_supported_file(&selected) {
        return Err("DOCUMENT_UNSUPPORTED_FORMAT".to_string());
    }
    let parent = selected
        .parent()
        .ok_or_else(|| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
    let root = fs::canonicalize(parent).map_err(|_| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
    let name = selected
        .file_name()
        .ok_or_else(|| "DOCUMENT_SAVE_AS_FAILED".to_string())?;
    let target = root.join(name);
    let registry = app.state::<Mutex<DocumentRegistry>>();
    let result = registry
        .lock()
        .expect("document registry")
        .register_save_as_preparation(window.label().to_string(), &document_id, target)?;
    if let Err(error) =
        start_provisional_watcher(&app, &result.token, &document_id, &root.join(name))
    {
        registry
            .lock()
            .expect("document registry")
            .cancel_save_as_preparation(&result.token);
        return Err(error);
    }
    Ok(Some(result))
}

#[tauri::command]
fn cancel_save_as(
    window: WebviewWindow,
    document_id: String,
    token: String,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    let mut registry = registry.lock().expect("document registry");
    if let Some(preparation) = registry.save_as_preparations.get(&token) {
        if preparation.window_label != window.label() || preparation.document_id != document_id {
            return Err("DOCUMENT_SAVE_AS_FAILED".to_string());
        }
    }
    registry.cancel_save_as_preparation(&token);
    Ok(())
}

#[tauri::command]
async fn prepare_recovery_save_to(
    window: WebviewWindow,
    app: AppHandle,
    request: PrepareRecoverySaveRequest,
) -> Result<Option<WriteUploadReservation>, String> {
    {
        app.state::<Mutex<DocumentRegistry>>()
            .lock()
            .expect("document registry")
            .recovery_record_for_action(
                window.label(),
                &request.record_id,
                &request.recovery_event_id,
                &request.action_token,
            )?;
    }
    let selected = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || {
            app.dialog()
                .file()
                .add_filter("Markdown", &["md", "markdown", "txt"])
                .set_file_name("recovered-content.md")
                .blocking_save_file()
        }
    })
    .await
    .map_err(|_| "RECOVERY_SAVE_FAILED".to_string())?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected = selected
        .into_path()
        .map_err(|_| "RECOVERY_SAVE_FAILED".to_string())?;
    if !is_supported_file(&selected) {
        return Err("DOCUMENT_UNSUPPORTED_FORMAT".to_string());
    }
    let parent = selected
        .parent()
        .ok_or_else(|| "RECOVERY_SAVE_FAILED".to_string())?;
    let root = fs::canonicalize(parent).map_err(|_| "RECOVERY_SAVE_FAILED".to_string())?;
    let name = selected
        .file_name()
        .ok_or_else(|| "RECOVERY_SAVE_FAILED".to_string())?;
    let target = root.join(name);
    app.state::<Mutex<DocumentRegistry>>()
        .lock()
        .expect("document registry")
        .reserve_recovery_copy(window.label().to_string(), request, target)
        .map(Some)
}

#[tauri::command]
async fn perform_recovery_action(
    window: WebviewWindow,
    app: AppHandle,
    record_id: String,
    recovery_event_id: String,
    action_token: String,
) -> Result<RecoveryActionCompleted, String> {
    let record = app
        .state::<Mutex<DocumentRegistry>>()
        .lock()
        .expect("document registry")
        .recovery_record_for_action(
            window.label(),
            &record_id,
            &recovery_event_id,
            &action_token,
        )?;
    if record.action == "revealPreservedItem" {
        let status = Command::new("open")
            .arg("-R")
            .arg(&record.path)
            .status()
            .map_err(|_| "RECOVERY_ACTION_FAILED".to_string())?;
        if !status.success() {
            return Err("RECOVERY_ACTION_FAILED".to_string());
        }
    } else {
        return Err("RECOVERY_ACTION_REQUIRES_UPLOAD".to_string());
    }
    app.state::<Mutex<DocumentRegistry>>()
        .lock()
        .expect("document registry")
        .complete_recovery_action(&record_id, None)
}

#[tauri::command]
fn refresh_recovery_action(
    window: WebviewWindow,
    record_id: String,
    recovery_event_id: String,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<RecoveryActionRefreshed, String> {
    registry
        .lock()
        .expect("document registry")
        .refresh_recovery_action(window.label(), &record_id, &recovery_event_id)
}

#[tauri::command]
fn acknowledge_recovery_record(
    window: WebviewWindow,
    record_id: String,
    recovery_event_id: String,
    ack_token: String,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<RecoveryAcknowledged, String> {
    registry
        .lock()
        .expect("document registry")
        .acknowledge_recovery(window.label(), &record_id, &recovery_event_id, &ack_token)
}

#[tauri::command]
fn request_recovery_ack(
    window: WebviewWindow,
    record_id: String,
    recovery_event_id: String,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<RecoveryActionCompleted, String> {
    registry
        .lock()
        .expect("document registry")
        .refresh_recovery_ack(window.label(), &record_id, &recovery_event_id)
}

#[tauri::command]
async fn insert_image_dialog(
    window: WebviewWindow,
    app: AppHandle,
    document_id: String,
    context_epoch: u64,
) -> Result<Option<ImageInsertResult>, String> {
    let document_path = {
        let registry = app.state::<Mutex<DocumentRegistry>>();
        let registry = registry.lock().expect("document registry");
        let context = registry
            .documents
            .get(&document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.context_epoch != context_epoch {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        context.path.clone()
    };
    let selected = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || {
            app.dialog()
                .file()
                .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
                .blocking_pick_file()
        }
    })
    .await
    .map_err(|_| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected = selected
        .into_path()
        .map_err(|_| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
    let copied = tauri::async_runtime::spawn_blocking(move || {
        copy_inserted_image(&document_path, &selected)
    })
    .await
    .map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())??;
    let registry = app.state::<Mutex<DocumentRegistry>>();
    let result = registry
        .lock()
        .expect("document registry")
        .register_image_insert(
            window.label().to_string(),
            document_id,
            context_epoch,
            copied,
        )?;
    Ok(Some(result))
}

#[tauri::command]
async fn insert_image_from_grant(
    window: WebviewWindow,
    app: AppHandle,
    document_id: String,
    context_epoch: u64,
    grant_token: String,
) -> Result<ImageInsertResult, String> {
    let (document_path, selected) = {
        let registry = app.state::<Mutex<DocumentRegistry>>();
        let mut registry = registry.lock().expect("document registry");
        registry
            .image_source_grants
            .retain(|_, candidate| candidate.expires_at > Instant::now());
        let grant = registry
            .image_source_grants
            .remove(&grant_token)
            .ok_or_else(|| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
        if grant.token != grant_token
            || grant.window_label != window.label()
            || grant.expires_at <= Instant::now()
        {
            return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
        }
        let context = registry
            .documents
            .get(&document_id)
            .ok_or_else(|| "DOCUMENT_NOT_FOUND".to_string())?;
        if context.context_epoch != context_epoch {
            return Err("DOCUMENT_WRITE_IDENTITY_CHANGED".to_string());
        }
        (context.path.clone(), grant.path)
    };
    let copied = tauri::async_runtime::spawn_blocking(move || {
        copy_inserted_image(&document_path, &selected)
    })
    .await
    .map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())??;
    let registry = app.state::<Mutex<DocumentRegistry>>();
    let result = registry
        .lock()
        .expect("document registry")
        .register_image_insert(
            window.label().to_string(),
            document_id,
            context_epoch,
            copied,
        );
    result
}

#[tauri::command]
fn finalize_image_insert(
    window: WebviewWindow,
    document_id: String,
    context_epoch: u64,
    rollback_token: String,
    committed: bool,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    let rollback = registry
        .lock()
        .expect("document registry")
        .image_insert_rollbacks
        .remove(&rollback_token)
        .ok_or_else(|| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
    if rollback.token != rollback_token
        || rollback.window_label != window.label()
        || rollback.document_id != document_id
        || rollback.context_epoch != context_epoch
        || rollback.expires_at <= Instant::now()
    {
        return Err("IMAGE_INSERT_WRITE_FAILED".to_string());
    }
    if !committed {
        remove_exact_name(&rollback.root, &rollback.name, &rollback.identity)?;
    }
    Ok(())
}

#[tauri::command]
fn reserve_document_write(
    window: WebviewWindow,
    request: ReserveDocumentWriteRequest,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<WriteUploadReservation, String> {
    registry
        .lock()
        .expect("document registry")
        .reserve_document_write(window.label().to_string(), request)
}

#[tauri::command]
fn reserve_clipboard_image(
    window: WebviewWindow,
    request: ReserveClipboardImageRequest,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<WriteUploadReservation, String> {
    registry
        .lock()
        .expect("document registry")
        .reserve_clipboard_image(window.label().to_string(), request)
}

#[tauri::command]
fn append_clipboard_image_chunk(
    window: WebviewWindow,
    request: tauri::ipc::Request<'_>,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    let upload_id = required_raw_write_header(&request, "x-yuyue-upload-id")?;
    let token = required_raw_write_header(&request, "x-yuyue-upload-token")?;
    let sequence = required_raw_write_header(&request, "x-yuyue-upload-sequence")?
        .parse::<u64>()
        .map_err(|_| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
    let claimed_length = required_raw_write_header(&request, "x-yuyue-upload-length")?
        .parse::<usize>()
        .map_err(|_| "IMAGE_INSERT_SOURCE_INVALID".to_string())?;
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
    };
    registry
        .lock()
        .expect("document registry")
        .append_clipboard_image_chunk(
            window.label(),
            upload_id,
            token,
            sequence,
            claimed_length,
            bytes,
        )
}

fn commit_clipboard_image_upload(
    mut upload: ClipboardImageUpload,
    document_path: PathBuf,
) -> Result<(ClipboardImageUpload, CopiedImage), String> {
    let copied = (|| {
        upload.staging.verify_identity()?;
        upload
            .staging
            .file
            .seek(SeekFrom::Start(0))
            .map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
        let mut bytes = Vec::with_capacity(upload.expected_bytes);
        upload
            .staging
            .file
            .read_to_end(&mut bytes)
            .map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())?;
        if bytes.len() != upload.expected_bytes {
            return Err("IMAGE_INSERT_SOURCE_INVALID".to_string());
        }
        let (bytes, mime) = prepare_image_bytes(&upload.extension, &bytes)
            .map_err(|_| "IMAGE_INSERT_UNSUPPORTED_FORMAT".to_string())?;
        store_prepared_insert_image(
            &document_path,
            if upload.source_stem.is_empty() {
                "clipboard-image"
            } else {
                &upload.source_stem
            },
            &upload.extension,
            PreparedImage { bytes, mime },
        )
    })();
    let cleanup = upload.staging.remove_exact();
    let copied = copied?;
    cleanup?;
    Ok((upload, copied))
}

#[tauri::command]
async fn finalize_clipboard_image(
    window: WebviewWindow,
    request: BeginDocumentWriteRequest,
    app: AppHandle,
) -> Result<ImageInsertResult, String> {
    let (upload, document_path) = app
        .state::<Mutex<DocumentRegistry>>()
        .lock()
        .expect("document registry")
        .take_clipboard_image_for_finalize(window.label(), request)?;
    let (upload, copied) = tauri::async_runtime::spawn_blocking(move || {
        commit_clipboard_image_upload(upload, document_path)
    })
    .await
    .map_err(|_| "IMAGE_INSERT_WRITE_FAILED".to_string())??;
    app.state::<Mutex<DocumentRegistry>>()
        .lock()
        .expect("document registry")
        .register_image_insert(
            window.label().to_string(),
            upload.document_id,
            upload.context_epoch,
            copied,
        )
}

#[tauri::command]
fn cancel_clipboard_image(
    window: WebviewWindow,
    request: BeginDocumentWriteRequest,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    registry
        .lock()
        .expect("document registry")
        .cancel_clipboard_image(window.label(), request)
}

#[tauri::command]
fn begin_document_write(
    window: WebviewWindow,
    request: BeginDocumentWriteRequest,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    registry
        .lock()
        .expect("document registry")
        .begin_document_write(window.label(), request)
}

fn required_raw_write_header<'a>(
    request: &'a tauri::ipc::Request<'_>,
    name: &str,
) -> Result<&'a str, String> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "DOCUMENT_WRITE_PROTOCOL".to_string())
}

#[tauri::command]
fn append_document_write_chunk(
    window: WebviewWindow,
    request: tauri::ipc::Request<'_>,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    (|| {
        let upload_id = required_raw_write_header(&request, "x-yuyue-upload-id")?;
        let token = required_raw_write_header(&request, "x-yuyue-upload-token")?;
        let sequence = required_raw_write_header(&request, "x-yuyue-upload-sequence")?
            .parse::<u64>()
            .map_err(|_| "DOCUMENT_WRITE_PROTOCOL".to_string())?;
        let claimed_length = required_raw_write_header(&request, "x-yuyue-upload-length")?
            .parse::<usize>()
            .map_err(|_| "DOCUMENT_WRITE_PROTOCOL".to_string())?;
        let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
            return Err("DOCUMENT_WRITE_PROTOCOL".to_string());
        };
        registry
            .lock()
            .expect("document registry")
            .append_document_write_chunk(
                window.label(),
                upload_id,
                token,
                sequence,
                claimed_length,
                bytes,
            )
    })()
}

#[tauri::command]
async fn finalize_document_write(
    window: WebviewWindow,
    request: FinalizeDocumentWriteRequest,
    app: AppHandle,
) -> Result<DocumentWriteOutcome, String> {
    let mut upload = {
        let state = app.state::<Mutex<DocumentRegistry>>();
        let mut registry = state.lock().expect("document registry");
        let upload = registry.take_document_write_for_finalize(window.label(), &request)?;
        if !registry
            .finalizing_documents
            .insert(upload.document_id.clone())
        {
            return Err("DOCUMENT_WRITE_BUSY".to_string());
        }
        upload
    };
    let document_id = upload.document_id.clone();
    let upload_id = upload.upload_id.clone();
    let provisional_token = match &upload.target {
        WriteTarget::SaveAs {
            preparation_token, ..
        } => Some(preparation_token.clone()),
        WriteTarget::Original { .. } | WriteTarget::RecoveryCopy { .. } => None,
    };
    let blocking = tauri::async_runtime::spawn_blocking(move || {
        let outcome = commit_document_upload(&mut upload);
        (upload, outcome)
    })
    .await
    .map_err(|_| "DOCUMENT_WRITE_FAILED".to_string());
    let state = app.state::<Mutex<DocumentRegistry>>();
    let mut registry = state.lock().expect("document registry");
    registry.finalizing_documents.remove(&document_id);
    WATCH_READ_GATE.1.notify_all();
    registry.release_document_write_permit(&upload_id);
    let (upload, outcome) = match blocking {
        Ok(value) => value,
        Err(error) => {
            if let Some(token) = provisional_token.as_deref() {
                cancel_provisional_watcher(token);
            }
            return Err(error);
        }
    };
    let outcome = match outcome {
        Ok(value) => value,
        Err(error) => {
            if let Some(token) = provisional_token.as_deref() {
                cancel_provisional_watcher(token);
            }
            return Err(error);
        }
    };
    let Some(context) = registry.documents.get(&document_id) else {
        return Err("DOCUMENT_NOT_FOUND".to_string());
    };
    let recovery_copy = matches!(&upload.target, WriteTarget::RecoveryCopy { .. });
    if context.context_epoch != upload.context_epoch
        || !recovery_copy
            && (context.revision != upload.expected_revision
                || context.fingerprint != upload.expected_fingerprint)
    {
        if let Some(token) = provisional_token.as_deref() {
            cancel_provisional_watcher(token);
        }
        return registry.document_write_conflict(&document_id, upload.context_epoch);
    }
    let result = registry.apply_document_write_commit(upload, outcome);
    drop(registry);
    if matches!(result, Ok(DocumentWriteOutcome::ContextRebound { .. })) {
        if let Some(token) = provisional_token.as_deref() {
            activate_provisional_watcher(&app, token, &document_id);
        }
    } else if let Some(token) = provisional_token.as_deref() {
        cancel_provisional_watcher(token);
    }
    result
}

#[tauri::command]
fn cancel_document_write(
    window: WebviewWindow,
    request: BeginDocumentWriteRequest,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    let mut registry = registry.lock().expect("document registry");
    registry.write_upload(&request.upload_id, &request.token, window.label())?;
    registry.cancel_document_write(&request.upload_id);
    Ok(())
}

#[tauri::command]
fn refresh_document_conflict(
    document_id: String,
    context_epoch: u64,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<ConflictRefresh, String> {
    registry
        .lock()
        .expect("document registry")
        .refresh_document_conflict(&document_id, context_epoch)
}

#[tauri::command]
async fn resolve_image_source(
    app: AppHandle,
    document_id: String,
    source: String,
    allow_remote: bool,
    render_lease_id: String,
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
                .local_image_resolution(&document_id, &source, &render_lease_id)?;
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
                .store_prepared_image(&document_id, identity, prepared, &render_lease_id)
        }
        ImageSourceClass::Data(source) => {
            let (identity, prepared) =
                tauri::async_runtime::spawn_blocking(move || prepare_data_image(&source))
                    .await
                    .map_err(|_| "IMAGE_DATA_INVALID".to_string())??;
            app.state::<Mutex<DocumentRegistry>>()
                .lock()
                .expect("document registry")
                .store_prepared_image(&document_id, identity, prepared, &render_lease_id)
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

#[tauri::command]
fn release_render_lease(
    document_id: String,
    render_lease_id: String,
    registry: State<'_, Mutex<DocumentRegistry>>,
) -> Result<(), String> {
    registry
        .lock()
        .expect("document registry")
        .release_render_lease(&document_id, &render_lease_id)
}

fn schedule_watcher_read(app: AppHandle, document_id: String) {
    let (gate_lock, _) = &*WATCH_READ_GATE;
    {
        let mut gate = gate_lock.lock().expect("watch read gate");
        if !gate.scheduled_documents.insert(document_id.clone()) {
            gate.pending_documents.insert(document_id);
            return;
        }
    }
    std::thread::spawn(move || {
        let (gate_lock, gate_signal) = &*WATCH_READ_GATE;
        loop {
            let mut gate = gate_lock.lock().expect("watch read gate");
            while gate.active >= 2
                || gate.reserved_bytes.saturating_add(MAX_DOCUMENT_INPUT_BYTES) > 20 * 1024 * 1024
            {
                gate = gate_signal.wait(gate).expect("watch read gate");
            }
            gate.active += 1;
            gate.reserved_bytes += MAX_DOCUMENT_INPUT_BYTES;
            gate.pending_documents.remove(&document_id);
            drop(gate);

            while app
                .state::<Mutex<DocumentRegistry>>()
                .lock()
                .is_ok_and(|registry| registry.finalizing_documents.contains(&document_id))
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            let target = app
                .state::<Mutex<DocumentRegistry>>()
                .lock()
                .ok()
                .and_then(|registry| registry.watcher_read_target(&document_id).ok());
            let change = target.and_then(|(path, context_epoch)| {
                let observed = fingerprint_path(&path).ok()?;
                let metadata = open_document_no_follow(&path)
                    .and_then(|file| metadata_signature(&file))
                    .ok();
                app.state::<Mutex<DocumentRegistry>>()
                    .lock()
                    .ok()
                    .and_then(|mut registry| {
                        registry
                            .apply_file_changed(&document_id, context_epoch, observed, metadata)
                            .ok()
                    })
            });
            if let Some(Some((source_revision, conflict_token))) = change {
                let _ = app.emit(
                    "file-changed",
                    FileChangedEvent {
                        document_id: document_id.clone(),
                        source_revision,
                        conflict_token: Some(conflict_token),
                    },
                );
            }

            let mut gate = gate_lock.lock().expect("watch read gate");
            gate.active = gate.active.saturating_sub(1);
            gate.reserved_bytes = gate.reserved_bytes.saturating_sub(MAX_DOCUMENT_INPUT_BYTES);
            let rerun = gate.pending_documents.remove(&document_id);
            if !rerun {
                gate.scheduled_documents.remove(&document_id);
            }
            gate_signal.notify_all();
            if !rerun {
                break;
            }
        }
    });
}

fn start_provisional_watcher(
    app: &AppHandle,
    token: &str,
    document_id: &str,
    path: &Path,
) -> Result<(), String> {
    let activation: Arc<Mutex<Option<(AppHandle, String)>>> = Arc::new(Mutex::new(None));
    let dirty = Arc::new(AtomicBool::new(false));
    let callback_activation = activation.clone();
    let callback_dirty = dirty.clone();
    let target_path = path.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: Result<notify::Event, _>| {
        let Ok(event) = event else { return };
        let relevant = event.paths.iter().any(|candidate| {
            candidate == &target_path
                || candidate.file_name().is_some()
                    && candidate.file_name() == target_path.file_name()
        });
        if !relevant
            || !matches!(
                event.kind,
                EventKind::Modify(ModifyKind::Data(_))
                    | EventKind::Modify(ModifyKind::Name(_))
                    | EventKind::Create(_)
                    | EventKind::Remove(_)
            )
        {
            return;
        }
        callback_dirty.store(true, Ordering::SeqCst);
        if let Ok(active) = callback_activation.lock() {
            if let Some((app, document_id)) = active.as_ref() {
                schedule_watcher_read(app.clone(), document_id.clone());
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
    PROVISIONAL_WATCHERS
        .lock()
        .map_err(|_| "DOCUMENT_WATCH_FAILED".to_string())?
        .insert(
            token.to_string(),
            ProvisionalWatcher {
                watcher,
                activation,
                dirty,
            },
        );
    let _ = (app, document_id);
    Ok(())
}

fn activate_provisional_watcher(app: &AppHandle, token: &str, document_id: &str) {
    let provisional = PROVISIONAL_WATCHERS
        .lock()
        .expect("provisional watchers")
        .remove(token)
        .expect("prepared watcher must remain ready until commit");
    *provisional
        .activation
        .lock()
        .expect("provisional watcher activation") = Some((app.clone(), document_id.to_string()));
    FILE_WATCHERS
        .lock()
        .expect("file watchers")
        .insert(document_id.to_string(), provisional.watcher);
    if provisional.dirty.load(Ordering::SeqCst) {
        schedule_watcher_read(app.clone(), document_id.to_string());
    }
}

fn cancel_provisional_watcher(token: &str) {
    PROVISIONAL_WATCHERS
        .lock()
        .expect("provisional watchers")
        .remove(token);
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
                schedule_watcher_read(app.clone(), watcher_document_id.clone());
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppExitAttemptStarted {
    attempt_id: String,
}

#[tauri::command]
fn confirm_app_exit(app: AppHandle) -> Result<AppExitAttemptStarted, String> {
    let attempt = APP_EXIT_ATTEMPT
        .lock()
        .map_err(|_| "APP_EXIT_FAILED".to_string())?
        .begin(Instant::now())?;
    let attempt_id = attempt.id.clone();
    let expiration_id = attempt.id.clone();
    let expiration_app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(APP_EXIT_ATTEMPT_TTL);
        let expired = APP_EXIT_ATTEMPT
            .lock()
            .map(|mut gate| gate.expire_if_matches(&expiration_id))
            .unwrap_or(false);
        if expired {
            if let Some(window) = expiration_app.get_webview_window("main") {
                let _ = window.emit(
                    "app-exit-attempt-expired",
                    AppExitAttemptStarted {
                        attempt_id: expiration_id,
                    },
                );
            }
        }
    });
    app.exit(0);
    Ok(AppExitAttemptStarted { attempt_id })
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod local_image_tests {
    use super::*;

    #[test]
    fn app_exit_attempt_is_bound_and_old_expiration_cannot_clear_a_new_attempt() {
        let now = Instant::now();
        let mut gate = AppExitAttemptGate::default();
        let first = gate.begin(now).expect("first exit attempt");
        assert!(gate.allows_current_attempt(now));
        assert!(matches!(gate.begin(now), Err(error) if error == "APP_EXIT_ALREADY_PENDING"));
        assert!(gate.expire_if_matches(&first.id));

        let second = gate.begin(now).expect("second exit attempt");
        assert_ne!(first.id, second.id);
        assert!(!gate.expire_if_matches(&first.id));
        assert!(gate.allows_current_attempt(now));
        assert!(gate.expire_if_matches(&second.id));
        assert!(!gate.allows_current_attempt(now));
    }

    #[test]
    fn app_exit_timer_remains_the_only_expiration_owner_in_both_race_orders() {
        let now = Instant::now();
        let after_ttl = now + APP_EXIT_ATTEMPT_TTL + Duration::from_millis(1);

        let mut event_first = AppExitAttemptGate::default();
        let event_first_attempt = event_first.begin(now).expect("event-first attempt");
        assert!(!event_first.allows_current_attempt(after_ttl));
        assert_eq!(
            event_first
                .current
                .as_ref()
                .map(|attempt| attempt.id.as_str()),
            Some(event_first_attempt.id.as_str())
        );
        assert!(event_first.expire_if_matches(&event_first_attempt.id));

        let mut timer_first = AppExitAttemptGate::default();
        let timer_first_attempt = timer_first.begin(now).expect("timer-first attempt");
        assert!(timer_first.expire_if_matches(&timer_first_attempt.id));
        assert!(!timer_first.allows_current_attempt(after_ttl));
    }

    #[test]
    fn document_write_outcome_uses_the_frontend_camel_case_contract() {
        let value = serde_json::to_value(DocumentWriteOutcome::Saved {
            document_id: "doc-a".to_string(),
            context_epoch: 2,
            source_revision: 4,
            commit_id: "commit-a".to_string(),
            write_generation: 3,
        })
        .expect("serialize document write outcome");

        assert_eq!(value["kind"], "saved");
        assert_eq!(value["documentId"], "doc-a");
        assert_eq!(value["contextEpoch"], 2);
        assert_eq!(value["sourceRevision"], 4);
        assert_eq!(value["commitId"], "commit-a");
        assert_eq!(value["writeGeneration"], 3);
        assert!(value.get("write_generation").is_none());
    }

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
                context_epoch: 0,
                revision: 0,
                fingerprint: FileFingerprint::default(),
                metadata: None,
                self_write_ledger: VecDeque::new(),
                conflict_token: None,
                resources: HashMap::new(),
                released_render_leases: HashSet::new(),
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
        let (changed_revision, _conflict_token) = registry
            .mark_file_changed(&document.document_id)
            .expect("mark file changed")
            .expect("external change must advance revision");
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
    fn duplicate_watcher_notifications_for_one_external_fingerprint_are_coalesced() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-duplicate-watch-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        fs::write(&document_path, "# before").expect("test markdown");
        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path.clone())
            .expect("open document");

        fs::write(&document_path, "# one external change").expect("external update");
        let first = registry
            .mark_file_changed(&document.document_id)
            .expect("first watcher notification");
        let second = registry
            .mark_file_changed(&document.document_id)
            .expect("duplicate watcher notification");

        assert!(first.is_some());
        assert!(second.is_none());
        assert_eq!(
            registry.documents[&document.document_id].revision,
            first.expect("first revision").0
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn a_consumed_self_write_fingerprint_cannot_hide_a_later_external_replay() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-ledger-replay-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        fs::write(&document_path, "# before").expect("test markdown");
        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path)
            .expect("open document");
        let context = registry
            .documents
            .get_mut(&document.document_id)
            .expect("document context");
        let before = context.fingerprint.clone();
        let mut self_written = before.clone();
        self_written.length = self_written.length.saturating_add(1);
        self_written.sha256[0] ^= 0xff;
        context.self_write_ledger.push_back(SelfWriteIntent {
            context_epoch: context.context_epoch,
            commit_id: "self-write".to_string(),
            before,
            after: self_written.clone(),
            source_revision: context.revision,
            created_at: Instant::now(),
        });

        assert!(registry
            .apply_file_changed(&document.document_id, 0, self_written.clone(), None)
            .expect("consume self write")
            .is_none());
        assert!(registry.documents[&document.document_id]
            .self_write_ledger
            .is_empty());
        let mut intervening_external = self_written.clone();
        intervening_external.sha256[1] ^= 0xff;
        registry
            .documents
            .get_mut(&document.document_id)
            .expect("document context")
            .fingerprint = intervening_external;
        assert!(registry
            .apply_file_changed(&document.document_id, 0, self_written, None)
            .expect("external replay")
            .is_some());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn raw_document_write_preserves_the_document_context_and_consumes_its_own_watcher_event() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-raw-write-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        fs::write(&document_path, "# before\n").expect("test markdown");

        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path.clone())
            .expect("open document");
        let content = "# after\n含有 UTF-8 😀\n".as_bytes();
        let reservation = registry
            .reserve_document_write(
                "test-webview".to_string(),
                ReserveDocumentWriteRequest {
                    document_id: document.document_id.clone(),
                    context_epoch: 0,
                    commit_id: "commit-a".to_string(),
                    write_generation: 1,
                    content_utf8_bytes: content.len(),
                    conflict_token: None,
                    save_as_token: None,
                },
            )
            .expect("reserve write");
        registry
            .begin_document_write(
                "test-webview",
                BeginDocumentWriteRequest {
                    upload_id: reservation.upload_id.clone(),
                    token: reservation.token.clone(),
                },
            )
            .expect("begin write");
        registry
            .append_document_write_chunk(
                "test-webview",
                &reservation.upload_id,
                &reservation.token,
                0,
                content.len(),
                content,
            )
            .expect("append raw bytes");
        let upload = registry
            .take_document_write_for_finalize(
                "test-webview",
                &FinalizeDocumentWriteRequest {
                    upload_id: reservation.upload_id,
                    token: reservation.token,
                },
            )
            .expect("finalize admission");
        let outcome = registry.finish_document_write(upload).expect("atomic save");

        assert!(matches!(
            outcome,
            DocumentWriteOutcome::Saved {
                source_revision: 1,
                ..
            }
        ));
        assert_eq!(
            fs::read_to_string(&document_path).expect("read saved document"),
            "# after\n含有 UTF-8 😀\n"
        );
        assert!(registry
            .mark_file_changed(&document.document_id)
            .expect("watch self write")
            .is_none());
        assert!(fs::read_dir(&directory)
            .expect("read test directory")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".yuyue-")));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn save_as_create_new_keeps_the_logical_document_and_advances_its_context() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-save-as-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let original = directory.join("original.md");
        let target = directory.join("副本 document.md");
        fs::write(&original, "# original\n").expect("original markdown");
        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(original.clone())
            .expect("open document");
        let prepared = registry
            .register_save_as_preparation(
                "test-webview".to_string(),
                &document.document_id,
                target.clone(),
            )
            .expect("prepare save as");
        let content = "# saved as\n中文 😀\n".as_bytes();
        let reservation = registry
            .reserve_document_write(
                "test-webview".to_string(),
                ReserveDocumentWriteRequest {
                    document_id: document.document_id.clone(),
                    context_epoch: 0,
                    commit_id: "save-as-commit".to_string(),
                    write_generation: 0,
                    content_utf8_bytes: content.len(),
                    conflict_token: None,
                    save_as_token: Some(prepared.token),
                },
            )
            .expect("reserve save as");
        registry
            .begin_document_write(
                "test-webview",
                BeginDocumentWriteRequest {
                    upload_id: reservation.upload_id.clone(),
                    token: reservation.token.clone(),
                },
            )
            .expect("begin save as");
        registry
            .append_document_write_chunk(
                "test-webview",
                &reservation.upload_id,
                &reservation.token,
                0,
                content.len(),
                content,
            )
            .expect("append save as bytes");
        let upload = registry
            .take_document_write_for_finalize(
                "test-webview",
                &FinalizeDocumentWriteRequest {
                    upload_id: reservation.upload_id,
                    token: reservation.token,
                },
            )
            .expect("finalize save as upload");
        let outcome = registry
            .finish_document_write(upload)
            .expect("commit save as");

        assert!(matches!(
            outcome,
            DocumentWriteOutcome::ContextRebound {
                context_epoch: 1,
                ..
            }
        ));
        assert_eq!(
            fs::read_to_string(&original).expect("old file remains"),
            "# original\n"
        );
        assert_eq!(
            fs::read_to_string(&target).expect("new file"),
            "# saved as\n中文 😀\n"
        );
        let context = registry
            .documents
            .get(&document.document_id)
            .expect("same logical document");
        assert_eq!(context.path, target);
        assert_eq!(context.context_epoch, 1);
        assert!(registry
            .mark_file_changed(&document.document_id)
            .expect("replay provisional watcher event")
            .is_none());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn edit_eligibility_fails_closed_for_a_hard_link() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-eligibility-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        fs::write(&document_path, "# linked\n").expect("document");
        fs::hard_link(&document_path, directory.join("other.md")).expect("hard link");
        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path)
            .expect("open document");

        let eligibility = registry
            .document_edit_eligibility(&document.document_id)
            .expect("eligibility result");

        assert!(!eligibility.eligible);
        assert_eq!(
            eligibility.reason.as_deref(),
            Some("DOCUMENT_WRITE_HARDLINK_UNSUPPORTED")
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn inserted_image_uses_the_derived_assets_directory_and_exact_rollback() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-image-insert-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document = directory.join("中文 文档.md");
        fs::write(&document, "# image\n").expect("document");
        let source = directory.join("配图 空格.png");
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .expect("png fixture");
        let png = png.into_inner();
        fs::write(&source, png).expect("source image");

        let (root, name, identity, markdown_url, alt) =
            copy_inserted_image(&document, &source).expect("copy image");
        assert_eq!(root, directory.join("中文 文档.assets"));
        assert_eq!(alt, "配图 空格");
        assert!(markdown_url.starts_with("%E4%B8%AD%E6%96%87%20%E6%96%87%E6%A1%A3.assets/"));
        assert!(root.join(&name).is_file());
        remove_exact_name(&root, &name, &identity).expect("exact rollback");
        assert!(!root.join(name).exists());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn inserted_image_rejects_svg_and_an_assets_symlink() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-image-reject-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document = directory.join("document.md");
        fs::write(&document, "# image\n").expect("document");
        let svg = directory.join("image.svg");
        fs::write(&svg, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").expect("svg");
        assert_eq!(
            copy_inserted_image(&document, &svg).map(|_| ()),
            Err("IMAGE_INSERT_UNSUPPORTED_FORMAT".to_string())
        );

        let outside = directory.join("outside");
        fs::create_dir(&outside).expect("outside");
        std::os::unix::fs::symlink(&outside, directory.join("document.assets"))
            .expect("assets symlink");
        let png = directory.join("image.png");
        let mut png_bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut png_bytes, ImageFormat::Png)
            .expect("png fixture");
        fs::write(&png, png_bytes.into_inner()).expect("png");
        assert_eq!(
            copy_inserted_image(&document, &png).map(|_| ()),
            Err("IMAGE_INSERT_WRITE_FAILED".to_string())
        );
        assert!(fs::read_dir(&outside)
            .expect("outside empty")
            .next()
            .is_none());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clipboard_image_uses_bounded_raw_chunks_and_releases_its_ingress_permit() {
        let directory =
            std::env::temp_dir().join(format!("mdreader-clipboard-image-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("test directory");
        let document_path = directory.join("document.md");
        fs::write(&document_path, "# clipboard\n").expect("document");
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("png fixture");
        let bytes = bytes.into_inner();
        let mut registry = DocumentRegistry::default();
        let document = registry
            .open_document(document_path)
            .expect("open document");
        let reservation = registry
            .reserve_clipboard_image(
                "test-webview".to_string(),
                ReserveClipboardImageRequest {
                    document_id: document.document_id.clone(),
                    context_epoch: 0,
                    extension: "png".to_string(),
                    source_stem: "剪贴板 图片".to_string(),
                    content_bytes: bytes.len(),
                },
            )
            .expect("reserve clipboard image");
        for (sequence, chunk) in bytes.chunks(17).enumerate() {
            registry
                .append_clipboard_image_chunk(
                    "test-webview",
                    &reservation.upload_id,
                    &reservation.token,
                    sequence as u64,
                    chunk.len(),
                    chunk,
                )
                .expect("append clipboard chunk");
        }
        let (upload, document_path) = registry
            .take_clipboard_image_for_finalize(
                "test-webview",
                BeginDocumentWriteRequest {
                    upload_id: reservation.upload_id,
                    token: reservation.token,
                },
            )
            .expect("take clipboard image");
        let (upload, copied) = commit_clipboard_image_upload(upload, document_path)
            .expect("prepare clipboard image outside registry lock");
        let inserted = registry
            .register_image_insert(
                "test-webview".to_string(),
                upload.document_id,
                upload.context_epoch,
                copied,
            )
            .expect("finalize clipboard image");
        assert!(inserted
            .markdown_url
            .contains("%E5%89%AA%E8%B4%B4%E6%9D%BF%20%E5%9B%BE%E7%89%87.png"));
        assert!(registry.write_coordinator.active_upload.is_none());
        assert_eq!(registry.write_coordinator.reserved_bytes, 0);
        let rollback = registry
            .image_insert_rollbacks
            .remove(&inserted.rollback_token)
            .expect("rollback token");
        remove_exact_name(&rollback.root, &rollback.name, &rollback.identity)
            .expect("rollback copied image");
        assert!(fs::read_dir(&directory)
            .expect("directory")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".yuyue-")));
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
                context_epoch: 0,
                revision: 0,
                fingerprint: FileFingerprint::default(),
                metadata: None,
                self_write_ledger: VecDeque::new(),
                conflict_token: None,
                resources: HashMap::new(),
                released_render_leases: HashSet::new(),
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
                context_epoch: 0,
                revision: 0,
                fingerprint: FileFingerprint::default(),
                metadata: None,
                self_write_ledger: VecDeque::new(),
                conflict_token: None,
                resources: HashMap::new(),
                released_render_leases: HashSet::new(),
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
                    context_epoch: 0,
                    revision: 0,
                    fingerprint: FileFingerprint::default(),
                    metadata: None,
                    self_write_ledger: VecDeque::new(),
                    conflict_token: None,
                    resources: HashMap::new(),
                    released_render_leases: HashSet::new(),
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
            .local_image_resolution(&document.document_id, "image.svg", "lease-test")
            .expect("resolve local image");
        assert!(resolution.cached_url.is_none());

        let prepared = prepare_local_image(&resolution.root, &resolution.identity)
            .expect("prepare image without the registry");
        assert_eq!(registry.total_resources, 0);
        let resolved = registry
            .store_prepared_image(
                &document.document_id,
                resolution.identity,
                prepared,
                "lease-test",
            )
            .expect("commit prepared image");

        assert!(resolved.url.starts_with("mdreader-image://localhost/"));
        assert_eq!(registry.total_resources, 1);
        let late_resolution = registry
            .local_image_resolution(&document.document_id, "image.svg", "lease-late")
            .expect("late resolution");
        registry
            .release_render_lease(&document.document_id, "lease-late")
            .expect("release before hydration completes");
        let late_prepared = prepare_local_image(&late_resolution.root, &late_resolution.identity)
            .expect("prepare late image");
        assert!(matches!(
            registry.store_prepared_image(
                &document.document_id,
                late_resolution.identity,
                late_prepared,
                "lease-late",
            ),
            Err(error) if error == "RENDER_LEASE_RELEASED"
        ));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn reload_keeps_visible_resource_snapshots_until_the_render_lease_is_released() {
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
        assert!(registry
            .local_image_resolution(&document.document_id, "image.svg", "lease-next")
            .expect("attach next render lease")
            .cached_url
            .is_some());

        fs::write(&document_path, "# refreshed").expect("updated markdown");
        registry
            .reload_document(&document.document_id, None)
            .expect("reload document");

        assert_eq!(registry.total_resources, 1);
        assert_ne!(registry.total_bytes, 0);
        assert_eq!(
            serve_image(&registry, &protocol_path).status(),
            http::StatusCode::OK
        );
        registry
            .release_render_lease(&document.document_id, "lease-legacy")
            .expect("release old render lease");
        assert_eq!(registry.total_resources, 1);
        assert_eq!(
            serve_image(&registry, &protocol_path).status(),
            http::StatusCode::OK
        );
        registry
            .release_render_lease(&document.document_id, "lease-next")
            .expect("release next render lease");
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

    #[test]
    fn recovery_ack_is_one_time_bound_and_resolves_only_after_every_record() {
        let mut registry = DocumentRegistry::default();
        for (index, id) in ["first", "second"].into_iter().enumerate() {
            registry.recovery_records.insert(
                id.to_string(),
                RecoveryRecord {
                    recovery_event_id: "event".to_string(),
                    window_label: "main".to_string(),
                    document_id: "doc".to_string(),
                    context_epoch: 2,
                    ordinal: index + 1,
                    path: PathBuf::from(format!("/{id}")),
                    action: "revealPreservedItem".to_string(),
                    action_token: format!("action-{id}"),
                    action_completed: true,
                    acknowledged: false,
                    ack_token: Some(format!("ack-{id}")),
                    saved_generation: None,
                    expires_at: Instant::now() + Duration::from_secs(30),
                },
            );
        }

        let partial = registry
            .acknowledge_recovery("main", "first", "event", "ack-first")
            .expect("first ack");
        assert!(!partial.resolved);
        assert!(registry
            .acknowledge_recovery("main", "first", "event", "ack-first")
            .is_err());
        assert!(registry
            .acknowledge_recovery("other", "second", "event", "ack-second")
            .is_err());
        let resolved = registry
            .acknowledge_recovery("main", "second", "event", "ack-second")
            .expect("last ack");
        assert!(resolved.resolved);
    }

    #[test]
    fn recovery_summary_preserves_completed_acknowledged_and_saved_generation_state() {
        let completed = RecoveryRecord {
            recovery_event_id: "event".to_string(),
            window_label: "main".to_string(),
            document_id: "doc".to_string(),
            context_epoch: 2,
            ordinal: 1,
            path: PathBuf::new(),
            action: "saveCurrentBufferCopy".to_string(),
            action_token: String::new(),
            action_completed: true,
            acknowledged: false,
            ack_token: Some("ack-current".to_string()),
            saved_generation: Some(8),
            expires_at: Instant::now() + Duration::from_secs(30),
        };
        let acknowledged = RecoveryRecord {
            acknowledged: true,
            ack_token: None,
            ..completed.clone()
        };

        let completed_value = serde_json::to_value(recovery_record_summary("current", &completed))
            .expect("serialize completed recovery summary");
        assert_eq!(completed_value["actionCompleted"], true);
        assert_eq!(completed_value["acknowledged"], false);
        assert_eq!(completed_value["ackToken"], "ack-current");
        assert_eq!(completed_value["savedGeneration"], 8);
        let acknowledged_value =
            serde_json::to_value(recovery_record_summary("preserved", &acknowledged))
                .expect("serialize acknowledged recovery summary");
        assert_eq!(acknowledged_value["actionCompleted"], true);
        assert_eq!(acknowledged_value["acknowledged"], true);
        assert!(acknowledged_value.get("ackToken").is_none());
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

#[tauri::command]
fn set_native_window_theme(window: WebviewWindow, theme: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        window
            .with_webview(move |webview| unsafe {
                let appearance_name = match theme.as_str() {
                    "dark" => NSAppearanceNameDarkAqua,
                    "light" => NSAppearanceNameAqua,
                    "system" => {
                        let native_window: &objc2_app_kit::NSWindow = &*webview.ns_window().cast();
                        native_window.setAppearance(None);
                        return;
                    }
                    _ => return,
                };
                let appearance = NSAppearance::appearanceNamed(appearance_name)
                    .expect("macOS Aqua appearance must be available");
                let native_window: &objc2_app_kit::NSWindow = &*webview.ns_window().cast();
                native_window.setAppearance(Some(&appearance));
            })
            .map_err(|_| "WINDOW_THEME_UPDATE_FAILED".to_string())?;
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (window, theme);

    Ok(())
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
                } else if let Some(path) = paths.iter().find(|path| is_supported_insert_image(path))
                {
                    let token = Uuid::new_v4().simple().to_string();
                    let grant = ImageSourceGrant {
                        token: token.clone(),
                        window_label: window.label().to_string(),
                        path: path.clone(),
                        expires_at: Instant::now() + Duration::from_secs(30),
                    };
                    let registered = window
                        .state::<Mutex<DocumentRegistry>>()
                        .lock()
                        .expect("document registry")
                        .register_image_source_grant(grant)
                        .is_ok();
                    if registered {
                        let _ =
                            window.emit("image-source-granted", ImageSourceGrantEvent { token });
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_document_dialog,
            export_current_pdf,
            set_menu_locale,
            set_native_window_theme,
            export_png_dialog,
            open_external_url,
            take_pending_import,
            reload_document,
            document_edit_eligibility,
            prepare_save_as,
            cancel_save_as,
            prepare_recovery_save_to,
            perform_recovery_action,
            refresh_recovery_action,
            acknowledge_recovery_record,
            request_recovery_ack,
            insert_image_dialog,
            insert_image_from_grant,
            finalize_image_insert,
            reserve_document_write,
            reserve_clipboard_image,
            append_clipboard_image_chunk,
            finalize_clipboard_image,
            cancel_clipboard_image,
            begin_document_write,
            append_document_write_chunk,
            finalize_document_write,
            refresh_document_conflict,
            cancel_document_write,
            resolve_image_source,
            release_render_lease,
            close_document,
            start_document_watch,
            stop_document_watch,
            confirm_app_exit
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = &event {
                let allows_current_attempt = APP_EXIT_ATTEMPT
                    .lock()
                    .map(|mut gate| gate.allows_current_attempt(Instant::now()))
                    .unwrap_or(false);
                if !allows_current_attempt {
                    api.prevent_exit();
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.emit("app-exit-requested", ());
                    }
                }
            }
            #[cfg(target_os = "macos")]
            {
                if let tauri::RunEvent::Opened { urls } = event {
                    if let Some(path) = urls
                        .into_iter()
                        .find_map(|url| url.to_file_path().ok())
                        .filter(|path| is_supported_file(path))
                    {
                        publish_document(app, path);
                    }
                }
            }
        });
}
