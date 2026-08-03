export type ParseStatus = "running" | "complete" | "cancelled" | "error";

export interface ParseSession {
  session_id: string;
  file_name: string;
  file_path: string;
  file_dir: string;
  modified_time: string;
  file_size: number;
  status: ParseStatus;
}

export interface RecordGroup {
  record_type: string;
  count: number;
}

export interface RecordSummary {
  id: string;
  record_type: string;
  index: number;
  offset: number;
  length: number;
  status: "parsed" | "unknown" | "error";
}

export interface RecordSummaryPage {
  records: RecordSummary[];
  total: number;
  page: number;
  page_size: number;
}

export interface RecordField {
  name: string;
  field_type: string;
  value: string;
  description: string;
  offset?: number;
  length?: number;
  // For PTR/MPR records whose optional-tail fields (RES_SCAL, UNITS, LO_LIMIT,
  // HI_LIMIT, ...) are omitted in this record but inherit from the first PTR/
  // MPR of the same TEST_NUM per STDF v4 §7.1. The FieldDetailPanel surfaces
  // it as a "继承自首条 PTR" hint next to the empty value.
  inherited_value?: string;
}

export interface FirstRecordSnapshot {
  record: RecordSummary;
  fields: RecordField[];
}

export type FileIssueSeverity = "error" | "warning";

export interface FileIssueLocation {
  offset: number;
  record_index: number | null;
  record_type: string;
  detail: string;
}

export interface FileIssue {
  code: string;
  severity: FileIssueSeverity;
  title: string;
  message: string;
  suggestion: string;
  count: number;
  affects_accuracy: boolean;
  samples: FileIssueLocation[];
}

export interface SessionSnapshot {
  session_id: string;
  groups: RecordGroup[];
  key_fields: Record<string, RecordField[]>;
  first_records: Record<string, FirstRecordSnapshot>;
  bytes_read: number;
  total_bytes: number;
  status: ParseStatus;
  issues: FileIssue[];
}

export interface TestItemColumn {
  record_type: string;
  test_num: number;
  test_name: string;
  low_limit: string;
  high_limit: string;
  unit: string;
  pmr_indices: string[];
}

// One matrix cell. Cells are positional — `TestItemPartRow.results[i]` belongs
// to `columns[i]` of the same page — so the cell carries no test identity.
export interface TestItemCell {
  value: string;
  status: string;
  // 0-based position of the source PTR/MPR/FTR record within its group,
  // used to jump straight to that record in the Records view.
  record_position?: number;
}

export interface TestItemPartRow {
  part_id: string;
  site_num: string;
  site_nums: string[];
  head_num: string;
  sbin_num: string;
  sbin_name: string;
  sbin_pf: string;
  hbin_num: string;
  hbin_name: string;
  hbin_pf: string;
  test_t: string;
  part_txt: string;
  results: TestItemCell[];
}

export interface TestItemColumnLite {
  key: string;
  record_type: string;
  test_num: number;
  test_name: string;
}

// One bin's share of the parsed parts, for the overview's yield section.
export interface BinStat {
  num: string;
  name: string;
  pf: string;
  count: number;
}

export interface BinSummary {
  session_id: string;
  total_parts: number;
  sbin_pass: number;
  hbin_pass: number;
  sbins: BinStat[];
  hbins: BinStat[];
  has_bin_pf: boolean;
  status: ParseStatus;
}

export interface TestItemPage {
  session_id: string;
  columns: TestItemColumn[];
  rows: TestItemPartRow[];
  total_columns: number;
  total_rows: number;
  row_offset: number;
  col_offset: number;
  // Size of the session's PMR lookup — the UI only shows a count pill, so the
  // map itself never crosses the IPC bridge.
  pmr_count: number;
  has_bin_pf: boolean;
  status: ParseStatus;
}

// Result of the on-demand DTR text extraction: the backend staged a txt with
// `count` lines (one per DTR record); saveDtrText copies it to a user path.
export interface DtrParseResult {
  session_id: string;
  count: number;
}

// One pin row of an expanded MPR cell. The grid cell only shows the parser's
// 16-element preview; the pin dialog fetches the complete array on demand.
export interface MprPinValue {
  pmr_index: string;
  pin_name: string;
  value: string;
  status: string;
}

export interface MprPinDetails {
  session_id: string;
  test_num: number;
  test_name: string;
  unit: string;
  low_limit: string;
  high_limit: string;
  pins: MprPinValue[];
}

export interface SearchResult {
  record: RecordSummary;
  field: RecordField;
}

export interface SearchResultPage {
  results: SearchResult[];
  total: number;
  page: number;
  page_size: number;
}

export interface ParseProgress {
  session_id: string;
  bytes_read: number;
  total_bytes: number;
}

export interface SearchProgress {
  session_id: string;
  scanned: number;
  total: number;
}

export interface ParseErrorEvent {
  session_id: string;
  message: string;
  offset?: number;
}

export interface StdfApi {
  openFile(): Promise<ParseSession | null>;
  openDroppedFile(path: string): Promise<ParseSession>;
  cancelParse(sessionId: string): Promise<void>;
  getSessionSnapshot(sessionId: string): Promise<SessionSnapshot>;
  getTestItemPage(
    sessionId: string,
    rowOffset: number,
    rowCount: number,
    colOffset: number,
    colCount: number,
    selected: string[],
    siteFilter: string
  ): Promise<TestItemPage>;
  getTestItemColumns(sessionId: string): Promise<TestItemColumnLite[]>;
  getBinSummary(sessionId: string): Promise<BinSummary>;
  saveCsvDialog(defaultName: string): Promise<string | null>;
  exportTestItemCsv(sessionId: string, path: string): Promise<void>;
  parseDtrText(sessionId: string): Promise<DtrParseResult>;
  getMprPinDetails(
    sessionId: string,
    testNum: number,
    recordPosition: number
  ): Promise<MprPinDetails>;
  saveXlsxDialog(defaultName: string): Promise<string | null>;
  exportMprPinsXlsx(
    sessionId: string,
    testNum: number,
    recordPosition: number,
    partId: string,
    siteNum: string,
    path: string
  ): Promise<void>;
  saveTxtDialog(defaultName: string): Promise<string | null>;
  saveDtrText(sessionId: string, path: string): Promise<void>;
  getRecordGroups(sessionId: string): Promise<RecordGroup[]>;
  getRecords(
    sessionId: string,
    group: string,
    page: number,
    pageSize: number
  ): Promise<RecordSummaryPage>;
  getRecordFields(sessionId: string, recordId: string): Promise<RecordField[]>;
  searchFields(
    sessionId: string,
    query: string,
    page: number,
    pageSize: number,
    onProgress?: (progress: SearchProgress) => void
  ): Promise<SearchResultPage>;
  onProgress(handler: (progress: ParseProgress) => void): Promise<() => void>;
  onSessionSnapshot(handler: (snapshot: SessionSnapshot) => void): Promise<() => void>;
  onNativeFileDrop(handler: (path: string) => void): Promise<() => void>;
  onParseComplete(handler: (sessionId: string) => void): Promise<() => void>;
  onParseError(handler: (event: ParseErrorEvent) => void): Promise<() => void>;
  onParseWarning(handler: (event: ParseErrorEvent) => void): Promise<() => void>;
}
