import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "./App";
import { createMockApi } from "./test/fixtures";

describe("App", () => {
  it("shows the file summary page after opening a STDF file", async () => {
    const api = createMockApi();
    const getRecordFields = vi.spyOn(api, "getRecordFields");
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));

    expect(await screen.findByRole("main", { name: "文件摘要" })).toBeInTheDocument();
    expect(screen.getAllByText("demo-1.stdf").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "记录" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "打开另一个文件" })).toBeInTheDocument();
    expect(screen.getByText("LOT_ID")).toBeInTheDocument();
    expect(screen.getByText("V29F7")).toBeInTheDocument();
    await waitFor(() => expect(api.getSessionSnapshot).toBeDefined());
    expect(getRecordFields).not.toHaveBeenCalled();
  });

  it("switches to the record explorer and can open another file", async () => {
    const api = createMockApi();
    const openFile = vi.spyOn(api, "openFile");
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));

    expect(await screen.findByRole("main", { name: "文件摘要" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "记录" }));
    expect(await screen.findByRole("main", { name: "STDF 工作台" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "FAR 1 条记录" })).toBeInTheDocument();
    expect(await screen.findByText("CPU 类型")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "打开另一个文件" }));
    expect(openFile).toHaveBeenCalledTimes(2);
    expect((await screen.findAllByText("demo-2.stdf")).length).toBeGreaterThan(0);
  });

  it("shows yield and bin statistics on the overview once parsing completes", async () => {
    const api = createMockApi();
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });

    act(() => {
      api.emitComplete("session-1");
    });

    // Yield summary: total / pass / yield percentage.
    expect(await screen.findByText("良率与 Bin 统计")).toBeInTheDocument();
    expect(screen.getByText("100")).toBeInTheDocument(); // total parts
    expect(screen.getAllByText("97.0%").length).toBeGreaterThan(0); // sbin-based yield
    // Both bin tables render with names and counts.
    expect(screen.getByText("FAIL_BIN")).toBeInTheDocument();
    expect(screen.getByText("GOOD")).toBeInTheDocument();
  });

  it("opens the test-item matrix view after parsing completes", async () => {
    const api = createMockApi();
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });

    act(() => {
      api.emitComplete("session-1");
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "测试项" })).not.toBeDisabled());
    await user.click(screen.getByRole("button", { name: "测试项" }));
    expect(await screen.findByRole("main", { name: "STDF 工作台" })).toBeInTheDocument();
    expect(await screen.findByText("PART-1")).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "测试项矩阵" })).toBeInTheDocument();
    expect(screen.getByText("VDD_CORE")).toBeInTheDocument();
    expect(screen.getByText("demo part")).toBeInTheDocument();
    expect(screen.getByText("SCAN_OK")).toBeInTheDocument();
  });

  it("virtualizes the test-item matrix instead of mounting every cell", async () => {
    const ROWS = 200;
    const COLS = 60;
    const columns = Array.from({ length: COLS }, (_, index) => ({
      record_type: "PTR",
      test_num: 1000 + index,
      test_name: `T_${index}`,
      low_limit: "0",
      high_limit: "1",
      unit: "V",
      pmr_indices: []
    }));
    const rows = Array.from({ length: ROWS }, (_, r) => ({
      part_id: `P-${r}`,
      site_num: "1",
      site_nums: ["1"],
      head_num: "1",
      sbin_num: "1",
      sbin_name: "PASS",
      sbin_pf: "P",
      hbin_num: "1",
      hbin_name: "GOOD",
      hbin_pf: "P",
      test_t: "10",
      part_txt: "",
      results: columns.map((_, c) => ({ value: `${r}.${c}`, status: "P" }))
    }));
    const api = createMockApi({
      getTestItemPage: async () => ({
        session_id: "session-1",
        columns,
        rows,
        total_columns: COLS,
        total_rows: ROWS,
        row_offset: 0,
        col_offset: 0,
        pmr_count: 0,
        has_bin_pf: true,
        status: "complete"
      })
    });
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });
    act(() => {
      api.emitComplete("session-1");
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "测试项" })).not.toBeDisabled());
    await user.click(screen.getByRole("button", { name: "测试项" }));

    expect(await screen.findByText("P-0")).toBeInTheDocument();
    // Only the scroll window (plus overscan) may be mounted — a full mount
    // would be 200 × (10 + 60) = 14,000 cells and is what makes big files lag.
    const table = screen.getByRole("table", { name: "测试项矩阵" });
    expect(table.querySelectorAll("td").length).toBeLessThan(3000);
    // The row counter still reports the full loaded set.
    expect(screen.getByText(/已加载 200/)).toBeInTheDocument();
  });

  it("serves the test-item page from cache when re-entering the tab", async () => {
    const api = createMockApi();
    const getTestItemPage = vi.spyOn(api, "getTestItemPage");
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });
    act(() => {
      api.emitComplete("session-1");
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "测试项" })).not.toBeDisabled());

    await user.click(screen.getByRole("button", { name: "测试项" }));
    expect(await screen.findByText("PART-1")).toBeInTheDocument();
    expect(getTestItemPage).toHaveBeenCalledTimes(1);

    // Leave and come back: the already-loaded window must not be re-fetched
    // (an 8MB IPC page + full table remount on every tab switch otherwise).
    await user.click(screen.getByRole("button", { name: "概览" }));
    await screen.findByRole("main", { name: "文件摘要" });
    await user.click(screen.getByRole("button", { name: "测试项" }));
    expect(await screen.findByText("PART-1")).toBeInTheDocument();
    expect(getTestItemPage).toHaveBeenCalledTimes(1);
  });

  it("loads the full column list only when the filter dialog opens", async () => {
    const api = createMockApi();
    const getTestItemColumns = vi.spyOn(api, "getTestItemColumns");
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });
    act(() => {
      api.emitComplete("session-1");
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "测试项" })).not.toBeDisabled());
    await user.click(screen.getByRole("button", { name: "测试项" }));
    await screen.findByText("PART-1");

    // The identity list can be several MB on big files — fetch it for the
    // dialog, not on tab entry.
    expect(getTestItemColumns).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /筛选测试项/ }));
    expect(await screen.findByRole("dialog", { name: "筛选测试项" })).toBeInTheDocument();
    await waitFor(() => expect(getTestItemColumns).toHaveBeenCalledTimes(1));
  });

  it("opens a column detail card with the full test name and copies it", async () => {
    const api = createMockApi();
    const user = userEvent.setup();
    // After setup(): user-event installs its own clipboard stub — override it
    // so we can observe what the component copies.
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true
    });

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });
    act(() => {
      api.emitComplete("session-1");
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "测试项" })).not.toBeDisabled());
    await user.click(screen.getByRole("button", { name: "测试项" }));
    await screen.findByText("PART-1");

    // Clicking a column's header opens a detail card where the full (possibly
    // truncated in the grid) name is visible, selectable and copyable.
    await user.click(screen.getByText("VDD_CORE"));
    const card = await screen.findByRole("dialog", { name: "测试项详情" });
    expect(within(card).getByText("VDD_CORE")).toBeInTheDocument();
    expect(within(card).getByText("100")).toBeInTheDocument(); // test num
    expect(within(card).getByText("1.0")).toBeInTheDocument(); // low limit

    await user.click(within(card).getByRole("button", { name: "复制名称" }));
    expect(writeText).toHaveBeenCalledWith("VDD_CORE");
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "测试项详情" })).not.toBeInTheDocument()
    );
  });

  it("keeps the test-item nav gated until parsing completes", async () => {
    const api = createMockApi({
      getSessionSnapshot: async () => ({
        session_id: "session-1",
        groups: [],
        key_fields: {},
        first_records: {},
        bytes_read: 0,
        total_bytes: 1024,
        status: "running"
      }),
      getTestItemPage: async () => {
        throw new Error("should not be called before complete");
      }
    });
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });

    expect(screen.getByRole("button", { name: "测试项" })).toBeDisabled();
  });

  it("does not let stale snapshot reset parse progress", async () => {
    const api = createMockApi();
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });

    act(() => {
      api.emitProgress({ session_id: "session-1", bytes_read: 600, total_bytes: 1000 });
      api.emitSnapshot({
        session_id: "session-1",
        groups: [],
        key_fields: {},
        first_records: {},
        bytes_read: 0,
        total_bytes: 1000,
        status: "running"
      });
    });

    expect(await screen.findByText("60%")).toBeInTheDocument();
  });

  it("updates summary key fields from later snapshot events", async () => {
    const api = createMockApi({
      getSessionSnapshot: async () => ({
        session_id: "session-1",
        groups: [],
        key_fields: {},
        first_records: {},
        bytes_read: 0,
        total_bytes: 1024,
        status: "running"
      })
    });
    const user = userEvent.setup();

    render(<App api={api} />);
    await user.click(screen.getByRole("button", { name: "打开 STDF 文件" }));
    await screen.findByRole("main", { name: "文件摘要" });

    act(() => {
      api.emitSnapshot({
        session_id: "session-1",
        groups: [{ record_type: "MIR", count: 1 }],
        key_fields: {
          MIR: [
            {
              name: "LOT_ID",
              field_type: "C*n",
              value: "EARLY-LOT",
              description: "Lot ID"
            }
          ]
        },
        first_records: {},
        bytes_read: 128,
        total_bytes: 1024,
        status: "running"
      });
    });

    expect(await screen.findByText("EARLY-LOT")).toBeInTheDocument();
  });
});
