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
