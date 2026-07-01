import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  ParseErrorEvent,
  ParseProgress,
  ParseSession,
  RecordBatchEvent,
  RecordField,
  RecordGroup,
  RecordSummaryPage,
  SearchProgress,
  SearchResultPage,
  SessionSnapshot,
  TestItemViewSnapshot,
  TestItemPage,
  TestItemColumnLite,
  StdfApi
} from "./types";

export const tauriApi: StdfApi = {
  async openFile() {
    const selected = await open({
      multiple: false,
      filters: [{ name: "STDF", extensions: ["stdf", "std", "gz", "zip"] }]
    });
    if (typeof selected !== "string") {
      return null;
    }
    return this.openDroppedFile(selected);
  },

  openDroppedFile(path: string) {
    return invoke<ParseSession>("open_stdf", { path });
  },

  cancelParse(sessionId: string) {
    return invoke<void>("cancel_parse", { sessionId });
  },

  getSessionSnapshot(sessionId: string) {
    return invoke<SessionSnapshot>("get_session_snapshot", { sessionId });
  },

  getTestItemView(sessionId: string) {
    return invoke<TestItemViewSnapshot>("get_test_item_view", { sessionId });
  },

  getTestItemPage(
    sessionId: string,
    rowOffset: number,
    rowCount: number,
    colOffset: number,
    colCount: number,
    selected: string[],
    siteFilter: string
  ) {
    return invoke<TestItemPage>("get_test_item_page", {
      sessionId,
      rowOffset,
      rowCount,
      colOffset,
      colCount,
      selected,
      siteFilter
    });
  },

  getTestItemColumns(sessionId: string) {
    return invoke<TestItemColumnLite[]>("get_test_item_columns", { sessionId });
  },

  saveCsvDialog(defaultName: string) {
    return save({
      defaultPath: defaultName,
      filters: [{ name: "CSV", extensions: ["csv"] }]
    });
  },

  exportTestItemCsv(sessionId: string, path: string) {
    return invoke<void>("export_test_item_csv", { sessionId, path });
  },

  getRecordGroups(sessionId: string) {
    return invoke<RecordGroup[]>("get_record_groups", { sessionId });
  },

  getRecords(sessionId: string, group: string, page: number, pageSize: number) {
    return invoke<RecordSummaryPage>("get_records", {
      sessionId,
      group,
      page,
      pageSize
    });
  },

  getRecordFields(sessionId: string, recordId: string) {
    return invoke<RecordField[]>("get_record_fields", { sessionId, recordId });
  },

  searchFields(
    sessionId: string,
    query: string,
    page: number,
    pageSize: number,
    onProgress?: (progress: SearchProgress) => void
  ) {
    // Tauri v2 Channel: progress is delivered on THIS invoke's dedicated
    // callback pipe, so there is no listen-side race with the invoke fire.
    const channel = new Channel<SearchProgress>();
    if (onProgress) channel.onmessage = onProgress;
    return invoke<SearchResultPage>("search_fields", {
      sessionId,
      query,
      page,
      pageSize,
      onProgress: channel
    });
  },

  async onProgress(handler: (progress: ParseProgress) => void) {
    if (!isTauriRuntime()) {
      return () => undefined;
    }
    return listen<ParseProgress>("parse-progress", (event) => handler(event.payload));
  },

  async onRecordBatch(handler: (event: RecordBatchEvent) => void) {
    if (!isTauriRuntime()) {
      return () => undefined;
    }
    return listen<RecordBatchEvent>("record-batch", (event) => handler(event.payload));
  },

  async onSessionSnapshot(handler: (snapshot: SessionSnapshot) => void) {
    if (!isTauriRuntime()) {
      return () => undefined;
    }
    return listen<SessionSnapshot>("session-snapshot", (event) => handler(event.payload));
  },

  async onNativeFileDrop(handler: (path: string) => void) {
    if (!isTauriRuntime()) {
      return () => undefined;
    }
    return getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop" && event.payload.paths[0]) {
        handler(event.payload.paths[0]);
      }
    });
  },

  async onParseComplete(handler: (sessionId: string) => void) {
    if (!isTauriRuntime()) {
      return () => undefined;
    }
    return listen<string>("parse-complete", (event) => handler(event.payload));
  },

  async onParseError(handler: (event: ParseErrorEvent) => void) {
    if (!isTauriRuntime()) {
      return () => undefined;
    }
    return listen<ParseErrorEvent>("parse-error", (event) => handler(event.payload));
  },

  async onParseWarning(handler: (event: ParseErrorEvent) => void) {
    if (!isTauriRuntime()) {
      return () => undefined;
    }
    return listen<ParseErrorEvent>("parse-warning", (event) => handler(event.payload));
  }
};

function isTauriRuntime() {
  const internals = (window as Window & { __TAURI_INTERNALS__?: { transformCallback?: unknown } })
    .__TAURI_INTERNALS__;
  return typeof internals?.transformCallback === "function";
}
