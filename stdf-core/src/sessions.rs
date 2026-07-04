use crate::parser::{
    parse_reader, ParseErrorEvent, ParseProgress, ParsedRecord, ParserError, RecordStatus,
};
use serde::Deserialize;
use serde::Serialize;
use std::borrow::Cow;
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

use flate2::read::ZlibDecoder;
// FxHashMap on the per-record maps: SipHash showed up in parse profiles at
// 26M records × several map touches each; Fx is a keyed-DoS-free context
// (local temp index), so the cheaper hash is a straight win.
use rustc_hash::FxHashMap;
use rusqlite::{params, Connection, OptionalExtension};

const SQLITE_COMMIT_BATCH_SIZE: usize = 50_000;
const PROGRESS_STEP_BYTES: u64 = 8 * 1_048_576;
const PARSE_BUFFER_SIZE: usize = 8 * 1_024 * 1_024;
/// Bound of the parse→SQLite-writer channel. Deep enough to ride out a batch
/// COMMIT flush (~50k rows) without stalling the parse thread; at ~200 bytes a
/// row the peak buffer stays around 15 MB.
const WRITE_QUEUE_DEPTH: usize = 65_536;

const RECORD_INSERT_SQL: &str = "INSERT INTO records \
    (record_type, idx, group_idx, rec_offset, rec_length, status, fields_json) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

