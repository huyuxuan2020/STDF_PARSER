import type {
  ParseProgress,
  ParseSession,
  RecordField,
  RecordGroup,
  RecordSummaryPage,
  SearchResultPage,
  SessionSnapshot,
  TestItemViewSnapshot,
  TestItemPage,
  StdfApi
} from "../types";

export type MockApi = StdfApi & {
  emitProgress(event: ParseProgress): void;
  emitSnapshot(event: SessionSnapshot): void;
  emitComplete(sessionId: string): void;
};

export function createMockApi(overrides: Partial<StdfApi> = {}): MockApi {
  const sessions: ParseSession[] = [
    {
    session_id: "session-1",
      file_name: "demo-1.stdf",
      file_path: "/samples/demo-1.stdf",
      file_dir: "/samples",
      modified_time: "2025-11-20 11:25:24",
    file_size: 1024,
    status: "running"
    },
    {
      session_id: "session-2",
      file_name: "demo-2.stdf",
      file_path: "/samples/demo-2.stdf",
      file_dir: "/samples",
      modified_time: "2025-11-21 08:30:00",
      file_size: 2048,
      status: "running"
    }
  ];
  let openCount = 0;

  const groups: RecordGroup[] = [
    { record_type: "FAR", count: 1 },
    { record_type: "MIR", count: 1 },
    { record_type: "PTR", count: 2 }
  ];

  const records: RecordSummaryPage = {
    page: 0,
    page_size: 50,
    total: 1,
    records: [
      {
        id: "session-1:0",
        record_type: "FAR",
        index: 0,
        offset: 0,
        length: 6,
        summary: "CPU_TYPE=2, STDF_VER=4",
        status: "parsed"
      }
    ]
  };

  const mirRecords: RecordSummaryPage = {
    page: 0,
    page_size: 50,
    total: 1,
    records: [
      {
        id: "session-1:1",
        record_type: "MIR",
        index: 1,
        offset: 6,
        length: 120,
        summary: "LOT_ID=V29F7, NODE_NAM=T1058",
        status: "parsed"
      }
    ]
  };

  const fields: RecordField[] = [
    {
      name: "CPU_TYPE",
      field_type: "U1",
      value: "2",
      description: "CPU 类型",
      offset: 4,
      length: 1
    },
    {
      name: "STDF_VER",
      field_type: "U1",
      value: "4",
      description: "STDF 版本",
      offset: 5,
      length: 1
    }
  ];

  const mirFields: RecordField[] = [
    {
      name: "LOT_ID",
      field_type: "C*n",
      value: "V29F7",
      description: "Lot ID",
      offset: 16,
      length: 5
    },
    {
      name: "NODE_NAM",
      field_type: "C*n",
      value: "T1058",
      description: "Node name",
      offset: 24,
      length: 6
    },
    {
      name: "EXEC_VER",
      field_type: "C*n",
      value: "1.2.0",
      description: "Exec version",
      offset: 36,
      length: 5
    }
  ];

  const snapshot: SessionSnapshot = {
    session_id: "session-1",
    groups,
    key_fields: { MIR: mirFields },
    first_records: {
      FAR: { record: records.records[0], fields },
      MIR: { record: mirRecords.records[0], fields: mirFields }
    },
    bytes_read: 128,
    total_bytes: 1024,
    status: "running"
  };

  const testItemView: TestItemViewSnapshot = {
    session_id: "session-1",
    columns: [
      {
        record_type: "PTR",
        test_num: 100,
        test_name: "VDD_CORE",
        low_limit: "1.0",
        high_limit: "1.2",
        unit: "V",
        pmr_indices: ["1"]
      },
      {
        record_type: "FTR",
        test_num: 220,
        test_name: "SCAN_OK",
        low_limit: "",
        high_limit: "",
        unit: "",
        pmr_indices: []
      }
    ],
    rows: [
      {
        part_id: "PART-1",
        site_num: "1",
        site_nums: ["1"],
        head_num: "1",
        sbin_num: "2",
        sbin_name: "PASS",
        sbin_pf: "P",
        hbin_num: "3",
        hbin_name: "GOOD",
        hbin_pf: "P",
        test_t: "50",
        part_txt: "demo part",
        results: [
          { test_num: 100, value: "1.05", status: "P" },
          { test_num: 220, value: "0b00000000", status: "P" }
        ]
      }
    ],
    total_columns: 2,
    total_rows: 1,
    pmr_lookup: {
      "1": { phy_nam: "PIN1", log_nam: "L1", head_num: "1", site_num: "1" }
    },
    status: "complete"
  };

  const testItemPage: TestItemPage = {
    session_id: testItemView.session_id,
    columns: testItemView.columns,
    rows: testItemView.rows,
    total_columns: testItemView.total_columns,
    total_rows: testItemView.total_rows,
    row_offset: 0,
    col_offset: 0,
    pmr_lookup: testItemView.pmr_lookup,
    has_bin_pf: true,
    status: testItemView.status
  };

  const listeners = {
    progress: [] as Array<(event: { session_id: string; bytes_read: number; total_bytes: number }) => void>,
    snapshot: [] as Array<(snapshot: SessionSnapshot) => void>,
    complete: [] as Array<(sessionId: string) => void>
  };

  const api: MockApi = {
    openFile: async () => sessions[Math.min(openCount++, sessions.length - 1)],
    openDroppedFile: async () => sessions[0],
    cancelParse: async () => undefined,
    getSessionSnapshot: async () => snapshot,
    getTestItemView: async () => testItemView,
    getTestItemPage: async () => testItemPage,
    getTestItemColumns: async () =>
      testItemView.columns.map((column) => ({
        key: `${column.record_type}:${column.test_num}`,
        record_type: column.record_type,
        test_num: column.test_num,
        test_name: column.test_name
      })),
    saveCsvDialog: async () => "/tmp/export.csv",
    exportTestItemCsv: async () => undefined,
    getRecordGroups: async () => groups,
    getRecords: async (_sessionId, group) => (group === "MIR" ? mirRecords : records),
    getRecordFields: async (_sessionId, recordId) => (recordId.endsWith(":1") ? mirFields : fields),
    searchFields: async (): Promise<SearchResultPage> => ({
      page: 0,
      page_size: 50,
      total: 1,
      results: [{ record: records.records[0], field: fields[1] }]
    }),
    onProgress: async (handler) => {
      listeners.progress.push(handler);
      return () => {
        listeners.progress = listeners.progress.filter((item) => item !== handler);
      };
    },
    onRecordBatch: async () => () => undefined,
    onSessionSnapshot: async (handler) => {
      listeners.snapshot.push(handler);
      return () => {
        listeners.snapshot = listeners.snapshot.filter((item) => item !== handler);
      };
    },
    onNativeFileDrop: async () => () => undefined,
    onParseComplete: async (handler) => {
      listeners.complete.push(handler);
      return () => {
        listeners.complete = listeners.complete.filter((item) => item !== handler);
      };
    },
    onParseError: async () => () => undefined,
    onParseWarning: async () => () => undefined,
    emitProgress: (event) => listeners.progress.forEach((handler) => handler(event)),
    emitSnapshot: (event) => listeners.snapshot.forEach((handler) => handler(event)),
    emitComplete: (sessionId) => listeners.complete.forEach((handler) => handler(sessionId))
  };
  return { ...api, ...overrides };
}
