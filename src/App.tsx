import {
  AlertCircle,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  CircleDot,
  Download,
  Filter,
  FolderOpen,
  LayoutDashboard,
  Loader2,
  Moon,
  Search,
  Sun,
  TableProperties,
  Table2,
  X,
  XCircle
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import type {
  ParseErrorEvent,
  ParseProgress,
  ParseSession,
  RecordField,
  RecordGroup,
  RecordSummary,
  SearchResult,
  SessionSnapshot,
  TestItemColumn,
  TestItemColumnLite,
  TestItemPage,
  TestItemPartRow,
  StdfApi
} from "./types";
import { tauriApi } from "./api";
import { UpdateChecker } from "./UpdateChecker";
import "./styles.css";

const PAGE_SIZE = 50;
// Test-item matrix pagination: rows (parts) and columns (test items) page
// independently so the table mounts a bounded number of cells on any file.
// Rows (parts) load incrementally — 500 at a time as you scroll. Test-item columns
// page in fixed-size chunks the user can pick from.
const TI_ROW_BATCH = 500;
const TI_COL_SIZE_OPTIONS = [200, 500, 1000];
const THEME_KEY = "stdf-theme";

/* ------------------------------------------------------------------ *
 * Shared Tailwind class tokens — small component-system style layer.  *
 * ------------------------------------------------------------------ */
const EYEBROW = "text-[11px] font-semibold uppercase tracking-wide text-muted-foreground";

const BTN_BASE =
  "inline-flex min-h-[40px] items-center justify-center gap-2 rounded-md px-3.5 text-sm font-medium transition duration-100 active:scale-[0.98] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";
const BTN_PRIMARY = `${BTN_BASE} bg-primary text-primary-foreground hover:bg-primary-hover`;
const BTN_SECONDARY = `${BTN_BASE} whitespace-nowrap border border-border-strong bg-card text-muted-foreground hover:bg-muted hover:text-foreground`;
const PAGER_BTN =
  "inline-flex h-7 select-none items-center gap-0.5 rounded-md border border-border-strong bg-card px-2 text-xs font-medium text-muted-foreground transition duration-100 hover:bg-muted hover:text-foreground active:scale-95 active:border-primary active:bg-primary-soft active:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";

const RAIL_ITEM =
  "flex h-14 w-14 flex-col items-center justify-center gap-1 rounded-xl text-[10px] font-medium transition duration-100 active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
const RAIL_ITEM_IDLE = "text-muted-foreground hover:bg-border/40 hover:text-foreground";
const RAIL_ITEM_ACTIVE = "bg-primary-soft text-primary";

const TABLE_SCROLL = "min-h-0 flex-1 overflow-auto rounded-lg border border-border";
const DATA_TABLE = "w-full table-fixed border-collapse text-[13px] [&_tbody_tr:hover]:bg-muted/50";
const TH =
  "sticky top-0 z-[1] border-b border-border bg-muted px-2.5 py-2.5 text-left align-top text-xs font-semibold text-muted-foreground";
const TD = "border-b border-border/70 px-2.5 py-2.5 text-left align-top text-foreground [overflow-wrap:anywhere]";
const MONO = "font-mono text-xs";

const STATUS_PILL_BASE =
  "inline-flex h-[30px] items-center gap-1.5 whitespace-nowrap rounded-full border px-2.5 text-xs font-medium";
const STATUS_PILL_TONE: Record<string, string> = {
  running: "border-primary-soft bg-primary-soft text-primary",
  complete: "border-success-border bg-success-soft text-success",
  cancelled: "border-border bg-muted text-muted-foreground",
  error: "border-danger-border bg-danger-soft text-danger"
};

const RECORD_STATUS_TONE: Record<RecordSummary["status"], string> = {
  parsed: "border-success-border bg-success-soft text-success",
  unknown: "border-warning-border bg-warning-soft text-warning",
  error: "border-danger-border bg-danger-soft text-danger"
};

// STDF V4 record-type full names — shown under the record type in 字段详情.
const RECORD_TYPE_INFO: Record<string, string> = {
  FAR: "File Attributes Record · 文件属性记录",
  ATR: "Audit Trail Record · 审计跟踪记录",
  MIR: "Master Information Record · 主信息记录",
  MRR: "Master Results Record · 主结果记录",
  PCR: "Part Count Record · 器件计数记录",
  HBR: "Hardware Bin Record · 硬 bin 记录",
  SBR: "Software Bin Record · 软 bin 记录",
  PMR: "Pin Map Record · 引脚映射记录",
  PGR: "Pin Group Record · 引脚组记录",
  PLR: "Pin List Record · 引脚列表记录",
  RDR: "Retest Data Record · 重测数据记录",
  SDR: "Site Description Record · 站点描述记录",
  WIR: "Wafer Information Record · 晶圆开始记录",
  WRR: "Wafer Results Record · 晶圆结果记录",
  WCR: "Wafer Configuration Record · 晶圆配置记录",
  PIR: "Part Information Record · 器件开始记录",
  PRR: "Part Results Record · 器件结果记录",
  TSR: "Test Synopsis Record · 测试摘要记录",
  PTR: "Parametric Test Record · 参数测试记录",
  MPR: "Multiple-Result Parametric Record · 多结果参数测试记录",
  FTR: "Functional Test Record · 功能测试记录",
  BPS: "Begin Program Section Record · 程序段开始记录",
  EPS: "End Program Section Record · 程序段结束记录",
  GDR: "Generic Data Record · 通用数据记录",
  DTR: "Datalog Text Record · datalog 文本记录",
  UNKNOWN: "Unknown Record · 未知记录（非 STDF V4 标准类型）"
};

const NAV_ITEMS = [
  { key: "summary", label: "概览", icon: LayoutDashboard },
  { key: "records", label: "记录", icon: Table2 },
  { key: "search", label: "搜索", icon: Search },
  { key: "test-items", label: "测试项", icon: TableProperties }
] as const;

type NavSection = (typeof NAV_ITEMS)[number]["key"];
type Theme = "light" | "dark";

// Records that hold OneData's default batch-level key fields (loaded for the overview).
const KEY_FIELD_RECORDS = ["MIR", "MRR", "WIR", "SDR"] as const;

// OneData default fields sourced from STDF when no FieldRule is configured (conf-spec §6).
// The 含义 column is taken from the parser's STDF field dictionary at render time
// (so it stays authoritative and consistent with the field-detail view, no hand translation).
// scope: "cp" only for wafer test, "ft" only for final test, undefined = both.
type KeyFieldSpec = { rec: string; field: string; oneData: string; scope?: "cp" | "ft" };
const ONEDATA_KEY_FIELDS: KeyFieldSpec[] = [
  { rec: "MIR", field: "PART_TYP", oneData: "partTyp" },
  { rec: "MIR", field: "LOT_ID", oneData: "lotId" },
  { rec: "MIR", field: "SBLOT_ID", oneData: "sblotId" },
  { rec: "MIR", field: "TEST_COD", oneData: "testCod" },
  { rec: "MIR", field: "FLOW_ID", oneData: "flowId" },
  { rec: "MIR", field: "RTST_COD", oneData: "rtstCod" },
  { rec: "MIR", field: "SETUP_ID", oneData: "setupId" },
  { rec: "MIR", field: "FLOOR_ID", oneData: "floorId" },
  { rec: "MIR", field: "TST_TEMP", oneData: "tstTemp" },
  { rec: "MIR", field: "NODE_NAM", oneData: "nodeNam" },
  { rec: "MIR", field: "TSTR_TYP", oneData: "tstrTyp" },
  { rec: "MIR", field: "JOB_NAM", oneData: "jobNam" },
  { rec: "MIR", field: "JOB_REV", oneData: "jobRev" },
  { rec: "MIR", field: "SPEC_NAM", oneData: "specNam" },
  { rec: "MIR", field: "SPEC_VER", oneData: "specVer" },
  { rec: "MIR", field: "OPER_FRQ", oneData: "operFrq" },
  { rec: "MIR", field: "PKG_TYP", oneData: "pkgTyp" },
  { rec: "MIR", field: "USER_TXT", oneData: "userTxt" },
  { rec: "MIR", field: "START_T", oneData: "startT" },
  { rec: "MRR", field: "FINISH_T", oneData: "finishT" },
  { rec: "WIR", field: "WAFER_ID", oneData: "waferId", scope: "cp" },
  { rec: "SDR", field: "CARD_ID", oneData: "probecardLoadboardId", scope: "cp" },
  { rec: "SDR", field: "LOAD_ID", oneData: "probecardLoadboardId", scope: "ft" }
];

interface AppProps {
  api?: StdfApi;
}

function readInitialTheme(): Theme {
  try {
    const saved = window.localStorage.getItem(THEME_KEY);
    if (saved === "dark" || saved === "light") return saved;
  } catch {
    /* ignore */
  }
  return "light";
}

export default function App({ api = tauriApi }: AppProps) {
  const [session, setSession] = useState<ParseSession | null>(null);
  const [groups, setGroups] = useState<RecordGroup[]>([]);
  const [selectedGroup, setSelectedGroup] = useState("");
  const [recordTotal, setRecordTotal] = useState(0);
  const [cursor, setCursor] = useState(0);
  const [selectedRecord, setSelectedRecord] = useState<RecordSummary | null>(null);
  const [fields, setFields] = useState<RecordField[]>([]);
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SearchResult[]>([]);
  const [searchTotal, setSearchTotal] = useState(0);
  const [searching, setSearching] = useState(false);
  // { scanned, total } while a search is running so SearchView can show a
  // determinate progress bar. Null before the first tick lands or after it
  // completes. `session_id` on the event is validated against the active
  // session before we accept the numbers.
  const [searchProgress, setSearchProgress] = useState<{ scanned: number; total: number } | null>(
    null
  );
  const [keyFields, setKeyFields] = useState<Record<string, RecordField[]>>({});
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  const [tiColumns, setTiColumns] = useState<TestItemColumn[]>([]);
  const [tiColTotal, setTiColTotal] = useState(0);
  const [tiRows, setTiRows] = useState<TestItemPartRow[]>([]);
  const [tiRowTotal, setTiRowTotal] = useState(0);
  const [tiPmrCount, setTiPmrCount] = useState(0);
  const [tiLoaded, setTiLoaded] = useState(false);
  const [tiLoadingMore, setTiLoadingMore] = useState(false);
  const [tiColPage, setTiColPage] = useState(0);
  const [tiColSize, setTiColSize] = useState(200);
  // Applied test-item selection — empty array means "show all".
  const [tiSelected, setTiSelected] = useState<string[]>([]);
  const [tiFilterOpen, setTiFilterOpen] = useState(false);
  const [tiAllColumns, setTiAllColumns] = useState<TestItemColumnLite[]>([]);
  const [tiColumnsLoading, setTiColumnsLoading] = useState(false);
  const [tiExporting, setTiExporting] = useState(false);
  const [tiExported, setTiExported] = useState(false);
  const [tiHasBinPf, setTiHasBinPf] = useState(true);
  // Bumped whenever the column window / selection changes, to drop stale "load more" responses.
  const tiEpoch = useRef(0);
  const [nav, setNav] = useState<NavSection>("summary");
  const [progress, setProgress] = useState<ParseProgress | null>(null);
  const [error, setError] = useState("");
  const [warning, setWarning] = useState("");
  const [isDragOver, setDragOver] = useState(false);
  const [theme, setTheme] = useState<Theme>(readInitialTheme);
  const groupRefreshTimer = useRef<number | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const sessionStatusRef = useRef<ParseSession["status"] | null>(null);

  useEffect(() => {
    const root = document.documentElement;
    if (theme === "dark") root.setAttribute("data-theme", "dark");
    else root.removeAttribute("data-theme");
    try {
      window.localStorage.setItem(THEME_KEY, theme);
    } catch {
      /* ignore */
    }
  }, [theme]);

  const toggleTheme = () => setTheme((current) => (current === "dark" ? "light" : "dark"));

  const progressPercent = useMemo(() => {
    if (!progress || progress.total_bytes === 0) return 0;
    const raw = Math.round((progress.bytes_read / progress.total_bytes) * 100);
    // Bytes can reach 100% before the parser finishes finalizing (building the
    // index/snapshot), so hold at 99% until the session is actually complete.
    if (session?.status === "complete") return 100;
    return Math.min(99, raw);
  }, [progress, session?.status]);

  const totalRecords = useMemo(
    () => groups.reduce((sum, group) => sum + group.count, 0),
    [groups]
  );

  const statusView = getStatusView(session?.status ?? null, progressPercent);

  useEffect(() => {
    let disposed = false;
    let cleanupProgress: (() => void) | undefined;
    let cleanupBatch: (() => void) | undefined;
    let cleanupSnapshot: (() => void) | undefined;
    let cleanupNativeDrop: (() => void) | undefined;
    let cleanupComplete: (() => void) | undefined;
    let cleanupError: (() => void) | undefined;
    let cleanupWarning: (() => void) | undefined;

    api.onProgress((event) => {
      if (!disposed && sessionIdRef.current === event.session_id) {
        setProgress((current) => {
          if (current?.session_id === event.session_id && current.bytes_read > event.bytes_read) {
            return current;
          }
          return event;
        });
      }
    }).then((cleanup) => {
      cleanupProgress = cleanup;
    });
    api.onRecordBatch((event) => {
      if (
        !disposed &&
        sessionIdRef.current === event.session_id &&
        sessionStatusRef.current === "complete"
      ) {
        scheduleGroupRefresh(event.session_id);
      }
    }).then((cleanup) => {
      cleanupBatch = cleanup;
    });
    api.onSessionSnapshot((nextSnapshot) => {
      if (!disposed && sessionIdRef.current === nextSnapshot.session_id) {
        applySnapshot(nextSnapshot);
      }
    }).then((cleanup) => {
      cleanupSnapshot = cleanup;
    });
    api.onNativeFileDrop((path) => {
      api.openDroppedFile(path).then(startSession).catch((err) => setError(String(err)));
    }).then((cleanup) => {
      cleanupNativeDrop = cleanup;
    });
    api.onParseComplete((sessionId) => {
      if (!disposed && sessionIdRef.current === sessionId) {
        sessionStatusRef.current = "complete";
        setSession((current) => (current ? { ...current, status: "complete" } : current));
        refreshGroups(sessionId);
      }
    }).then((cleanup) => {
      cleanupComplete = cleanup;
    });
    api.onParseError((event: ParseErrorEvent) => {
      if (!disposed) setError(event.message);
    }).then((cleanup) => {
      cleanupError = cleanup;
    });
    api.onParseWarning((event: ParseErrorEvent) => {
      if (!disposed && sessionIdRef.current === event.session_id) setWarning(event.message);
    }).then((cleanup) => {
      cleanupWarning = cleanup;
    });

    return () => {
      disposed = true;
      cleanupProgress?.();
      cleanupBatch?.();
      cleanupSnapshot?.();
      cleanupNativeDrop?.();
      cleanupComplete?.();
      cleanupError?.();
      cleanupWarning?.();
      if (groupRefreshTimer.current !== null) {
        window.clearTimeout(groupRefreshTimer.current);
      }
    };
  }, [api]);

  useEffect(() => {
    if (!session || !selectedGroup) return;
    const first = snapshot?.first_records[selectedGroup];
    if (cursor === 0 && first) {
      setRecordTotal(groups.find((group) => group.record_type === selectedGroup)?.count ?? 1);
      setSelectedRecord(first.record);
      setFields(first.fields);
      return;
    }
    const page = Math.floor(cursor / PAGE_SIZE);
    api.getRecords(session.session_id, selectedGroup, page, PAGE_SIZE).then((res) => {
      setRecordTotal(res.total);
      const rec = res.records[cursor % PAGE_SIZE] ?? res.records[0] ?? null;
      setSelectedRecord(rec);
      if (rec) {
        api.getRecordFields(session.session_id, rec.id).then(setFields);
      } else {
        setFields([]);
      }
    });
  }, [api, session, selectedGroup, cursor, snapshot, groups]);

  useEffect(() => {
    if (!session) return;
    setNav("summary");
    setKeyFields(snapshot?.key_fields ?? {});
    tiEpoch.current += 1;
    setTiColumns([]);
    setTiRows([]);
    setTiColTotal(0);
    setTiRowTotal(0);
    setTiPmrCount(0);
    setTiLoaded(false);
    setTiHasBinPf(true);
    setTiColPage(0);
    setTiSelected([]);
    setTiFilterOpen(false);
    setTiAllColumns([]);
  }, [session?.session_id]);

  // Load the records that hold OneData's default key fields (MIR/MRR/WIR/SDR). Data arrives
  // incrementally via SQLite, so fetch each type once it appears in groups (MRR is last).
  useEffect(() => {
    if (!session || session.status !== "complete") return;
    KEY_FIELD_RECORDS.forEach((type) => {
      if (keyFields[type]) return;
      if (!groups.some((group) => group.record_type === type && group.count > 0)) return;
      api.getRecords(session.session_id, type, 0, 1).then((page) => {
        const next = page.records[0] ?? null;
        if (next) {
          api.getRecordFields(session.session_id, next.id).then((fields) => {
            setKeyFields((prev) => ({ ...prev, [type]: fields }));
          });
        }
      });
    });
  }, [api, session, groups, keyFields]);

  useEffect(() => {
    if (!session) return;
    const trimmed = query.trim();
    if (session.status !== "complete") {
      setSearchResults([]);
      setSearchTotal(0);
      setSearching(false);
      setSearchProgress(null);
      return;
    }
    // Require >= 2 chars: a 1-char query over a huge file matches almost everything and is slow.
    if (trimmed.length < 2) {
      setSearchResults([]);
      setSearchTotal(0);
      setSearching(false);
      setSearchProgress(null);
      return;
    }
    let active = true;
    setSearching(true);
    setSearchProgress(null);
    const timer = window.setTimeout(() => {
      api
        .searchFields(session.session_id, trimmed, 0, PAGE_SIZE, (p) => {
          // Progress ticks arrive on this invoke's dedicated Channel — bail
          // if the caller has moved on to a newer query.
          if (!active) return;
          setSearchProgress({ scanned: p.scanned, total: p.total });
        })
        .then((page) => {
          if (!active) return; // ignore stale responses from older queries
          setSearchResults(page.results);
          setSearchTotal(page.total);
          setSearching(false);
          setSearchProgress(null);
        })
        .catch(() => {
          if (active) {
            setSearching(false);
            setSearchProgress(null);
          }
        });
    }, 350);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [api, query, session]);

  useEffect(() => {
    if (nav !== "records") return;
    const onKey = (event: KeyboardEvent) => {
      const el = document.activeElement;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA")) return;
      if (event.key === "ArrowLeft" && recordTotal > 1) {
        event.preventDefault();
        setCursor((current) => Math.max(0, current - 1));
      } else if (event.key === "ArrowRight" && recordTotal > 1) {
        event.preventDefault();
        setCursor((current) => Math.min(recordTotal - 1, current + 1));
      } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        if (groups.length === 0) return;
        const idx = groups.findIndex((group) => group.record_type === selectedGroup);
        const nextIdx = event.key === "ArrowUp" ? idx - 1 : idx + 1;
        if (idx === -1 || nextIdx < 0 || nextIdx >= groups.length) return;
        event.preventDefault();
        setSelectedGroup(groups[nextIdx].record_type);
        setCursor(0);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [nav, recordTotal, groups, selectedGroup]);

  // Load the first batch of rows whenever the column window or filters change.
  // The test-item nav is only reachable after parsing completes, so we gate on
  // nav + session id rather than the (transiently re-emitted) session status.
  useEffect(() => {
    const sessionId = session?.session_id;
    if (!sessionId || nav !== "test-items") {
      return;
    }
    let active = true;
    tiEpoch.current += 1;
    const epoch = tiEpoch.current;
    api
      .getTestItemPage(
        sessionId,
        0,
        TI_ROW_BATCH,
        tiColPage * tiColSize,
        tiColSize,
        tiSelected,
        ""
      )
      .then((page) => {
        if (!active || epoch !== tiEpoch.current) return;
        setTiColumns(page.columns);
        setTiColTotal(page.total_columns);
        setTiRows(page.rows);
        setTiRowTotal(page.total_rows);
        setTiPmrCount(Object.keys(page.pmr_lookup).length);
        setTiHasBinPf(page.has_bin_pf);
        setTiLoaded(true);
      });
    return () => {
      active = false;
    };
  }, [api, session?.session_id, nav, tiColPage, tiColSize, tiSelected]);

  // Lazily load the full column list (identities only) for the filter dialog.
  useEffect(() => {
    const sessionId = session?.session_id;
    if (!sessionId || nav !== "test-items" || tiAllColumns.length > 0) {
      return;
    }
    let active = true;
    setTiColumnsLoading(true);
    api
      .getTestItemColumns(sessionId)
      .then((cols) => {
        if (active) setTiAllColumns(cols);
      })
      .finally(() => {
        if (active) setTiColumnsLoading(false);
      });
    return () => {
      active = false;
    };
  }, [api, session?.session_id, nav, tiAllColumns.length]);

  // Append the next batch of rows for infinite scroll. Tagged with the current
  // epoch so a response that arrives after a column/selection change is discarded.
  function loadMoreTestRows() {
    const sessionId = session?.session_id;
    if (!sessionId || tiLoadingMore || tiRows.length >= tiRowTotal) {
      return;
    }
    const epoch = tiEpoch.current;
    setTiLoadingMore(true);
    api
      .getTestItemPage(
        sessionId,
        tiRows.length,
        TI_ROW_BATCH,
        tiColPage * tiColSize,
        tiColSize,
        tiSelected,
        ""
      )
      .then((page) => {
        if (epoch !== tiEpoch.current) return;
        setTiRows((prev) => [...prev, ...page.rows]);
        setTiRowTotal(page.total_rows);
      })
      .finally(() => setTiLoadingMore(false));
  }

  // Export the full test-item matrix to a CSV the user picks via a save dialog.
  // Default name = STDF filename with a .csv extension; the backend writes the file.
  async function exportTestItemsCsv() {
    const sessionId = session?.session_id;
    if (!sessionId || tiExporting) return;
    const base = (session?.file_name || "export")
      .replace(/\.(gz|zip)$/i, "")
      .replace(/\.(stdf|std)$/i, "");
    const path = await api.saveCsvDialog(`${base}.csv`);
    if (!path) return;
    setTiExporting(true);
    setTiExported(false);
    try {
      await api.exportTestItemCsv(sessionId, path);
      setTiExported(true);
      window.setTimeout(() => setTiExported(false), 2500);
    } catch (err) {
      setError(`导出 CSV 失败：${String(err)}`);
    } finally {
      setTiExporting(false);
    }
  }

  async function refreshGroups(sessionId: string) {
    const nextGroups = await api.getRecordGroups(sessionId);
    setGroups(nextGroups);
    setSelectedGroup((current) => current || nextGroups[0]?.record_type || "");
  }

  function scheduleGroupRefresh(sessionId: string) {
    if (groupRefreshTimer.current !== null) {
      return;
    }
    groupRefreshTimer.current = window.setTimeout(() => {
      groupRefreshTimer.current = null;
      refreshGroups(sessionId);
    }, 250);
  }

  function applySnapshot(nextSnapshot: SessionSnapshot) {
    setSnapshot(nextSnapshot);
    setGroups(nextSnapshot.groups);
    setKeyFields(nextSnapshot.key_fields);
    setProgress((current) => {
      if (current?.session_id === nextSnapshot.session_id && current.bytes_read > nextSnapshot.bytes_read) {
        return current;
      }
      return {
        session_id: nextSnapshot.session_id,
        bytes_read: nextSnapshot.bytes_read,
        total_bytes: nextSnapshot.total_bytes
      };
    });
    sessionStatusRef.current = nextSnapshot.status;
    setSession((current) =>
      current && current.session_id === nextSnapshot.session_id
        ? { ...current, status: nextSnapshot.status }
        : current
    );
    setSelectedGroup((current) => current || nextSnapshot.groups[0]?.record_type || "");
  }

  async function startSession(nextSession: ParseSession | null) {
    if (!nextSession) return;
    sessionIdRef.current = nextSession.session_id;
    sessionStatusRef.current = nextSession.status;
    setSession(nextSession);
    setProgress({
      session_id: nextSession.session_id,
      bytes_read: 0,
      total_bytes: nextSession.file_size
    });
    setError("");
    setWarning("");
    setQuery("");
    setNav("summary");
    setGroups([]);
    setSelectedGroup("");
    setSearchResults([]);
    setSearchTotal(0);
    setRecordTotal(0);
    setCursor(0);
    setSelectedRecord(null);
    setFields([]);
    setKeyFields({});
    setSnapshot(null);
    tiEpoch.current += 1;
    setTiColumns([]);
    setTiRows([]);
    setTiColTotal(0);
    setTiRowTotal(0);
    setTiPmrCount(0);
    setTiLoaded(false);
    setTiHasBinPf(true);
    setTiColPage(0);
    setTiSelected([]);
    setTiFilterOpen(false);
    setTiAllColumns([]);
    const initialSnapshot = await api.getSessionSnapshot(nextSession.session_id);
    applySnapshot(initialSnapshot);
    void hydrateEarlySnapshot(nextSession.session_id);
  }

  async function hydrateEarlySnapshot(sessionId: string) {
    for (let attempt = 0; attempt < 20; attempt += 1) {
      if (sessionIdRef.current !== sessionId) return;
      await new Promise((resolve) => window.setTimeout(resolve, 100));
      const nextSnapshot = await api.getSessionSnapshot(sessionId);
      if (sessionIdRef.current !== sessionId) return;
      applySnapshot(nextSnapshot);
      if (Object.keys(nextSnapshot.key_fields).length > 0 || nextSnapshot.status !== "running") {
        return;
      }
    }
  }

  async function openAnotherFile() {
    try {
      await api.openFile().then(startSession);
    } catch (err) {
      setError(String(err));
    }
  }

  function handleDrop(event: React.DragEvent<HTMLDivElement>) {
    event.preventDefault();
    setDragOver(false);
    const file = event.dataTransfer.files[0];
    const path = "path" in file ? String((file as File & { path?: string }).path) : "";
    if (path) {
      api.openDroppedFile(path).then(startSession).catch((err) => setError(String(err)));
    }
  }

  return (
    <div className="flex h-dvh min-h-[720px] overflow-hidden bg-background">
      <NavRail
        nav={nav}
        onNavigate={setNav}
        onOpenAnotherFile={openAnotherFile}
        theme={theme}
        onToggleTheme={toggleTheme}
        hasSession={!!session}
        parseComplete={session?.status === "complete"}
      />
      <main
        className="flex min-w-0 flex-1 flex-col overflow-hidden"
        aria-label={nav === "summary" ? "文件摘要" : "STDF 工作台"}
      >
        {session ? (
          <>
            <TopBar
              session={session}
              statusView={statusView}
              progressPercent={progressPercent}
              progress={progress}
              totalRecords={totalRecords}
              nav={nav}
            />
            {error && (
              <div
                className="flex items-center gap-2 border-b border-danger-border bg-danger-soft px-4 py-2.5 text-sm text-danger"
                role="alert"
              >
                <AlertCircle size={16} />
                <span>{error}</span>
              </div>
            )}
            {warning && (
              <div
                className="flex items-start gap-2 border-b border-warning-border bg-warning-soft px-4 py-2.5 text-sm text-warning"
                role="status"
              >
                <AlertCircle size={16} className="mt-0.5 shrink-0" />
                <span>{warning}</span>
              </div>
            )}
            {nav === "summary" && (
              <OverviewView
                session={session}
                keyFields={keyFields}
                groups={groups}
                onOpenRecordType={(type) => {
                  setSelectedGroup(type);
                  setCursor(0);
                  setNav("records");
                }}
              />
            )}
            {nav === "test-items" && (
              <>
              <TestItemsView
                session={session}
                loaded={tiLoaded}
                columns={tiColumns}
                rows={tiRows}
                colTotal={tiColTotal}
                rowTotal={tiRowTotal}
                pmrCount={tiPmrCount}
                colPage={tiColPage}
                colSize={tiColSize}
                selectedCount={tiSelected.length}
                loadingMore={tiLoadingMore}
                hasMore={tiRows.length < tiRowTotal}
                hasBinPf={tiHasBinPf}
                exporting={tiExporting}
                exported={tiExported}
                onExport={exportTestItemsCsv}
                onColPageChange={setTiColPage}
                onColSizeChange={(size) => {
                  setTiColSize(size);
                  setTiColPage(0);
                }}
                onOpenFilter={() => setTiFilterOpen(true)}
                onLoadMore={loadMoreTestRows}
              />
              {tiFilterOpen && (
                <TestItemFilterDialog
                  columns={tiAllColumns}
                  loading={tiColumnsLoading}
                  applied={tiSelected}
                  onClose={() => setTiFilterOpen(false)}
                  onConfirm={(keys) => {
                    setTiSelected(keys);
                    setTiColPage(0);
                    setTiFilterOpen(false);
                  }}
                />
              )}
              </>
            )}
            {nav === "records" && (
              <RecordsView
                groups={groups}
                selectedGroup={selectedGroup}
                onSelectGroup={(group) => {
                  setSelectedGroup(group);
                  setCursor(0);
                }}
                selectedRecord={selectedRecord}
                fields={fields}
                cursor={cursor}
                recordTotal={recordTotal}
                onCursorChange={setCursor}
              />
            )}
            {nav === "search" && (
              <SearchView
                query={query}
                setQuery={setQuery}
                searchResults={searchResults}
                searchTotal={searchTotal}
                searching={searching}
                searchProgress={searchProgress}
                parseComplete={session.status === "complete"}
              />
            )}
          </>
        ) : (
          <NoFileView
            onOpen={() => api.openFile().then(startSession)}
            isDragOver={isDragOver}
            setDragOver={setDragOver}
            onDrop={handleDrop}
            error={error}
          />
        )}
      </main>
    </div>
  );
}

function NoFileView({
  onOpen,
  isDragOver,
  setDragOver,
  onDrop,
  error
}: {
  onOpen(): void;
  isDragOver: boolean;
  setDragOver(value: boolean): void;
  onDrop(event: React.DragEvent<HTMLDivElement>): void;
  error: string;
}) {
  return (
    <section
      className="flex flex-1 items-center justify-center overflow-auto p-10"
      aria-label="选择 STDF 文件"
      onDragOver={(event) => {
        event.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={onDrop}
    >
      <div
        className={`flex min-h-[360px] w-full max-w-[640px] flex-col items-center justify-center gap-5 rounded-2xl border-2 border-dashed bg-card p-12 text-center transition-colors ${
          isDragOver ? "border-primary bg-primary-soft" : "border-border-strong"
        }`}
      >
        <img src="/logo.png" alt="" className="h-16 w-16" aria-hidden="true" />
        <div className="flex max-w-[460px] flex-col gap-1.5">
          <h1 className="text-xl font-semibold text-foreground">选择一个 STDF 文件</h1>
          <p className="text-muted-foreground">把 .stdf / .std 文件拖到这里，或点下面的按钮打开。</p>
          <p className="text-muted-foreground">按 record type 浏览字段、值和中文说明。</p>
        </div>
        <button className={BTN_PRIMARY} type="button" onClick={onOpen}>
          <FolderOpen size={18} />
          打开 STDF 文件
        </button>
        <p className="text-xs text-muted-foreground">支持 STDF V4 / V4-2007。</p>
        {error && (
          <p
            className="max-w-[520px] rounded-md border border-danger-border bg-danger-soft px-3 py-2 text-sm text-danger"
            role="alert"
          >
            {error}
          </p>
        )}
      </div>
    </section>
  );
}

function NavRail({
  nav,
  onNavigate,
  onOpenAnotherFile,
  theme,
  onToggleTheme,
  hasSession,
  parseComplete
}: {
  nav: NavSection;
  onNavigate(section: NavSection): void;
  onOpenAnotherFile(): void;
  theme: Theme;
  onToggleTheme(): void;
  hasSession: boolean;
  parseComplete: boolean;
}) {
  return (
    <nav className="flex w-[72px] shrink-0 flex-col items-center gap-1.5 border-r border-border bg-muted py-3" aria-label="导航">
      <img src="/logo.png" alt="" className="mb-2 h-9 w-9" aria-hidden="true" />
      {NAV_ITEMS.map((item) => {
        const active = nav === item.key;
        const disabled = (!hasSession && item.key !== "summary") || (item.key === "test-items" && !parseComplete);
        const Icon = item.icon;
        return (
          <button
            key={item.key}
            type="button"
            aria-label={item.label}
            aria-pressed={active}
            title={item.label}
            disabled={disabled}
            onClick={() => onNavigate(item.key)}
            className={`${RAIL_ITEM} ${active ? RAIL_ITEM_ACTIVE : RAIL_ITEM_IDLE} disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-foreground`}
          >
            <Icon size={19} aria-hidden="true" />
            <span>{item.label}</span>
          </button>
        );
      })}
      <div className="mt-auto flex flex-col items-center gap-1.5">
        <UpdateChecker />
        <ThemeToggle theme={theme} onToggle={onToggleTheme} rail />
        <button
          type="button"
          aria-label="打开另一个文件"
          title="打开另一个文件"
          onClick={onOpenAnotherFile}
          className={`${RAIL_ITEM} ${RAIL_ITEM_IDLE}`}
        >
          <FolderOpen size={19} aria-hidden="true" />
          <span>打开</span>
        </button>
      </div>
    </nav>
  );
}

function ThemeToggle({ theme, onToggle, rail = false }: { theme: Theme; onToggle(): void; rail?: boolean }) {
  const Icon = theme === "dark" ? Sun : Moon;
  const label = theme === "dark" ? "切换到浅色主题" : "切换到深色主题";
  const className = rail
    ? `${RAIL_ITEM} ${RAIL_ITEM_IDLE}`
    : "flex h-9 w-9 items-center justify-center rounded-lg border border-border bg-card text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
  return (
    <button type="button" onClick={onToggle} aria-label={label} title={label} className={className}>
      <Icon size={rail ? 19 : 18} aria-hidden="true" />
      {rail && <span>{theme === "dark" ? "浅色" : "深色"}</span>}
    </button>
  );
}

function StatusPill({ statusView }: { statusView: ReturnType<typeof getStatusView> }) {
  const Icon = statusView.icon;
  return (
    <div className={`${STATUS_PILL_BASE} ${STATUS_PILL_TONE[statusView.tone]}`}>
      <Icon size={16} aria-hidden="true" className={statusView.spin ? "animate-spin" : undefined} />
      <span>{statusView.label}</span>
    </div>
  );
}

function TopBar({
  session,
  statusView,
  progressPercent,
  progress,
  totalRecords,
  nav
}: {
  session: ParseSession;
  statusView: ReturnType<typeof getStatusView>;
  progressPercent: number;
  progress: ParseProgress | null;
  totalRecords: number;
  nav: NavSection;
}) {
  return (
    <header className="flex items-center gap-4 border-b border-border bg-card px-4 py-2" aria-label="文件解析状态">
      <div className="flex min-w-0 flex-1 items-center">
        {nav === "summary" ? (
          <span className="truncate text-[13px] text-muted-foreground">
            {formatBytes(session.file_size)} · {totalRecords.toLocaleString()} records
          </span>
        ) : (
          <span className="min-w-0 truncate text-[13px] text-muted-foreground" title={session.file_name}>
            {session.file_name}
          </span>
        )}
      </div>
      {nav === "summary" && (
        <>
          <StatusPill statusView={statusView} />
          <div className="hidden w-[340px] lg:block">
            <div className="flex justify-between gap-3 text-xs text-muted-foreground" aria-label="解析进度">
              <span>{progressPercent}%</span>
              <span>
                {formatBytes(progress?.bytes_read ?? 0)} / {formatBytes(progress?.total_bytes ?? session.file_size)}
              </span>
            </div>
            <div
              className="mt-1 h-2 overflow-hidden rounded-full bg-muted"
              role="progressbar"
              aria-valuenow={progressPercent}
              aria-valuemin={0}
              aria-valuemax={100}
            >
              <div
                className="h-full rounded-full bg-primary transition-[width] duration-200"
                style={{ width: `${progressPercent}%` }}
              />
            </div>
          </div>
        </>
      )}
    </header>
  );
}

function OverviewView({
  session,
  keyFields,
  groups,
  onOpenRecordType
}: {
  session: ParseSession;
  keyFields: Record<string, RecordField[]>;
  groups: RecordGroup[];
  onOpenRecordType(recordType: string): void;
}) {
  // CP files carry wafer records (WIR/WRR); otherwise treat as FT.
  const isCp = groups.some(
    (group) => (group.record_type === "WIR" || group.record_type === "WRR") && group.count > 0
  );
  const complete = session.status === "complete";
  const present = new Set(groups.filter((group) => group.count > 0).map((group) => group.record_type));
  const rows = ONEDATA_KEY_FIELDS.filter((spec) => {
    if (spec.scope === "cp") return isCp;
    if (spec.scope === "ft") return !isCp;
    return true;
  }).map((spec) => {
    const parsed = keyFields[spec.rec]?.find((field) => field.name === spec.field);
    // 含义 comes straight from the parser's STDF field dictionary (authoritative).
    return { ...spec, value: parsed?.value ?? "", meaning: parsed?.description ?? "" };
  });
  return (
    <section className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden p-[18px]">
      <div className="shrink-0">
        <span className={EYEBROW}>File Summary</span>
        <h1
          className="mt-1 text-[13px] font-normal leading-snug text-foreground [overflow-wrap:anywhere]"
          title={session.file_name}
        >
          {session.file_name}
        </h1>
      </div>
      <PairStats groups={groups} complete={complete} />
      <section className="flex min-h-0 min-w-0 flex-1 flex-col rounded-xl border border-border bg-card p-[18px]">
        <div className="mb-3 flex min-w-0 items-center justify-between gap-4">
          <span className={EYEBROW}>关键字段</span>
          <span className="inline-flex shrink-0 items-center rounded-full border border-primary-soft bg-primary-soft px-2.5 py-0.5 text-xs font-medium text-primary">
            {isCp ? "CP" : "FT"}
          </span>
        </div>
        <div className={TABLE_SCROLL}>
          <table className={DATA_TABLE}>
            <thead>
              <tr>
                <th className={`${TH} w-[22%]`}>STDF 字段</th>
                <th className={`${TH} w-[30%]`}>值</th>
                <th className={`${TH} w-[26%]`}>含义</th>
                <th className={`${TH} w-[22%]`}>OneData 字段</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => {
                const clickable = present.has(row.rec);
                return (
                  <tr key={`${row.rec}.${row.field}`}>
                    <td
                      className={`${TD} ${MONO} ${
                        clickable ? "cursor-pointer text-primary hover:underline" : ""
                      }`}
                      onClick={clickable ? () => onOpenRecordType(row.rec) : undefined}
                      title={clickable ? `查看 ${row.rec} 记录` : undefined}
                    >
                      {row.field}
                    </td>
                    <td className={`${TD} ${MONO}`}>{displayValue(row.field, row.value)}</td>
                    <td className={TD}>{row.meaning}</td>
                    <td className={`${TD} ${MONO}`}>{row.oneData}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </section>
    </section>
  );
}

// STDF open/close record pairs whose counts should match (一眼校验是否对得上).
const RECORD_PAIRS: Array<[string, string, string]> = [
  ["PIR", "PRR", "器件 开始 / 结束"],
  ["WIR", "WRR", "晶圆 开始 / 结束"],
  ["BPS", "EPS", "程序段 开始 / 结束"],
  ["MIR", "MRR", "文件 主信息 / 主结果"]
];

function PairStats({ groups, complete }: { groups: RecordGroup[]; complete: boolean }) {
  const counts = new Map(groups.map((g) => [g.record_type, g.count]));
  const rows = RECORD_PAIRS.map(([a, b, label]) => ({
    a,
    b,
    label,
    ca: counts.get(a) ?? 0,
    cb: counts.get(b) ?? 0
  })).filter((r) => r.ca > 0 || r.cb > 0);
  if (rows.length === 0) return null;
  return (
    <section className="shrink-0 rounded-xl border border-border bg-card px-4 py-3">
      <div className="mb-2 flex items-center justify-between gap-3">
        <span className={EYEBROW}>配对统计</span>
        {!complete && <span className="text-xs text-muted-foreground">解析完成后校验…</span>}
      </div>
      {complete ? (
        <div className="grid grid-cols-4 gap-2">
          {rows.map((r) => {
            const ok = r.ca === r.cb;
            const Icon = ok ? CheckCircle2 : AlertCircle;
            return (
              <div key={r.a} className="min-w-0 rounded-lg border border-border bg-muted px-2.5 py-1.5">
                <div className="flex items-center justify-between gap-1.5">
                  <span className="text-[13px] font-medium text-foreground">
                    {r.a} ↔ {r.b}
                  </span>
                  <span
                    className={`inline-flex items-center gap-0.5 whitespace-nowrap rounded-full px-1.5 text-[11px] font-medium ${
                      ok ? "bg-success-soft text-success" : "bg-danger-soft text-danger"
                    }`}
                  >
                    <Icon size={11} aria-hidden="true" />
                    {ok ? "匹配" : "不匹配"}
                  </span>
                </div>
                <div className="mt-0.5 flex items-center justify-between gap-2 text-xs text-muted-foreground">
                  <span className="truncate">{r.label}</span>
                  <span className="font-mono tabular-nums">
                    {r.ca.toLocaleString()} / {r.cb.toLocaleString()}
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      ) : (
        <p className="text-[13px] text-muted-foreground">等待文件解析完成后再校验成对记录数量。</p>
      )}
    </section>
  );
}

function RecordsView({
  groups,
  selectedGroup,
  onSelectGroup,
  selectedRecord,
  fields,
  cursor,
  recordTotal,
  onCursorChange
}: {
  groups: RecordGroup[];
  selectedGroup: string;
  onSelectGroup(value: string): void;
  selectedRecord: RecordSummary | null;
  fields: RecordField[];
  cursor: number;
  recordTotal: number;
  onCursorChange(index: number): void;
}) {
  return (
    <section
      className="grid min-h-0 flex-1 grid-cols-[minmax(180px,220px)_minmax(360px,1fr)] overflow-hidden"
      aria-label="工作台"
    >
      <aside className="flex min-h-0 flex-col overflow-hidden border-r border-border bg-muted px-2.5 py-3" aria-label="Record 类型">
        <div className="flex items-center justify-between px-1.5 pb-2">
          <span className={EYEBROW}>Record Types</span>
          <span className="text-xs tabular-nums text-muted-foreground">{groups.length.toLocaleString()}</span>
        </div>
        <nav className="flex min-h-0 flex-1 flex-col gap-1 overflow-auto pr-0.5">
          {groups.map((group) => {
            const active = group.record_type === selectedGroup;
            return (
              <button
                key={group.record_type}
                type="button"
                aria-pressed={active}
                aria-label={`${group.record_type} ${group.count} 条记录`}
                title={RECORD_TYPE_INFO[group.record_type] ?? group.record_type}
                onClick={() => onSelectGroup(group.record_type)}
                className={`flex min-h-[34px] w-full items-center justify-between gap-2 rounded-md border px-2.5 text-[13px] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
                  active
                    ? "border-primary/30 bg-primary-soft font-semibold text-primary"
                    : "border-transparent text-muted-foreground hover:bg-border/40 hover:text-foreground"
                }`}
              >
                <span className="truncate">{group.record_type}</span>
                <small className="shrink-0 text-[11px] tabular-nums opacity-80">{group.count.toLocaleString()}</small>
              </button>
            );
          })}
        </nav>
      </aside>
      <FieldDetailPanel
        selectedRecord={selectedRecord}
        fields={fields}
        cursor={cursor}
        recordTotal={recordTotal}
        onCursorChange={onCursorChange}
      />
    </section>
  );
}

function FieldDetailPanel({
  selectedRecord,
  fields,
  cursor,
  recordTotal,
  onCursorChange
}: {
  selectedRecord: RecordSummary | null;
  fields: RecordField[];
  cursor: number;
  recordTotal: number;
  onCursorChange(index: number): void;
}) {
  const hasPager = recordTotal > 1;
  const [draft, setDraft] = useState(String(cursor + 1));
  useEffect(() => {
    setDraft(String(cursor + 1));
  }, [cursor]);
  const commitJump = () => {
    const parsed = Number.parseInt(draft, 10);
    if (Number.isNaN(parsed)) {
      setDraft(String(cursor + 1));
      return;
    }
    onCursorChange(Math.min(Math.max(parsed, 1), recordTotal) - 1);
  };
  return (
    <section className="flex min-h-0 flex-col gap-3 overflow-hidden bg-card p-4" aria-label="字段详情">
      <div className="flex shrink-0 items-start justify-between gap-3 border-b border-border pb-3">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className={EYEBROW}>字段详情</span>
          <strong className="block text-sm font-semibold text-foreground">
            {selectedRecord ? selectedRecord.record_type : "未选择记录"}
          </strong>
          {selectedRecord && RECORD_TYPE_INFO[selectedRecord.record_type] && (
            <span className="text-xs text-muted-foreground">
              {RECORD_TYPE_INFO[selectedRecord.record_type]}
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {hasPager && (
            <div className="flex items-center gap-1.5 text-xs text-muted-foreground" title="← / → 方向键可翻页">
              <button
                type="button"
                className={PAGER_BTN}
                disabled={cursor <= 0}
                onClick={() => onCursorChange(cursor - 1)}
                aria-label="上一条记录"
              >
                <ChevronLeft size={14} />
                上一条
              </button>
              <span className="flex items-center gap-1">
                <input
                  type="number"
                  min={1}
                  max={recordTotal}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") commitJump();
                  }}
                  onBlur={commitJump}
                  aria-label="跳转到第几条记录"
                  className="h-7 w-16 rounded-md border border-border-strong bg-card px-2 text-center text-xs tabular-nums text-foreground outline-none focus:border-primary focus:ring-2 focus:ring-ring"
                />
                <span className="tabular-nums">/ {recordTotal.toLocaleString()}</span>
              </span>
              <button
                type="button"
                className={PAGER_BTN}
                disabled={cursor >= recordTotal - 1}
                onClick={() => onCursorChange(cursor + 1)}
                aria-label="下一条记录"
              >
                下一条
                <ChevronRight size={14} />
              </button>
            </div>
          )}
          {selectedRecord && (
            <div className="group relative shrink-0">
              <div
                className={`inline-flex min-h-[28px] cursor-help items-center gap-1.5 whitespace-nowrap rounded-full border px-2.5 text-xs font-medium ${RECORD_STATUS_TONE[selectedRecord.status]}`}
              >
                <CircleDot size={14} aria-hidden="true" />
                <span>{formatRecordStatus(selectedRecord.status)}</span>
              </div>
              <div
                role="tooltip"
                className="pointer-events-none absolute right-0 top-full z-20 mt-1.5 hidden w-72 rounded-lg border border-border bg-card p-2.5 text-xs leading-relaxed text-foreground shadow-lg group-hover:block"
              >
                {recordStatusHint(selectedRecord.status)}
              </div>
            </div>
          )}
        </div>
      </div>
      {selectedRecord ? (
        <FieldsTable fields={fields} />
      ) : (
        <EmptyState
          title="未选择 record"
          body="从左侧选择 record type 后，这里会显示字段名、值和中文说明，可用上一条 / 下一条切换记录。"
        />
      )}
    </section>
  );
}

function SearchView({
  query,
  setQuery,
  searchResults,
  searchTotal,
  searching,
  searchProgress,
  parseComplete
}: {
  query: string;
  setQuery(value: string): void;
  searchResults: SearchResult[];
  searchTotal: number;
  searching: boolean;
  searchProgress: { scanned: number; total: number } | null;
  parseComplete: boolean;
}) {
  const trimmed = query.trim();
  const pct =
    searchProgress && searchProgress.total > 0
      ? Math.min(100, Math.round((searchProgress.scanned / searchProgress.total) * 100))
      : null;
  return (
    <section className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden p-[18px]" aria-label="搜索">
      <div className="flex items-center justify-between gap-3">
        <div>
          <span className={EYEBROW}>Search</span>
          <strong className="block text-sm font-semibold text-foreground">
            {searching
              ? pct != null
                ? `搜索中… ${pct}%`
                : "搜索中…"
              : trimmed.length >= 2
                ? `${searchTotal.toLocaleString()} 个结果`
                : "全文搜索"}
          </strong>
        </div>
      </div>
      <label className="flex min-h-[42px] items-center gap-2 rounded-lg border border-border-strong bg-card px-3 text-muted-foreground focus-within:border-primary focus-within:ring-2 focus-within:ring-ring">
        {searching ? (
          <Loader2 size={18} className="animate-spin" aria-hidden="true" />
        ) : (
          <Search size={18} aria-hidden="true" />
        )}
        <input
          className="w-full min-w-0 border-0 bg-transparent text-foreground outline-none placeholder:text-muted-foreground/70"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="搜索 record type / 字段名 / 字段值（至少 2 个字符）"
        />
      </label>
      {searching && (
        <div className="flex flex-col gap-1.5">
          <div
            className="h-1.5 w-full overflow-hidden rounded-full bg-muted"
            role="progressbar"
            aria-valuenow={pct ?? undefined}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <div
              className={`h-full bg-primary transition-[width] duration-150 ${
                pct == null ? "w-1/3 animate-pulse" : ""
              }`}
              style={pct != null ? { width: `${pct}%` } : undefined}
            />
          </div>
          {searchProgress && searchProgress.total > 0 && (
            <span className="text-right text-[11px] tabular-nums text-muted-foreground">
              {searchProgress.scanned.toLocaleString()} / {searchProgress.total.toLocaleString()} 条记录
            </span>
          )}
        </div>
      )}
      {!parseComplete ? (
        <EmptyState
          title="等待解析完成后搜索"
          body="为了保持摘要和字段浏览不卡顿，解析过程中暂停全量搜索；文件解析完成后即可搜索全部字段。"
        />
      ) : trimmed.length < 2 ? (
        <EmptyState
          title="输入至少 2 个字符开始搜索"
          body="可搜索 record type、字段名或字段值。大文件为全量搜索，输入后会稍等片刻再出结果。"
        />
      ) : searching ? (
        <EmptyState title="搜索中…" body="正在全量检索，较大文件需要几秒，请稍候。" />
      ) : (
        <SearchResultsTable results={searchResults} />
      )}
    </section>
  );
}

const TI_FILTER_INPUT =
  "h-8 w-[180px] rounded-md border border-border-strong bg-card px-2.5 text-xs text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

function ColumnPager({
  page,
  size,
  total,
  onPageChange,
  onSizeChange
}: {
  page: number;
  size: number;
  total: number;
  onPageChange: (page: number) => void;
  onSizeChange: (size: number) => void;
}) {
  const totalPages = Math.max(1, Math.ceil(total / size));
  const start = total === 0 ? 0 : page * size + 1;
  const end = Math.min((page + 1) * size, total);
  return (
    <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5 text-xs text-muted-foreground">
      <span className="font-medium text-foreground">测试项</span>
      <span className="flex items-center gap-1.5">
        每页
        <span className="inline-flex overflow-hidden rounded-md border border-border-strong">
          {TI_COL_SIZE_OPTIONS.map((option, index) => (
            <button
              key={option}
              type="button"
              onClick={() => onSizeChange(option)}
              className={`h-7 px-2.5 text-xs tabular-nums transition ${
                index > 0 ? "border-l border-border-strong" : ""
              } ${
                size === option
                  ? "bg-primary-soft text-primary"
                  : "bg-card text-muted-foreground hover:bg-muted hover:text-foreground"
              }`}
            >
              {option}
            </button>
          ))}
        </span>
      </span>
      <button type="button" className={PAGER_BTN} disabled={page <= 0} onClick={() => onPageChange(page - 1)}>
        <ChevronLeft size={14} />
      </button>
      <span className="flex items-center gap-1">
        第
        <input
          type="number"
          min={1}
          max={totalPages}
          value={page + 1}
          onChange={(event) => {
            const next = Number(event.target.value);
            if (Number.isFinite(next)) {
              onPageChange(Math.min(Math.max(0, Math.trunc(next) - 1), totalPages - 1));
            }
          }}
          className="h-7 w-[60px] rounded-md border border-border-strong bg-card px-2 text-xs tabular-nums text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label="跳到测试项页"
        />
        / {totalPages.toLocaleString()} 页
      </span>
      <button
        type="button"
        className={PAGER_BTN}
        disabled={page >= totalPages - 1}
        onClick={() => onPageChange(page + 1)}
      >
        <ChevronRight size={14} />
      </button>
      <span className="tabular-nums">
        {start.toLocaleString()}–{end.toLocaleString()} / {total.toLocaleString()} 项
      </span>
    </div>
  );
}

// Renders a bin column value: number + (name) + colored pass/fail flag.
// Colored pass/fail letter for the bin PF columns ("-" when the file omits it).
function pfCell(pf: string): ReactNode {
  if (!pf) {
    return <span className="text-muted-foreground">-</span>;
  }
  const tone = pf === "F" ? "text-danger" : pf === "P" ? "text-success" : "text-muted-foreground";
  return <span className={`font-semibold ${tone}`}>{pf}</span>;
}

// Per-part info columns shown on the left (no longer frozen — the whole table
// scrolls freely). They auto-fit their content; only the text columns are capped.
// Bin number / name / PF each get their own column so they are always present —
// empty cells when the file's HBR/SBR don't carry that field.
type LeftCol = {
  key: string;
  label: string;
  get: (row: TestItemPartRow) => string;
  title?: (row: TestItemPartRow) => string | undefined;
  render?: (row: TestItemPartRow) => ReactNode;
  maxWidth?: number;
};

const LEFT_COLS: LeftCol[] = [
  { key: "part_id", label: "PartID", get: (r) => r.part_id || "-", title: (r) => r.part_id || undefined },
  { key: "site", label: "Site", get: (r) => r.site_num || "-" },
  { key: "sbin_num", label: "SBIN#", get: (r) => r.sbin_num || "-" },
  { key: "sbin_name", label: "SBIN Name", get: (r) => r.sbin_name || "-", title: (r) => r.sbin_name || undefined, maxWidth: 160 },
  { key: "sbin_pf", label: "SBIN PF", get: (r) => r.sbin_pf || "-", render: (r) => pfCell(r.sbin_pf) },
  { key: "hbin_num", label: "HBIN#", get: (r) => r.hbin_num || "-" },
  { key: "hbin_name", label: "HBIN Name", get: (r) => r.hbin_name || "-", title: (r) => r.hbin_name || undefined, maxWidth: 160 },
  { key: "hbin_pf", label: "HBIN PF", get: (r) => r.hbin_pf || "-", render: (r) => pfCell(r.hbin_pf) },
  { key: "test_t", label: "TEST_T", get: (r) => r.test_t || "-" },
  { key: "part_txt", label: "PART_TXT", get: (r) => r.part_txt || "-", title: (r) => r.part_txt || undefined, maxWidth: 220 }
];

// The test-item header is transposed into one row per metadata field, so each of
// Test Type / Num / Name / Low / High / Unit becomes its own row across all columns.
const META_ROWS: { key: string; label: string; value: (column: TestItemColumn) => string; mono?: boolean }[] = [
  { key: "type", label: "Test Type", value: (c) => c.record_type },
  { key: "num", label: "Test Num", value: (c) => String(c.test_num), mono: true },
  { key: "name", label: "Test Name", value: (c) => c.test_name || "-" },
  { key: "low", label: "Low Limit", value: (c) => c.low_limit || "-", mono: true },
  { key: "high", label: "High Limit", value: (c) => c.high_limit || "-", mono: true },
  { key: "unit", label: "Unit", value: (c) => (c.record_type === "FTR" ? "P/F" : c.unit || "-") }
];

// Non-sticky header cell tokens (borders come from the table-level selectors).
const HDR_LABEL = "px-2.5 py-1.5 text-right align-middle text-[11px] font-semibold text-muted-foreground bg-muted";
const HDR_VALUE = "px-2 py-1 align-middle text-[11px] text-foreground bg-card";
const HDR_COL = "px-2.5 py-1.5 text-left align-middle text-[11px] font-semibold text-foreground bg-muted";
const TEST_COL_WIDTH = 120;

function TestItemsView({
  session,
  loaded,
  columns,
  rows,
  colTotal,
  rowTotal,
  pmrCount,
  colPage,
  colSize,
  selectedCount,
  loadingMore,
  hasMore,
  hasBinPf,
  exporting,
  exported,
  onExport,
  onColPageChange,
  onColSizeChange,
  onOpenFilter,
  onLoadMore
}: {
  session: ParseSession;
  loaded: boolean;
  columns: TestItemColumn[];
  rows: TestItemPartRow[];
  colTotal: number;
  rowTotal: number;
  pmrCount: number;
  colPage: number;
  colSize: number;
  selectedCount: number;
  loadingMore: boolean;
  hasMore: boolean;
  hasBinPf: boolean;
  exporting: boolean;
  exported: boolean;
  onExport: () => void;
  onColPageChange: (page: number) => void;
  onColSizeChange: (size: number) => void;
  onOpenFilter: () => void;
  onLoadMore: () => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el || !hasMore || loadingMore) return;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 320) {
      onLoadMore();
    }
  };

  return (
    <section className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden p-[18px]" aria-label="测试项">
      <div className="flex items-center justify-between gap-3">
        <div>
          <span className={EYEBROW}>Test Items</span>
          <strong className="block text-sm font-semibold text-foreground">
            {loaded
              ? `共 ${colTotal.toLocaleString()} 个测试项 · ${rowTotal.toLocaleString()} 个 Part/Site 行`
              : session.status === "complete"
                ? "加载测试项…"
                : "等待解析完成"}
          </strong>
        </div>
      </div>
      {!loaded ? (
        <EmptyState
          title={session.status === "complete" ? "加载测试项视图…" : "等待解析完成"}
          body="测试项页只在解析完成后开放，用于汇总 PART_ID、SITE、bin 名称/PF 以及换算后的 PTR / FTR / MPR 测试列。"
        />
      ) : (
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-border bg-card">
          {/* Filters */}
          <div className="flex flex-wrap items-center gap-3 border-b border-border bg-muted px-3 py-2">
            <button
              type="button"
              onClick={onOpenFilter}
              className="inline-flex h-8 items-center gap-1.5 rounded-md border border-border-strong bg-card px-3 text-xs font-medium text-foreground transition hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <Filter size={13} />
              筛选测试项
              <span className="text-muted-foreground">{selectedCount === 0 ? "全部" : `已选 ${selectedCount.toLocaleString()}`}</span>
            </button>
            <button
              type="button"
              onClick={onExport}
              disabled={exporting || rowTotal === 0}
              className="inline-flex h-8 items-center gap-1.5 rounded-md border border-border-strong bg-card px-3 text-xs font-medium text-foreground transition hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
            >
              {exporting ? <Loader2 size={13} className="animate-spin" /> : <Download size={13} />}
              {exporting ? "导出中…" : exported ? "已导出 ✓" : "导出 CSV"}
            </button>
            <span className="ml-auto rounded-full border border-border bg-card px-2.5 py-1 text-xs text-muted-foreground">
              {pmrCount.toLocaleString()} PMR
            </span>
          </div>
          {/* Column pager + row counter */}
          <div className="flex flex-wrap items-center justify-between gap-x-5 gap-y-2 border-b border-border bg-card px-3 py-2">
            <ColumnPager
              page={colPage}
              size={colSize}
              total={colTotal}
              onPageChange={onColPageChange}
              onSizeChange={onColSizeChange}
            />
            <span className="flex items-center gap-1.5 text-xs tabular-nums text-muted-foreground">
              {loadingMore && <Loader2 size={13} className="animate-spin" />}
              已加载 {rows.length.toLocaleString()} / {rowTotal.toLocaleString()} 行
            </span>
          </div>
          {!hasBinPf && (
            <div className="flex items-start gap-1.5 border-b border-warning-border bg-warning-soft px-3 py-1.5 text-[12px] text-warning">
              <AlertCircle size={13} className="mt-0.5 shrink-0" />
              <span>
                本文件的 bin 记录未包含通过/失败标记(PF)，SBIN PF / HBIN PF 列为空；导出的 PASSFG 与良率按「软 bin 1 = 通过」约定判定。
              </span>
            </div>
          )}
          {rowTotal === 0 || colTotal === 0 ? (
            <div className="px-3 py-6 text-[13px] text-muted-foreground">没有匹配筛选条件的测试项或 PART。</div>
          ) : (
            <div ref={scrollRef} onScroll={handleScroll} className="min-h-0 overflow-auto">
              <table
                className="w-max border-separate border-spacing-0 text-[13px] [&_td]:border-b [&_td]:border-r [&_td]:border-border/80 [&_th]:border-b [&_th]:border-r [&_th]:border-border [&_tbody_tr:hover]:bg-muted/30"
                aria-label="测试项矩阵"
              >
                <thead>
                  {/* One row per test-item metadata field (Type / Num / Name / Low / High / Unit). */}
                  {META_ROWS.map((meta) => (
                    <tr key={meta.key}>
                      <th colSpan={LEFT_COLS.length} className={HDR_LABEL}>
                        {meta.label}
                      </th>
                      {columns.map((column) => {
                        const value = meta.value(column);
                        // Test names are long; let that row wrap so the full name shows
                        // instead of being clipped. The other rows stay single-line.
                        const isName = meta.key === "name";
                        return (
                          <th
                            key={`${meta.key}:${column.record_type}:${column.test_num}`}
                            className={`${HDR_VALUE} font-normal ${isName ? "text-left" : "text-center"} ${
                              meta.key === "type" ? "text-primary" : ""
                            }`}
                            style={{ width: TEST_COL_WIDTH, minWidth: TEST_COL_WIDTH, maxWidth: TEST_COL_WIDTH }}
                          >
                            <span
                              className={`${
                                isName
                                  ? "line-clamp-2 [overflow-wrap:anywhere]"
                                  : "block truncate"
                              } ${meta.mono ? "font-mono tabular-nums" : ""}`}
                              title={value}
                            >
                              {value}
                            </span>
                          </th>
                        );
                      })}
                    </tr>
                  ))}
                  {/* Header row for the per-part info columns. */}
                  <tr>
                    {LEFT_COLS.map((col) => (
                      <th
                        key={col.key}
                        className={`${HDR_COL} whitespace-nowrap`}
                        style={col.maxWidth ? { maxWidth: col.maxWidth } : undefined}
                      >
                        {col.label}
                      </th>
                    ))}
                    {columns.length > 0 && <th colSpan={columns.length} className="bg-muted" />}
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row) => (
                    <tr key={`${row.part_id}:${row.site_num}`}>
                      {LEFT_COLS.map((col) => (
                        <td
                          key={col.key}
                          className={`${TD} ${MONO} align-middle whitespace-nowrap`}
                          style={col.maxWidth ? { maxWidth: col.maxWidth } : undefined}
                          title={col.title?.(row)}
                        >
                          {col.render ? (
                            col.render(row)
                          ) : (
                            <span className={`block ${col.maxWidth ? "truncate" : ""}`}>{col.get(row)}</span>
                          )}
                        </td>
                      ))}
                      {columns.map((column, index) => {
                        const cell = row.results[index];
                        const status = cell?.status;
                        // FTR carries a pass/fail flag rather than a measured value,
                        // so show the verdict; PTR/MPR show the scaled result(s).
                        const display =
                          column.record_type === "FTR"
                            ? status || "-"
                            : cell?.value || status || "-";
                        const textTone =
                          status === "F"
                            ? "text-danger font-semibold"
                            : status === "P"
                              ? "text-success"
                              : "text-muted-foreground";
                        return (
                          <td
                            key={`${row.part_id}:${row.site_num}:${column.record_type}:${column.test_num}`}
                            className={`${TD} align-middle text-center ${status === "F" ? "bg-danger-soft" : ""}`}
                            style={{ width: TEST_COL_WIDTH, minWidth: TEST_COL_WIDTH, maxWidth: TEST_COL_WIDTH }}
                          >
                            <span
                              title={cell?.value || undefined}
                              className={`block w-full truncate text-center tabular-nums ${textTone}`}
                            >
                              {display}
                            </span>
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                </tbody>
              </table>
              {hasMore && (
                <div className="px-3 py-3 text-center text-xs text-muted-foreground">
                  {loadingMore ? "加载中…" : "继续下滑加载更多 part"}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

// Modal multi-select for choosing which test-item columns to show. Fuzzy search
// narrows the list; select-all/clear act on the matches; the selection applies on
// confirm. An empty result ([] passed up) means "show all".
const FILTER_DISPLAY_CAP = 500;

function TestItemFilterDialog({
  columns,
  loading,
  applied,
  onClose,
  onConfirm
}: {
  columns: TestItemColumnLite[];
  loading: boolean;
  applied: string[];
  onClose: () => void;
  onConfirm: (selected: string[]) => void;
}) {
  const allKeys = useMemo(() => columns.map((c) => c.key), [columns]);
  const [draft, setDraft] = useState<Set<string>>(() => new Set(applied.length === 0 ? allKeys : applied));
  const [query, setQuery] = useState("");
  const [displayLimit, setDisplayLimit] = useState(FILTER_DISPLAY_CAP);

  // Re-seed the draft once the column list arrives (it may load after the dialog opens).
  useEffect(() => {
    setDraft(new Set(applied.length === 0 ? allKeys : applied));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [allKeys]);

  const needle = query.trim().toLowerCase();
  const filtered = useMemo(() => {
    if (!needle) return columns;
    return columns.filter(
      (c) =>
        c.test_name.toLowerCase().includes(needle) ||
        String(c.test_num).includes(needle) ||
        c.record_type.toLowerCase().includes(needle)
    );
  }, [columns, needle]);
  const displayed = filtered.slice(0, displayLimit);

  const toggle = (key: string) =>
    setDraft((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  const selectMatches = () =>
    setDraft((prev) => {
      const next = new Set(prev);
      filtered.forEach((c) => next.add(c.key));
      return next;
    });
  const clearMatches = () =>
    setDraft((prev) => {
      const next = new Set(prev);
      filtered.forEach((c) => next.delete(c.key));
      return next;
    });

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
      onClick={onClose}
      role="presentation"
    >
      <div
        className="flex max-h-[80vh] w-[560px] max-w-full flex-col overflow-hidden rounded-xl border border-border bg-card shadow-xl"
        onClick={(event) => event.stopPropagation()}
        role="dialog"
        aria-label="筛选测试项"
      >
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <strong className="text-sm font-semibold text-foreground">筛选测试项</strong>
          <button type="button" className={PAGER_BTN} onClick={onClose} aria-label="关闭">
            <X size={14} />
          </button>
        </div>
        <div className="border-b border-border px-4 py-2.5">
          <input
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setDisplayLimit(FILTER_DISPLAY_CAP);
            }}
            placeholder="模糊匹配：编号 / 名称 / 类型"
            className={`${TI_FILTER_INPUT} w-full`}
            aria-label="模糊匹配测试项"
            autoFocus
          />
          <div className="mt-2 flex items-center justify-between text-xs text-muted-foreground">
            <span className="tabular-nums">
              已选 {draft.size.toLocaleString()} / {columns.length.toLocaleString()}
            </span>
            <span className="flex gap-3">
              <button type="button" className="text-primary hover:underline" onClick={selectMatches}>
                全选{needle ? "（匹配）" : ""}
              </button>
              <button type="button" className="text-primary hover:underline" onClick={clearMatches}>
                取消全选{needle ? "（匹配）" : ""}
              </button>
            </span>
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto px-2 py-1">
          {loading ? (
            <div className="px-2 py-8 text-center text-xs text-muted-foreground">加载测试项列表…</div>
          ) : displayed.length === 0 ? (
            <div className="px-2 py-8 text-center text-xs text-muted-foreground">没有匹配的测试项。</div>
          ) : (
            displayed.map((column) => (
              <label
                key={column.key}
                className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1 text-[13px] hover:bg-muted"
              >
                <input
                  type="checkbox"
                  checked={draft.has(column.key)}
                  onChange={() => toggle(column.key)}
                  className="accent-primary"
                />
                <span className="font-mono text-[11px] text-primary">{column.record_type}</span>
                <span className="font-mono text-xs text-foreground">{column.test_num}</span>
                <span className="truncate text-muted-foreground" title={column.test_name || undefined}>
                  {column.test_name || "-"}
                </span>
              </label>
            ))
          )}
          {filtered.length > displayLimit && (
            <div className="px-2 py-2 text-center">
              <button
                type="button"
                onClick={() => setDisplayLimit((limit) => limit + FILTER_DISPLAY_CAP)}
                className="rounded-md border border-border-strong bg-card px-3 py-1 text-[11px] text-muted-foreground transition hover:bg-muted hover:text-foreground"
              >
                继续加载 {FILTER_DISPLAY_CAP} 个（剩余 {(filtered.length - displayLimit).toLocaleString()}）
              </button>
            </div>
          )}
        </div>
        <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
          <button type="button" className={BTN_SECONDARY} onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className={BTN_PRIMARY}
            disabled={draft.size === 0}
            onClick={() => onConfirm(draft.size === columns.length ? [] : Array.from(draft))}
          >
            确认
          </button>
        </div>
      </div>
    </div>
  );
}

function SearchResultsTable({ results }: { results: SearchResult[] }) {
  if (results.length === 0) {
    return <EmptyState title="没有搜索结果" body="尝试搜索 record type、字段名或字段值。" />;
  }

  return (
    <div className={TABLE_SCROLL}>
      <table className={DATA_TABLE} aria-label="搜索结果">
        <thead>
          <tr>
            <th className={TH}>Record</th>
            <th className={TH}>Field</th>
            <th className={TH}>Value</th>
            <th className={TH}>中文说明</th>
          </tr>
        </thead>
        <tbody>
          {results.map((result) => (
            <tr key={`${result.record.id}:${result.field.name}`}>
              <td className={TD}>{result.record.record_type}</td>
              <td className={`${TD} ${MONO}`}>{result.field.name}</td>
              <td className={`${TD} ${MONO}`}>{displayValue(result.field.name, result.field.value)}</td>
              <td className={TD}>{result.field.description || "未提供"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function FieldsTable({ fields }: { fields: RecordField[] }) {
  if (fields.length === 0) {
    return (
      <EmptyState
        title="该 record 无数据字段"
        body="EPS（程序段结束标记）等 record 按 STDF V4 规范本身不含数据字段，属正常情况，并非仍在加载。"
      />
    );
  }

  return (
    <div className={TABLE_SCROLL}>
      <table className={DATA_TABLE} aria-label="字段详情表">
        <thead>
          <tr>
            <th className={`${TH} w-[26%]`}>字段名</th>
            <th className={`${TH} w-[30%]`}>值</th>
            <th className={`${TH} w-[44%]`}>中文说明</th>
          </tr>
        </thead>
        <tbody>
          {fields.map((field) => (
            <tr key={field.name}>
              <td className={`${TD} ${MONO}`}>{field.name}</td>
              <td className={`${TD} ${MONO}`}>{displayValue(field.name, field.value)}</td>
              <td className={TD}>{field.description || "未提供"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function EmptyState({ title, body }: { title: string; body: string }) {
  return (
    <div className="flex min-h-[160px] flex-1 flex-col items-center justify-center rounded-xl border border-dashed border-border-strong p-6 text-center text-muted-foreground">
      <strong className="text-foreground">{title}</strong>
      <p className="mt-1.5 max-w-[320px]">{body}</p>
    </div>
  );
}

function getStatusView(status: ParseSession["status"] | null, progressPercent: number) {
  void progressPercent;
  if (status === "complete") {
    return { label: "解析完成", tone: "complete", icon: CheckCircle2, spin: false };
  }
  if (status === "cancelled") {
    return { label: "已取消", tone: "cancelled", icon: XCircle, spin: false };
  }
  if (status === "error") {
    return { label: "解析错误", tone: "error", icon: AlertCircle, spin: false };
  }
  return { label: "解析中", tone: "running", icon: Loader2, spin: true };
}

// STDF U*4 epoch-second timestamp fields — shown as human-readable local time.
const TIME_FIELDS = new Set(["SETUP_T", "START_T", "FINISH_T", "MOD_TIM"]);

function displayValue(name: string, value: string): string {
  if (!value) return ""; // empty / omitted field → blank, not "空值"
  if (TIME_FIELDS.has(name) && /^\d+$/.test(value)) {
    const seconds = Number(value);
    if (Number.isFinite(seconds) && seconds > 0) {
      const date = new Date(seconds * 1000);
      const pad = (n: number) => String(n).padStart(2, "0");
      return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
    }
  }
  return value;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function formatRecordStatus(status: RecordSummary["status"]) {
  if (status === "parsed") return "已解析";
  if (status === "error") return "解析错误";
  return "未知 record";
}

function recordStatusHint(status: RecordSummary["status"]) {
  if (status === "error") {
    return "解析错误：该 record 的必填字段缺失，或某个变长字段在读取过程中被截断，数据可能不完整。";
  }
  if (status === "unknown") {
    return "未知 record：不在当前支持的 STDF V4 类型表中，仅显示原始 payload 预览。";
  }
  return "已成功解析该 record 的全部字段。";
}