/// One parsed record on its way to the SQLite writer thread.
struct RecordRow {
    record_type: &'static str,
    idx: i64,
    group_idx: i64,
    offset: i64,
    length: i64,
    status: &'static str,
    blob: Vec<u8>,
}
// Upper bound on distinct test-item columns held in memory per session. Columns
// are paginated in the UI, so this is a safety cap (each part's result vector and
// the column universe scale with it) rather than a display limit. Note it only
// costs memory for files that actually have this many distinct tests.
const TEST_ITEM_COLUMN_LIMIT: usize = 100_000;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParseSession {
    pub session_id: String,
    pub file_name: String,
    pub file_path: String,
    pub file_dir: String,
    pub modified_time: String,
    pub file_size: u64,
    pub status: ParseStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ParseStatus {
    Running,
    Complete,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecordGroup {
    pub record_type: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecordSummary {
    pub id: String,
    pub record_type: String,
    pub index: usize,
    pub offset: u64,
    pub length: u16,
    pub status: RecordStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecordSummaryPage {
    pub records: Vec<RecordSummary>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

pub type RecordField = crate::parser::ParsedField;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FirstRecordSnapshot {
    pub record: RecordSummary,
    pub fields: Vec<RecordField>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub groups: Vec<RecordGroup>,
    pub key_fields: HashMap<String, Vec<RecordField>>,
    pub first_records: HashMap<String, FirstRecordSnapshot>,
    pub bytes_read: u64,
    pub total_bytes: u64,
    pub status: ParseStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TestItemColumn {
    pub record_type: String,
    pub test_num: u32,
    pub test_name: String,
    pub low_limit: String,
    pub high_limit: String,
    pub unit: String,
    pub pmr_indices: Vec<String>,
}

/// Internal, non-serialized scaling metadata resolved from the first PTR/MPR seen
/// for a test column. Later records for the same test usually omit limits/scale,
/// so we cache the base-unit limits here to judge pass/fail consistently.
#[derive(Debug, Clone, Copy, Default)]
struct ColumnMeta {
    res_scal: i32,
    low_base: Option<f64>,
    high_base: Option<f64>,
}

/// One matrix cell. Cells are positional — `TestItemPartRow.results[i]` belongs to
/// `columns[i]` of the same page/snapshot — so the cell carries no test identity.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TestItemCell {
    pub value: String,
    pub status: String,
    /// Position of the source record within its record-type group (0-based).
    /// Used by the frontend to jump from a test-item cell to the exact PTR/
    /// MPR/FTR record row in the Records view. `None` when no record fed
    /// this cell (e.g. a placeholder for a missing test on this part).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_position: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TestItemPartRow {
    pub part_id: String,
    pub site_num: String,
    pub site_nums: Vec<String>,
    pub head_num: String,
    pub sbin_num: String,
    pub sbin_name: String,
    pub sbin_pf: String,
    pub hbin_num: String,
    pub hbin_name: String,
    pub hbin_pf: String,
    pub test_t: String,
    pub part_txt: String,
    pub results: Vec<TestItemCell>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TestItemPmrEntry {
    pub phy_nam: String,
    pub log_nam: String,
    pub head_num: String,
    pub site_num: String,
}

/// A windowed slice of the test-item matrix: `rows[r].results` is projected to the
/// same order as `columns`, so the frontend renders only the requested page of
/// rows × columns regardless of how large the full matrix is.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TestItemPage {
    pub session_id: String,
    pub columns: Vec<TestItemColumn>,
    pub rows: Vec<TestItemPartRow>,
    pub total_columns: usize,
    pub total_rows: usize,
    pub row_offset: usize,
    pub col_offset: usize,
    /// Size of the session's PMR lookup. The UI only shows a count pill, so the
    /// map itself (potentially thousands of entries) never crosses the IPC bridge.
    pub pmr_count: usize,
    /// Whether the file's HBR/SBR carry a pass/fail flag. When false, pass/fail
    /// (PASSFG / export yield) falls back to the "soft bin 1 = pass" convention.
    pub has_bin_pf: bool,
    pub status: ParseStatus,
}

/// Column identity only — feeds the multi-select test-item filter dialog without
/// shipping per-part results.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TestItemColumnLite {
    pub key: String,
    pub record_type: String,
    pub test_num: u32,
    pub test_name: String,
}

/// One bin's share of the parsed parts, for the overview's yield section.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BinStat {
    pub num: String,
    pub name: String,
    pub pf: String,
    pub count: usize,
}

/// Yield / bin distribution over all parsed parts. Pass verdicts follow the
/// same convention as the CSV export: the bin's PF flag when the file carries
/// one, otherwise "bin 1 = pass".
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BinSummary {
    pub session_id: String,
    pub total_parts: usize,
    pub sbin_pass: usize,
    pub hbin_pass: usize,
    pub sbins: Vec<BinStat>,
    pub hbins: Vec<BinStat>,
    pub has_bin_pf: bool,
    pub status: ParseStatus,
}

#[derive(Clone, Default)]
struct PendingPartContext {
    head_num: String,
    site_num: String,
    site_nums: Vec<String>,
    test_t: String,
    part_txt: String,
    sbin_num: String,
    sbin_name: String,
    sbin_pf: String,
    hbin_num: String,
    hbin_name: String,
    hbin_pf: String,
    results: FxHashMap<(String, u32), TestItemCell>,
}

/// Official bin counts from HBR/SBR records. HEAD_NUM=255 marks the tester's
/// overall summary record; other head numbers are per-site partials that must
/// only be summed when no summary record exists (never both).
#[derive(Clone, Copy, Default)]
struct DeclaredBinCounts {
    summary: Option<u64>,
    site_sum: u64,
}

impl DeclaredBinCounts {
    fn resolve(&self, prefer_summary: bool) -> u64 {
        if prefer_summary {
            self.summary.unwrap_or(0)
        } else {
            self.site_sum
        }
    }
}

#[derive(Clone, Default)]
struct TestItemAccumulator {
    columns_by_key: FxHashMap<(String, u32), TestItemColumn>,
    column_meta: FxHashMap<(String, u32), ColumnMeta>,
    column_order: Vec<(String, u32)>,
    pmr_lookup: FxHashMap<String, TestItemPmrEntry>,
    hbin_names: FxHashMap<String, String>,
    hbin_pf: FxHashMap<String, String>,
    sbin_names: FxHashMap<String, String>,
    sbin_pf: FxHashMap<String, String>,
    hbin_declared: FxHashMap<String, DeclaredBinCounts>,
    sbin_declared: FxHashMap<String, DeclaredBinCounts>,
    /// Last-touchdown (sbin, hbin) per die identity — X/Y coordinates when the
    /// file carries them (CP), else PART_ID:site. Retested dies overwrite their
    /// earlier touchdown, so counting this map yields per-die final results.
    die_bins: FxHashMap<String, (String, String)>,
    open_parts: FxHashMap<(String, String), PendingPartContext>,
    part_rows: FxHashMap<(String, String), TestItemPartRow>,
    part_order: Vec<(String, String)>,
    /// Per-record-type counter used to stamp the position each PTR/MPR/FTR
    /// occupies within `get_records(group=...)`. Incremented in parse order,
    /// which matches SQLite's rowid ordering the records view queries by.
    record_type_positions: FxHashMap<&'static str, usize>,
    /// STDF v4 lets a PTR/MPR omit scale/limit/unit fields when they match
    /// the first record for the same TEST_NUM. This map remembers the raw
    /// values from that first record so the field-detail view can surface
    /// them as "继承自首条 PTR" hints on subsequent records.
    inheritable_defaults: FxHashMap<(String, u32), HashMap<String, String>>,
}

/// Field names PTR/MPR may omit and inherit from the first record of the
/// same TEST_NUM. STDF v4 §7.1 and §7.2 — the "optional data" tail after
/// TEST_TXT/ALARM_ID. C_*FMT / LO_SPEC / HI_SPEC are included because they
/// share the same inheritance rule even though most files omit them.
const INHERITABLE_FIELD_NAMES: &[&str] = &[
    "OPT_FLAG",
    "RES_SCAL",
    "LLM_SCAL",
    "HLM_SCAL",
    "LO_LIMIT",
    "HI_LIMIT",
    "UNITS",
    "C_RESFMT",
    "C_LLMFMT",
    "C_HLMFMT",
    "LO_SPEC",
    "HI_SPEC",
];

/// Wrapper returned by `get_record_fields`. `inherited_value` is populated
/// only when this record omitted the field and a first-record value exists
/// for the same TEST_NUM — see [`INHERITABLE_FIELD_NAMES`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EnrichedField {
    #[serde(flatten)]
    pub field: RecordField,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherited_value: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredField {
    Compact(CompactFieldOwned),
    Object(RecordField),
}

#[derive(Deserialize)]
struct CompactFieldOwned(
    String,
    String,
    String,
    String,
    Option<u64>,
    Option<u16>,
);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub record: RecordSummary,
    pub field: RecordField,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchResultPage {
    pub results: Vec<SearchResult>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchProgress {
    pub session_id: String,
    pub scanned: usize,
    pub total: usize,
}

/// Result of an on-demand DTR text extraction: how many DTR records were
/// written (one line each) to the session's temp txt, ready for save_dtr_text.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DtrParseResult {
    pub session_id: String,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Progress(ParseProgress),
    Snapshot(SessionSnapshot),
    Complete(String),
    /// Non-fatal warning about an abnormal-but-parseable file (empty / no FAR / truncated tail).
    Warning(ParseErrorEvent),
    Error(ParseErrorEvent),
}

#[derive(Default)]
pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<String, SessionState>>>,
}

#[derive(Clone)]
struct SessionState {
    session: ParseSession,
    db_path: PathBuf,
    cancel_flag: Arc<AtomicBool>,
    snapshot: SessionSnapshot,
    test_items: TestItemAccumulator,
    /// Lazily-opened read connection shared by the short record queries
    /// (page/fields/groups), so arrow-keying doesn't reopen SQLite per press
    /// and the page cache stays warm. Long scans (search) still open their own
    /// connection so they never block these — see `open_db`.
    read_conn: Arc<Mutex<Option<Connection>>>,
}

impl SessionManager {
    pub fn open_stdf(
        &self,
        path: String,
        on_event: impl Fn(SessionEvent) + Send + 'static,
    ) -> Result<ParseSession, String> {
        let path_buf = PathBuf::from(&path);
        let metadata = std::fs::metadata(&path_buf).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("请选择裸 .stdf 或 .std 文件".to_string());
        }
        let extension = path_buf
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(
            extension.as_str(),
            "stdf"
                | "std"
                | "gz"
                | "zip"
                | "7z"
                | "rar"
                | "xz"
                | "bz2"
                | "zst"
                | "tar"
                | "tgz"
                | "txz"
                | "tbz"
                | "tbz2"
        ) {
            return Err(
                "仅支持 .stdf / .std，或 zip / gz / 7z / rar / tar / xz / bz2 / zst 压缩包"
                    .to_string(),
            );
        }

        let session_id = Uuid::new_v4().to_string();
        let db_path = temp_workspace_dir().join(format!("{session_id}.db"));
        // Create the SQLite table synchronously so the frontend can query immediately
        // (avoids a "no such table: records" race before the parse thread starts writing).
        create_session_db(&db_path)?;
        let session = ParseSession {
            session_id: session_id.clone(),
            file_name: path_buf
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("unknown.stdf")
                .to_string(),
            file_path: path_buf.to_string_lossy().to_string(),
            file_dir: path_buf
                .parent()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default(),
            modified_time: metadata
                .modified()
                .ok()
                .map(format_system_time)
                .unwrap_or_default(),
            file_size: metadata.len(),
            status: ParseStatus::Running,
        };
        let snapshot = SessionSnapshot {
            session_id: session_id.clone(),
            groups: Vec::new(),
            key_fields: HashMap::new(),
            first_records: HashMap::new(),
            bytes_read: 0,
            total_bytes: metadata.len(),
            status: ParseStatus::Running,
        };
        let cancel_flag = Arc::new(AtomicBool::new(false));
        // The viewer shows one file at a time: opening a new file evicts every
        // previous session — cancel its parse, drop the in-memory accumulator,
        // and delete its on-disk index. Without this every opened file leaked
        // a multi-GB database (and its matrix accumulator) until the NEXT app
        // launch, since startup cleanup was the only reclamation. Unlinking is
        // safe even if the old parse/writer still holds the file open — the
        // space is reclaimed when the last handle closes.
        {
            let mut guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
            for (old_id, old) in guard.drain() {
                old.cancel_flag.store(true, Ordering::SeqCst);
                let _ = std::fs::remove_file(&old.db_path);
                let _ = std::fs::remove_file(dtr_text_path(&old_id));
            }
        }
        self.sessions
            .lock()
            .map_err(|_| "session lock poisoned")?
            .insert(
                session_id.clone(),
                SessionState {
                    session: session.clone(),
                    db_path: db_path.clone(),
                    cancel_flag: Arc::clone(&cancel_flag),
                    snapshot,
                    test_items: TestItemAccumulator::default(),
                    read_conn: Arc::new(Mutex::new(None)),
                },
            );

        let sessions = Arc::clone(&self.sessions);
        thread::spawn(move || {
            // The table was created synchronously in open_stdf; just open the existing db.
            let conn = match Connection::open(&db_path) {
                Ok(conn) => {
                    let _ = conn.execute_batch(
                        "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; \
                         PRAGMA temp_store=MEMORY; PRAGMA cache_size=-262144;",
                    );
                    conn
                }
                Err(error) => {
                    on_event(SessionEvent::Error(ParseErrorEvent {
                        session_id: session_id.clone(),
                        message: error.to_string(),
                        offset: None,
                    }));
                    return;
                }
            };

            let mut index = 0_usize;
            let mut first_record_type: Option<&'static str> = None;
            // Per-record-type position counters (parse order). Stored as
            // `group_idx` so record pages seek by (record_type, group_idx)
            // instead of walking an OFFSET.
            let mut group_counters: FxHashMap<&'static str, usize> = FxHashMap::default();
            let mut last_progress_bytes = 0_u64;
            let mut last_snapshot_bytes = 0_u64;

            // Validate the insert against the fresh schema up front so a broken
            // schema still aborts the parse with an error event (per-row insert
            // errors stay best-effort, as before).
            if let Err(error) = conn.prepare(RECORD_INSERT_SQL).map(drop) {
                on_event(SessionEvent::Error(ParseErrorEvent {
                    session_id: session_id.clone(),
                    message: error.to_string(),
                    offset: None,
                }));
                return;
            }

            // SQLite writing runs on its own thread: profiling showed the bulk
            // load's page flushing (pwrite) eating ~half the parse thread, so
            // parsing and disk writes now overlap. The bounded channel keeps
            // memory in check; a single writer preserves insertion order, which
            // get_record_fields' rowid == idx + 1 seek relies on.
            let (row_tx, row_rx) = mpsc::sync_channel::<RecordRow>(WRITE_QUEUE_DEPTH);
            let writer = thread::spawn(move || {
                let _ = conn.execute_batch("BEGIN;");
                let Ok(mut insert) = conn.prepare(RECORD_INSERT_SQL) else {
                    for _ in row_rx {}
                    return;
                };
                let mut since_commit = 0_usize;
                for row in row_rx {
                    let _ = insert.execute(params![
                        row.record_type,
                        row.idx,
                        row.group_idx,
                        row.offset,
                        row.length,
                        row.status,
                        row.blob,
                    ]);
                    since_commit += 1;
                    if since_commit >= SQLITE_COMMIT_BATCH_SIZE {
                        let _ = conn.execute_batch("COMMIT; BEGIN;");
                        since_commit = 0;
                    }
                }
                drop(insert);
                let _ = conn.execute_batch("COMMIT;");
                // Composite index: record pages seek straight to (type, position)
                // instead of OFFSET-walking millions of index entries.
                let _ = conn.execute_batch(
                    "CREATE INDEX IF NOT EXISTS idx_records_type_pos ON records (record_type, group_idx);",
                );
            });

            let result = with_input_reader(&path_buf, metadata.len(), |reader, parse_total| {
                let mut reader = BufReader::with_capacity(PARSE_BUFFER_SIZE, reader);
                parse_reader(
                    &mut reader,
                    parse_total,
                    |record| {
                        if cancel_flag.load(Ordering::SeqCst) {
                            return false;
                        }
                        if first_record_type.is_none() {
                            first_record_type = Some(record.record_type);
                        }
                        // Snapshot bookkeeping and the test-item accumulator share
                        // one lock acquisition — this runs once per record.
                        let snapshot_changed = apply_record_to_session(
                            &sessions,
                            &session_id,
                            &record,
                            index,
                            record.offset + 4 + u64::from(record.length),
                            parse_total,
                        );
                        if snapshot_changed {
                            if let Some(snapshot) = get_snapshot_clone(&sessions, &session_id) {
                                on_event(SessionEvent::Snapshot(snapshot));
                            }
                        }
                        let fields_blob = encode_fields_blob(&record);
                        let group_idx = {
                            let counter = group_counters
                                .entry(record.record_type)
                                .or_insert(0);
                            let position = *counter;
                            *counter += 1;
                            position
                        };
                        // Blocks only when the writer falls a full queue behind
                        // (backpressure), which keeps peak memory bounded.
                        let _ = row_tx.send(RecordRow {
                            record_type: record.record_type,
                            idx: index as i64,
                            group_idx: group_idx as i64,
                            offset: record.offset as i64,
                            length: record.length as i64,
                            status: status_str(&record.status),
                            blob: fields_blob,
                        });
                        index += 1;
                        true
                    },
                    |bytes_read, total_bytes| {
                        if bytes_read == total_bytes
                            || bytes_read.saturating_sub(last_progress_bytes) >= PROGRESS_STEP_BYTES
                        {
                            last_progress_bytes = bytes_read;
                            on_event(SessionEvent::Progress(ParseProgress {
                                session_id: session_id.clone(),
                                bytes_read,
                                total_bytes,
                            }));
                            let should_emit_snapshot =
                                bytes_read == total_bytes
                                    || bytes_read.saturating_sub(last_snapshot_bytes)
                                        >= PROGRESS_STEP_BYTES;
                            if should_emit_snapshot {
                                last_snapshot_bytes = bytes_read;
                                if let Some(snapshot) = update_snapshot_progress(
                                    &sessions,
                                    &session_id,
                                    bytes_read,
                                    total_bytes,
                                ) {
                                    on_event(SessionEvent::Snapshot(snapshot));
                                }
                            }
                        }
                    },
                )
            });
            // Close the channel so the writer drains, commits, and builds the
            // index; records must be fully queryable before Complete fires.
            drop(row_tx);
            let _ = writer.join();

            let result = match result {
                Ok(result) => result,
                Err(message) => {
                    on_event(SessionEvent::Error(ParseErrorEvent {
                        session_id: session_id.clone(),
                        message,
                        offset: None,
                    }));
                    return;
                }
            };

            let cancelled = cancel_flag.load(Ordering::SeqCst);
            // A truncated tail (incomplete touchdown record) is NOT fatal — keep what parsed.
            // Only real I/O failures are fatal errors.
            let fatal = matches!(&result, Err(ParserError::Io(_)));

            // Collect non-fatal warnings about abnormal-but-parseable files.
            let mut warnings: Vec<String> = Vec::new();
            if !cancelled {
                if index == 0 {
                    warnings.push("文件为空或未解析到任何标准 record。".to_string());
                } else if first_record_type != Some("FAR") {
                    warnings.push(format!(
                        "文件未以 FAR 记录开头（首条为 {}），非标准 STDF；已按现有内容解析。",
                        first_record_type.unwrap_or("未知")
                    ));
                }
                if let Err(ParserError::TruncatedPayload { offset, expected }) = &result {
                    warnings.push(format!(
                        "文件末尾存在不完整记录（offset {offset} 处需要 {expected} 字节，可能是 touchdown 截断）；已解析前 {index} 条。"
                    ));
                }
            }

            let status = if cancelled {
                ParseStatus::Cancelled
            } else if fatal {
                ParseStatus::Error
            } else {
                ParseStatus::Complete
            };

            if let Ok(mut guard) = sessions.lock() {
                if let Some(state) = guard.get_mut(&session_id) {
                    state.snapshot.status = status.clone();
                    state.session.status = status;
                }
            }

            if !cancelled && !warnings.is_empty() {
                on_event(SessionEvent::Warning(ParseErrorEvent {
                    session_id: session_id.clone(),
                    message: warnings.join("；"),
                    offset: None,
                }));
            }

            if fatal {
                if let Err(error) = result {
                    on_event(SessionEvent::Error(ParseErrorEvent {
                        session_id: session_id.clone(),
                        message: parser_error_message(error),
                        offset: None,
                    }));
                }
            } else {
                if let Some(snapshot) = get_snapshot_clone(&sessions, &session_id) {
                    on_event(SessionEvent::Snapshot(snapshot));
                }
                on_event(SessionEvent::Complete(session_id.clone()));
            }
        });

        Ok(session)
    }

    pub fn cancel_parse(&self, session_id: &str) -> Result<(), String> {
        let mut guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
        let state = guard
            .get_mut(session_id)
            .ok_or_else(|| "解析会话不存在".to_string())?;
        state.cancel_flag.store(true, Ordering::SeqCst);
        state.session.status = ParseStatus::Cancelled;
        state.snapshot.status = ParseStatus::Cancelled;
        Ok(())
    }

    pub fn get_session_snapshot(&self, session_id: &str) -> Result<SessionSnapshot, String> {
        let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
        Ok(guard
            .get(session_id)
            .ok_or_else(|| "解析会话不存在".to_string())?
            .snapshot
            .clone())
    }

    /// Open a fresh connection. Used by long-running scans (search) so they
    /// never hold the shared read connection hostage for 30-60s.
    fn open_db(&self, session_id: &str) -> Result<Connection, String> {
        let db_path = {
            let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
            guard
                .get(session_id)
                .ok_or_else(|| "解析会话不存在".to_string())?
                .db_path
                .clone()
        };
        Connection::open(&db_path).map_err(|error| error.to_string())
    }

    /// Run a short query on the session's cached read connection (opened on
    /// first use). The global sessions lock is released before touching
    /// SQLite, so queries never stall the parse thread.
    fn with_read_conn<T>(
        &self,
        session_id: &str,
        f: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let (db_path, cell) = {
            let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
            let state = guard
                .get(session_id)
                .ok_or_else(|| "解析会话不存在".to_string())?;
            (state.db_path.clone(), Arc::clone(&state.read_conn))
        };
        let mut slot = cell.lock().map_err(|_| "read connection lock poisoned")?;
        if slot.is_none() {
            *slot = Some(Connection::open(&db_path).map_err(|error| error.to_string())?);
        }
        f(slot.as_ref().expect("connection initialized above"))
    }

    pub fn get_record_groups(&self, session_id: &str) -> Result<Vec<RecordGroup>, String> {
        // The snapshot maintains live per-type counts in first-appearance order
        // (same order as GROUP BY ... ORDER BY MIN(rowid)). Once parsing is
        // done they are final, so a full-table GROUP BY would be pure waste.
        {
            let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
            let state = guard
                .get(session_id)
                .ok_or_else(|| "解析会话不存在".to_string())?;
            if matches!(state.snapshot.status, ParseStatus::Complete) {
                return Ok(state.snapshot.groups.clone());
            }
        }
        self.with_read_conn(session_id, |conn| {
            let mut stmt = conn
                .prepare("SELECT record_type, COUNT(*) FROM records GROUP BY record_type ORDER BY MIN(rowid)")
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(RecordGroup {
                        record_type: row.get::<_, String>(0)?,
                        count: row.get::<_, i64>(1)? as usize,
                    })
                })
                .map_err(|error| error.to_string())?;
            let mut groups = Vec::new();
            for row in rows {
                groups.push(row.map_err(|error| error.to_string())?);
            }
            Ok(groups)
        })
    }

    pub fn get_records(
        &self,
        session_id: &str,
        group: &str,
        page: usize,
        page_size: usize,
    ) -> Result<RecordSummaryPage, String> {
        // Once parsing completes the per-type totals are final in the snapshot;
        // re-running COUNT(*) per page flip would index-scan the whole group.
        let cached_total: Option<usize> = {
            let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
            let state = guard
                .get(session_id)
                .ok_or_else(|| "解析会话不存在".to_string())?;
            if matches!(state.snapshot.status, ParseStatus::Complete) {
                Some(
                    state
                        .snapshot
                        .groups
                        .iter()
                        .find(|g| g.record_type == group)
                        .map(|g| g.count)
                        .unwrap_or(0),
                )
            } else {
                None
            }
        };
        self.with_read_conn(session_id, |conn| {
            let total: usize = match cached_total {
                Some(total) => total,
                None => conn
                    .query_row(
                        "SELECT COUNT(*) FROM records WHERE record_type = ?1",
                        params![group],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| error.to_string())? as usize,
            };
            // Keyset pagination: group_idx is the record's position within its
            // type (parse order), so the composite index seeks straight to the
            // page instead of OFFSET-walking every earlier entry.
            let mut stmt = conn
                .prepare(
                    "SELECT record_type, idx, rec_offset, rec_length, status FROM records \
                     WHERE record_type = ?1 AND group_idx >= ?2 ORDER BY group_idx LIMIT ?3",
                )
                .map_err(|error| error.to_string())?;
            let start = page.saturating_mul(page_size) as i64;
            let rows = stmt
                .query_map(params![group, start, page_size as i64], |row| {
                    row_to_summary(session_id, row)
                })
                .map_err(|error| error.to_string())?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(|error| error.to_string())?);
            }
            Ok(RecordSummaryPage {
                records,
                total,
                page,
                page_size,
            })
        })
    }

    pub fn get_record_fields(
        &self,
        session_id: &str,
        record_id: &str,
    ) -> Result<Vec<EnrichedField>, String> {
        let (record_type, fields_blob): (String, Vec<u8>) =
            self.with_read_conn(session_id, |conn| {
                // Record ids are "{session}:{parse_index}" and rows are inserted
                // in parse order into a fresh table by a single writer, so
                // rowid == parse_index + 1. Seek by rowid (O(log n)) and verify
                // the stored idx matches — a full scan here made every
                // arrow-key field lookup take seconds on multi-GB files.
                record_id
                    .rsplit(':')
                    .next()
                    .and_then(|tail| tail.parse::<i64>().ok())
                    .and_then(|parse_index| {
                        conn.query_row(
                            "SELECT record_type, fields_json, idx FROM records WHERE rowid = ?1",
                            params![parse_index + 1],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, Vec<u8>>(1)?,
                                    row.get::<_, i64>(2)?,
                                ))
                            },
                        )
                        .optional()
                        .ok()
                        .flatten()
                        .filter(|(_, _, idx)| *idx == parse_index)
                    })
                    .map(|(record_type, blob, _)| (record_type, blob))
                    .ok_or_else(|| "record 不存在".to_string())
            })?;
        let fields = decode_fields_blob(&fields_blob).map_err(|error| error.to_string())?;
        // For PTR/MPR, look up the first-record defaults keyed by TEST_NUM so
        // omitted RES_SCAL/UNITS/LO_LIMIT/... show what they effectively are.
        let inherited = if matches!(record_type.as_str(), "PTR" | "MPR") {
            let test_num = field_value(&fields, "TEST_NUM")
                .parse::<u32>()
                .unwrap_or(0);
            self.sessions
                .lock()
                .ok()
                .and_then(|guard| {
                    guard
                        .get(session_id)
                        .and_then(|s| {
                            s.test_items
                                .inheritable_defaults
                                .get(&(record_type.clone(), test_num))
                                .cloned()
                        })
                })
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(fields
            .into_iter()
            .map(|field| {
                let inherited_value = if field.value.trim().is_empty() {
                    inherited.get(field.name.as_ref()).cloned()
                } else {
                    None
                };
                EnrichedField {
                    field,
                    inherited_value,
                }
            })
            .collect())
    }

    pub fn search_fields(
        &self,
        session_id: &str,
        query: &str,
        page: usize,
        page_size: usize,
        mut on_progress: impl FnMut(usize, usize),
    ) -> Result<SearchResultPage, String> {
        let conn = self.open_db(session_id)?;
        let needle = query.to_ascii_lowercase();
        // Fetch total row count up-front so the frontend can render a
        // determinate progress bar. COUNT(*) on rowid is cheap in SQLite.
        let total_rows: usize = conn
            .query_row("SELECT COUNT(*) FROM records", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| error.to_string())? as usize;
        on_progress(0, total_rows);
        let mut stmt = conn
            .prepare(
                "SELECT record_type, idx, rec_offset, rec_length, status, fields_json \
                 FROM records ORDER BY rowid",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row_to_summary(session_id, row)?, row.get::<_, Vec<u8>>(5)?))
            })
            .map_err(|error| error.to_string())?;
        let mut results = Vec::new();
        // Throttle progress events by wall-clock time, not by row count. On
        // huge files a row-based cap still fires many times per second and
        // saturates the main-thread IPC queue that WebKit needs for input.
        // 200ms == 5 events/sec is smooth enough for a progress bar and
        // leaves the UI thread free to keep the cursor responsive.
        use std::time::{Duration, Instant};
        const EMIT_INTERVAL: Duration = Duration::from_millis(200);
        let mut scanned: usize = 0;
        let mut last_emit = Instant::now();
        for row in rows {
            let (summary, fields_blob) = row.map_err(|error| error.to_string())?;
            let fields = decode_fields_blob(&fields_blob).unwrap_or_default();
            let type_match = summary.record_type.to_ascii_lowercase().contains(&needle);
            for field in fields {
                if type_match
                    || field.name.to_ascii_lowercase().contains(&needle)
                    || field.value.to_ascii_lowercase().contains(&needle)
                {
                    results.push(SearchResult {
                        record: summary.clone(),
                        field,
                    });
                }
            }
            scanned += 1;
            if last_emit.elapsed() >= EMIT_INTERVAL {
                on_progress(scanned, total_rows);
                last_emit = Instant::now();
            }
        }
        // Final tick so the bar always reaches 100% before the payload lands.
        on_progress(scanned, total_rows);
        let total = results.len();
        Ok(SearchResultPage {
            results: paginate(results, page, page_size),
            total,
            page,
            page_size,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_test_item_page(
        &self,
        session_id: &str,
        row_offset: usize,
        row_count: usize,
        col_offset: usize,
        col_count: usize,
        selected: &[String],
        site_filter: &str,
    ) -> Result<TestItemPage, String> {
        let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
        let state = guard
            .get(session_id)
            .ok_or_else(|| "解析会话不存在".to_string())?;
        Ok(build_test_item_page(
            session_id,
            &state.test_items,
            &state.snapshot.status,
            row_offset,
            row_count,
            col_offset,
            col_count,
            selected,
            site_filter,
        ))
    }

    /// Lightweight list of every test-item column (identity only), used to populate
    /// the multi-select filter dialog.
    pub fn get_test_item_columns(
        &self,
        session_id: &str,
    ) -> Result<Vec<TestItemColumnLite>, String> {
        let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
        let state = guard
            .get(session_id)
            .ok_or_else(|| "解析会话不存在".to_string())?;
        let acc = &state.test_items;
        let universe = acc.column_order.len().min(TEST_ITEM_COLUMN_LIMIT);
        Ok(acc.column_order[..universe]
            .iter()
            .filter_map(|key| acc.columns_by_key.get(key))
            .map(|col| TestItemColumnLite {
                key: column_key(col),
                record_type: col.record_type.clone(),
                test_num: col.test_num,
                test_name: col.test_name.clone(),
            })
            .collect())
    }

    /// Yield / bin distribution for the overview page.
    pub fn get_bin_summary(&self, session_id: &str) -> Result<BinSummary, String> {
        let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
        let state = guard
            .get(session_id)
            .ok_or_else(|| "解析会话不存在".to_string())?;
        Ok(build_bin_summary(
            session_id,
            &state.test_items,
            &state.snapshot.status,
        ))
    }

    /// Build the full test-item matrix as an STS8300-style CSV and write it to `path`.
    /// Done entirely in Rust so a potentially huge CSV never crosses the IPC bridge.
    pub fn export_test_item_csv(&self, session_id: &str, path: &str) -> Result<(), String> {
        let csv = {
            let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
            let state = guard
                .get(session_id)
                .ok_or_else(|| "解析会话不存在".to_string())?;
            build_test_item_csv(&state.snapshot, &state.test_items)
        };
        std::fs::write(path, csv).map_err(|error| format!("写入 CSV 失败: {error}"))
    }

    /// Re-scan the session's source file (plain / gz / zip alike) and extract
    /// every DTR TEXT_DAT into a temp txt, one line per record in file order.
    /// The txt never crosses the IPC bridge — save_dtr_text copies it to the
    /// user-chosen destination afterwards.
    pub fn parse_dtr_text(&self, session_id: &str) -> Result<DtrParseResult, String> {
        let (file_path, file_size) = {
            let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
            let state = guard
                .get(session_id)
                .ok_or_else(|| "解析会话不存在".to_string())?;
            (state.session.file_path.clone(), state.session.file_size)
        };
        let txt_path = dtr_text_path(session_id);
        let scan = File::create(&txt_path)
            .map_err(|error| format!("写入 DTR 文本失败: {error}"))
            .and_then(|file| {
                let mut writer = BufWriter::new(file);
                let count = with_input_reader(Path::new(&file_path), file_size, |reader, _| {
                    extract_dtr_text(reader, &mut writer)
                })??;
                writer
                    .flush()
                    .map_err(|error| format!("写入 DTR 文本失败: {error}"))?;
                Ok(count)
            });
        match scan {
            Ok(count) => Ok(DtrParseResult {
                session_id: session_id.to_string(),
                count,
            }),
            Err(error) => {
                // Never leave a partial txt behind — save_dtr_text treats its
                // existence as "parse succeeded".
                let _ = std::fs::remove_file(&txt_path);
                Err(error)
            }
        }
    }

    /// Copy the session's parsed DTR txt to `path`. Errors if parse_dtr_text
    /// has not succeeded for this session yet.
    pub fn save_dtr_text(&self, session_id: &str, path: &str) -> Result<(), String> {
        {
            let guard = self.sessions.lock().map_err(|_| "session lock poisoned")?;
            if !guard.contains_key(session_id) {
                return Err("解析会话不存在".to_string());
            }
        }
        let txt_path = dtr_text_path(session_id);
        if !txt_path.exists() {
            return Err("DTR 文本尚未解析，请先解析".to_string());
        }
        std::fs::copy(&txt_path, path)
            .map(|_| ())
            .map_err(|error| format!("写入 TXT 失败: {error}"))
    }
}

/// Walk record headers, skipping every payload except DTR (50,30), whose
/// TEXT_DAT becomes one line of `writer`. A truncated tail record ends the
/// scan quietly — same tolerance as the main parse, which keeps what it read.
fn extract_dtr_text(reader: &mut dyn Read, writer: &mut impl Write) -> Result<usize, String> {
    let mut reader = BufReader::with_capacity(PARSE_BUFFER_SIZE, reader);
    let mut count = 0_usize;
    loop {
        let mut header = [0_u8; 4];
        match reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(format!("读取 STDF 失败: {error}")),
        }
        let length = u64::from(u16::from_le_bytes([header[0], header[1]]));
        if header[2] == 50 && header[3] == 30 {
            let mut payload = vec![0_u8; length as usize];
            match reader.read_exact(&mut payload) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(format!("读取 STDF 失败: {error}")),
            }
            let text = crate::parser::decode_dtr_text(&payload);
            writer
                .write_all(text.as_bytes())
                .and_then(|_| writer.write_all(b"\n"))
                .map_err(|error| format!("写入 DTR 文本失败: {error}"))?;
            count += 1;
        } else {
            let skipped = io::copy(&mut (&mut reader).take(length), &mut io::sink())
                .map_err(|error| format!("读取 STDF 失败: {error}"))?;
            if skipped < length {
                break;
            }
        }
    }
    Ok(count)
}

/// Dedicated temp directory for decompressed files and per-session indexes.
/// Wiped on app startup, so leftovers from a crash never accumulate.
pub fn temp_workspace_dir() -> PathBuf {
    std::env::temp_dir().join("stdf-parser")
}

