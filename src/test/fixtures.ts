import type {
  MprPinDetails,
  ParseErrorEvent,
  ParseProgress,
  ParseSession,
  RecordField,
  RecordGroup,
  RecordSummaryPage,
  SearchResultPage,
  SessionSnapshot,
  TestItemPage,
  StdfApi
} from "../types";

export type MockApi = StdfApi & {
  emitProgress(event: ParseProgress): void;
  emitSnapshot(event: SessionSnapshot): void;
  emitComplete(sessionId: string): void;
  emitError(event: ParseErrorEvent): void;
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
    { record_type: "PTR", count: 2 },
    { record_type: "DTR", count: 3 }
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
        status: "parsed"
      }
    ]
  };

  // DTR rows exist in the record list but expose no expanded fields — the
  // panel offers the on-demand parse & txt download instead.
  const dtrRecords: RecordSummaryPage = {
    page: 0,
    page_size: 50,
    total: 3,
    records: [5, 6, 7].map((idx, position) => ({
      id: `session-1:${idx}`,
      record_type: "DTR",
      index: idx,
      offset: 300 + position * 20,
      length: 16,
      status: "parsed" as const
    }))
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
    status: "running",
    issues: []
  };

  const testItemPage: TestItemPage = {
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
      },
      {
        record_type: "MPR",
        test_num: 300,
        test_name: "Continuity:Continuity[1]",
        low_limit: "200",
        high_limit: "800",
        unit: "mV",
        pmr_indices: ["101", "102", "103"]
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
          { value: "1.05", status: "P" },
          { value: "0b00000000", status: "P" },
          {
            value: "300, 320, 340, 360, 380, 400, 420, 440, 460, 480, 500, 520, 540, 560, 580, 600, ...",
            status: "F",
            record_position: 0
          }
        ]
      }
    ],
    total_columns: 3,
    total_rows: 1,
    row_offset: 0,
    col_offset: 0,
    pmr_count: 1,
    has_bin_pf: true,
    status: "complete"
  };

  const listeners = {
    progress: [] as Array<(event: { session_id: string; bytes_read: number; total_bytes: number }) => void>,
    snapshot: [] as Array<(snapshot: SessionSnapshot) => void>,
    complete: [] as Array<(sessionId: string) => void>,
    error: [] as Array<(event: ParseErrorEvent) => void>
  };

  const api: MockApi = {
    openFile: async () => sessions[Math.min(openCount++, sessions.length - 1)],
    openDroppedFile: async () => sessions[0],
    cancelParse: async () => undefined,
    getSessionSnapshot: async () => snapshot,
    getBinSummary: async () => ({
      session_id: "session-1",
      total_parts: 100,
      sbin_pass: 97,
      hbin_pass: 96,
      sbins: [
        { num: "1", name: "PASS", pf: "P", count: 97 },
        { num: "7", name: "FAIL_BIN", pf: "F", count: 3 }
      ],
      hbins: [
        { num: "1", name: "GOOD", pf: "P", count: 96 },
        { num: "2", name: "BAD", pf: "F", count: 4 }
      ],
      has_bin_pf: true,
      status: "complete" as const
    }),
    getTestItemPage: async () => testItemPage,
    // 40 pins so the dialog exercises the ">16 preview" case in dev/mock mode.
    getMprPinDetails: async (): Promise<MprPinDetails> => ({
      session_id: "session-1",
      test_num: 300,
      test_name: "Continuity:Continuity[1]",
      unit: "mV",
      low_limit: "200",
      high_limit: "800",
      pins: Array.from({ length: 40 }, (_, index) => {
        const value = 300 + index * 20;
        return {
          pmr_index: String(101 + index),
          pin_name: `PIN${101 + index}`,
          value: String(value),
          status: value > 800 ? "F" : "P"
        };
      })
    }),
    getTestItemColumns: async () =>
      testItemPage.columns.map((column) => ({
        key: `${column.record_type}:${column.test_num}`,
        record_type: column.record_type,
        test_num: column.test_num,
        test_name: column.test_name
      })),
    saveCsvDialog: async () => "/tmp/export.csv",
    exportTestItemCsv: async () => undefined,
    saveXlsxDialog: async () => "/tmp/pins.xlsx",
    exportMprPinsXlsx: async () => undefined,
    parseDtrText: async () => ({ session_id: "session-1", count: 3 }),
    saveTxtDialog: async () => "/tmp/export.txt",
    saveDtrText: async () => undefined,
    getRecordGroups: async () => groups,
    getRecords: async (_sessionId, group) =>
      group === "MIR" ? mirRecords : group === "DTR" ? dtrRecords : records,
    getRecordFields: async (_sessionId, recordId) => {
      if (recordId.endsWith(":1")) return mirFields;
      if (dtrRecords.records.some((record) => record.id === recordId)) return [];
      return fields;
    },
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
    onParseError: async (handler) => {
      listeners.error.push(handler);
      return () => {
        listeners.error = listeners.error.filter((item) => item !== handler);
      };
    },
    onParseWarning: async () => () => undefined,
    emitProgress: (event) => listeners.progress.forEach((handler) => handler(event)),
    emitSnapshot: (event) => listeners.snapshot.forEach((handler) => handler(event)),
    emitComplete: (sessionId) => listeners.complete.forEach((handler) => handler(sessionId)),
    emitError: (event) => listeners.error.forEach((handler) => handler(event))
  };
  return { ...api, ...overrides };
}
