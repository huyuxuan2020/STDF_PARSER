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
  summary: string;
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

export interface SessionSnapshot {
  session_id: string;
  groups: RecordGroup[];
  key_fields: Record<string, RecordField[]>;
  first_records: Record<string, FirstRecordSnapshot>;
  bytes_read: number;
  total_bytes: number;
  status: ParseStatus;
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

export interface TestItemCell {
  test_num: number;
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

export interface TestItemPmrEntry {
  phy_nam: string;
  log_nam: string;
  head_num: string;
  site_num: string;
}

export interface TestItemViewSnapshot {
  session_id: string;
  columns: TestItemColumn[];
  rows: TestItemPartRow[];
  total_columns: number;
  total_rows: number;
  pmr_lookup: Record<string, TestItemPmrEntry>;
  status: ParseStatus;
}

export interface TestItemColumnLite {
  key: string;
  record_type: string;
  test_num: number;
  test_name: string;
}

export interface TestItemPage {
  session_id: string;
  columns: TestItemColumn[];
  rows: TestItemPartRow[];
  total_columns: number;
  total_rows: number;
  row_offset: number;
  col_offset: number;
  pmr_lookup: Record<string, TestItemPmrEntry>;
  has_bin_pf: boolean;
  status: ParseStatus;
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

export interface RecordBatchEvent {
  session_id: string;
  records: RecordSummary[];
}

export interface StdfApi {
  openFile(): Promise<ParseSession | null>;
  openDroppedFile(path: string): Promise<ParseSession>;
  cancelParse(sessionId: string): Promise<void>;
  getSessionSnapshot(sessionId: string): Promise<SessionSnapshot>;
  getTestItemView(sessionId: string): Promise<TestItemViewSnapshot>;
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
  saveCsvDialog(defaultName: string): Promise<string | null>;
  exportTestItemCsv(sessionId: string, path: string): Promise<void>;
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
  onRecordBatch(handler: (event: RecordBatchEvent) => void): Promise<() => void>;
  onSessionSnapshot(handler: (snapshot: SessionSnapshot) => void): Promise<() => void>;
  onNativeFileDrop(handler: (path: string) => void): Promise<() => void>;
  onParseComplete(handler: (sessionId: string) => void): Promise<() => void>;
  onParseError(handler: (event: ParseErrorEvent) => void): Promise<() => void>;
  onParseWarning(handler: (event: ParseErrorEvent) => void): Promise<() => void>;
}