/// Where parse_dtr_text stages a session's extracted DTR text before the user
/// picks a destination. Lives in the startup-wiped temp workspace.
fn dtr_text_path(session_id: &str) -> PathBuf {
    temp_workspace_dir().join(format!("{session_id}.dtr.txt"))
}

/// Open a fresh per-session SQLite database. It is a rebuildable temp index, so
/// favor write throughput over crash recovery guarantees.
fn create_session_db(db_path: &Path) -> Result<(), String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let _ = std::fs::remove_file(db_path);
    let conn = Connection::open(db_path).map_err(|error| error.to_string())?;
    // page_size must precede the first CREATE TABLE. 8K pages halve the pwrite
    // syscall count for the multi-GB bulk load (profiling showed page flushing
    // dominating the parse thread).
    // The record id ("{session}:{idx}") is NOT stored — it is derived from idx
    // at query time; storing it added ~45 redundant bytes to every row.
    conn.execute_batch(
        "PRAGMA page_size=8192; \
         PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; \
         PRAGMA temp_store=MEMORY; PRAGMA cache_size=-262144; \
         CREATE TABLE records (\
            record_type TEXT, idx INTEGER, group_idx INTEGER, \
            rec_offset INTEGER, rec_length INTEGER, \
            status TEXT, fields_json BLOB);",
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Columns: record_type, idx, rec_offset, rec_length, status. The record id is
/// synthesized from the session id + parse index (it is not stored).
fn row_to_summary(session_id: &str, row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordSummary> {
    let index = row.get::<_, i64>(1)? as usize;
    Ok(RecordSummary {
        id: format!("{session_id}:{index}"),
        record_type: row.get(0)?,
        index,
        offset: row.get::<_, i64>(2)? as u64,
        length: row.get::<_, i64>(3)? as u16,
        status: status_from_str(&row.get::<_, String>(4)?),
    })
}

fn status_str(status: &RecordStatus) -> &'static str {
    match status {
        RecordStatus::Parsed => "parsed",
        RecordStatus::Unknown => "unknown",
        RecordStatus::Error => "error",
    }
}

fn status_from_str(value: &str) -> RecordStatus {
    match value {
        "error" => RecordStatus::Error,
        "unknown" => RecordStatus::Unknown,
        _ => RecordStatus::Parsed,
    }
}

/// Per-record session-state update. Groups/first-record/key-field bookkeeping and
/// the test-item accumulator are applied under a single lock acquisition — this
/// runs once per record, so the lock traffic matters on multi-million-record files.
fn apply_record_to_session(
    sessions: &Arc<Mutex<HashMap<String, SessionState>>>,
    session_id: &str,
    record: &ParsedRecord,
    idx: usize,
    bytes_read: u64,
    total_bytes: u64,
) -> bool {
    let Ok(mut guard) = sessions.lock() else {
        return false;
    };
    let Some(state) = guard.get_mut(session_id) else {
        return false;
    };
    let changed =
        update_session_snapshot(&mut state.snapshot, session_id, record, idx, bytes_read, total_bytes);
    update_test_item_accumulator(&mut state.test_items, record.record_type, idx, &record.fields);
    changed
}

fn update_session_snapshot(
    snapshot: &mut SessionSnapshot,
    session_id: &str,
    record: &ParsedRecord,
    idx: usize,
    bytes_read: u64,
    total_bytes: u64,
) -> bool {
    let mut changed = false;
    snapshot.bytes_read = bytes_read;
    snapshot.total_bytes = total_bytes;
    if let Some(group) = snapshot
        .groups
        .iter_mut()
        .find(|group| group.record_type == record.record_type)
    {
        group.count += 1;
    } else {
        snapshot.groups.push(RecordGroup {
            record_type: record.record_type.to_string(),
            count: 1,
        });
        changed = true;
    }

    if !snapshot.first_records.contains_key(record.record_type) {
        // Full summaries (with their formatted id) are built only for the
        // handful of first-of-type records, never in the per-record hot path.
        snapshot.first_records.insert(
            record.record_type.to_string(),
            FirstRecordSnapshot {
                record: record_summary(session_id, idx, record),
                fields: record.fields.to_vec(),
            },
        );
        changed = true;
    }

    if is_key_record(record.record_type)
        && !snapshot.key_fields.contains_key(record.record_type)
    {
        snapshot
            .key_fields
            .insert(record.record_type.to_string(), record.fields.to_vec());
        changed = true;
    }
    changed
}

fn update_test_item_accumulator(
    acc: &mut TestItemAccumulator,
    record_type: &'static str,
    idx: usize,
    fields: &[RecordField],
) {
    match record_type {
        "PMR" => {
            let pmr_indx = field_value(fields, "PMR_INDX");
            if !pmr_indx.is_empty() {
                acc.pmr_lookup.insert(
                    pmr_indx.to_string(),
                    TestItemPmrEntry {
                        phy_nam: field_value(fields, "PHY_NAM").to_string(),
                        log_nam: field_value(fields, "LOG_NAM").to_string(),
                        head_num: field_value(fields, "HEAD_NUM").to_string(),
                        site_num: field_value(fields, "SITE_NUM").to_string(),
                    },
                );
            }
        }
        "PIR" => {
            let site_key = site_key(fields);
            let entry = acc.open_parts.entry(site_key).or_default();
            entry.head_num = field_value(fields, "HEAD_NUM").to_string();
            entry.site_num = field_value(fields, "SITE_NUM").to_string();
            if entry.site_nums.is_empty() && !entry.site_num.is_empty() {
                entry.site_nums.push(entry.site_num.clone());
            }
        }
        "HBR" => {
            let key = field_value(fields, "HBIN_NUM");
            if !key.is_empty() {
                acc.hbin_names
                    .insert(key.to_string(), field_value(fields, "HBIN_NAM").to_string());
                acc.hbin_pf
                    .insert(key.to_string(), field_value(fields, "HBIN_PF").to_string());
                let count = field_value(fields, "HBIN_CNT").parse::<u64>().unwrap_or(0);
                let declared = acc.hbin_declared.entry(key.to_string()).or_default();
                if field_value(fields, "HEAD_NUM") == "255" {
                    declared.summary = Some(count);
                } else {
                    declared.site_sum += count;
                }
            }
        }
        "SBR" => {
            let key = field_value(fields, "SBIN_NUM");
            if !key.is_empty() {
                acc.sbin_names
                    .insert(key.to_string(), field_value(fields, "SBIN_NAM").to_string());
                acc.sbin_pf
                    .insert(key.to_string(), field_value(fields, "SBIN_PF").to_string());
                let count = field_value(fields, "SBIN_CNT").parse::<u64>().unwrap_or(0);
                let declared = acc.sbin_declared.entry(key.to_string()).or_default();
                if field_value(fields, "HEAD_NUM") == "255" {
                    declared.summary = Some(count);
                } else {
                    declared.site_sum += count;
                }
            }
        }
        "PTR" | "FTR" | "MPR" => {
            let test_num = field_value(fields, "TEST_NUM")
                .parse::<u32>()
                .unwrap_or(0);
            let key = (record_type.to_string(), test_num);
            // The first PTR/MPR for a test establishes the column (limits, unit,
            // scaling). Later records for the same test usually omit those, so we
            // resolve and cache them only once.
            if !acc.columns_by_key.contains_key(&key) {
                // Snapshot the raw inheritable field values here — later records
                // that omit these fields will surface them as "继承自首条 PTR"
                // hints in the field-detail view.
                let mut defaults: HashMap<String, String> = HashMap::new();
                for name in INHERITABLE_FIELD_NAMES {
                    let v = field_value(fields, name);
                    if !v.is_empty() {
                        defaults.insert((*name).to_string(), v.to_string());
                    }
                }
                if !defaults.is_empty() {
                    acc.inheritable_defaults.insert(key.clone(), defaults);
                }
                let test_name =
                    first_non_empty(fields, &["TEST_TXT", "TEST_NAM", "SEQ_NAME", "VECT_NAM"]);
                let pmr_indices = first_non_empty_array(fields, &["RTN_INDX", "PGM_INDX"]);
                let (column, meta) =
                    resolve_column(record_type, test_num, test_name.to_string(), pmr_indices, fields);
                acc.column_order.push(key.clone());
                acc.columns_by_key.insert(key.clone(), column);
                acc.column_meta.insert(key.clone(), meta);
            }
            let meta = acc.column_meta.get(&key).copied().unwrap_or_default();
            // Stamp the cell with its 0-based position within `record_type`'s
            // group so the frontend can jump straight to this record in the
            // Records view. Counter increments in parse order (== rowid order).
            let position = {
                let counter = acc
                    .record_type_positions
                    .entry(record_type)
                    .or_insert(0);
                let p = *counter;
                *counter += 1;
                p
            };
            let mut cell = match record_type {
                "PTR" => build_ptr_cell(fields, &meta),
                "MPR" => build_mpr_cell(fields, &meta),
                _ => build_ftr_cell(fields),
            };
            cell.record_position = Some(position);

            let site_key = site_key(fields);
            let entry = acc.open_parts.entry(site_key).or_default();
            if entry.head_num.is_empty() {
                entry.head_num = field_value(fields, "HEAD_NUM").to_string();
            }
            if entry.site_num.is_empty() {
                entry.site_num = field_value(fields, "SITE_NUM").to_string();
            }
            if entry.site_nums.is_empty() && !entry.site_num.is_empty() {
                entry.site_nums.push(entry.site_num.clone());
            }
            entry.results.insert(key, cell);
        }
        "PRR" => {
            let site_key = site_key(fields);
            let mut pending = acc.open_parts.remove(&site_key).unwrap_or_default();
            let site_num = field_value(fields, "SITE_NUM").to_string();
            let head_num = field_value(fields, "HEAD_NUM").to_string();
            if pending.head_num.is_empty() {
                pending.head_num = head_num.clone();
            }
            if pending.site_num.is_empty() {
                pending.site_num = site_num.clone();
            }
            if pending.site_nums.is_empty() && !pending.site_num.is_empty() {
                pending.site_nums.push(pending.site_num.clone());
            }
            let sbin_num = field_value(fields, "SOFT_BIN").to_string();
            let hbin_num = field_value(fields, "HARD_BIN").to_string();
            // Track final bins per die: X/Y is the die identity on wafer files
            // (retests reappear at the same coordinates under a fresh PART_ID);
            // -32768 is the STDF "no coordinate" sentinel.
            {
                let x = field_value(fields, "X_COORD");
                let y = field_value(fields, "Y_COORD");
                let has_xy = !x.is_empty() && !y.is_empty() && !(x == "-32768" || y == "-32768");
                let die_key = if has_xy {
                    format!("xy:{x}:{y}")
                } else {
                    format!(
                        "id:{}:{}",
                        field_value(fields, "PART_ID"),
                        field_value(fields, "SITE_NUM")
                    )
                };
                acc.die_bins
                    .insert(die_key, (sbin_num.clone(), hbin_num.clone()));
            }
            pending.sbin_num = sbin_num.clone();
            pending.sbin_name = acc.sbin_names.get(&sbin_num).cloned().unwrap_or_default();
            pending.sbin_pf = acc.sbin_pf.get(&sbin_num).cloned().unwrap_or_default();
            pending.hbin_num = hbin_num.clone();
            pending.hbin_name = acc.hbin_names.get(&hbin_num).cloned().unwrap_or_default();
            pending.hbin_pf = acc.hbin_pf.get(&hbin_num).cloned().unwrap_or_default();
            pending.test_t = field_value(fields, "TEST_T").to_string();
            pending.part_txt = field_value(fields, "PART_TXT").to_string();
            let part_id = first_non_empty(fields, &["PART_ID"]);
            let part_id = if part_id.is_empty() {
                format!("{}:{}:{}", head_num, site_num, idx)
            } else {
                part_id.to_string()
            };
            let key = (part_id.clone(), site_num.clone());
            let mut row_entry = acc.part_rows.remove(&key).unwrap_or_else(|| {
                acc.part_order.push(key.clone());
                TestItemPartRow {
                    part_id: part_id.clone(),
                    site_num: site_num.clone(),
                    site_nums: Vec::new(),
                    head_num: head_num.clone(),
                    sbin_num: String::new(),
                    sbin_name: String::new(),
                    sbin_pf: String::new(),
                    hbin_num: String::new(),
                    hbin_name: String::new(),
                    hbin_pf: String::new(),
                    test_t: String::new(),
                    part_txt: String::new(),
                    results: Vec::new(),
                }
            });
            if row_entry.site_nums.is_empty() {
                row_entry.site_nums.push(site_num.clone());
            }
            row_entry.site_num = site_num;
            row_entry.head_num = head_num;
            row_entry.sbin_num = pending.sbin_num;
            row_entry.sbin_name = pending.sbin_name;
            row_entry.sbin_pf = pending.sbin_pf;
            row_entry.hbin_num = pending.hbin_num;
            row_entry.hbin_name = pending.hbin_name;
            row_entry.hbin_pf = pending.hbin_pf;
            row_entry.test_t = pending.test_t;
            row_entry.part_txt = pending.part_txt;
            row_entry.results = materialize_results(
                &acc.columns_by_key,
                &acc.column_order,
                TEST_ITEM_COLUMN_LIMIT,
                &pending.results,
            );
            acc.part_rows.insert(key, row_entry);
        }
        _ => {}
    }
}

/// Stable identity for a test-item column, shared with the frontend selection set.
fn column_key(col: &TestItemColumn) -> String {
    format!("{}:{}", col.record_type, col.test_num)
}

/// A column passes when no explicit selection is given (show all) or its key is in
/// the selected set.
fn column_selected(col: &TestItemColumn, selected: &std::collections::HashSet<String>) -> bool {
    selected.is_empty() || selected.contains(&column_key(col))
}

fn site_matches(row: &TestItemPartRow, site: &str) -> bool {
    if site.is_empty() {
        return true;
    }
    row.site_num == site || row.site_nums.iter().any(|value| value == site)
}

/// Look up a bin name/PF by number, preferring the (now-complete) HBR/SBR map and
/// falling back to whatever was captured on the row at parse time.
fn bin_lookup(map: &FxHashMap<String, String>, key: &str, fallback: &str) -> String {
    map.get(key)
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// Look up a key-record field value (e.g. MIR JOB_NAM) captured in the snapshot.
fn key_field(snapshot: &SessionSnapshot, record: &str, field: &str) -> String {
    snapshot
        .key_fields
        .get(record)
        .and_then(|fields| fields.iter().find(|item| item.name == field))
        .map(|item| item.value.to_string())
        .unwrap_or_default()
}

/// Minimal RFC-4180 CSV cell escaping.
fn escape_csv(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn push_meta(out: &mut String, text: &str) {
    out.push_str(&escape_csv(text));
    out.push('\n');
}

fn push_row(out: &mut String, fields: &[String]) {
    let escaped: Vec<String> = fields.iter().map(|field| escape_csv(field)).collect();
    out.push_str(&escaped.join(","));
    out.push('\n');
}

fn pct(n: usize, total: usize) -> String {
    if total == 0 {
        "0.00".to_string()
    } else {
        format!("{:.2}", n as f64 / total as f64 * 100.0)
    }
}

/// Order bin keys by numeric value (non-numeric keys sort after, lexically).
fn bin_num_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// Whether a bin is a "pass" bin: the bin pass/fail flag (case-insensitive) when
/// present, else the convention that bin number 1 is the pass bin.
fn bin_passed(pf_map: &FxHashMap<String, String>, num: &str, stored_pf: &str) -> bool {
    let pf = bin_lookup(pf_map, num, stored_pf);
    let pf = pf.trim();
    if pf.eq_ignore_ascii_case("P") {
        true
    } else if pf.eq_ignore_ascii_case("F") {
        false
    } else {
        num.trim() == "1"
    }
}

/// Format STDF epoch seconds as "YYYY-MM-DD HH:MM:SS" (dependency-free, civil-from-days).
/// STDF carries no timezone, so the raw seconds are formatted as-is.
fn format_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hh, mm, ss
    )
}

/// Format an elapsed-seconds duration as "{days} day {h}:{mm}:{ss}" (sample style).
fn format_duration(secs: i64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    format!(
        "{} day {}:{:02}:{:02}",
        days,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Per-bin counts: the tester's declared HBR/SBR totals when the file carries
/// them (they already account for retests), else computed from the final
/// result of each die (see `TestItemAccumulator::die_bins`).
fn resolve_bin_counts(
    declared: &FxHashMap<String, DeclaredBinCounts>,
    die_bins: &FxHashMap<String, (String, String)>,
    pick: impl Fn(&(String, String)) -> &String,
) -> HashMap<String, usize> {
    if !declared.is_empty() {
        let prefer_summary = declared.values().any(|d| d.summary.is_some());
        let counts: HashMap<String, usize> = declared
            .iter()
            .map(|(num, d)| (num.clone(), d.resolve(prefer_summary) as usize))
            .collect();
        if counts.values().sum::<usize>() > 0 {
            return counts;
        }
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for bins in die_bins.values() {
        *counts.entry(pick(bins).clone()).or_insert(0) += 1;
    }
    counts
}

fn build_bin_summary(
    session_id: &str,
    acc: &TestItemAccumulator,
    status: &ParseStatus,
) -> BinSummary {
    let sbin_counts = resolve_bin_counts(&acc.sbin_declared, &acc.die_bins, |(sbin, _)| sbin);
    let hbin_counts = resolve_bin_counts(&acc.hbin_declared, &acc.die_bins, |(_, hbin)| hbin);

    let to_stats = |counts: &HashMap<String, usize>,
                    names: &FxHashMap<String, String>,
                    pf: &FxHashMap<String, String>| {
        let mut order: Vec<&String> = counts.keys().collect();
        order.sort_by(|a, b| bin_num_order(a, b));
        order
            .into_iter()
            .map(|num| BinStat {
                num: num.clone(),
                name: bin_lookup(names, num, ""),
                pf: bin_lookup(pf, num, ""),
                count: counts[num],
            })
            .collect::<Vec<_>>()
    };
    let sbins = to_stats(&sbin_counts, &acc.sbin_names, &acc.sbin_pf);
    let hbins = to_stats(&hbin_counts, &acc.hbin_names, &acc.hbin_pf);

    let passed = |counts: &HashMap<String, usize>, pf: &FxHashMap<String, String>| {
        counts
            .iter()
            .filter(|(num, _)| bin_passed(pf, num, ""))
            .map(|(_, count)| *count)
            .sum::<usize>()
    };
    let sbin_pass = passed(&sbin_counts, &acc.sbin_pf);
    let hbin_pass = passed(&hbin_counts, &acc.hbin_pf);
    let total = hbin_counts.values().sum::<usize>();

    let has_bin_pf = acc
        .hbin_pf
        .values()
        .chain(acc.sbin_pf.values())
        .any(|value| !value.trim().is_empty());

    BinSummary {
        session_id: session_id.to_string(),
        total_parts: total,
        sbin_pass,
        hbin_pass,
        sbins,
        hbins,
        has_bin_pf,
        status: status.clone(),
    }
}

fn build_test_item_csv(snapshot: &SessionSnapshot, acc: &TestItemAccumulator) -> String {
    let mut out = String::new();
    out.push('\u{FEFF}'); // UTF-8 BOM so Excel reads Chinese/units correctly.

    let cols: Vec<&TestItemColumn> = acc
        .column_order
        .iter()
        .take(TEST_ITEM_COLUMN_LIMIT)
        .filter_map(|key| acc.columns_by_key.get(key))
        .collect();
    let col_count = cols.len();

    let parts: Vec<&TestItemPartRow> = acc
        .part_order
        .iter()
        .filter_map(|key| acc.part_rows.get(key))
        .collect();

    // Bin totals/yield come from the shared summary (declared HBR/SBR counts,
    // retest-deduplicated) so the CSV header agrees with the overview page.
    // PASSFG per data ROW still follows each touchdown's own soft bin.
    let bins = build_bin_summary(&snapshot.session_id, acc, &snapshot.status);
    let total = bins.total_parts;
    let hbin_pass = bins.hbin_pass;
    let sbin_pass = bins.sbin_pass;
    let part_pass = |row: &TestItemPartRow| bin_passed(&acc.sbin_pf, &row.sbin_num, &row.sbin_pf);

    let avg_t = {
        let mut sum = 0.0f64;
        let mut n = 0u64;
        for row in &parts {
            if let Ok(value) = row.test_t.trim().parse::<f64>() {
                sum += value;
                n += 1;
            }
        }
        if n > 0 {
            (sum / n as f64).round() as i64
        } else {
            0
        }
    };

    // The soft-bin section shows each bin's hard-bin counterpart — derive the
    // mapping from the first part seen in that soft bin.
    let mut sbin_to_hbin: HashMap<String, String> = HashMap::new();
    for row in &parts {
        sbin_to_hbin
            .entry(row.sbin_num.clone())
            .or_insert_with(|| row.hbin_num.clone());
    }

    // ----- metadata block -----
    let node = key_field(snapshot, "MIR", "NODE_NAM");
    let job = key_field(snapshot, "MIR", "JOB_NAM");
    let job_rev = key_field(snapshot, "MIR", "JOB_REV");
    let program = if job_rev.is_empty() {
        job
    } else {
        format!("{} Rev {}", job, job_rev)
    };
    let start_secs = key_field(snapshot, "MIR", "START_T").trim().parse::<i64>().ok();
    let finish_secs = key_field(snapshot, "MRR", "FINISH_T").trim().parse::<i64>().ok();
    let start = start_secs.filter(|s| *s > 0).map(format_unix).unwrap_or_default();
    let finish = finish_secs.filter(|s| *s > 0).map(format_unix).unwrap_or_default();
    let total_time = match (start_secs, finish_secs) {
        (Some(s), Some(f)) if s > 0 && f >= s => format_duration(f - s),
        _ => String::new(),
    };
    // CP files carry wafer records (WIR/WRR); show wafer id for CP, sub-lot id for FT.
    let is_cp = snapshot
        .groups
        .iter()
        .any(|group| group.record_type == "WIR" || group.record_type == "WRR");

    push_meta(&mut out, &format!("Date:{}", start));
    push_meta(&mut out, &format!("Tester ID:{}", node));
    push_meta(&mut out, &format!("User:{}", key_field(snapshot, "MIR", "OPER_NAM")));
    push_meta(&mut out, &format!("Program:{}", program));
    push_meta(&mut out, "Handler:");
    push_meta(&mut out, "Site: All Sites");
    push_meta(&mut out, &format!("Lot Id:{}", key_field(snapshot, "MIR", "LOT_ID")));
    if is_cp {
        push_meta(&mut out, &format!("Wafer Id:{}", key_field(snapshot, "WIR", "WAFER_ID")));
    } else {
        push_meta(&mut out, &format!("Sblot Id:{}", key_field(snapshot, "MIR", "SBLOT_ID")));
    }
    push_meta(&mut out, "");
    push_meta(&mut out, &format!("Average Test Time(ms): {}", avg_t));
    push_meta(&mut out, "Idle Time:");
    push_meta(&mut out, &format!("Beginning Time: {}", start));
    push_meta(&mut out, &format!("Ending Time: {}", finish));
    push_meta(&mut out, &format!("Total Testing Time: {}", total_time));
    push_meta(&mut out, "");
    // Hard-bin summary section (its own Total / Pass / Fail).
    push_meta(&mut out, &format!("Total: {}", total));
    push_meta(&mut out, &format!("Pass: {}   {}%", hbin_pass, pct(hbin_pass, total)));
    push_meta(&mut out, &format!("Fail: {}   {}%", total - hbin_pass, pct(total - hbin_pass, total)));
    for bin in &bins.hbins {
        let label = if bin.name.is_empty() {
            format!("HBin[{}]", bin.num)
        } else {
            format!("HBin[{}] {}", bin.num, bin.name)
        };
        push_meta(&mut out, &format!("{}  {}  {}%", label, bin.count, pct(bin.count, total)));
    }
    push_meta(&mut out, "");

    // Soft-bin summary section (its own Total / Pass / Fail).
    push_meta(&mut out, &format!("Total: {}", total));
    push_meta(&mut out, &format!("Pass: {}   {}%", sbin_pass, pct(sbin_pass, total)));
    push_meta(&mut out, &format!("Fail: {}   {}%", total - sbin_pass, pct(total - sbin_pass, total)));
    for bin in &bins.sbins {
        let label = if bin.name.is_empty() {
            format!("SBin[{}]", bin.num)
        } else {
            format!("SBin[{}] {}", bin.num, bin.name)
        };
        let hbin = sbin_to_hbin.get(&bin.num).cloned().unwrap_or_default();
        push_meta(
            &mut out,
            &format!("{}  {}  {}%  {}", label, bin.count, pct(bin.count, total), hbin),
        );
    }
    push_meta(&mut out, "");
    push_meta(&mut out, "");

    // ----- header rows ----- (left columns: SITE_NUM, PART_ID, PASSFG, HARD_BIN, SOFT_BIN, T_TIME)
    let mut labels: Vec<String> = ["SITE_NUM", "PART_ID", "PASSFG", "HARD_BIN", "SOFT_BIN", "T_TIME"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for col in &cols {
        labels.push(if col.test_name.is_empty() {
            format!("{}_{}", col.record_type, col.test_num)
        } else {
            col.test_name.clone()
        });
    }
    push_row(&mut out, &labels);

    // "ms" sits under T_TIME (now the 6th left column).
    let mut unit_row: Vec<String> = ["Unit", "", "", "", "", "ms"].iter().map(|s| s.to_string()).collect();
    unit_row.extend(cols.iter().map(|col| col.unit.clone()));
    push_row(&mut out, &unit_row);

    let mut low_row: Vec<String> = ["LimitL", "", "", "", "", ""].iter().map(|s| s.to_string()).collect();
    low_row.extend(cols.iter().map(|col| col.low_limit.clone()));
    push_row(&mut out, &low_row);

    let mut high_row: Vec<String> = ["LimitU", "", "", "", "", ""].iter().map(|s| s.to_string()).collect();
    high_row.extend(cols.iter().map(|col| col.high_limit.clone()));
    push_row(&mut out, &high_row);

    push_meta(&mut out, "");

    // ----- data rows -----
    for row in &parts {
        let passfg = if part_pass(row) { "TRUE" } else { "FALSE" };
        let mut fields: Vec<String> = vec![
            row.site_num.clone(),
            row.part_id.clone(),
            passfg.to_string(),
            row.hbin_num.clone(),
            row.sbin_num.clone(),
            row.test_t.clone(),
        ];
        for index in 0..col_count {
            fields.push(
                row.results
                    .get(index)
                    .map(|cell| cell.value.clone())
                    .unwrap_or_default(),
            );
        }
        push_row(&mut out, &fields);
    }

    out
}

/// Slice the in-memory matrix down to a window of rows × columns after applying the
/// test-item and site filters. Because the full matrix is already accumulated during
/// parsing, this is O(page) and keeps the UI responsive on large files.
#[allow(clippy::too_many_arguments)]
fn build_test_item_page(
    session_id: &str,
    acc: &TestItemAccumulator,
    status: &ParseStatus,
    row_offset: usize,
    row_count: usize,
    col_offset: usize,
    col_count: usize,
    selected: &[String],
    site_filter: &str,
) -> TestItemPage {
    // Rows only materialize cells for the first TEST_ITEM_COLUMN_LIMIT columns, so
    // the addressable column universe is bounded the same way to stay aligned.
    let universe = acc.column_order.len().min(TEST_ITEM_COLUMN_LIMIT);
    let selected_set: std::collections::HashSet<String> = selected.iter().cloned().collect();
    let site_needle = site_filter.trim();

    // (index within column_order, column) for columns passing the selection.
    let filtered_cols: Vec<(usize, &TestItemColumn)> = acc.column_order[..universe]
        .iter()
        .enumerate()
        .filter_map(|(pos, key)| acc.columns_by_key.get(key).map(|col| (pos, col)))
        .filter(|(_, col)| column_selected(col, &selected_set))
        .collect();
    let total_columns = filtered_cols.len();

    let page_cols: Vec<(usize, &TestItemColumn)> = filtered_cols
        .iter()
        .skip(col_offset)
        .take(col_count)
        .map(|(pos, col)| (*pos, *col))
        .collect();
    let columns: Vec<TestItemColumn> = page_cols.iter().map(|(_, col)| (*col).clone()).collect();

    let filtered_rows: Vec<&TestItemPartRow> = acc
        .part_order
        .iter()
        .filter_map(|key| acc.part_rows.get(key))
        .filter(|row| site_matches(row, site_needle))
        .collect();
    let total_rows = filtered_rows.len();

    let rows: Vec<TestItemPartRow> = filtered_rows
        .iter()
        .skip(row_offset)
        .take(row_count)
        .map(|row| TestItemPartRow {
            part_id: row.part_id.clone(),
            site_num: row.site_num.clone(),
            site_nums: row.site_nums.clone(),
            head_num: row.head_num.clone(),
            sbin_num: row.sbin_num.clone(),
            // Resolve bin name/PF here (query time): HBR/SBR usually arrive at the
            // end of the file, after the PRRs, so they aren't known when the part row
            // is first built — but the maps are complete by the time we serve a page.
            sbin_name: bin_lookup(&acc.sbin_names, &row.sbin_num, &row.sbin_name),
            sbin_pf: bin_lookup(&acc.sbin_pf, &row.sbin_num, &row.sbin_pf),
            hbin_num: row.hbin_num.clone(),
            hbin_name: bin_lookup(&acc.hbin_names, &row.hbin_num, &row.hbin_name),
            hbin_pf: bin_lookup(&acc.hbin_pf, &row.hbin_num, &row.hbin_pf),
            test_t: row.test_t.clone(),
            part_txt: row.part_txt.clone(),
            // Project each row's results onto the requested column window.
            results: page_cols
                .iter()
                .map(|(pos, _)| {
                    row.results.get(*pos).cloned().unwrap_or_else(|| TestItemCell {
                        value: String::new(),
                        status: String::new(),
                        record_position: None,
                    })
                })
                .collect(),
        })
        .collect();

    let has_bin_pf = acc
        .hbin_pf
        .values()
        .chain(acc.sbin_pf.values())
        .any(|value| !value.trim().is_empty());

    TestItemPage {
        session_id: session_id.to_string(),
        columns,
        rows,
        total_columns,
        total_rows,
        row_offset,
        col_offset,
        pmr_count: acc.pmr_lookup.len(),
        has_bin_pf,
        status: status.clone(),
    }
}

fn update_snapshot_progress(
    sessions: &Arc<Mutex<HashMap<String, SessionState>>>,
    session_id: &str,
    bytes_read: u64,
    total_bytes: u64,
) -> Option<SessionSnapshot> {
    let mut guard = sessions.lock().ok()?;
    let state = guard.get_mut(session_id)?;
    state.snapshot.bytes_read = bytes_read;
    state.snapshot.total_bytes = total_bytes;
    Some(state.snapshot.clone())
}

fn get_snapshot_clone(
    sessions: &Arc<Mutex<HashMap<String, SessionState>>>,
    session_id: &str,
) -> Option<SessionSnapshot> {
    let guard = sessions.lock().ok()?;
    Some(guard.get(session_id)?.snapshot.clone())
}

fn is_key_record(record_type: &str) -> bool {
    matches!(
        record_type,
        "MIR" | "MRR" | "WIR" | "WRR" | "SDR" | "WCR" | "PCR" | "HBR" | "SBR" | "FAR" | "ATR"
    )
}

// Borrowed lookups on purpose: these run several times per record on the
// parse hot path, and the previous clone-per-call showed up as allocator time
// on million-record files. Callers own only what they actually store.
fn field_value<'a>(fields: &'a [RecordField], name: &str) -> &'a str {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.value.as_str())
        .unwrap_or("")
}

fn first_non_empty<'a>(fields: &'a [RecordField], names: &[&str]) -> &'a str {
    for name in names {
        let value = field_value(fields, name);
        if !value.is_empty() {
            return value;
        }
    }
    ""
}

fn first_non_empty_array(fields: &[RecordField], names: &[&str]) -> Vec<String> {
    for name in names {
        let values = field_values_from_array(fields, name);
        if !values.is_empty() {
            return values;
        }
    }
    Vec::new()
}

fn field_values_from_array(fields: &[RecordField], name: &str) -> Vec<String> {
    let raw = field_value(fields, name);
    if raw.is_empty() {
        return Vec::new();
    }
    let start = raw.find('[');
    let end = raw.rfind(']');
    match (start, end) {
        (Some(start), Some(end)) if end > start + 1 => raw[start + 1..end]
            .split(',')
            .map(|item| item.trim().trim_matches('"').to_string())
            .filter(|item| !item.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn site_key(fields: &[RecordField]) -> (String, String) {
    (
        field_value(fields, "HEAD_NUM").to_string(),
        field_value(fields, "SITE_NUM").to_string(),
    )
}

fn parse_float(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// STDF "inverted" SI prefix table: a positive scaling exponent denotes a small
/// unit (milli/micro/...), a negative one a large unit (kilo/mega/...). RESULT,
/// LO_LIMIT and HI_LIMIT are stored in base SI units; the exponent is a display
/// hint, so the shown value is `stored * 10^scale` under the prefixed unit
/// (e.g. 0.161 V at scale 3 shows as "161 mV"; 32000 Hz at scale -3 as "32 KHz").
fn scale_prefix(scale: i32) -> &'static str {
    match scale {
        -12 => "T",
        -9 => "G",
        -6 => "M",
        -3 => "K",
        2 => "%",
        3 => "m",
        6 => "u",
        9 => "n",
        12 => "p",
        15 => "f",
        _ => "",
    }
}

/// Parse a stringified I*1 scaling exponent (e.g. "-3"); absent/garbage -> 0.
fn parse_scale(value: &str) -> i32 {
    value.trim().parse::<i32>().unwrap_or(0)
}

/// Convert a stored value to a scale-normalized magnitude for limit comparison.
/// Result and limit share the same exponent (limit scales default to the result
/// scale), so this factor cancels and the verdict is unaffected by its direction.
fn to_base(stored: f64, scale: i32) -> f64 {
    stored * 10f64.powi(-scale)
}

/// Express a stored base-unit value in the column's display scale: the shown
/// number is `stored * 10^scale`, matching the SI prefix on the column's unit
/// (0.161 V at scale 3 -> 161 "mV"; 32000 Hz at scale -3 -> 32 "KHz").
fn to_display(stored: f64, scale: i32) -> f64 {
    stored * 10f64.powi(scale)
}

/// Render a value with ~10 significant digits, trimming floating-point noise and
/// trailing zeros (4.999999999 -> "5", 0.0023 -> "0.0023").
fn format_num(value: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    let magnitude = value.abs().log10().floor() as i32;
    let factor = 10f64.powi(9 - magnitude);
    let rounded = (value * factor).round() / factor;
    format!("{}", rounded)
}

/// Render a stored base-unit limit in the column's display scale (`value * 10^scale`)
/// so it matches the SI prefix on the column's unit. Scale 0 keeps the original
/// string verbatim, avoiding any floating-point reformatting.
fn display_limit(raw: &str, scale: i32) -> String {
    match parse_float(raw) {
        None => String::new(),
        Some(_) if scale == 0 => raw.to_string(),
        Some(value) => format_num(to_display(value, scale)),
    }
}

/// Judge a base-unit value against optional base-unit limits. With no limit on
/// either side there is no verdict (empty status) rather than a default pass.
fn judge(value: f64, low: Option<f64>, high: Option<f64>) -> String {
    if low.is_none() && high.is_none() {
        return String::new();
    }
    let within = low.is_none_or(|l| value >= l) && high.is_none_or(|h| value <= h);
    if within {
        "P".to_string()
    } else {
        "F".to_string()
    }
}

/// Build a test-item column plus its cached scaling metadata from the first PTR/MPR
/// record seen for the test. FTR columns carry no limits/units.
fn resolve_column(
    record_type: &str,
    test_num: u32,
    test_name: String,
    pmr_indices: Vec<String>,
    fields: &[RecordField],
) -> (TestItemColumn, ColumnMeta) {
    let parametric = record_type == "PTR" || record_type == "MPR";
    if !parametric {
        return (
            TestItemColumn {
                record_type: record_type.to_string(),
                test_num,
                test_name,
                low_limit: String::new(),
                high_limit: String::new(),
                unit: String::new(),
                pmr_indices,
            },
            ColumnMeta::default(),
        );
    }

    let res_scal = parse_scale(&field_value(fields, "RES_SCAL"));
    // Limit scales default to the result scale when the file omits them.
    let llm_raw_scal = field_value(fields, "LLM_SCAL");
    let llm_scal = if llm_raw_scal.trim().is_empty() {
        res_scal
    } else {
        parse_scale(&llm_raw_scal)
    };
    let hlm_raw_scal = field_value(fields, "HLM_SCAL");
    let hlm_scal = if hlm_raw_scal.trim().is_empty() {
        res_scal
    } else {
        parse_scale(&hlm_raw_scal)
    };
    let units = field_value(fields, "UNITS");
    let lo_raw = field_value(fields, "LO_LIMIT");
    let hi_raw = field_value(fields, "HI_LIMIT");

    let unit = if units.is_empty() {
        String::new()
    } else {
        format!("{}{}", scale_prefix(res_scal), units)
    };

    (
        TestItemColumn {
            record_type: record_type.to_string(),
            test_num,
            test_name,
            low_limit: display_limit(&lo_raw, res_scal),
            high_limit: display_limit(&hi_raw, res_scal),
            unit,
            pmr_indices,
        },
        ColumnMeta {
            res_scal,
            low_base: parse_float(&lo_raw).map(|v| to_base(v, llm_scal)),
            high_base: parse_float(&hi_raw).map(|v| to_base(v, hlm_scal)),
        },
    )
}

fn build_ptr_cell(fields: &[RecordField], meta: &ColumnMeta) -> TestItemCell {
    let result = field_value(fields, "RESULT");
    let parsed = parse_float(result);
    let status = match parsed {
        Some(value) => judge(to_base(value, meta.res_scal), meta.low_base, meta.high_base),
        None => String::new(),
    };
    let value = if result.is_empty() {
        field_value(fields, "TEST_FLG").to_string()
    } else {
        match parsed {
            Some(v) if meta.res_scal != 0 => format_num(to_display(v, meta.res_scal)),
            _ => result.to_string(),
        }
    };
    TestItemCell {
        value,
        status,
        record_position: None,
    }
}

/// MPR collapses its multi-pin result array into one cell: values are shown joined,
/// and the verdict fails if any pin falls outside the shared limits. The result
/// array is capped at 16 elements upstream in the parser.
fn build_mpr_cell(fields: &[RecordField], meta: &ColumnMeta) -> TestItemCell {
    let results = field_values_from_array(fields, "RTN_RSLT");
    if results.is_empty() {
        return TestItemCell {
            value: field_value(fields, "TEST_FLG").to_string(),
            status: String::new(),
            record_position: None,
        };
    }
    let mut judged = false;
    let mut failed = false;
    for raw in &results {
        if let Some(value) = parse_float(raw) {
            match judge(to_base(value, meta.res_scal), meta.low_base, meta.high_base).as_str() {
                "P" => judged = true,
                "F" => {
                    judged = true;
                    failed = true;
                }
                _ => {}
            }
        }
    }
    let status = if !judged {
        String::new()
    } else if failed {
        "F".to_string()
    } else {
        "P".to_string()
    };
    let value = if meta.res_scal == 0 {
        results.join(", ")
    } else {
        results
            .iter()
            .map(|raw| match parse_float(raw) {
                Some(v) => format_num(to_display(v, meta.res_scal)),
                None => raw.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    TestItemCell {
        value,
        status,
        record_position: None,
    }
}

fn build_ftr_cell(fields: &[RecordField]) -> TestItemCell {
    let flag = field_value(fields, "TEST_FLG");
    let status = if flag.contains("0b00000000") { "P" } else { "F" };
    TestItemCell {
        value: flag.to_string(),
        status: status.to_string(),
        record_position: None,
    }
}

fn materialize_results(
    columns_by_key: &FxHashMap<(String, u32), TestItemColumn>,
    column_order: &[(String, u32)],
    column_limit: usize,
    results: &FxHashMap<(String, u32), TestItemCell>,
) -> Vec<TestItemCell> {
    column_order
        .iter()
        .take(column_limit)
        .filter(|key| columns_by_key.contains_key(*key))
        .map(|key| {
            results.get(key).cloned().unwrap_or(TestItemCell {
                value: String::new(),
                status: String::new(),
                record_position: None,
            })
        })
        .collect()
}

/// Encode a record's fields for the session index. Field names, types and
/// descriptions are static per record layout, so for spec-aligned records
/// (the overwhelmingly common case) only the VALUES are stored, plus the
/// REC_TYP/REC_SUB pair identifying the layout — decode rebuilds the rest
/// from the parser's spec table. Storing full copies per record blew a 3.2GB
/// STDF up into a 35GB index and made the bulk load write-bound.
/// Spec-less records (UNKNOWN raw previews) and any spec mismatch fall back
/// to the legacy full-copy format.
fn encode_fields_blob(record: &ParsedRecord) -> Vec<u8> {
    if let Some(specs) = crate::parser::record_specs(record.rec_typ, record.rec_sub) {
        let aligned = record.fields.len() <= specs.len()
            && record
                .fields
                .iter()
                .zip(specs)
                .all(|(field, spec)| field.name == spec.name);
        if aligned {
            let mut blob = Vec::with_capacity(10 + record.fields.len() * 12);
            blob.extend_from_slice(b"GBF2");
            blob.push(record.rec_typ);
            blob.push(record.rec_sub);
            blob.extend_from_slice(&(record.fields.len() as u32).to_le_bytes());
            for field in &record.fields {
                write_blob_str(&mut blob, &field.value);
            }
            return blob;
        }
    }
    encode_gbf1_fields(&record.fields)
}

fn decode_gbf2_fields(fields_blob: &[u8]) -> Result<Vec<RecordField>, String> {
    if fields_blob.len() < 10 {
        return Err("GBF2 blob 头不完整".to_string());
    }
    let rec_typ = fields_blob[4];
    let rec_sub = fields_blob[5];
    let specs = crate::parser::record_specs(rec_typ, rec_sub)
        .ok_or_else(|| format!("未知记录布局 ({rec_typ},{rec_sub})"))?;
    let count = u32::from_le_bytes([
        fields_blob[6],
        fields_blob[7],
        fields_blob[8],
        fields_blob[9],
    ]) as usize;
    if count > specs.len() {
        return Err("GBF2 字段数超出布局定义".to_string());
    }
    let mut cursor = 10_usize;
    let mut fields = Vec::with_capacity(count);
    for spec in specs.iter().take(count) {
        let value = read_blob_string(fields_blob, &mut cursor)?;
        fields.push(RecordField {
            name: Cow::Borrowed(spec.name),
            field_type: Cow::Borrowed(spec.field_type),
            value: value.into(),
            description: Cow::Borrowed(spec.description),
            offset: None,
            length: None,
        });
    }
    Ok(fields)
}

/// Legacy full-copy encoding: every field carries its own name/type/description.
fn encode_gbf1_fields(fields: &[RecordField]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(fields.len().saturating_mul(40));
    blob.extend_from_slice(b"GBF1");
    blob.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for field in fields {
        write_blob_str(&mut blob, field.name.as_ref());
        write_blob_str(&mut blob, field.field_type.as_ref());
        write_blob_str(&mut blob, &field.value);
        write_blob_str(&mut blob, field.description.as_ref());
        blob.extend_from_slice(&field.offset.unwrap_or(u64::MAX).to_le_bytes());
        blob.extend_from_slice(&field.length.unwrap_or(u16::MAX).to_le_bytes());
    }
    blob
}

fn decode_fields_blob(fields_blob: &[u8]) -> Result<Vec<RecordField>, String> {
    if fields_blob.starts_with(b"GBF2") {
        return decode_gbf2_fields(fields_blob);
    }
    if fields_blob.starts_with(b"GBF1") {
        return decode_gbf1_fields(fields_blob);
    }
    let json = if fields_blob.first() == Some(&0x78) {
        let mut decoder = ZlibDecoder::new(fields_blob);
        let mut json = Vec::new();
        decoder.read_to_end(&mut json).map_err(|error| error.to_string())?;
        json
    } else {
        fields_blob.to_vec()
    };
    serde_json::from_slice::<Vec<StoredField>>(&json)
        .map(|fields| {
            fields
                .into_iter()
                .map(|field| match field {
                    StoredField::Compact(CompactFieldOwned(
                        name,
                        field_type,
                        value,
                        description,
                        offset,
                        length,
                    )) => RecordField {
                        name: Cow::Owned(name),
                        field_type: Cow::Owned(field_type),
                        value: value.into(),
                        description: Cow::Owned(description),
                        offset,
                        length,
                    },
                    StoredField::Object(field) => field,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

fn write_blob_str(blob: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    blob.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    blob.extend_from_slice(bytes);
}

fn decode_gbf1_fields(fields_blob: &[u8]) -> Result<Vec<RecordField>, String> {
    let mut cursor = 4_usize;
    let field_count = read_blob_u32(fields_blob, &mut cursor)? as usize;
    let mut fields = Vec::with_capacity(field_count);
    for _ in 0..field_count {
        let name = read_blob_string(fields_blob, &mut cursor)?;
        let field_type = read_blob_string(fields_blob, &mut cursor)?;
        let value = read_blob_string(fields_blob, &mut cursor)?;
        let description = read_blob_string(fields_blob, &mut cursor)?;
        let raw_offset = read_blob_u64(fields_blob, &mut cursor)?;
        let raw_length = read_blob_u16(fields_blob, &mut cursor)?;
        fields.push(RecordField {
            name: Cow::Owned(name),
            field_type: Cow::Owned(field_type),
            value: value.into(),
            description: Cow::Owned(description),
            offset: (raw_offset != u64::MAX).then_some(raw_offset),
            length: (raw_length != u16::MAX).then_some(raw_length),
        });
    }
    Ok(fields)
}

fn read_blob_string(blob: &[u8], cursor: &mut usize) -> Result<String, String> {
    let len = read_blob_u32(blob, cursor)? as usize;
    let end = cursor
        .checked_add(len)
        .ok_or_else(|| "字段缓存长度溢出".to_string())?;
    if end > blob.len() {
        return Err("字段缓存不完整".to_string());
    }
    let value = String::from_utf8_lossy(&blob[*cursor..end]).to_string();
    *cursor = end;
    Ok(value)
}

fn read_blob_u16(blob: &[u8], cursor: &mut usize) -> Result<u16, String> {
    let end = cursor.saturating_add(2);
    if end > blob.len() {
        return Err("字段缓存不完整".to_string());
    }
    let value = u16::from_le_bytes(blob[*cursor..end].try_into().map_err(|_| "字段缓存不完整")?);
    *cursor = end;
    Ok(value)
}

fn read_blob_u32(blob: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let end = cursor.saturating_add(4);
    if end > blob.len() {
        return Err("字段缓存不完整".to_string());
    }
    let value = u32::from_le_bytes(blob[*cursor..end].try_into().map_err(|_| "字段缓存不完整")?);
    *cursor = end;
    Ok(value)
}

fn read_blob_u64(blob: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let end = cursor.saturating_add(8);
    if end > blob.len() {
        return Err("字段缓存不完整".to_string());
    }
    let value = u64::from_le_bytes(blob[*cursor..end].try_into().map_err(|_| "字段缓存不完整")?);
    *cursor = end;
    Ok(value)
}

/// Detect compression by magic bytes and stream the selected STDF payload into
/// the parser without writing a decompressed temp file.
/// Route the input to the right decoder. The input may be a bare STDF, a
/// single-file compressed stream (gz / xz / bz2 / zst), or an archive holding
/// the STDF among other files (zip / 7z / rar / tar — including tar wrapped
/// in any single-file compressor, e.g. .tar.gz). Detection is magic-byte
/// based rather than extension based, so renamed files keep working. Archive
/// entry policy matches the original zip behavior everywhere: the first
/// *.stdf / *.std file entry wins.
fn with_input_reader<T>(
    path: &Path,
    fallback_total: u64,
    parse: impl FnOnce(&mut dyn Read, u64) -> T,
) -> Result<T, String> {
    let mut magic = [0_u8; 8];
    {
        let mut probe = File::open(path).map_err(|error| error.to_string())?;
        let _ = probe.read(&mut magic);
    }

    // Single-file decompressors all share the tar sniffing path, because a
    // .tar.gz/.tar.xz/... shows the compressor's magic outside and the tar
    // layout only inside the decompressed stream.
    if magic.starts_with(&[0x1f, 0x8b]) {
        let input = File::open(path).map_err(|error| error.to_string())?;
        return parse_decompressed(flate2::read::GzDecoder::new(input), fallback_total, parse);
    }
    if magic.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        let input = File::open(path).map_err(|error| error.to_string())?;
        return parse_decompressed(liblzma::read::XzDecoder::new(input), fallback_total, parse);
    }
    if magic.starts_with(b"BZh") {
        let input = File::open(path).map_err(|error| error.to_string())?;
        return parse_decompressed(bzip2::read::BzDecoder::new(input), fallback_total, parse);
    }
    if magic.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        let input = File::open(path).map_err(|error| error.to_string())?;
        let decoder =
            zstd::stream::read::Decoder::new(input).map_err(|error| error.to_string())?;
        return parse_decompressed(decoder, fallback_total, parse);
    }

    if magic.starts_with(&[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c]) {
        return parse_sevenz(path, fallback_total, parse);
    }
    if magic.starts_with(b"Rar!\x1a\x07") {
        return parse_rar(path, fallback_total, parse);
    }

    let is_zip = magic.starts_with(&[0x50, 0x4b, 0x03, 0x04]);
    if !is_zip {
        // Plain uncompressed tar carries no leading magic — "ustar" sits at
        // offset 257. Bare .stdf/.std extensions skip the sniff so payload
        // bytes can never reroute a real STDF.
        if !has_stdf_extension(path) && file_is_tar(path)? {
            let input = File::open(path).map_err(|error| error.to_string())?;
            return parse_tar_stream(input, fallback_total, parse);
        }
        let mut file = File::open(path).map_err(|error| error.to_string())?;
        return Ok(parse(&mut file, fallback_total));
    }

    if let Some(message) = incomplete_zip_message(path) {
        return Err(message);
    }

    // Central-directory reader first: Finder/ditto zips keep entry sizes only
    // in the data descriptor + central directory, which the pure streaming
    // reader rejects ("file length is not available in the local header").
    let input = File::open(path).map_err(|error| error.to_string())?;
    match zip::ZipArchive::new(BufReader::with_capacity(PARSE_BUFFER_SIZE, input)) {
        Ok(mut archive) => {
            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|error| format!("zip 解析失败: {error}"))?;
                if entry.is_file() && is_stdf_entry_name(entry.name()) {
                    let total_bytes = if entry.size() == 0 {
                        fallback_total
                    } else {
                        entry.size()
                    };
                    return Ok(parse(&mut entry, total_bytes));
                }
            }
            return Err("zip 包内没有可解析的文件".to_string());
        }
        // No usable central directory (truncated download, exotic writer):
        // fall back to walking local headers as before.
        Err(_) => {}
    }

    let input = File::open(path).map_err(|error| error.to_string())?;
    let mut zip_stream = BufReader::with_capacity(PARSE_BUFFER_SIZE, input);
    loop {
        let mut entry = zip::read::read_zipfile_from_stream(&mut zip_stream).map_err(|error| {
            incomplete_zip_message(path).unwrap_or_else(|| format!("zip 解析失败: {error}"))
        })?;
        let Some(mut entry) = entry.take() else {
            return Err("zip 包内没有可解析的文件".to_string());
        };
        if entry.is_file() && is_stdf_entry_name(entry.name()) {
            let total_bytes = if entry.size() == 0 {
                fallback_total
            } else {
                entry.size()
            };
            return Ok(parse(&mut entry, total_bytes));
        }
        io::copy(&mut entry, &mut io::sink()).map_err(|error| error.to_string())?;
    }
}

fn has_stdf_extension(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.ends_with(".stdf") || name.ends_with(".std")
}

/// Entry predicate shared by every archive container: the first *.stdf/*.std
/// file wins — but macOS metadata companions do not count. bsdtar/Finder
/// inject AppleDouble "._foo.stdf" entries (and zip an "__MACOSX/" tree)
/// whose payload is resource-fork bytes, not STDF; bsdtar even places them
/// BEFORE the real file, so without this filter the junk entry wins.
fn is_stdf_entry_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase().replace('\\', "/");
    if lower.contains("__macosx/") {
        return false;
    }
    let base = lower.rsplit('/').next().unwrap_or(&lower);
    if base.starts_with("._") {
        return false;
    }
    base.ends_with(".stdf") || base.ends_with(".std")
}

/// POSIX ("ustar\0") and GNU ("ustar ") tar both carry "ustar" at offset 257
/// of the first header block.
const TAR_MAGIC_OFFSET: usize = 257;

fn tar_magic_present(head: &[u8]) -> bool {
    head.len() >= TAR_MAGIC_OFFSET + 5
        && &head[TAR_MAGIC_OFFSET..TAR_MAGIC_OFFSET + 5] == b"ustar"
}

fn file_is_tar(path: &Path) -> Result<bool, String> {
    let mut head = [0_u8; TAR_MAGIC_OFFSET + 5];
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut filled = 0_usize;
    while filled < head.len() {
        match file.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(filled == head.len() && tar_magic_present(&head))
}

/// A decompressed single-file stream is either the STDF itself or a tar
/// wrapping it (.tar.gz / .tar.xz / ...): peek the first header block, sniff
/// the tar magic, then stitch the peeked bytes back in front of the decoder.
fn parse_decompressed<T>(
    mut decoder: impl Read,
    fallback_total: u64,
    parse: impl FnOnce(&mut dyn Read, u64) -> T,
) -> Result<T, String> {
    let mut head = vec![0_u8; TAR_MAGIC_OFFSET + 5];
    let mut filled = 0_usize;
    while filled < head.len() {
        match decoder.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => return Err(format!("解压失败: {error}")),
        }
    }
    head.truncate(filled);
    let is_tar = tar_magic_present(&head);
    let mut rejoined = io::Cursor::new(head).chain(decoder);
    if is_tar {
        parse_tar_stream(rejoined, fallback_total, parse)
    } else {
        Ok(parse(&mut rejoined, fallback_total))
    }
}

fn parse_tar_stream<T>(
    reader: impl Read,
    fallback_total: u64,
    parse: impl FnOnce(&mut dyn Read, u64) -> T,
) -> Result<T, String> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|error| format!("tar 解析失败: {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("tar 解析失败: {error}"))?;
        let is_file = entry.header().entry_type().is_file();
        let name = entry
            .path()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        if is_file && is_stdf_entry_name(&name) {
            let total = if entry.size() == 0 {
                fallback_total
            } else {
                entry.size()
            };
            return Ok(parse(&mut entry, total));
        }
    }
    Err("tar 包内没有 .stdf/.std 文件".to_string())
}

fn parse_sevenz<T>(
    path: &Path,
    fallback_total: u64,
    parse: impl FnOnce(&mut dyn Read, u64) -> T,
) -> Result<T, String> {
    let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|error| format!("7z 解析失败: {error}"))?;
    let mut parse = Some(parse);
    let mut output: Option<T> = None;
    reader
        .for_each_entries(|entry, entry_reader| {
            if entry.is_directory() || !is_stdf_entry_name(entry.name()) {
                // Drain unconditionally: solid blocks decode sequentially and
                // the iterator must not be left mid-entry.
                std::io::copy(entry_reader, &mut std::io::sink())?;
                return Ok(true);
            }
            let total = if entry.size() == 0 {
                fallback_total
            } else {
                entry.size()
            };
            let parse = parse.take().expect("target entry visited once");
            output = Some(parse(entry_reader, total));
            Ok(false)
        })
        .map_err(|error| format!("7z 解压失败: {error}"))?;
    output.ok_or_else(|| "7z 包内没有 .stdf/.std 文件".to_string())
}

fn parse_rar<T>(
    path: &Path,
    fallback_total: u64,
    parse: impl FnOnce(&mut dyn Read, u64) -> T,
) -> Result<T, String> {
    let mut archive = unrar::Archive::new(path)
        .open_for_processing()
        .map_err(|error| format!("rar 解析失败: {error}"))?;
    while let Some(before_file) = archive
        .read_header()
        .map_err(|error| format!("rar 解析失败: {error}"))?
    {
        let entry = before_file.entry();
        let name = entry.filename.to_string_lossy().to_string();
        let unpacked = entry.unpacked_size;
        if entry.is_file() && is_stdf_entry_name(&name) {
            // unrar's C API pushes data through callbacks rather than exposing
            // a Read, so stage the entry in the temp workspace and stream from
            // there. Startup cleanup already covers crash leftovers.
            let staged = temp_workspace_dir().join(format!("{}.rar.stdf", Uuid::new_v4()));
            if let Some(parent) = staged.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let outcome = match before_file.extract_to(&staged) {
                Ok(_) => {
                    let total = if unpacked == 0 { fallback_total } else { unpacked };
                    File::open(&staged)
                        .map_err(|error| error.to_string())
                        .map(|mut file| parse(&mut file, total))
                }
                Err(error) => Err(format!("rar 解压失败: {error}")),
            };
            let _ = std::fs::remove_file(&staged);
            return outcome;
        }
        archive = before_file
            .skip()
            .map_err(|error| format!("rar 解析失败: {error}"))?;
    }
    Err("rar 包内没有 .stdf/.std 文件".to_string())
}

fn incomplete_zip_message(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let file_len = metadata.len();
    let mut file = File::open(path).ok()?;
    let mut header = [0_u8; 30];
    file.read_exact(&mut header).ok()?;

    if &header[0..4] != b"PK\x03\x04" {
        return None;
    }

    let compressed_size_32 = u32::from_le_bytes(header[18..22].try_into().ok()?);
    let file_name_len = u16::from_le_bytes(header[26..28].try_into().ok()?) as u64;
    let extra_len = u16::from_le_bytes(header[28..30].try_into().ok()?) as usize;
    let extra_offset = 30_u64.checked_add(file_name_len)?;
    let data_offset = extra_offset.checked_add(extra_len as u64)?;

    let mut compressed_size = if compressed_size_32 == u32::MAX {
        None
    } else {
        Some(compressed_size_32 as u64)
    };
    if compressed_size.is_none() {
        file.seek(SeekFrom::Start(extra_offset)).ok()?;
        let mut extra = vec![0_u8; extra_len];
        file.read_exact(&mut extra).ok()?;
        let mut cursor = 0_usize;
        while cursor + 4 <= extra.len() {
            let header_id = u16::from_le_bytes(extra[cursor..cursor + 2].try_into().ok()?);
            let field_len = u16::from_le_bytes(extra[cursor + 2..cursor + 4].try_into().ok()?)
                as usize;
            cursor += 4;
            if cursor + field_len > extra.len() {
                break;
            }
            if header_id == 0x0001 {
                let field = &extra[cursor..cursor + field_len];
                let field_cursor = 8_usize;
                if field_cursor + 8 <= field.len() {
                    compressed_size = Some(u64::from_le_bytes(
                        field[field_cursor..field_cursor + 8].try_into().ok()?,
                    ));
                }
                break;
            }
            cursor += field_len;
        }
    }

    let compressed_size = compressed_size?;
    let expected_len = data_offset.checked_add(compressed_size)?;
    if expected_len > file_len {
        return Some(format!(
            "zip 压缩包不完整：文件实际大小 {} 字节，但压缩头声明至少需要 {} 字节。请重新下载或让对方重新发送原始文件。",
            file_len, expected_len
        ));
    }

    None
}

fn record_summary(session_id: &str, index: usize, record: &ParsedRecord) -> RecordSummary {
    RecordSummary {
        id: format!("{session_id}:{index}"),
        record_type: record.record_type.to_string(),
        index,
        offset: record.offset,
        length: record.length,
        status: record.status.clone(),
    }
}

fn parser_error_message(error: ParserError) -> String {
    error.to_string()
}

fn format_system_time(value: SystemTime) -> String {
    value
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_default()
}

fn paginate<T>(items: Vec<T>, page: usize, page_size: usize) -> Vec<T> {
    let start = page.saturating_mul(page_size);
    items.into_iter().skip(start).take(page_size).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::mpsc, time::Duration};

    fn record(rec_typ: u8, rec_sub: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        bytes.push(rec_typ);
        bytes.push(rec_sub);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn cn(value: &str) -> Vec<u8> {
        let mut bytes = vec![value.len() as u8];
        bytes.extend_from_slice(value.as_bytes());
        bytes
    }

    fn u1(value: u8) -> Vec<u8> {
        vec![value]
    }

    fn u2(value: u16) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    fn u4(value: u32) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    fn i1(value: i8) -> Vec<u8> {
        vec![value as u8]
    }

    fn i2(value: i16) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    fn i4(value: i32) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    fn r4(value: f32) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    fn c1(value: &str) -> Vec<u8> {
        vec![value.as_bytes().first().copied().unwrap_or_default()]
    }

    fn dn(bit_count: u16) -> Vec<u8> {
        bit_count.to_le_bytes().to_vec()
    }

    fn push_payload(buf: &mut Vec<u8>, parts: &[Vec<u8>]) {
        for part in parts {
            buf.extend_from_slice(part);
        }
    }

    struct FixtureSessionResult {
        groups: Vec<RecordGroup>,
        fields: Vec<EnrichedField>,
        search_total: usize,
    }

    fn parse_fixture_session(path: &Path) -> FixtureSessionResult {
        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(path.to_string_lossy().to_string(), move |event| {
                tx.send(event).expect("send event");
            })
            .expect("open session");

        let mut completed = false;
        for _ in 0..100 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let groups = manager
            .get_record_groups(&session.session_id)
            .expect("groups");
        let page = manager
            .get_records(&session.session_id, "PTR", 0, 1)
            .expect("records");
        let record = page.records.first().expect("PTR record");
        let fields = manager
            .get_record_fields(&session.session_id, &record.id)
            .expect("fields");
        let search_total = manager
            .search_fields(&session.session_id, "VDD_STREAM", 0, 10, |_, _| {})
            .expect("search")
            .total;

        FixtureSessionResult {
            groups,
            fields,
            search_total,
        }
    }

    fn decompressed_temp_files() -> Vec<PathBuf> {
        let mut files = temp_workspace_dir()
            .read_dir()
            .ok()
            .into_iter()
            .flat_map(|entries| entries.filter_map(Result::ok))
            .map(|entry| entry.path())
            .filter(|path| {
                matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("std" | "stdf")
                )
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    #[test]
    fn parser_reads_common_records_and_field_descriptions() {
        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));
        bytes.extend(record(5, 10, &[1, 0, 0, 0, 2, 0, 0, 0]));
        let mut ptr = Vec::new();
        ptr.extend_from_slice(&100_u32.to_le_bytes());
        ptr.extend_from_slice(&[1, 2, 0, 0]);
        ptr.extend_from_slice(&1.25_f32.to_le_bytes());
        ptr.extend(cn("VDD"));
        ptr.extend(cn(""));
        bytes.extend(record(15, 10, &ptr));
        let mut prr = vec![1, 2, 0];
        prr.extend_from_slice(&3_u16.to_le_bytes());
        prr.extend_from_slice(&1_u16.to_le_bytes());
        prr.extend_from_slice(&1_u16.to_le_bytes());
        prr.extend_from_slice(&12_i16.to_le_bytes());
        prr.extend_from_slice(&34_i16.to_le_bytes());
        prr.extend_from_slice(&50_u32.to_le_bytes());
        prr.extend(cn("PART-1"));
        prr.extend(cn(""));
        prr.push(0);
        bytes.extend(record(5, 20, &prr));

        let mut parsed = Vec::new();
        parse_reader(
            &mut Cursor::new(bytes.clone()),
            bytes.len() as u64,
            |parsed_record| {
                parsed.push(parsed_record);
                true
            },
            |_, _| {},
        )
        .expect("fixture should parse");

        assert_eq!(
            parsed
                .iter()
                .map(|item| item.record_type)
                .collect::<Vec<_>>(),
            vec!["FAR", "PIR", "PTR", "PRR"]
        );
        assert_eq!(parsed[0].fields[1].name, "STDF_VER");
        assert_eq!(parsed[0].fields[1].description, "STDF 版本");
        assert_eq!(field_value(&parsed[2].fields, "TEST_NUM"), "100");
        assert_eq!(field_value(&parsed[3].fields, "HEAD_NUM"), "1");
    }

    #[test]
    fn parser_reports_truncated_payload_without_panic() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8_u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 10, 2, 4]);
        let result = parse_reader(&mut Cursor::new(bytes), 8, |_| true, |_, _| {});
        assert!(matches!(result, Err(ParserError::TruncatedPayload { .. })));
    }

    #[test]
    fn session_manager_pages_searches_and_cancels() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("demo.stdf");
        let mut bytes = Vec::new();
        for _ in 0..80 {
            bytes.extend(record(0, 10, &[2, 4]));
        }
        std::fs::write(&file_path, bytes).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        assert_eq!(session.file_name, "demo.stdf");
        assert_eq!(session.file_path, file_path.to_string_lossy());
        assert_eq!(session.file_dir, dir.path().to_string_lossy());
        assert!(!session.modified_time.is_empty());

        let mut completed = false;
        for _ in 0..500 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let groups = manager
            .get_record_groups(&session.session_id)
            .expect("groups");
        assert_eq!(
            groups,
            vec![RecordGroup {
                record_type: "FAR".to_string(),
                count: 80
            }]
        );
        let page = manager
            .get_records(&session.session_id, "FAR", 1, 25)
            .expect("records page");
        assert_eq!(page.records.len(), 25);
        assert_eq!(page.total, 80);
        let search = manager
            .search_fields(&session.session_id, "STDF_VER", 0, 10, |_, _| {})
            .expect("search");
        assert_eq!(search.total, 80);

        manager.cancel_parse(&session.session_id).expect("cancel");
    }

    #[test]
    fn session_searches_fields_without_storing_duplicate_search_blob() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("search.stdf");
        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));
        let mut ptr = Vec::new();
        ptr.extend_from_slice(&101_u32.to_le_bytes());
        ptr.extend_from_slice(&[1, 1, 0, 0]);
        ptr.extend_from_slice(&3.3_f32.to_le_bytes());
        ptr.extend(cn("VDD_CORE"));
        ptr.extend(cn(""));
        bytes.extend(record(15, 10, &ptr));
        std::fs::write(&file_path, bytes).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");

        let mut completed = false;
        for _ in 0..100 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let search = manager
            .search_fields(&session.session_id, "VDD_CORE", 0, 10, |_, _| {})
            .expect("search");
        assert_eq!(search.total, 1);
        assert_eq!(search.results[0].field.name, "TEST_TXT");

        let conn = manager.open_db(&session.session_id).expect("open db");
        let mut stmt = conn
            .prepare("PRAGMA table_info(records)")
            .expect("table info");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("columns");
        let columns = rows
            .collect::<Result<Vec<_>, _>>()
            .expect("collect columns");
        assert!(
            !columns.iter().any(|column| column == "search_blob"),
            "records table should not duplicate field JSON into search_blob"
        );
    }

    fn build_test_item_fixture() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));

        let mut hbr = Vec::new();
        push_payload(
            &mut hbr,
            &[
                u1(1),
                u1(0),
                u2(3),
                u4(2),
                c1("P"),
                cn("GOOD"),
            ],
        );
        bytes.extend(record(1, 40, &hbr));

        let mut sbr = Vec::new();
        push_payload(
            &mut sbr,
            &[
                u1(1),
                u1(0),
                u2(2),
                u4(2),
                c1("P"),
                cn("PASS"),
            ],
        );
        bytes.extend(record(1, 50, &sbr));

        let mut pmr = Vec::new();
        push_payload(
            &mut pmr,
            &[
                u2(1),
                u2(0),
                cn(""),
                cn("PIN1"),
                cn("L1"),
                u1(1),
                u1(2),
            ],
        );
        bytes.extend(record(1, 60, &pmr));

        for (site_num, result, test_t, part_txt) in [
            (2_u8, 1.05_f32, 603_502_u32, "site two"),
            (5_u8, 1.15_f32, 648_983_u32, "site five"),
        ] {
            let mut pir = Vec::new();
            push_payload(&mut pir, &[u1(1), u1(site_num)]);
            bytes.extend(record(5, 10, &pir));

            let mut ptr = Vec::new();
            push_payload(
                &mut ptr,
                &[
                    u4(21_000_001),
                    u1(1),
                    u1(site_num),
                    u1(0),
                    u1(0),
                    r4(result),
                    cn("VDD_CORE"),
                    cn(""),
                    u1(0),
                    i1(0),
                    i1(0),
                    i1(0),
                    r4(1.0),
                    r4(1.2),
                    cn("V"),
                ],
            );
            bytes.extend(record(15, 10, &ptr));

            let mut ftr = Vec::new();
            push_payload(
                &mut ftr,
                &[
                    u4(22_000_001),
                    u1(1),
                    u1(site_num),
                    u1(0),
                    u1(0),
                    u4(0),
                    u4(0),
                    u4(0),
                    u4(0),
                    i4(0),
                    i4(0),
                    i2(0),
                    u2(0),
                    u2(0),
                    u2(0),
                    u2(0),
                    u2(0),
                    u2(0),
                    dn(0),
                    cn("FTR_VEC"),
                ],
            );
            bytes.extend(record(15, 20, &ftr));

            let mut prr = Vec::new();
            push_payload(
                &mut prr,
                &[
                    u1(1),
                    u1(site_num),
                    u1(0),
                    u2(2),
                    u2(3),
                    u2(2),
                    i2(0),
                    i2(0),
                    u4(test_t),
                    cn("PART-1"),
                    cn(part_txt),
                ],
            );
            bytes.extend(record(5, 20, &prr));
        }

        bytes
    }

    #[test]
    fn test_item_view_groups_by_part_id_and_splits_sites_in_source_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("test-items.stdf");
        std::fs::write(&file_path, build_test_item_fixture()).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");

        let mut completed = false;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let view = manager
            .get_test_item_page(&session.session_id, 0, 100, 0, 100, &[], "")
            .expect("test item page");
        assert_eq!(view.columns.len(), 2);
        assert_eq!(
            view.columns
                .iter()
                .map(|column| column.record_type.as_str())
                .collect::<Vec<_>>(),
            vec!["PTR", "FTR"]
        );
        assert_eq!(
            view.rows
                .iter()
                .map(|row| row.part_id.as_str())
                .collect::<Vec<_>>(),
            vec!["PART-1", "PART-1"]
        );
        assert_eq!(
            view.rows
                .iter()
                .map(|row| row.site_num.as_str())
                .collect::<Vec<_>>(),
            vec!["2", "5"]
        );
        assert_eq!(view.rows[0].results.len(), 2);
        assert_eq!(view.rows[0].results[0].status, "P");
        assert_eq!(view.rows[0].results[1].status, "P");
        assert_eq!(view.pmr_count, 1);
        // PMR contents (pin names) stay in the accumulator for future use —
        // verify the parse populated them even though pages only ship a count.
        {
            let guard = manager.sessions.lock().expect("sessions lock");
            let state = guard.get(&session.session_id).expect("session state");
            assert_eq!(
                state.test_items.pmr_lookup.get("1").map(|e| e.phy_nam.as_str()),
                Some("PIN1")
            );
        }
        assert_eq!(view.status, ParseStatus::Complete);
    }

    #[test]
    fn test_item_view_resolves_hbin_and_sbin_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("test-items-bins.stdf");
        std::fs::write(&file_path, build_test_item_fixture()).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");

        let mut completed = false;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let view = manager
            .get_test_item_page(&session.session_id, 0, 100, 0, 100, &[], "")
            .expect("test item page");
        assert_eq!(view.rows.len(), 2);
        for row in &view.rows {
            assert_eq!(row.sbin_num, "2");
            assert_eq!(row.sbin_name, "PASS");
            assert_eq!(row.hbin_num, "3");
            assert_eq!(row.hbin_name, "GOOD");
            assert_eq!(row.part_txt, if row.site_num == "2" { "site two" } else { "site five" });
        }
    }

    #[test]
    fn scaling_helpers_follow_stdf_inverted_prefix_convention() {
        assert_eq!(scale_prefix(3), "m");
        assert_eq!(scale_prefix(-3), "K");
        assert_eq!(scale_prefix(6), "u");
        assert_eq!(scale_prefix(2), "%");
        assert_eq!(scale_prefix(0), "");

        assert_eq!(parse_scale("-3"), -3);
        assert_eq!(parse_scale(""), 0);
        assert_eq!(parse_scale("oops"), 0);

        // Stored values are base SI units; displayed = stored * 10^scale.
        // 0.161 V at milli (+3) -> 161 mV; 32000 Hz at kilo (-3) -> 32 KHz.
        assert!((to_display(0.161, 3) - 161.0).abs() < 1e-9);
        assert!((to_display(32000.0, -3) - 32.0).abs() < 1e-9);

        // to_base only normalizes for limit comparison (cancels against the limit scale).
        assert!((to_base(2.3, 3) - 0.0023).abs() < 1e-12);

        // display_limit renders a base limit in the column scale; scale 0 is verbatim.
        assert_eq!(display_limit("1.0", 0), "1.0");
        assert_eq!(display_limit("0.05", 3), "50");

        // Pass/fail is judged on base units; no limit means no verdict.
        assert_eq!(judge(1.05, Some(1.0), Some(1.2)), "P");
        assert_eq!(judge(1.30, Some(1.0), Some(1.2)), "F");
        assert_eq!(judge(0.5, None, None), "");
    }

    #[test]
    fn test_item_page_filters_windows_and_projects_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("test-items-page.stdf");
        std::fs::write(&file_path, build_test_item_fixture()).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        let mut completed = false;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");
        let sid = session.session_id;

        // Full page: 2 columns (PTR, FTR) x 2 rows (sites 2, 5).
        let full = manager.get_test_item_page(&sid, 0, 50, 0, 50, &[], "").expect("page");
        assert_eq!(full.total_columns, 2);
        assert_eq!(full.total_rows, 2);
        assert_eq!(full.columns.len(), 2);
        assert_eq!(full.rows[0].results.len(), 2);

        // Selecting a single column narrows the page and projects each row onto it.
        let only = [format!("FTR:{}", 22_000_001)];
        let ftr = manager.get_test_item_page(&sid, 0, 50, 0, 50, &only, "").expect("page");
        assert_eq!(ftr.total_columns, 1);
        assert_eq!(ftr.columns[0].record_type, "FTR");
        assert_eq!(ftr.rows[0].results.len(), 1);

        // The lightweight column list exposes the same keys the dialog selects on.
        let cols = manager.get_test_item_columns(&sid).expect("columns");
        assert_eq!(cols.len(), 2);
        assert!(cols.iter().any(|c| c.key == "FTR:22000001"));

        // Site filter narrows the rows.
        let site5 = manager.get_test_item_page(&sid, 0, 50, 0, 50, &[], "5").expect("page");
        assert_eq!(site5.total_rows, 1);
        assert_eq!(site5.rows[0].site_num, "5");

        // Column window: offset 1 yields only the 2nd column.
        let win = manager.get_test_item_page(&sid, 0, 50, 1, 50, &[], "").expect("page");
        assert_eq!(win.col_offset, 1);
        assert_eq!(win.columns.len(), 1);
        assert_eq!(win.columns[0].record_type, "FTR");
        assert_eq!(win.rows[0].results.len(), 1);

        // Row offset past the end yields no rows but still reports the true total.
        let past = manager.get_test_item_page(&sid, 10, 50, 0, 50, &[], "").expect("page");
        assert_eq!(past.total_rows, 2);
        assert!(past.rows.is_empty());
    }

    #[test]
    fn test_item_page_wire_shape_ships_pmr_count_and_no_per_cell_test_num() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("test-items-wire.stdf");
        std::fs::write(&file_path, build_test_item_fixture()).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        let mut completed = false;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let page = manager
            .get_test_item_page(&session.session_id, 0, 50, 0, 50, &[], "")
            .expect("page");
        let json = serde_json::to_value(&page).expect("serialize page");

        // The PMR map itself never crosses the IPC bridge — only its size does
        // (the UI renders just a "N PMR" pill).
        assert!(json.get("pmr_lookup").is_none(), "pmr_lookup should not be serialized");
        assert_eq!(
            json.get("pmr_count").and_then(|v| v.as_u64()),
            Some(1),
            "pmr_count should carry the lookup size"
        );

        // Cells align to `columns` by index, so a per-cell test_num is pure
        // payload overhead (~20% of a page).
        let cell = &json["rows"][0]["results"][0];
        assert!(cell.get("value").is_some(), "cell keeps its value");
        assert!(cell.get("test_num").is_none(), "cell test_num should not be serialized");
    }

    #[test]
    fn records_pagination_is_stable_across_pages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("paging.stdf");
        let mut bytes = Vec::new();
        for _ in 0..130 {
            bytes.extend(record(0, 10, &[2, 4]));
        }
        std::fs::write(&file_path, bytes).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        let mut completed = false;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");
        let sid = &session.session_id;

        let p0 = manager.get_records(sid, "FAR", 0, 50).expect("page 0");
        assert_eq!(p0.total, 130);
        assert_eq!(p0.records.len(), 50);
        assert_eq!(p0.records[0].index, 0);
        assert_eq!(p0.records[49].index, 49);

        let p2 = manager.get_records(sid, "FAR", 2, 50).expect("page 2");
        assert_eq!(p2.total, 130);
        assert_eq!(p2.records.len(), 30);
        assert_eq!(p2.records[0].index, 100);
        assert_eq!(p2.records[29].index, 129);

        // Field detail resolves for a deep record id.
        let fields = manager
            .get_record_fields(sid, &p2.records[10].id)
            .expect("fields");
        assert_eq!(field_value(&fields_of(&fields), "STDF_VER"), "4");

        // Unknown group → empty page with zero total.
        let none = manager.get_records(sid, "PTR", 0, 50).expect("empty group");
        assert_eq!(none.total, 0);
        assert!(none.records.is_empty());

        // Page past the end → empty but still reports the true total.
        let past = manager.get_records(sid, "FAR", 9, 50).expect("past end");
        assert_eq!(past.total, 130);
        assert!(past.records.is_empty());
    }

    #[test]
    fn records_pages_follow_group_order_when_types_interleave() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("interleaved.stdf");
        std::fs::write(&file_path, build_test_item_fixture()).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        let mut completed = false;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");
        let sid = &session.session_id;

        // PTRs sit interleaved between PIR/FTR/PRR records, so their global
        // indices are not contiguous — pages must still walk them in parse
        // order with correct global `index` stamps and resolvable ids.
        let ptrs = manager.get_records(sid, "PTR", 0, 50).expect("ptr page");
        assert!(ptrs.total >= 2, "fixture should hold at least two PTRs");
        assert_eq!(ptrs.records.len(), ptrs.total);
        let indices: Vec<usize> = ptrs.records.iter().map(|r| r.index).collect();
        let mut sorted = indices.clone();
        sorted.sort_unstable();
        assert_eq!(indices, sorted, "parse order == ascending global index");
        assert!(indices[0] > 0, "PTR is not the first record in the file");

        let fields = manager
            .get_record_fields(sid, &ptrs.records[ptrs.total - 1].id)
            .expect("ptr fields");
        assert!(!field_value(&fields_of(&fields), "TEST_NUM").is_empty());
    }

    /// Unwrap EnrichedField wrappers back to plain fields for assertions.
    fn fields_of(fields: &[EnrichedField]) -> Vec<RecordField> {
        fields.iter().map(|f| f.field.clone()).collect()
    }

    #[test]
    fn opening_a_new_file_evicts_previous_sessions_and_their_databases() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("evict.stdf");
        std::fs::write(&file_path, build_test_item_fixture()).expect("write fixture");

        let manager = SessionManager::default();
        let open_and_wait = |manager: &SessionManager| {
            let (tx, rx) = mpsc::channel();
            let session = manager
                .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                    let _ = tx.send(event);
                })
                .expect("open session");
            for _ in 0..200 {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(SessionEvent::Complete(_)) => return session,
                    Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
            panic!("parser should complete");
        };

        let first = open_and_wait(&manager);
        let first_db = temp_workspace_dir().join(format!("{}.db", first.session_id));
        assert!(first_db.exists(), "first session should have an index db");

        // The viewer shows one file at a time — a second open must evict the
        // first session and delete its database, otherwise every opened file
        // leaks a multi-GB index until the next app launch.
        let second = open_and_wait(&manager);
        assert!(
            manager.get_session_snapshot(&first.session_id).is_err(),
            "first session should be evicted"
        );
        assert!(!first_db.exists(), "evicted session's database should be deleted");
        assert!(manager.get_session_snapshot(&second.session_id).is_ok());
        let second_db = temp_workspace_dir().join(format!("{}.db", second.session_id));
        assert!(second_db.exists(), "live session keeps its database");
        // Leave no trace behind for other tests / the shared temp dir.
        let _ = std::fs::remove_file(second_db);
    }

    /// Retest wafer: two dies, three touchdowns. Die (10,20) fails (bin 2)
    /// then passes on retest (bin 1, a NEW PART_ID); die (11,20) fails.
    /// The optional HBR/SBR records carry the tester's official (already
    /// deduplicated) counts: bin 1 = 1, bin 2 = 1.
    fn build_retest_fixture(with_bin_records: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));
        if with_bin_records {
            for (typ, num, pf, name) in [
                (1_u8, 1_u16, "P", "GOOD"),
                (1, 2, "F", "BAD"),
                (2, 1, "P", "pass"),
                (2, 2, "F", "fail"),
            ] {
                let mut bin = Vec::new();
                push_payload(
                    &mut bin,
                    &[u1(255), u1(0), u2(num), u4(1), c1(pf), cn(name)],
                );
                bytes.extend(record(1, if typ == 1 { 40 } else { 50 }, &bin));
            }
        }
        for (hbin, sbin, x, y, part) in [
            (2_u16, 2_u16, 10, 20, "P1"),
            (2, 2, 11, 20, "P2"),
            (1, 1, 10, 20, "P3"),
        ] {
            let mut pir = Vec::new();
            push_payload(&mut pir, &[u1(1), u1(1)]);
            bytes.extend(record(5, 10, &pir));
            let mut prr = Vec::new();
            push_payload(
                &mut prr,
                &[
                    u1(1),
                    u1(1),
                    u1(0),
                    u2(0),
                    u2(hbin),
                    u2(sbin),
                    i2(x),
                    i2(y),
                    u4(1000),
                    cn(part),
                    cn(""),
                ],
            );
            bytes.extend(record(5, 20, &prr));
        }
        bytes
    }

    fn parse_to_completion(bytes: Vec<u8>, name: &str) -> (SessionManager, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join(name);
        std::fs::write(&file_path, bytes).expect("write fixture");
        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => return (manager, session.session_id),
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        panic!("parser should complete");
    }

    #[test]
    fn bin_summary_prefers_declared_counts_on_retest_files() {
        let (manager, sid) = parse_to_completion(build_retest_fixture(true), "retest.stdf");
        let summary = manager.get_bin_summary(&sid).expect("bin summary");
        // 3 touchdowns but 2 dies — the HBR/SBR declared counts are the truth.
        assert_eq!(summary.total_parts, 2);
        assert_eq!(summary.hbin_pass, 1);
        assert_eq!(summary.sbin_pass, 1);
        let hbin1 = summary.hbins.iter().find(|b| b.num == "1").expect("hbin 1");
        assert_eq!(hbin1.count, 1);
        assert_eq!(hbin1.name, "GOOD");
        let hbin2 = summary.hbins.iter().find(|b| b.num == "2").expect("hbin 2");
        assert_eq!(hbin2.count, 1);
    }

    #[test]
    fn bin_summary_dedupes_dies_by_coordinates_without_bin_records() {
        let (manager, sid) = parse_to_completion(build_retest_fixture(false), "retest-nobins.stdf");
        let summary = manager.get_bin_summary(&sid).expect("bin summary");
        // No HBR/SBR: fall back to per-die last-result over (X,Y) coordinates.
        assert_eq!(summary.total_parts, 2);
        assert!(!summary.has_bin_pf);
        let hbin1 = summary.hbins.iter().find(|b| b.num == "1").expect("hbin 1");
        assert_eq!(hbin1.count, 1);
        let hbin2 = summary.hbins.iter().find(|b| b.num == "2").expect("hbin 2");
        assert_eq!(hbin2.count, 1);
        // "bin 1 = pass" convention applies without PF flags.
        assert_eq!(summary.sbin_pass, 1);
    }

    #[test]
    fn bin_summary_reports_totals_pass_counts_and_per_bin_stats() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("bin-summary.stdf");
        std::fs::write(&file_path, build_test_item_fixture()).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        let mut completed = false;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let summary = manager
            .get_bin_summary(&session.session_id)
            .expect("bin summary");
        assert_eq!(summary.total_parts, 2);
        assert_eq!(summary.sbin_pass, 2);
        assert_eq!(summary.hbin_pass, 2);
        assert!(summary.has_bin_pf);
        assert_eq!(summary.sbins.len(), 1);
        assert_eq!(summary.sbins[0].num, "2");
        assert_eq!(summary.sbins[0].name, "PASS");
        assert_eq!(summary.sbins[0].pf, "P");
        assert_eq!(summary.sbins[0].count, 2);
        assert_eq!(summary.hbins.len(), 1);
        assert_eq!(summary.hbins[0].num, "3");
        assert_eq!(summary.hbins[0].name, "GOOD");
        assert_eq!(summary.hbins[0].count, 2);
        assert_eq!(summary.status, ParseStatus::Complete);
    }

    #[test]
    fn fields_blob_stores_values_only_for_spec_backed_records() {
        // Parse one real FAR record so the blob comes from the production path.
        let bytes = record(0, 10, &[2, 4]);
        let mut cursor = Cursor::new(bytes);
        let mut parsed = Vec::new();
        crate::parser::parse_reader(
            &mut cursor,
            0,
            |r| {
                parsed.push(r);
                true
            },
            |_, _| {},
        )
        .expect("parse fixture");
        let rec = &parsed[0];
        assert_eq!(rec.record_type, "FAR");

        let blob = encode_fields_blob(rec);
        let contains = |needle: &[u8]| blob.windows(needle.len()).any(|w| w == needle);
        // Names/types/descriptions are static per record layout — storing them
        // per record blew a 3.2GB file up into a 35GB index. Only values (and
        // which layout to rebuild from) may hit the disk.
        assert!(!contains("STDF 版本".as_bytes()), "description serialized into blob");
        assert!(!contains(b"STDF_VER"), "field name serialized into blob");

        let decoded = decode_fields_blob(&blob).expect("decode");
        assert_eq!(decoded.len(), rec.fields.len());
        assert_eq!(decoded[1].name, "STDF_VER");
        assert_eq!(decoded[1].value, "4");
        assert_eq!(decoded[1].description, "STDF 版本");
        assert_eq!(decoded[0].name, "CPU_TYPE");
        assert_eq!(decoded[0].value, "2");
    }

    #[test]
    fn record_summaries_ship_no_display_summary_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("records-wire.stdf");
        std::fs::write(&file_path, build_test_item_fixture()).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        let mut completed = false;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let page = manager
            .get_records(&session.session_id, "FAR", 0, 10)
            .expect("records");
        assert_eq!(page.total, 1);
        // The UI never displays a record "summary" preview — building one per
        // record (format! per field) and shipping it is parse-time waste.
        let json = serde_json::to_value(&page).expect("serialize");
        assert!(
            json["records"][0].get("summary").is_none(),
            "summary should not exist on the record wire format"
        );
    }

    #[test]
    fn test_item_page_resolves_bin_name_pf_from_late_hbr_sbr() {
        // Simulate a part that closed (PRR) before its HBR/SBR were seen — the common
        // case, since bin records sit at the end of the file.
        let mut acc = TestItemAccumulator::default();
        let key = ("PART-1".to_string(), "0".to_string());
        acc.part_order.push(key.clone());
        acc.part_rows.insert(
            key,
            TestItemPartRow {
                part_id: "PART-1".to_string(),
                site_num: "0".to_string(),
                site_nums: vec!["0".to_string()],
                head_num: "1".to_string(),
                sbin_num: "5".to_string(),
                sbin_name: String::new(),
                sbin_pf: String::new(),
                hbin_num: "7".to_string(),
                hbin_name: String::new(),
                hbin_pf: String::new(),
                test_t: "100".to_string(),
                part_txt: String::new(),
                results: Vec::new(),
            },
        );
        // HBR/SBR arrive afterwards.
        acc.sbin_names.insert("5".to_string(), "SOFT_PASS".to_string());
        acc.sbin_pf.insert("5".to_string(), "P".to_string());
        acc.hbin_names.insert("7".to_string(), "HARD_FAIL".to_string());
        acc.hbin_pf.insert("7".to_string(), "F".to_string());

        let page = build_test_item_page("s", &acc, &ParseStatus::Complete, 0, 10, 0, 10, &[], "");
        let row = &page.rows[0];
        assert_eq!(row.sbin_name, "SOFT_PASS");
        assert_eq!(row.sbin_pf, "P");
        assert_eq!(row.hbin_name, "HARD_FAIL");
        assert_eq!(row.hbin_pf, "F");
    }

    #[test]
    fn export_csv_mirrors_sts8300_layout() {
        let mut acc = TestItemAccumulator::default();
        let key = ("PTR".to_string(), 100u32);
        acc.column_order.push(key.clone());
        acc.columns_by_key.insert(
            key,
            TestItemColumn {
                record_type: "PTR".to_string(),
                test_num: 100,
                test_name: "VDD".to_string(),
                low_limit: "1".to_string(),
                high_limit: "2".to_string(),
                unit: "mV".to_string(),
                pmr_indices: Vec::new(),
            },
        );
        acc.sbin_names.insert("1".to_string(), "PASS".to_string());
        acc.sbin_pf.insert("1".to_string(), "P".to_string());
        acc.hbin_names.insert("1".to_string(), "GOOD".to_string());
        acc.hbin_pf.insert("1".to_string(), "P".to_string());
        // What the PRR arm records during a real parse — the bin summary
        // counts dies from here (no HBR/SBR declared counts in this fixture).
        acc.die_bins
            .insert("id:P1:0".to_string(), ("1".to_string(), "1".to_string()));

        let pkey = ("P1".to_string(), "0".to_string());
        acc.part_order.push(pkey.clone());
        acc.part_rows.insert(
            pkey,
            TestItemPartRow {
                part_id: "P1".to_string(),
                site_num: "0".to_string(),
                site_nums: vec!["0".to_string()],
                head_num: "1".to_string(),
                sbin_num: "1".to_string(),
                sbin_name: String::new(),
                sbin_pf: String::new(),
                hbin_num: "1".to_string(),
                hbin_name: String::new(),
                hbin_pf: String::new(),
                test_t: "1500".to_string(),
                part_txt: String::new(),
                results: vec![TestItemCell {
                    value: "1.5".to_string(),
                    status: "P".to_string(),
                    record_position: None,
                }],
            },
        );

        let snapshot = SessionSnapshot {
            session_id: "s".to_string(),
            groups: Vec::new(),
            key_fields: HashMap::new(),
            first_records: HashMap::new(),
            bytes_read: 0,
            total_bytes: 0,
            status: ParseStatus::Complete,
        };

        let csv = build_test_item_csv(&snapshot, &acc);
        assert!(csv.starts_with('\u{FEFF}'));
        assert!(csv.contains("SITE_NUM,PART_ID,PASSFG,HARD_BIN,SOFT_BIN,T_TIME,VDD\n"));
        assert!(csv.contains("Unit,,,,,ms,mV\n"));
        assert!(csv.contains("LimitL,,,,,,1\n"));
        assert!(csv.contains("LimitU,,,,,,2\n"));
        assert!(csv.contains("0,P1,TRUE,1,1,1500,1.5\n"));
        assert!(csv.contains("Total: 1\n"));
        assert!(csv.contains("HBin[1] GOOD  1  100.00%\n"));
        assert!(csv.contains("SBin[1] PASS  1  100.00%  1\n"));
        // Station line and TEST_NUM column are intentionally absent.
        assert!(!csv.contains("Station"));
        assert!(!csv.contains("TEST_NUM"));
    }

    #[test]
    fn session_snapshot_exposes_key_fields_before_sqlite_queries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("snapshot.stdf");
        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));
        let mut mir = Vec::new();
        mir.extend_from_slice(&1_u32.to_le_bytes());
        mir.extend_from_slice(&2_u32.to_le_bytes());
        mir.push(3);
        mir.extend([b'P', b'R', b' ']);
        mir.extend_from_slice(&10_u16.to_le_bytes());
        mir.push(b' ');
        mir.extend(cn("LOT-SNAPSHOT"));
        mir.extend(cn("PART-A"));
        mir.extend(cn("NODE-1"));
        mir.extend(cn("T2000"));
        mir.extend(cn("JOB-A"));
        bytes.extend(record(1, 10, &mir));
        let mut ptr = Vec::new();
        ptr.extend_from_slice(&101_u32.to_le_bytes());
        ptr.extend_from_slice(&[1, 1, 0, 0]);
        ptr.extend_from_slice(&3.3_f32.to_le_bytes());
        ptr.extend(cn("VDD_CORE"));
        ptr.extend(cn(""));
        bytes.extend(record(15, 10, &ptr));
        std::fs::write(&file_path, bytes).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");

        let mut saw_mir_snapshot = false;
        let mut saw_ptr_first = false;
        for _ in 0..100 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Snapshot(snapshot)) => {
                    if let Some(mir_fields) = snapshot.key_fields.get("MIR") {
                        assert!(snapshot.bytes_read > 0);
                        assert_eq!(
                            mir_fields
                                .iter()
                                .find(|field| field.name == "LOT_ID")
                                .map(|field| field.value.as_str()),
                            Some("LOT-SNAPSHOT")
                        );
                        assert_eq!(
                            snapshot
                                .groups
                                .iter()
                                .find(|group| group.record_type == "MIR")
                                .map(|group| group.count),
                            Some(1)
                        );
                        saw_mir_snapshot = true;
                    }
                    if snapshot.first_records.contains_key("PTR") {
                        saw_ptr_first = true;
                    }
                    if saw_mir_snapshot && saw_ptr_first {
                        break;
                    }
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(saw_mir_snapshot, "expected MIR snapshot event");
        assert!(saw_ptr_first, "expected first PTR snapshot");

        manager.cancel_parse(&session.session_id).expect("cancel");
        let cancelled = manager
            .get_session_snapshot(&session.session_id)
            .expect("snapshot");
        assert_eq!(cancelled.status, ParseStatus::Cancelled);
    }

    #[test]
    fn session_streams_zip_and_matches_plain_stdf_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stdf_path = dir.path().join("fixture.std");
        let zip_path = dir.path().join("fixture.std.zip");
        let existing_decompressed = decompressed_temp_files();

        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));
        let mut ptr = Vec::new();
        ptr.extend_from_slice(&202_u32.to_le_bytes());
        ptr.extend_from_slice(&[1, 1, 0, 0]);
        ptr.extend_from_slice(&1.8_f32.to_le_bytes());
        ptr.extend(cn("VDD_STREAM"));
        ptr.extend(cn(""));
        bytes.extend(record(15, 10, &ptr));
        std::fs::write(&stdf_path, &bytes).expect("write stdf");

        {
            let file = File::create(&zip_path).expect("create zip");
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("fixture.std", options)
                .expect("start zip entry");
            std::io::Write::write_all(&mut zip, &bytes).expect("write zip entry");
            zip.finish().expect("finish zip");
        }

        let plain = parse_fixture_session(&stdf_path);
        let zipped = parse_fixture_session(&zip_path);

        assert_eq!(plain.groups, zipped.groups);
        assert_eq!(plain.fields, zipped.fields);
        assert_eq!(plain.search_total, zipped.search_total);
        assert_eq!(
            existing_decompressed,
            decompressed_temp_files(),
            "zip parsing should not create a decompressed stdf temp file"
        );
    }

    #[test]
    fn session_opens_every_supported_archive_container() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stdf_path = dir.path().join("fixture.std");

        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));
        let mut ptr = Vec::new();
        ptr.extend_from_slice(&202_u32.to_le_bytes());
        ptr.extend_from_slice(&[1, 1, 0, 0]);
        ptr.extend_from_slice(&1.8_f32.to_le_bytes());
        ptr.extend(cn("VDD_ARCHIVE"));
        ptr.extend(cn(""));
        bytes.extend(record(15, 10, &ptr));
        std::fs::write(&stdf_path, &bytes).expect("write stdf");
        let plain = parse_fixture_session(&stdf_path);

        let sevenz_path = dir.path().join("fixture.std.7z");
        {
            let mut writer =
                sevenz_rust2::ArchiveWriter::create(&sevenz_path).expect("create 7z");
            writer
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file("fixture.std"),
                    Some(bytes.as_slice()),
                )
                .expect("push 7z entry");
            writer.finish().expect("finish 7z");
        }

        let gz_path = dir.path().join("fixture.std.gz");
        {
            let file = File::create(&gz_path).expect("create gz");
            let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            std::io::Write::write_all(&mut encoder, &bytes).expect("write gz");
            encoder.finish().expect("finish gz");
        }

        let xz_path = dir.path().join("fixture.std.xz");
        {
            let file = File::create(&xz_path).expect("create xz");
            let mut encoder = liblzma::write::XzEncoder::new(file, 6);
            std::io::Write::write_all(&mut encoder, &bytes).expect("write xz");
            encoder.finish().expect("finish xz");
        }

        let bz2_path = dir.path().join("fixture.std.bz2");
        {
            let file = File::create(&bz2_path).expect("create bz2");
            let mut encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
            std::io::Write::write_all(&mut encoder, &bytes).expect("write bz2");
            encoder.finish().expect("finish bz2");
        }

        let zst_path = dir.path().join("fixture.std.zst");
        {
            let file = File::create(&zst_path).expect("create zst");
            let mut encoder = zstd::stream::write::Encoder::new(file, 0).expect("zst encoder");
            std::io::Write::write_all(&mut encoder, &bytes).expect("write zst");
            encoder.finish().expect("finish zst");
        }

        let tar_path = dir.path().join("fixture.tar");
        {
            let file = File::create(&tar_path).expect("create tar");
            let mut builder = tar::Builder::new(file);
            // bsdtar-style AppleDouble companion BEFORE the real file: its
            // name also ends in ".std", so the picker must skip it by the
            // "._" prefix, not by extension.
            let junk = b"AppleDouble resource fork bytes, not STDF";
            let mut junk_header = tar::Header::new_gnu();
            junk_header.set_size(junk.len() as u64);
            junk_header.set_mode(0o644);
            junk_header.set_cksum();
            builder
                .append_data(&mut junk_header, "._fixture.std", junk.as_slice())
                .expect("append junk entry");
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "fixture.std", bytes.as_slice())
                .expect("append tar entry");
            builder.finish().expect("finish tar");
        }

        // tar wrapped in gzip: the compressor magic sits outside, the ustar
        // magic only appears inside the decompressed stream.
        let targz_path = dir.path().join("fixture.tar.gz");
        {
            let file = File::create(&targz_path).expect("create tar.gz");
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "fixture.std", bytes.as_slice())
                .expect("append tar.gz entry");
            builder
                .into_inner()
                .expect("finish tar stream")
                .finish()
                .expect("finish gz stream");
        }

        for path in [
            &sevenz_path,
            &gz_path,
            &xz_path,
            &bz2_path,
            &zst_path,
            &tar_path,
            &targz_path,
        ] {
            let unpacked = parse_fixture_session(path);
            assert_eq!(plain.groups, unpacked.groups, "groups mismatch for {path:?}");
            assert_eq!(plain.fields, unpacked.fields, "fields mismatch for {path:?}");
        }
    }

    #[test]
    #[ignore = "requires STDF_RAR_SAMPLE_PATH (rar has no open-source encoder to build a fixture with)"]
    fn session_opens_rar_sample_from_env() {
        let Ok(path) = std::env::var("STDF_RAR_SAMPLE_PATH") else {
            eprintln!("skipped: set STDF_RAR_SAMPLE_PATH to a .rar holding a .stdf/.std");
            return;
        };
        let result = parse_fixture_session(Path::new(&path));
        assert!(
            result.groups.iter().any(|group| group.count > 0),
            "rar sample parsed no records"
        );
    }

    #[test]
    fn parses_dtr_text_and_saves_txt_for_plain_and_zip_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stdf_path = dir.path().join("dtr-fixture.std");
        let zip_path = dir.path().join("dtr-fixture.std.zip");

        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));
        bytes.extend(record(50, 30, &cn("first line")));
        // A PTR between the DTRs so the scan has non-DTR payloads to skip over.
        let mut ptr = Vec::new();
        ptr.extend_from_slice(&202_u32.to_le_bytes());
        ptr.extend_from_slice(&[1, 1, 0, 0]);
        ptr.extend_from_slice(&1.8_f32.to_le_bytes());
        ptr.extend(cn("VDD_STREAM"));
        ptr.extend(cn(""));
        bytes.extend(record(15, 10, &ptr));
        bytes.extend(record(50, 30, &cn("良率 100%")));
        bytes.extend(record(50, 30, &cn("")));
        bytes.extend(record(50, 30, &[2, 0x41, 0xFF]));
        // Length byte claims 10 chars but only 2 follow — clamp like cn() does.
        bytes.extend(record(50, 30, &[10, b'h', b'i']));
        std::fs::write(&stdf_path, &bytes).expect("write stdf");

        {
            let file = File::create(&zip_path).expect("create zip");
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("dtr-fixture.std", options)
                .expect("start zip entry");
            std::io::Write::write_all(&mut zip, &bytes).expect("write zip entry");
            zip.finish().expect("finish zip");
        }

        // One text line per DTR in file order; empty text stays an empty line,
        // and the invalid 0xFF byte decodes lossily instead of failing.
        let expected = "first line\n良率 100%\n\nA\u{FFFD}\nhi\n";

        for (label, path) in [("plain", &stdf_path), ("zip", &zip_path)] {
            let manager = SessionManager::default();
            let (tx, rx) = mpsc::channel();
            let session = manager
                .open_stdf(path.to_string_lossy().to_string(), move |event| {
                    let _ = tx.send(event);
                })
                .expect("open session");
            let mut completed = false;
            for _ in 0..100 {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(SessionEvent::Complete(_)) => {
                        completed = true;
                        break;
                    }
                    Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
            assert!(completed, "{label}: parser should complete");

            let result = manager
                .parse_dtr_text(&session.session_id)
                .expect("parse dtr text");
            assert_eq!(result.session_id, session.session_id, "{label}");
            assert_eq!(result.count, 5, "{label}: DTR record count");

            let dest = dir.path().join(format!("dtr-out-{label}.txt"));
            manager
                .save_dtr_text(&session.session_id, &dest.to_string_lossy())
                .expect("save dtr text");
            assert_eq!(
                std::fs::read_to_string(&dest).expect("read saved txt"),
                expected,
                "{label}: saved txt content"
            );
        }
    }

    #[test]
    fn save_dtr_text_requires_a_successful_parse_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stdf_path = dir.path().join("no-dtr.std");
        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));
        std::fs::write(&stdf_path, &bytes).expect("write stdf");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(stdf_path.to_string_lossy().to_string(), move |event| {
                let _ = tx.send(event);
            })
            .expect("open session");
        for _ in 0..100 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => break,
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }

        let dest = dir.path().join("never-parsed.txt");
        let error = manager
            .save_dtr_text(&session.session_id, &dest.to_string_lossy())
            .expect_err("save before parse should fail");
        assert!(error.contains("解析"), "error should mention parsing: {error}");
        assert!(!dest.exists(), "no txt should be written before a parse");

        // A file without any DTR records still parses, with a zero count.
        let result = manager
            .parse_dtr_text(&session.session_id)
            .expect("parse dtr text");
        assert_eq!(result.count, 0);
    }

    #[test]
    fn parsed_field_json_shape_stays_string_based() {
        let mut bytes = Vec::new();
        bytes.extend(record(0, 10, &[2, 4]));

        let mut parsed = Vec::new();
        parse_reader(
            &mut Cursor::new(bytes),
            6,
            |parsed_record| {
                parsed.push(parsed_record);
                true
            },
            |_, _| {},
        )
        .expect("fixture should parse");

        let json = serde_json::to_value(&parsed[0].fields[0]).expect("field json");
        assert_eq!(json["name"], "CPU_TYPE");
        assert_eq!(json["field_type"], "U*1");
        assert_eq!(json["description"], "CPU 类型");

        let decoded: RecordField = serde_json::from_value(json).expect("decode field");
        assert_eq!(decoded.name, "CPU_TYPE");
        assert_eq!(decoded.field_type, "U*1");
        assert_eq!(decoded.description, "CPU 类型");

        let compact = encode_fields_blob(&parsed[0]);
        assert!(
            !compact.starts_with(b"["),
            "fields should be stored as compressed blob"
        );
        let restored = decode_fields_blob(&compact).expect("decode compact fields");
        // Values-only storage: static metadata is rebuilt from the spec table;
        // per-field byte offsets are not persisted (nothing displays them).
        assert_eq!(restored.len(), parsed[0].fields.len());
        for (restored, original) in restored.iter().zip(&parsed[0].fields) {
            assert_eq!(restored.name, original.name);
            assert_eq!(restored.field_type, original.field_type);
            assert_eq!(restored.value, original.value);
            assert_eq!(restored.description, original.description);
        }
    }

    #[test]
    fn parser_uses_stdf_v4_record_type_names() {
        let mut bytes = Vec::new();
        bytes.extend(record(15, 10, &[0; 12]));
        bytes.extend(record(15, 20, &[0; 12]));
        bytes.extend(record(50, 30, &cn("comment")));
        bytes.extend(record(10, 30, &[0; 8]));
        bytes.extend(record(20, 10, &[]));
        bytes.extend(record(20, 20, &[]));

        let mut parsed = Vec::new();
        parse_reader(
            &mut Cursor::new(bytes.clone()),
            bytes.len() as u64,
            |parsed_record| {
                parsed.push(parsed_record.record_type);
                true
            },
            |_, _| {},
        )
        .expect("fixture should parse");

        assert_eq!(parsed, vec!["PTR", "FTR", "DTR", "TSR", "BPS", "EPS"]);
    }

    #[test]
    fn session_manager_parses_large_files_with_bounded_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("large.stdf");
        let mut bytes = Vec::new();
        for _ in 0..2_500 {
            bytes.extend(record(0, 10, &[2, 4]));
        }
        std::fs::write(&file_path, bytes).expect("write fixture");

        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(file_path.to_string_lossy().to_string(), move |event| {
                tx.send(event).expect("send event");
            })
            .expect("open session");

        let mut completed = false;
        let mut progress_events = 0;
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Progress(_)) => {
                    progress_events += 1;
                }
                Ok(SessionEvent::Snapshot(_)) => {}
                Ok(SessionEvent::Warning(_)) => {}
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Err(_) => {}
            }
        }

        assert!(completed, "parser should complete");
        assert_eq!(
            manager
                .get_record_groups(&session.session_id)
                .expect("groups")[0]
                .count,
            2_500
        );
        assert!(
            progress_events <= 8,
            "expected throttled progress, got {progress_events}"
        );
    }

    #[test]
    fn resolve_input_reports_incomplete_zip64_local_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("broken.std.zip");
        let entry_name = b"broken.std";

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x03\x04");
        bytes.extend_from_slice(&45_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&8_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&(entry_name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&20_u16.to_le_bytes());
        bytes.extend_from_slice(entry_name);
        bytes.extend_from_slice(&0x0001_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(&2_000_u64.to_le_bytes());
        bytes.extend_from_slice(&1_000_u64.to_le_bytes());
        bytes.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        std::fs::write(&file_path, bytes).expect("write broken zip");

        let error = match with_input_reader(&file_path, 0, |_, _| ()) {
            Ok(_) => panic!("zip should be rejected as incomplete"),
            Err(error) => error,
        };

        assert!(error.contains("zip 压缩包不完整"), "{error}");
        assert!(error.contains("重新下载"), "{error}");
    }

    #[test]
    #[ignore = "local benchmark for large session parse throughput"]
    fn session_manager_opens_real_customer_sample_quickly() {
        let path = match std::env::var("STDF_SAMPLE_PATH") {
            Ok(p) => PathBuf::from(p),
            Err(_) => {
                eprintln!("skipped: set STDF_SAMPLE_PATH to run this benchmark");
                return;
            }
        };
        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let started = std::time::Instant::now();
        let session = manager
            .open_stdf(path.to_string_lossy().to_string(), move |event| {
                tx.send(event).expect("send event");
            })
            .expect("open session");

        let mut completed = false;
        for _ in 0..2_000 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");
        let elapsed = started.elapsed();
        let ptr_count = manager
            .get_record_groups(&session.session_id)
            .expect("groups")
            .into_iter()
            .find(|group| group.record_type == "PTR")
            .map(|group| group.count);

        assert_eq!(ptr_count, Some(158_009));
        assert!(
            elapsed < Duration::from_secs(12),
            "expected session parse under 12s, got {elapsed:?}"
        );
    }

    #[test]
    #[ignore = "local benchmark for large real sample test item view"]
    fn test_item_view_real_customer_sample_is_bounded() {
        let path = match std::env::var("STDF_SAMPLE_PATH") {
            Ok(p) => PathBuf::from(p),
            Err(_) => {
                eprintln!("skipped: set STDF_SAMPLE_PATH to run this benchmark");
                return;
            }
        };
        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let session = manager
            .open_stdf(path.to_string_lossy().to_string(), move |event| {
                tx.send(event).expect("send event");
            })
            .expect("open session");

        let mut completed = false;
        for _ in 0..2_000 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(completed, "parser should complete");

        let started = std::time::Instant::now();
        let page = manager
            .get_test_item_page(&session.session_id, 0, 500, 0, 200, &[], "")
            .expect("test item page");
        let elapsed = started.elapsed();
        eprintln!(
            "test item page: {:?}, columns={}/{}, rows={}/{}, pmr={}",
            elapsed,
            page.columns.len(),
            page.total_columns,
            page.rows.len(),
            page.total_rows,
            page.pmr_count
        );
        assert!(elapsed < Duration::from_secs(3), "test-item page took {elapsed:?}");
        assert!(
            page.columns.len() <= 500,
            "a page should not force the UI to render {} columns at once",
            page.columns.len()
        );
    }

    #[test]
    #[ignore = "local benchmark for configured large sample parse completion"]
    fn session_manager_opens_configured_sample_to_completion() {
        let path = std::env::var("STDF_SAMPLE_PATH")
            .map(PathBuf::from)
            .expect("set STDF_SAMPLE_PATH to a local .std/.stdf/.zip sample");
        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let started = std::time::Instant::now();
        let session = manager
            .open_stdf(path.to_string_lossy().to_string(), move |event| {
                tx.send(event).expect("send event");
            })
            .expect("open session");

        let mut completed = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(20 * 60);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SessionEvent::Complete(_)) => {
                    completed = true;
                    break;
                }
                Ok(SessionEvent::Error(error)) => panic!("unexpected parse error: {error:?}"),
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("event channel closed before completion: {error}"),
            }
        }
        assert!(completed, "parser should complete within the benchmark window");
        let elapsed = started.elapsed();
        let total_records: usize = manager
            .get_record_groups(&session.session_id)
            .expect("groups")
            .into_iter()
            .map(|group| group.count)
            .sum();
        eprintln!(
            "completed {:?} in {:?}; total records: {}",
            path.file_name().unwrap_or_default(),
            elapsed,
            total_records
        );
        assert!(total_records > 0);
    }
}
