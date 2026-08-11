import {
  AlertCircle,
  AlertTriangle,
  ArrowLeftRight,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  CircleDot,
  Download,
  Filter,
  FolderOpen,
  Hourglass,
  Inbox,
  LayoutDashboard,
  Loader2,
  Moon,
  MousePointerClick,
  Search,
  SearchX,
  Sun,
  TableProperties,
  Table2,
  X,
  XCircle,
  type LucideIcon
} from "lucide-react";
import {
  type Dispatch,
  type ReactNode,
  type SetStateAction,
  useEffect,
  useMemo,
  useRef,
  useState
} from "react";
import { createPortal } from "react-dom";
import { computeMatrixWindow } from "./matrixWindow";
import { RecordTextInspector, type TextParseState } from "./RecordTextInspector";
import type {
  DtrPreview,
  GdrPreview,
  MprPinDetails,
  FileIssue,
  ParseErrorEvent,
  ParseProgress,
  ParseSession,
  RecordField,
  RecordGroup,
  RecordSummary,
  SearchResult,
  SessionSnapshot,
  TestItemColumn,
  BinSummary,
  TestItemColumnLite,
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
const TEXT_PREVIEW_RECORD_TYPES = new Set(["DTR", "GDR"]);

function isTextPreviewRecordType(recordType: string | undefined): boolean {
  return recordType !== undefined && TEXT_PREVIEW_RECORD_TYPES.has(recordType);
}

/* ------------------------------------------------------------------ *
 * Shared Tailwind class tokens — small component-system style layer.  *
 * ------------------------------------------------------------------ */
const EYEBROW = "text-[11px] font-semibold uppercase tracking-wide text-muted-foreground";

// One page gutter: every top-level view surface (header, banners, view
// sections, the records detail pane) shares the same 20px inset so content
// on every page sits on a single left rhythm line.
const PAGE_PAD = "p-5";

const BTN_BASE =
  "inline-flex min-h-[40px] items-center justify-center gap-2 rounded-md px-3.5 text-sm font-medium transition duration-100 active:scale-[0.98] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";
const BTN_PRIMARY = `${BTN_BASE} bg-primary text-primary-foreground hover:bg-primary-hover`;
const BTN_SECONDARY = `${BTN_BASE} whitespace-nowrap border border-border-strong bg-card text-muted-foreground hover:bg-muted hover:text-foreground`;
const PAGER_BTN =
  "inline-flex h-7 select-none items-center gap-0.5 rounded-md border border-border-strong bg-card px-2 text-xs font-medium text-muted-foreground transition duration-100 hover:bg-muted hover:text-foreground active:scale-95 active:border-primary active:bg-primary-soft active:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";

const RAIL_ITEM =
  "relative flex h-14 w-14 flex-col items-center justify-center gap-1 rounded-xl text-[11px] font-medium transition duration-100 active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
const RAIL_ITEM_IDLE = "text-muted-foreground hover:bg-foreground/5 hover:text-foreground";
const RAIL_ITEM_ACTIVE = "bg-primary-soft text-primary";

// overscroll-contain: reaching an inner table's scroll end must not chain the
// wheel into scrolling the page — the page scrolls only under the cursor's
// own real estate.
const TABLE_SCROLL = "min-h-0 flex-1 overflow-auto overscroll-contain rounded-lg border border-border";
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
  { key: "records", label: "明细", icon: Table2 },
  { key: "test-items", label: "测试项", icon: TableProperties },
  { key: "search", label: "搜索", icon: Search }
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

const IDLE_DTR_PARSE: TextParseState<DtrPreview> = {
  phase: "idle",
  count: 0,
  previews: [],
  message: "",
  saving: false,
  saved: false
};

const IDLE_GDR_PARSE: TextParseState<GdrPreview> = {
  phase: "idle",
  count: 0,
  previews: [],
  message: "",
  saving: false,
  saved: false
};

type TextStateSetter<TPreview> = Dispatch<SetStateAction<TextParseState<TPreview>>>;

async function parseTextRecords<TPreview>({
  sessionId,
  state,
  idleState,
  setState,
  parse,
  isCurrent
}: {
  sessionId: string | undefined;
  state: TextParseState<TPreview>;
  idleState: TextParseState<TPreview>;
  setState: TextStateSetter<TPreview>;
  parse(sessionId: string): Promise<{ count: number; previews: TPreview[] }>;
  isCurrent(sessionId: string): boolean;
}) {
  if (!sessionId || state.phase === "parsing") return;
  setState({ ...idleState, phase: "parsing" });
  try {
    const result = await parse(sessionId);
    if (!isCurrent(sessionId)) return;
    setState({
      ...idleState,
      phase: "done",
      count: result.count,
      previews: result.previews
    });
  } catch (error) {
    if (!isCurrent(sessionId)) return;
    setState({ ...idleState, phase: "error", message: String(error) });
  }
}

async function downloadTextRecords<TPreview>({
  sessionId,
  state,
  setState,
  defaultName,
  selectPath,
  save,
  isCurrent
}: {
  sessionId: string | undefined;
  state: TextParseState<TPreview>;
  setState: TextStateSetter<TPreview>;
  defaultName: string;
  selectPath(defaultName: string): Promise<string | null>;
  save(sessionId: string, path: string): Promise<void>;
  isCurrent(sessionId: string): boolean;
}) {
  if (!sessionId || state.phase !== "done" || state.saving) return;
  const path = await selectPath(defaultName);
  if (!path || !isCurrent(sessionId)) return;
  setState((previous) => ({ ...previous, saving: true, saved: false, message: "" }));
  try {
    await save(sessionId, path);
    if (!isCurrent(sessionId)) return;
    setState((previous) => ({ ...previous, saving: false, saved: true }));
    window.setTimeout(() => {
      if (isCurrent(sessionId)) {
        setState((previous) => ({ ...previous, saved: false }));
      }
    }, 2500);
  } catch (error) {
    if (!isCurrent(sessionId)) return;
    setState((previous) => ({
      ...previous,
      saving: false,
      message: `保存 TXT 失败：${String(error)}`
    }));
  }
}

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
  // Yield / bin distribution for the overview, fetched once parsing completes.
  const [binSummary, setBinSummary] = useState<BinSummary | null>(null);
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
  const [dtrParse, setDtrParse] = useState<TextParseState<DtrPreview>>(IDLE_DTR_PARSE);
  const [gdrParse, setGdrParse] = useState<TextParseState<GdrPreview>>(IDLE_GDR_PARSE);
  const [tiHasBinPf, setTiHasBinPf] = useState(true);
  // Bumped whenever the column window / selection changes, to drop stale "load more" responses.
  const tiEpoch = useRef(0);
  // Serialized [session, colPage, colSize, selection] of the loaded (or in-flight)
  // window — re-entering the tab with an unchanged key skips the refetch.
  const tiFetchKeyRef = useRef("");
  const [nav, setNav] = useState<NavSection>("summary");
  const [progress, setProgress] = useState<ParseProgress | null>(null);
  const [error, setError] = useState("");
  const [warning, setWarning] = useState("");
  const [isDragOver, setDragOver] = useState(false);
  const [theme, setTheme] = useState<Theme>(readInitialTheme);
  const sessionIdRef = useRef<string | null>(null);
  const sessionStatusRef = useRef<ParseSession["status"] | null>(null);
  // Serializes competing file opens (double drop events, rapid re-drops). The
  // backend keeps a single session and evicts every older one on open, so a
  // superseded open chain must go silent — otherwise its doomed follow-up
  // calls reject with "解析会话不存在" and paint a stale error banner over the
  // session that actually won.
  const openSeqRef = useRef(0);

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

  // ⌘O / Ctrl+O opens the file picker from anywhere — mirrors the rail button
  // and backs the shortcut hint on the launch screen.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (
        (event.metaKey || event.ctrlKey) &&
        !event.shiftKey &&
        !event.altKey &&
        event.key.toLowerCase() === "o"
      ) {
        event.preventDefault();
        openAnotherFile();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // openAnotherFile is a hoisted function declaration whose only moving part is `api`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api]);

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
    api.onSessionSnapshot((nextSnapshot) => {
      if (!disposed && sessionIdRef.current === nextSnapshot.session_id) {
        applySnapshot(nextSnapshot);
      }
    }).then((cleanup) => {
      cleanupSnapshot = cleanup;
    });
    api.onNativeFileDrop((path) => {
      // The disposed guard matters here: in dev StrictMode the first mount's
      // listener can outlive its cleanup (registration is async, so cleanup
      // runs before the unlisten fn exists). Without the guard one physical
      // drop fires two open_stdf calls and the second evicts the first's
      // session mid-flight.
      if (!disposed) openSessionFrom(api.openDroppedFile(path));
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
      // Same session guard as warnings: an evicted session's parse thread can
      // still emit errors while it winds down — those belong to a session the
      // UI has already left behind.
      if (!disposed && sessionIdRef.current === event.session_id) setError(event.message);
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
      cleanupSnapshot?.();
      cleanupNativeDrop?.();
      cleanupComplete?.();
      cleanupError?.();
      cleanupWarning?.();
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
    setBinSummary(null);
    tiEpoch.current += 1;
    // Whenever the loaded rows are cleared, the fetch-key cache MUST be cleared
    // with them — otherwise the page-load effect thinks the window is already
    // loaded and the view hangs on the loading state. (Bit us via dev
    // fast-refresh, which re-runs effects with refs preserved.)
    tiFetchKeyRef.current = "";
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
    setDtrParse(IDLE_DTR_PARSE);
    setGdrParse(IDLE_GDR_PARSE);
  }, [session?.session_id]);

  // Fetch the yield/bin distribution once parsing completes.
  useEffect(() => {
    const sessionId = session?.session_id;
    if (!sessionId || session?.status !== "complete") return;
    let active = true;
    api.getBinSummary(sessionId).then((summary) => {
      if (active && sessionIdRef.current === sessionId) setBinSummary(summary);
    });
    return () => {
      active = false;
    };
  }, [api, session?.session_id, session?.status]);

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
      if (event.key === "ArrowLeft" && recordTotal > 1 && !isTextPreviewRecordType(selectedGroup)) {
        event.preventDefault();
        setCursor((current) => Math.max(0, current - 1));
      } else if (event.key === "ArrowRight" && recordTotal > 1 && !isTextPreviewRecordType(selectedGroup)) {
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
  // The fetch key remembers which window is already loaded (or in flight), so
  // merely re-entering the tab reuses the cached rows instead of re-shipping a
  // multi-MB page and remounting the table. A new session/window changes the key.
  useEffect(() => {
    const sessionId = session?.session_id;
    if (!sessionId || nav !== "test-items") {
      return;
    }
    const fetchKey = JSON.stringify([sessionId, tiColPage, tiColSize, tiSelected]);
    if (tiFetchKeyRef.current === fetchKey) {
      return;
    }
    tiFetchKeyRef.current = fetchKey;
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
        setTiPmrCount(page.pmr_count);
        setTiHasBinPf(page.has_bin_pf);
        setTiLoaded(true);
      })
      .catch(() => {
        // Allow a retry on the next tab entry instead of caching the failure.
        if (tiFetchKeyRef.current === fetchKey) {
          tiFetchKeyRef.current = "";
        }
      });
    return () => {
      active = false;
    };
  }, [api, session?.session_id, nav, tiColPage, tiColSize, tiSelected]);

  // Load the full column list (identities only) the first time the filter
  // dialog opens — it can be several MB on big files, so not on tab entry.
  useEffect(() => {
    const sessionId = session?.session_id;
    if (!sessionId || !tiFilterOpen || tiAllColumns.length > 0) {
      return;
    }
    let active = true;
    setTiColumnsLoading(true);
    api
      .getTestItemColumns(sessionId)
      .then((cols) => {
        if (!active) return;
        setTiAllColumns(cols);
        setTiColumnsLoading(false);
      })
      .catch(() => {
        if (active) setTiColumnsLoading(false);
      });
    return () => {
      active = false;
    };
    // tiAllColumns is intentionally checked but not observed: changing it after
    // a successful request must not clean up that same request before loading clears.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, session?.session_id, tiFilterOpen]);

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

  // STDF filename without compression/format extensions — the stem every
  // export (CSV or staged record TXT) derives its default filename from.
  function sessionBaseName() {
    return (session?.file_name || "export")
      .replace(/\.(gz|zip)$/i, "")
      .replace(/\.(stdf|std)$/i, "");
  }

  // Export the full test-item matrix to a CSV the user picks via a save dialog.
  // Default name = STDF filename with a .csv extension; the backend writes the file.
  async function exportTestItemsCsv() {
    const sessionId = session?.session_id;
    if (!sessionId || tiExporting) return;
    const path = await api.saveCsvDialog(`${sessionBaseName()}.csv`);
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

  // GDR/DTR stay out of the initial record index; these actions re-scan the
  // source on demand and stage a context-separated TXT in the backend.
  async function handleParseDtr() {
    await parseTextRecords({
      sessionId: session?.session_id,
      state: dtrParse,
      idleState: IDLE_DTR_PARSE,
      setState: setDtrParse,
      parse: api.parseDtrText,
      isCurrent: (sessionId) => sessionIdRef.current === sessionId
    });
  }

  async function handleDownloadDtr() {
    await downloadTextRecords({
      sessionId: session?.session_id,
      state: dtrParse,
      setState: setDtrParse,
      defaultName: `${sessionBaseName()}_DTR.txt`,
      selectPath: api.saveTxtDialog,
      save: api.saveDtrText,
      isCurrent: (sessionId) => sessionIdRef.current === sessionId
    });
  }

  async function handleParseGdr() {
    await parseTextRecords({
      sessionId: session?.session_id,
      state: gdrParse,
      idleState: IDLE_GDR_PARSE,
      setState: setGdrParse,
      parse: api.parseGdrText,
      isCurrent: (sessionId) => sessionIdRef.current === sessionId
    });
  }

  async function handleDownloadGdr() {
    await downloadTextRecords({
      sessionId: session?.session_id,
      state: gdrParse,
      setState: setGdrParse,
      defaultName: `${sessionBaseName()}_GDR.txt`,
      selectPath: api.saveTxtDialog,
      save: api.saveGdrText,
      isCurrent: (sessionId) => sessionIdRef.current === sessionId
    });
  }

  async function refreshGroups(sessionId: string) {
    const nextGroups = await api.getRecordGroups(sessionId);
    setGroups(nextGroups);
    setSelectedGroup((current) => current || nextGroups[0]?.record_type || "");
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
    setBinSummary(null);
    tiEpoch.current += 1;
    tiFetchKeyRef.current = "";
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
    setDtrParse(IDLE_DTR_PARSE);
    setGdrParse(IDLE_GDR_PARSE);
    const initialSnapshot = await api.getSessionSnapshot(nextSession.session_id);
    // A newer open may have taken over while the snapshot was in flight —
    // its state must not be clobbered with this session's data.
    if (sessionIdRef.current !== nextSession.session_id) return;
    applySnapshot(initialSnapshot);
    void hydrateEarlySnapshot(nextSession.session_id);
  }

  async function hydrateEarlySnapshot(sessionId: string) {
    for (let attempt = 0; attempt < 20; attempt += 1) {
      if (sessionIdRef.current !== sessionId) return;
      await new Promise((resolve) => window.setTimeout(resolve, 100));
      let nextSnapshot: SessionSnapshot;
      try {
        nextSnapshot = await api.getSessionSnapshot(sessionId);
      } catch {
        // Session evicted by a newer open — stop polling quietly.
        return;
      }
      if (sessionIdRef.current !== sessionId) return;
      applySnapshot(nextSnapshot);
      if (Object.keys(nextSnapshot.key_fields).length > 0 || nextSnapshot.status !== "running") {
        return;
      }
    }
  }

  // Every file-open entry point (dialog, native drop, HTML drop) funnels
  // through here so competing opens can't interleave: only the latest one may
  // adopt its session or surface its failure.
  function openSessionFrom(pending: Promise<ParseSession | null>) {
    const seq = ++openSeqRef.current;
    pending
      .then((next) => {
        if (seq === openSeqRef.current) return startSession(next);
      })
      .catch((err) => {
        if (seq === openSeqRef.current) setError(String(err));
      });
  }

  function openAnotherFile() {
    openSessionFrom(api.openFile());
  }

  function handleDrop(event: React.DragEvent<HTMLDivElement>) {
    event.preventDefault();
    setDragOver(false);
    const file = event.dataTransfer.files[0];
    const path = "path" in file ? String((file as File & { path?: string }).path) : "";
    if (path) {
      openSessionFrom(api.openDroppedFile(path));
    }
  }

  return (
    // Inset-canvas shell: the nav rail sits directly on the deeper window
    // tone, and the whole content area floats on it as one elevated panel.
    <div className="flex h-dvh min-h-[720px] overflow-hidden bg-shell">
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
        className="my-2 mr-2 flex min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-border/60 bg-background shadow-card"
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
            />
            {error && (
              <div
                className="flex items-center gap-2 border-b border-danger-border bg-danger-soft px-5 py-2.5 text-sm text-danger"
                role="alert"
              >
                <AlertCircle size={16} className="shrink-0" />
                <span className="min-w-0 flex-1">{error}</span>
                <button
                  type="button"
                  aria-label="关闭错误提示"
                  onClick={() => setError("")}
                  className="shrink-0 rounded-md p-1 transition-colors hover:bg-danger/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <X size={14} aria-hidden="true" />
                </button>
              </div>
            )}
            {warning && (
              <div
                className="flex items-start gap-2 border-b border-warning-border bg-warning-soft px-5 py-2.5 text-sm text-warning"
                role="status"
              >
                <AlertCircle size={16} className="mt-0.5 shrink-0" />
                <span className="min-w-0 flex-1">{warning}</span>
                <button
                  type="button"
                  aria-label="关闭警告提示"
                  onClick={() => setWarning("")}
                  className="shrink-0 rounded-md p-1 transition-colors hover:bg-warning/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <X size={14} aria-hidden="true" />
                </button>
              </div>
            )}
            {nav === "summary" && (
              <OverviewView
                session={session}
                keyFields={keyFields}
                groups={groups}
                binSummary={binSummary}
                issues={snapshot?.issues ?? []}
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
                onJumpToRecord={(recordType, position) => {
                  setSelectedGroup(recordType);
                  setCursor(position);
                  setNav("records");
                }}
                fetchMprPins={(testNum, position) =>
                  api.getMprPinDetails(session.session_id, testNum, position)
                }
                exportMprPins={async (testNum, position, partId, siteNum) => {
                  // Default name mirrors the CSV export convention; part id may
                  // carry characters macOS filenames reject, so sanitize it.
                  const safePart = (partId || "part").replace(/[^\w.-]+/g, "_");
                  const path = await api.saveXlsxDialog(
                    `${sessionBaseName()}_MPR${testNum}_${safePart}_pins.xlsx`
                  );
                  if (!path) return false;
                  await api.exportMprPinsXlsx(
                    session.session_id,
                    testNum,
                    position,
                    partId,
                    siteNum,
                    path
                  );
                  return true;
                }}
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
                dtrParse={dtrParse}
                gdrParse={gdrParse}
                onParseDtr={handleParseDtr}
                onDownloadDtr={handleDownloadDtr}
                onParseGdr={handleParseGdr}
                onDownloadGdr={handleDownloadGdr}
                parseComplete={session.status === "complete"}
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
            onOpen={openAnotherFile}
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

// Code-drawn wafer bin map for the launch screen: die tone falls off from the
// wafer center and a few fail dies are scattered in, so it reads as test data
// rather than decoration. Built from theme tokens — light/dark and any zoom
// level need no separate assets.
function WaferMark({ className }: { className?: string }) {
  const dies: ReactNode[] = [];
  const fails = new Set(["2:7", "5:2", "8:9", "9:4"]);
  for (let row = 0; row < 12; row += 1) {
    for (let col = 0; col < 12; col += 1) {
      const x = 15 + col * 10;
      const y = 15 + row * 10;
      const dist = Math.hypot(x - 70, y - 70);
      if (dist > 53) continue;
      const fail = fails.has(`${row}:${col}`);
      const opacity = fail ? 0.62 : dist < 20 ? 0.68 : dist < 36 ? 0.42 : 0.2;
      dies.push(
        <rect
          key={`${row}:${col}`}
          x={x - 4.3}
          y={y - 4.3}
          width={8.6}
          height={8.6}
          rx={1.5}
          fill={fail ? "var(--danger)" : "var(--primary)"}
          fillOpacity={opacity}
        />
      );
    }
  }
  return (
    <svg viewBox="0 0 140 140" className={className} aria-hidden="true">
      <circle cx="70" cy="70" r="62" fill="var(--bg)" stroke="var(--border-strong)" strokeWidth="1.5" />
      <circle cx="70" cy="70" r="56" fill="none" stroke="var(--border)" strokeDasharray="3 5" />
      {dies}
      {/* Bottom notch — the wafer's orientation mark. */}
      <circle cx="70" cy="132" r="4.5" fill="var(--card)" stroke="var(--border-strong)" strokeWidth="1.5" />
    </svg>
  );
}

// Launch-screen chips show the common ones; bz2 / zst / tar.* combos work
// too (the backend sniffs magic bytes, the picker filter lists everything).
const FILE_EXT_CHIPS = [".stdf", ".std", ".zip", ".gz", ".7z", ".rar", ".tar", ".xz"];

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
      className="launch-bg flex flex-1 items-center justify-center overflow-auto p-10"
      aria-label="选择 STDF 文件"
      onDragOver={(event) => {
        event.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={onDrop}
    >
      <div className="relative flex w-full max-w-[560px] flex-col items-center gap-4">
        {/* Resting state reads as a welcome card (solid hairline + card
            elevation); the dashed drop-zone treatment only appears while a
            drag actually hovers, so the launch screen stops looking like an
            upload form. Border stays 1px in both states (no layout shift) —
            the drag emphasis comes from the ring, tint and scale. */}
        <div
          className={`flex w-full flex-col items-center gap-6 rounded-2xl border bg-card/80 px-10 pb-9 pt-10 text-center backdrop-blur-sm transition-all duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] ${
            isDragOver
              ? "scale-[1.015] border-dashed border-primary bg-primary-soft/80 shadow-lg shadow-primary/10 ring-2 ring-primary/30"
              : "border-border/60 shadow-card"
          }`}
        >
          <WaferMark
            className={`h-36 w-36 transition-transform duration-300 ${isDragOver ? "scale-105" : ""}`}
          />
          <div className="flex flex-col gap-2">
            <h1 className="text-balance text-2xl font-semibold tracking-tight text-foreground">
              把 STDF 文件拖到这里
            </h1>
            <p className="max-w-[400px] text-pretty text-sm leading-relaxed text-muted-foreground">
              解析后可查看良率与 bin 分布，按 record 浏览字段和中文说明，全文检索，并把测试项矩阵导出为
              CSV。
            </p>
          </div>
          {/* Button-in-button CTA: pill body + nested icon disc that nudges
              forward on hover (transform only). */}
          <button
            type="button"
            onClick={onOpen}
            className="group inline-flex h-11 items-center gap-3 rounded-full bg-primary pl-5 pr-1.5 text-sm font-medium text-primary-foreground transition duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] hover:bg-primary-hover active:scale-[0.98] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            打开 STDF 文件
            <span
              aria-hidden="true"
              className="flex h-8 w-8 items-center justify-center rounded-full bg-primary-foreground/15 transition-transform duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] group-hover:translate-x-0.5"
            >
              <FolderOpen size={15} />
            </span>
          </button>
          <div
            className="flex flex-wrap items-center justify-center gap-1.5 text-xs text-muted-foreground"
            title="也支持 bz2 / zst，以及 tar.gz / tar.xz 等组合包"
          >
            {FILE_EXT_CHIPS.map((ext) => (
              <span
                key={ext}
                className="rounded-md border border-border bg-muted px-2 py-0.5 font-mono text-[11px]"
              >
                {ext}
              </span>
            ))}
            <span className="px-0.5">STDF V4 / V4-2007</span>
          </div>
          {error && (
            <p
              className="max-w-[440px] rounded-md border border-danger-border bg-danger-soft px-3 py-2 text-sm text-danger"
              role="alert"
            >
              {error}
            </p>
          )}
        </div>
        <p className="text-xs text-muted-foreground">
          也可以按{" "}
          <kbd className="rounded border border-border-strong bg-card px-1.5 py-0.5 font-mono text-[11px] text-foreground">
            ⌘ O
          </kbd>{" "}
          打开文件选择器
        </p>
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
    <nav className="flex w-[72px] shrink-0 flex-col items-center gap-1.5 py-3" aria-label="导航">
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
            {active && (
              <span
                aria-hidden="true"
                className="absolute -left-2 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-full bg-primary"
              />
            )}
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
          title="打开另一个文件（⌘O）"
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

// Document header, identical on every page: filename (the session's primary
// context) leads, meta trails, live parse stats + status sit on the right.
// Parse progress renders as a hairline bar along the header's bottom edge —
// present while running, settles at 100% and fades out once the session ends.
function TopBar({
  session,
  statusView,
  progressPercent,
  progress,
  totalRecords
}: {
  session: ParseSession;
  statusView: ReturnType<typeof getStatusView>;
  progressPercent: number;
  progress: ParseProgress | null;
  totalRecords: number;
}) {
  const running = statusView.tone === "running";
  return (
    <header
      className="relative flex h-12 shrink-0 items-center gap-3 border-b border-border bg-card px-5"
      aria-label="文件解析状态"
    >
      <div className="flex min-w-0 flex-1 items-baseline gap-2.5">
        <span className="truncate text-[13px] font-medium text-foreground" title={session.file_name}>
          {session.file_name}
        </span>
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
          {formatBytes(session.file_size)} · {totalRecords.toLocaleString()} records
        </span>
      </div>
      {running && (
        <div
          className="flex shrink-0 items-center gap-2 text-xs tabular-nums text-muted-foreground"
          aria-label="解析进度"
        >
          <span>{progressPercent}%</span>
          <span className="hidden lg:inline">
            {formatBytes(progress?.bytes_read ?? 0)} / {formatBytes(progress?.total_bytes ?? session.file_size)}
          </span>
        </div>
      )}
      <StatusPill statusView={statusView} />
      <div
        className="absolute inset-x-0 bottom-0 h-0.5"
        role="progressbar"
        aria-valuenow={progressPercent}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-hidden={!running}
        // Opacity waits for the width to land at 100% before fading, and the
        // same delay applies on the way back in (imperceptible at parse start).
        style={{ opacity: running ? 1 : 0, transition: "opacity 600ms ease 250ms" }}
      >
        <div
          className={`relative h-full overflow-hidden bg-primary transition-[width] duration-300 ${
            progressPercent > 0 && progressPercent < 100 ? "progress-shine" : ""
          }`}
          style={{ width: `${progressPercent}%` }}
        />
      </div>
    </header>
  );
}

function OverviewView({
  session,
  keyFields,
  groups,
  binSummary,
  issues,
  onOpenRecordType
}: {
  session: ParseSession;
  keyFields: Record<string, RecordField[]>;
  groups: RecordGroup[];
  binSummary: BinSummary | null;
  issues: FileIssue[];
  onOpenRecordType(recordType: string): void;
}) {
  // Auto-detect: CP files carry wafer records (WIR/WRR); otherwise FT. The
  // heuristic sometimes gets it wrong (test flows without WIR, or FT files
  // that inherit WIR from a preceding CP), so the user can override the
  // badge with a click. The override resets when the file changes.
  const autoIsCp = groups.some(
    (group) => (group.record_type === "WIR" || group.record_type === "WRR") && group.count > 0
  );
  const [override, setOverride] = useState<"cp" | "ft" | null>(null);
  useEffect(() => {
    setOverride(null);
  }, [session.session_id]);
  const isCp = override ? override === "cp" : autoIsCp;
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
    // The overview grew past one screen (yield hero + key fields + pair checks),
    // so the whole page scrolls instead of squeezing sections into the viewport.
    // Section enter is staggered via --stagger for a light "灵动" cascade.
    <section className={`flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto ${PAGE_PAD}`}>
      <FileIssuesPanel issues={issues} complete={complete} />
      <BinYieldCard summary={binSummary} complete={complete} />
      <section
        className="fade-rise flex min-w-0 shrink-0 flex-col rounded-xl border border-border/60 bg-card p-4 shadow-card"
        style={{ ["--stagger" as string]: 1 }}
      >
        <div className="mb-3 flex min-w-0 items-center justify-between gap-4">
          <div className="flex min-w-0 items-baseline gap-2">
            <h2 className="text-[15px] font-semibold text-foreground">关键字段</h2>
            <span
              className="truncate text-xs text-muted-foreground"
              title={session.file_name}
            >
              {session.file_name}
            </span>
          </div>
          <button
            type="button"
            onClick={() => setOverride(isCp ? "ft" : "cp")}
            title={
              override
                ? `已手动切换为 ${isCp ? "CP" : "FT"}（自动识别为 ${autoIsCp ? "CP" : "FT"}），点击切换`
                : `自动识别为 ${isCp ? "CP" : "FT"}，点击切换`
            }
            className="inline-flex shrink-0 items-center gap-1 rounded-full border border-primary-soft bg-primary-soft px-2.5 py-0.5 text-xs font-medium text-primary transition hover:bg-primary hover:text-primary-foreground active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            {isCp ? "CP" : "FT"}
            <ArrowLeftRight size={11} aria-hidden="true" />
          </button>
        </div>
        {/* Renders at natural height — the ~20 rows scroll with the page as a
            single layer instead of nesting a second scroll region. */}
        <div className="overflow-hidden rounded-lg border border-border">
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
      <PairStats groups={groups} complete={complete} />
    </section>
  );
}

function FileIssuesPanel({ issues, complete }: { issues: FileIssue[]; complete: boolean }) {
  if (issues.length === 0) return null;

  const errorCount = issues
    .filter((issue) => issue.severity === "error")
    .reduce((sum, issue) => sum + issue.count, 0);
  const warningCount = issues
    .filter((issue) => issue.severity === "warning")
    .reduce((sum, issue) => sum + issue.count, 0);
  const affectedRecords = issues.reduce((sum, issue) => sum + issue.affected_records, 0);
  const affectsAccuracy = issues.some((issue) => issue.affects_accuracy);
  const hasErrors = errorCount > 0;
  const PanelIcon = hasErrors ? AlertCircle : AlertTriangle;

  return (
    <section
      className={`fade-rise shrink-0 overflow-hidden rounded-xl border bg-card shadow-card ${
        hasErrors ? "border-danger-border/70" : "border-warning-border/80"
      }`}
      aria-label="文件检查"
      style={{ ["--stagger" as string]: 0 }}
    >
      <div
        className={`flex flex-wrap items-start justify-between gap-3 border-b px-4 py-3.5 ${
          hasErrors
            ? "border-danger-border/60 bg-danger-soft"
            : "border-warning-border/70 bg-warning-soft"
        }`}
      >
        <div className="flex min-w-0 items-start gap-3">
          <span
            className={`mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-white ${
              hasErrors ? "bg-danger" : "bg-warning"
            }`}
          >
            <PanelIcon size={18} aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <h2 className="text-[15px] font-semibold text-foreground">文件检查发现问题</h2>
            <p className="mt-0.5 max-w-[760px] text-sm leading-5 text-muted-foreground">
              {complete ? "文件已继续解析完成。" : "软件正在继续解析文件。"}
              {affectsAccuracy
                ? "部分位置之后的记录、良率和统计结果可能不准确，请优先查看第一处错误。"
                : "已读取的内容仍可查看，下面列出了需要留意的位置。"}
            </p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2 text-xs tabular-nums">
          {errorCount > 0 && (
            <span className="inline-flex items-center gap-1.5 rounded-md border border-danger-border bg-card px-2 py-1 font-medium text-danger">
              <AlertCircle size={13} aria-hidden="true" />
              {errorCount.toLocaleString()} 个错误
            </span>
          )}
          {warningCount > 0 && (
            <span className="inline-flex items-center gap-1.5 rounded-md border border-warning-border bg-card px-2 py-1 font-medium text-warning">
              <AlertTriangle size={13} aria-hidden="true" />
              {warningCount.toLocaleString()} 个提醒
            </span>
          )}
          {affectedRecords > 0 && (
            <span className="inline-flex items-center rounded-md border border-border-strong bg-card px-2 py-1 font-medium text-foreground">
              影响 {affectedRecords.toLocaleString()} 条记录
            </span>
          )}
        </div>
      </div>

      <div className="divide-y divide-border">
        {issues.map((issue, index) => {
          const error = issue.severity === "error";
          const Icon = error ? AlertCircle : AlertTriangle;
          return (
            <article key={issue.code} className="px-4 py-3.5">
              <div className="flex items-start gap-3">
                <Icon
                  size={17}
                  aria-hidden="true"
                  className={`mt-0.5 shrink-0 ${error ? "text-danger" : "text-warning"}`}
                />
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <h3 className="text-sm font-semibold text-foreground">{issue.title}</h3>
                    {issue.count > 1 && (
                      <span className="text-xs tabular-nums text-muted-foreground">
                        共 {issue.count.toLocaleString()} 处
                      </span>
                    )}
                    {issue.affected_records > 0 && (
                      <span className="text-xs tabular-nums text-muted-foreground">
                        影响 {issue.affected_records.toLocaleString()} 条记录
                      </span>
                    )}
                    {index === 0 && issue.affects_accuracy && (
                      <span className="rounded bg-danger-soft px-1.5 py-0.5 text-[11px] font-medium text-danger">
                        建议优先处理
                      </span>
                    )}
                  </div>
                  <dl className="mt-2.5 grid gap-3 sm:grid-cols-2">
                    <div
                      className={`border-l-2 pl-3 ${error ? "border-danger" : "border-warning"}`}
                    >
                      <dt className={`text-xs font-semibold ${error ? "text-danger" : "text-warning"}`}>
                        文件中实际是
                      </dt>
                      <dd className="mt-1 break-words text-sm leading-5 text-foreground">
                        {issue.actual}
                      </dd>
                    </div>
                    <div className="border-l-2 border-primary pl-3">
                      <dt className="text-xs font-semibold text-primary">正常情况下应为</dt>
                      <dd className="mt-1 break-words text-sm leading-5 text-foreground">
                        {issue.expected}
                      </dd>
                    </div>
                  </dl>
                  <p className="mt-2.5 text-sm leading-6 text-muted-foreground">
                    <span className="font-medium text-foreground">因此报错：</span>
                    {issue.message}
                  </p>
                  <p className="mt-1.5 text-sm leading-6 text-foreground">
                    <span className="font-medium">建议：</span>
                    {issue.suggestion}
                  </p>

                  {issue.samples.length > 0 && (
                    <details className="group mt-2.5">
                      <summary className="inline-flex cursor-pointer list-none items-center gap-1 text-xs font-medium text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
                        查看出错位置和判断依据
                        <ChevronDown
                          size={14}
                          aria-hidden="true"
                          className="transition-transform group-open:rotate-180"
                        />
                      </summary>
                      <div className="mt-2 overflow-hidden rounded-md border border-border bg-muted/45">
                        {issue.samples.map((sample, sampleIndex) => (
                          <div
                            key={`${sample.offset}-${sampleIndex}`}
                            className="border-b border-border px-3 py-2.5 last:border-b-0"
                          >
                            <div className="flex flex-wrap gap-x-2 text-xs text-muted-foreground">
                              {sample.record_index != null && (
                                <span>第 {(sample.record_index + 1).toLocaleString()} 条 record</span>
                              )}
                              <span>{sample.record_type}</span>
                              <span>
                                文件位置 {formatBytes(sample.offset)}（byte {sample.offset.toLocaleString()}）
                              </span>
                            </div>
                            <p className="mt-1 break-words text-xs leading-5 text-foreground">
                              {sample.detail}
                            </p>
                          </div>
                        ))}
                      </div>
                    </details>
                  )}
                </div>
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}

// Yield hero + per-bin distribution tables (SBIN / HBIN). Yield is the
// headline metric of the page, so the pass % gets the visual weight; the
// supporting counts sit to its right. Matches the CSV export's pass
// convention (PF flag, else "bin 1 = pass").
function BinYieldCard({ summary, complete }: { summary: BinSummary | null; complete: boolean }) {
  const ready = complete && summary !== null;
  const total = summary?.total_parts ?? 0;
  const pass = summary?.sbin_pass ?? 0;
  const fail = total - pass;
  const yieldNum = total > 0 ? (pass / total) * 100 : null;
  // Once parsing finishes, count the yield digit from 0 up to the real
  // value once. Small "lands with intent" flourish, cheap (single rAF).
  const displayYield = useCountUp(ready ? yieldNum : null, 720);
  return (
    <section
      className="fade-rise flex shrink-0 flex-col gap-4 rounded-xl border border-border/60 bg-card bg-gradient-to-br from-primary-soft/45 to-transparent p-4 shadow-card"
      style={{ ["--stagger" as string]: 0 }}
    >
      {/* Hero row: giant yield % on the left, supporting metrics stack on the right. */}
      <div className="flex flex-wrap items-center gap-x-8 gap-y-4">
        <div className="flex min-w-[140px] flex-col">
          <div className="text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
            良率
          </div>
          {ready ? (
            <div
              key={yieldNum ?? "empty"}
              className="number-land mt-0.5 flex items-baseline gap-1 font-semibold tabular-nums text-foreground"
            >
              <span className="text-[44px] leading-none tracking-tight">
                {yieldNum == null ? "-" : displayYield.toFixed(1)}
              </span>
              {yieldNum != null && (
                <span className="text-lg font-medium text-muted-foreground">%</span>
              )}
            </div>
          ) : (
            <div className="mt-1.5 h-10 w-32 skeleton" aria-label="正在解析良率" />
          )}
        </div>
        <div className="flex flex-1 flex-wrap items-stretch gap-x-6 gap-y-2 border-l border-border pl-6">
          <YieldStat label="Total" value={total} ready={ready} tone="foreground" />
          <YieldStat label="Pass" value={pass} ready={ready} tone="success" />
          <YieldStat label="Fail" value={fail} ready={ready} tone="danger" />
        </div>
      </div>
      {/* Pass/fail stacked bar ties the hero number to the counts at a glance. */}
      {ready && total > 0 && (
        <div className="flex h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden="true">
          <div className="h-full bg-success" style={{ width: `${(pass / total) * 100}%` }} />
          <div className="h-full flex-1 bg-danger" />
        </div>
      )}
      {ready && !summary.has_bin_pf && (
        <p className="text-xs text-warning">
          本文件的 bin 记录未包含通过/失败标记(PF)，良率按「软 bin 1 = 通过」约定判定。
        </p>
      )}
      {/* Bin distributions: side-by-side; header sits inline (no eyebrow noise). */}
      <div className="grid grid-cols-2 gap-4">
        {[
          { title: "SBIN 分布", binLabel: "SBIN#", bins: summary?.sbins ?? [] },
          { title: "HBIN 分布", binLabel: "HBIN#", bins: summary?.hbins ?? [] }
        ].map((group) => {
          // The share bars normalize against the group's dominant bin so
          // sub-percent fail bins still register visually.
          const maxCount = group.bins.reduce((max, bin) => Math.max(max, bin.count), 0);
          return (
          <div key={group.title} className="min-w-0">
            {/* Height caps at ~6 rows and long lists scroll inside; short lists
                collapse to their content instead of framing empty space.
                No section title — the SBIN#/HBIN# table headers carry the label. */}
            <div className="max-h-[236px] overflow-auto overscroll-contain rounded-lg border border-border">
              {ready ? (
                <table className={DATA_TABLE}>
                  <thead>
                    <tr>
                      <th className={`${TH} w-[14%]`}>{group.binLabel}</th>
                      <th className={`${TH} w-[32%]`}>名称</th>
                      <th className={`${TH} w-[10%]`}>P/F</th>
                      <th className={`${TH} w-[18%] text-right`}>数量</th>
                      <th className={`${TH} w-[26%] text-right`}>占比</th>
                    </tr>
                  </thead>
                  <tbody>
                    {group.bins.map((bin, i) => (
                      <tr
                        key={bin.num}
                        className="fade-rise"
                        style={{ ["--stagger" as string]: Math.min(i, 12) }}
                      >
                        <td className={`${TD} ${MONO}`}>{bin.num}</td>
                        <td className={`${TD}`}>
                          <span className="block truncate" title={bin.name || undefined}>
                            {bin.name || "-"}
                          </span>
                        </td>
                        <td className={TD}>{pfCell(bin.pf)}</td>
                        <td className={`${TD} ${MONO} text-right`}>{bin.count.toLocaleString()}</td>
                        <td className={`${TD} text-right`}>
                          <ShareCell count={bin.count} total={total} max={maxCount} pf={bin.pf} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <BinTableSkeleton />
              )}
            </div>
          </div>
          );
        })}
      </div>
    </section>
  );
}

// 占比 cell: the true percentage as text plus a small bar normalized to the
// group's largest bin (`max`), toned by the bin's pass/fail flag.
function ShareCell({ count, total, max, pf }: { count: number; total: number; max: number; pf: string }) {
  if (total <= 0) return <span className="text-muted-foreground">-</span>;
  const share = (count / total) * 100;
  const rel = max > 0 ? Math.max((count / max) * 100, 2.5) : 0;
  const fill = pf === "F" ? "bg-danger" : pf === "P" ? "bg-success" : "bg-primary";
  return (
    <span className="flex items-center justify-end gap-2 whitespace-nowrap">
      <span className="h-1.5 w-10 shrink-0 overflow-hidden rounded-full bg-muted" aria-hidden="true">
        <span className={`block h-full rounded-full ${fill}`} style={{ width: `${rel}%` }} />
      </span>
      <span className={`${MONO} tabular-nums`}>{share.toFixed(1)}%</span>
    </span>
  );
}

function YieldStat({
  label,
  value,
  ready,
  tone
}: {
  label: string;
  value: number;
  ready: boolean;
  tone: "foreground" | "success" | "danger";
}) {
  const toneClass = { foreground: "text-foreground", success: "text-success", danger: "text-danger" }[tone];
  return (
    <div className="flex min-w-[92px] flex-col">
      <div className="text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      {ready ? (
        <div className={`number-land mt-0.5 text-xl font-semibold tabular-nums ${toneClass}`}>
          {value.toLocaleString()}
        </div>
      ) : (
        <div className="mt-1.5 h-6 w-16 skeleton" aria-hidden="true" />
      )}
    </div>
  );
}

// Rows of shimmering placeholders sized like the real bin table so the
// layout doesn't jump when data lands.
function BinTableSkeleton() {
  return (
    <div className="flex h-full flex-col divide-y divide-border/50 p-2">
      {Array.from({ length: 6 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 py-2">
          <div className="skeleton h-3 w-8 shrink-0" />
          <div className="skeleton h-3 flex-1" />
          <div className="skeleton h-3 w-4 shrink-0" />
          <div className="skeleton h-3 w-12 shrink-0" />
          <div className="skeleton h-3 w-10 shrink-0" />
        </div>
      ))}
    </div>
  );
}

// Animates a numeric value from 0 up to `target` over `duration` ms using
// requestAnimationFrame. Ease-out cubic — quick at first, gentle landing.
// Only reveals the first time target transitions from null → number, so
// re-entering the overview after parse-complete shows the settled value
// instantly instead of replaying the reveal. Also falls through to target
// under prefers-reduced-motion (and when rAF isn't available, e.g. jsdom).
function useCountUp(target: number | null, duration = 700): number {
  const [value, setValue] = useState<number>(target ?? 0);
  // shouldReveal = we've observed null and are waiting on the first number.
  // If the component mounts with target already known, we skip the reveal.
  const shouldRevealRef = useRef<boolean>(target == null);
  useEffect(() => {
    if (target == null) {
      setValue(0);
      shouldRevealRef.current = true;
      return;
    }
    const reduced =
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (!shouldRevealRef.current || reduced || typeof requestAnimationFrame !== "function") {
      setValue(target);
      shouldRevealRef.current = false;
      return;
    }
    shouldRevealRef.current = false;
    let raf = 0;
    let start = 0;
    const from = 0;
    const step = (t: number) => {
      if (!start) start = t;
      const p = Math.min(1, (t - start) / duration);
      const eased = 1 - Math.pow(1 - p, 3);
      setValue(from + (target - from) * eased);
      if (p < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [target, duration]);
  return value;
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
    <section
      className="fade-rise shrink-0 rounded-xl border border-border/60 bg-card px-4 py-3 shadow-card"
      style={{ ["--stagger" as string]: 2 }}
    >
      {complete ? (
        <div className="grid grid-cols-4 gap-2">
          {rows.map((r, i) => {
            const ok = r.ca === r.cb;
            const Icon = ok ? CheckCircle2 : AlertCircle;
            return (
              <div
                key={r.a}
                className="fade-rise min-w-0 rounded-lg border border-border bg-muted px-2.5 py-1.5"
                style={{ ["--stagger" as string]: 3 + i }}
              >
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
        <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
          <span className="dot-pulse inline-block h-1.5 w-1.5 rounded-full bg-primary" />
          等待文件解析完成后再校验成对记录数量。
        </div>
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
  onCursorChange,
  dtrParse,
  gdrParse,
  onParseDtr,
  onDownloadDtr,
  onParseGdr,
  onDownloadGdr,
  parseComplete
}: {
  groups: RecordGroup[];
  selectedGroup: string;
  onSelectGroup(value: string): void;
  selectedRecord: RecordSummary | null;
  fields: RecordField[];
  cursor: number;
  recordTotal: number;
  onCursorChange(index: number): void;
  dtrParse: TextParseState<DtrPreview>;
  gdrParse: TextParseState<GdrPreview>;
  onParseDtr(): void;
  onDownloadDtr(): void;
  onParseGdr(): void;
  onDownloadGdr(): void;
  parseComplete: boolean;
}) {
  return (
    <section
      className="fade-rise grid min-h-0 flex-1 grid-cols-[minmax(180px,220px)_minmax(360px,1fr)] overflow-hidden"
      aria-label="工作台"
    >
      {/* pt-5 keeps the aside's eyebrow on the same baseline as the detail
          pane's PAGE_PAD inset. */}
      <aside className="flex min-h-0 flex-col overflow-hidden border-r border-border bg-muted px-2.5 pb-3 pt-5" aria-label="Record 类型">
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
        selectedGroup={selectedGroup}
        selectedRecord={selectedRecord}
        fields={fields}
        cursor={cursor}
        recordTotal={recordTotal}
        onCursorChange={onCursorChange}
        dtrParse={dtrParse}
        gdrParse={gdrParse}
        onParseDtr={onParseDtr}
        onDownloadDtr={onDownloadDtr}
        onParseGdr={onParseGdr}
        onDownloadGdr={onDownloadGdr}
        parseComplete={parseComplete}
      />
    </section>
  );
}

function FieldDetailPanel({
  selectedGroup,
  selectedRecord,
  fields,
  cursor,
  recordTotal,
  onCursorChange,
  dtrParse,
  gdrParse,
  onParseDtr,
  onDownloadDtr,
  onParseGdr,
  onDownloadGdr,
  parseComplete
}: {
  selectedGroup: string;
  selectedRecord: RecordSummary | null;
  fields: RecordField[];
  cursor: number;
  recordTotal: number;
  onCursorChange(index: number): void;
  dtrParse: TextParseState<DtrPreview>;
  gdrParse: TextParseState<GdrPreview>;
  onParseDtr(): void;
  onDownloadDtr(): void;
  onParseGdr(): void;
  onDownloadGdr(): void;
  parseComplete: boolean;
}) {
  const hasPager = recordTotal > 1 && !isTextPreviewRecordType(selectedGroup);
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
    <section className={`flex min-h-0 flex-col gap-3 overflow-hidden bg-card ${PAGE_PAD}`} aria-label="字段详情">
      <div className="flex shrink-0 items-start justify-between gap-3 border-b border-border pb-3">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className={EYEBROW}>字段详情</span>
          <strong className="block text-[15px] font-semibold text-foreground">
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
                className="pop-in pointer-events-none absolute right-0 top-full z-20 mt-1.5 hidden w-72 rounded-lg border border-border bg-card p-2.5 text-xs leading-relaxed text-foreground shadow-overlay group-hover:block"
              >
                {recordStatusHint(selectedRecord.status)}
              </div>
            </div>
          )}
        </div>
      </div>
      {selectedRecord ? (
        selectedRecord.record_type === "DTR" ? (
          <RecordTextInspector
            kind="DTR"
            state={dtrParse}
            onParse={onParseDtr}
            onDownload={onDownloadDtr}
            parseComplete={parseComplete}
          />
        ) : selectedRecord.record_type === "GDR" ? (
          <RecordTextInspector
            kind="GDR"
            state={gdrParse}
            onParse={onParseGdr}
            onDownload={onDownloadGdr}
            parseComplete={parseComplete}
          />
        ) : (
          <FieldsTable fields={fields} />
        )
      ) : (
        <EmptyState
          icon={MousePointerClick}
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
    <section className={`fade-rise flex min-h-0 flex-1 flex-col gap-3 overflow-hidden ${PAGE_PAD}`} aria-label="搜索">
      <div className="flex items-center justify-between gap-3">
        <div>
          <span className={EYEBROW}>Search</span>
          <strong className="block text-[15px] font-semibold text-foreground">
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
            className="relative h-1.5 w-full overflow-hidden rounded-full bg-muted"
            role="progressbar"
            aria-valuenow={pct ?? undefined}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            {pct == null ? (
              <div className="indeterminate-sweep absolute inset-y-0 left-0 w-1/3 rounded-full bg-primary" />
            ) : (
              // overlay-fade softens the swap from the sweeping segment to the
              // determinate bar when the first progress tick lands.
              <div
                className="overlay-fade relative h-full overflow-hidden rounded-full bg-primary transition-[width] duration-150 progress-shine"
                style={{ width: `${pct}%` }}
              />
            )}
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
          icon={Hourglass}
          title="等待解析完成后搜索"
          body="为了保持摘要和字段浏览不卡顿，解析过程中暂停全量搜索；文件解析完成后即可搜索全部字段。"
        />
      ) : trimmed.length < 2 ? (
        <EmptyState
          icon={Search}
          title="输入至少 2 个字符开始搜索"
          body="可搜索 record type、字段名或字段值。大文件为全量搜索，输入后会稍等片刻再出结果。"
        />
      ) : searching ? (
        <EmptyState icon={Loader2} spin title="搜索中…" body="正在全量检索，较大文件需要几秒，请稍候。" />
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

// Per-part info columns shown on the left (not frozen — the whole table scrolls
// freely). Widths auto-fit their sampled content, like the value columns.
// Bin number / name / PF each get their own column so they are always present —
// empty cells when the file's HBR/SBR don't carry that field.
type LeftCol = {
  key: string;
  label: string;
  // Per-column alignment: text scans from the left, numbers compare along the
  // right edge, single-letter P/F flags read best centered.
  align: "left" | "right" | "center";
  get: (row: TestItemPartRow) => string;
  title?: (row: TestItemPartRow) => string | undefined;
  render?: (row: TestItemPartRow) => ReactNode;
};

const ALIGN_CLASS = { left: "text-left", right: "text-right", center: "text-center" } as const;

const LEFT_COLS: LeftCol[] = [
  { key: "part_id", label: "PartID", align: "left", get: (r) => r.part_id || "-", title: (r) => r.part_id || undefined },
  { key: "site", label: "Site", align: "right", get: (r) => r.site_num || "-" },
  { key: "sbin_num", label: "SBIN#", align: "right", get: (r) => r.sbin_num || "-" },
  { key: "sbin_name", label: "SBIN Name", align: "left", get: (r) => r.sbin_name || "-", title: (r) => r.sbin_name || undefined },
  { key: "sbin_pf", label: "SBIN PF", align: "center", get: (r) => r.sbin_pf || "-", render: (r) => pfCell(r.sbin_pf) },
  { key: "hbin_num", label: "HBIN#", align: "right", get: (r) => r.hbin_num || "-" },
  { key: "hbin_name", label: "HBIN Name", align: "left", get: (r) => r.hbin_name || "-", title: (r) => r.hbin_name || undefined },
  { key: "hbin_pf", label: "HBIN PF", align: "center", get: (r) => r.hbin_pf || "-", render: (r) => pfCell(r.hbin_pf) },
  { key: "test_t", label: "TEST_T", align: "right", get: (r) => r.test_t || "-" },
  { key: "part_txt", label: "PART_TXT", align: "left", get: (r) => r.part_txt || "-", title: (r) => r.part_txt || undefined }
];
// Fixed body-row height (px) — vertical windowing positions rows by this.
const TI_ROW_H = 40;
// Auto-fit bounds for value columns. The max leaves room for long test names
// on a single line; anything longer truncates with the detail card as backup.
const TI_COL_MIN = 84;
const TI_COL_MAX = 420;
// Auto-fit bounds for the left part-info columns.
const TI_LEFT_MIN = 56;
const TI_LEFT_MAX = 240;

// Text measurement for auto-fitting column widths. Falls back to a char-count
// heuristic where canvas 2D is unavailable (jsdom).
let measureCtx: CanvasRenderingContext2D | null | undefined;
function textWidth(text: string, font: string): number {
  if (measureCtx === undefined) {
    measureCtx =
      typeof document !== "undefined" ? document.createElement("canvas").getContext("2d") : null;
  }
  if (!measureCtx) return text.length * 7.2;
  measureCtx.font = font;
  return measureCtx.measureText(text).width;
}

const FONT_MONO_12 = '12px ui-monospace, SFMono-Regular, Menlo, monospace';
const FONT_SANS_11 = '11px -apple-system, "PingFang SC", "Helvetica Neue", sans-serif';

// Width a value column needs: widest of its metadata rows (the name may wrap
// to two lines) and a sample of its loaded values, clamped to sane bounds.
function fitColumnWidth(column: TestItemColumn, sample: TestItemPartRow[], index: number): number {
  // The full single-line name drives the width — names must not wrap.
  let w = Math.max(
    textWidth(column.test_name || "-", FONT_SANS_11),
    textWidth(column.low_limit || "-", FONT_MONO_12),
    textWidth(column.high_limit || "-", FONT_MONO_12),
    textWidth(column.record_type === "FTR" ? "P/F" : column.unit || "-", FONT_SANS_11)
  );
  for (const row of sample) {
    const value = row.results[index]?.value;
    // Values render in 12px mono (right-aligned numeric column), measure in kind.
    if (value) w = Math.max(w, textWidth(value, FONT_MONO_12));
  }
  return Math.min(TI_COL_MAX, Math.max(TI_COL_MIN, Math.ceil(w) + 20));
}

// Width a part-info column needs: its header label or the widest sampled value.
function fitLeftColumnWidth(col: LeftCol, sample: TestItemPartRow[]): number {
  let w = textWidth(col.label, `600 ${FONT_SANS_11}`);
  for (const row of sample) {
    w = Math.max(w, textWidth(col.get(row), FONT_MONO_12));
  }
  return Math.min(TI_LEFT_MAX, Math.max(TI_LEFT_MIN, Math.ceil(w) + 22));
}

// Compact "type num · low~high unit" line under each column's name in the
// header. The full values live in the click-to-open detail card.
function columnMetaLine(column: TestItemColumn): string {
  if (column.record_type === "FTR") return "P/F";
  const low = column.low_limit || "-";
  const high = column.high_limit || "-";
  return `${low} ~ ${high} ${column.unit || ""}`.trim();
}

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
  onLoadMore,
  onJumpToRecord,
  fetchMprPins,
  exportMprPins
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
  onJumpToRecord: (recordType: string, position: number) => void;
  fetchMprPins: (testNum: number, recordPosition: number) => Promise<MprPinDetails>;
  /** Save-dialog + backend xlsx write; resolves false when the user cancels. */
  exportMprPins: (
    testNum: number,
    recordPosition: number,
    partId: string,
    siteNum: string
  ) => Promise<boolean>;
}) {
  // Custom right-click menu on test-value cells so we can offer a "跳转到源记录"
  // action alongside "复制" (browser's default menu wouldn't have the jump).
  // Cmd/Ctrl-C on a text selection still uses native copy, so that path is
  // preserved for users who don't reach for the right-click menu.
  const [cellMenu, setCellMenu] = useState<{
    x: number;
    y: number;
    value: string;
    recordType: string;
    recordPosition: number | undefined;
    column: TestItemColumn;
    row: TestItemPartRow;
  } | null>(null);
  // Column detail card: opened by clicking any header cell of a test column.
  // Shows the full (grid-truncated) name selectable, plus copy shortcuts.
  const [colInfo, setColInfo] = useState<{ x: number; y: number; column: TestItemColumn } | null>(
    null
  );
  // MPR pin dialog: an MPR cell only previews its multi-pin result array, so
  // clicking one opens a modal that fetches and lists the complete per-pin
  // values (the backend re-reads the source file on demand).
  const [pinDialog, setPinDialog] = useState<{
    column: TestItemColumn;
    row: TestItemPartRow;
    recordPosition: number;
  } | null>(null);
  useEffect(() => {
    if (!cellMenu && !colInfo) return;
    const close = () => {
      setCellMenu(null);
      setColInfo(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    // Any mousedown, scroll, or Escape dismisses the menu / detail card.
    window.addEventListener("mousedown", close);
    window.addEventListener("scroll", close, true);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("keydown", onKey);
    };
  }, [cellMenu, colInfo]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const theadRef = useRef<HTMLTableSectionElement>(null);
  // Scroll/viewport metrics driving the render window. Scroll offsets are
  // bucketized to one row / one column, so scrolling only re-renders when the
  // window actually shifts a full row/column — no extra throttling needed.
  const [view, setView] = useState({ top: 0, left: 0, height: 0, width: 0, headerH: 0 });

  const measureView = () => {
    const el = scrollRef.current;
    if (!el) return;
    const next = {
      top: Math.floor(el.scrollTop / TI_ROW_H) * TI_ROW_H,
      // Columns are variable-width, so bucketize by a fixed quantum (well under
      // the minimum column width) purely to dedupe scroll-driven re-renders.
      left: Math.floor(el.scrollLeft / 48) * 48,
      height: el.clientHeight,
      width: el.clientWidth,
      headerH: theadRef.current?.offsetHeight ?? 0
    };
    setView((current) =>
      current.top === next.top &&
      current.left === next.left &&
      current.height === next.height &&
      current.width === next.width &&
      current.headerH === next.headerH
        ? current
        : next
    );
  };

  // Capture the real viewport once the table mounts, and follow panel resizes.
  useEffect(() => {
    if (!loaded) return;
    measureView();
    const el = scrollRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measureView);
    observer.observe(el);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded]);

  const handleScroll = () => {
    measureView();
    const el = scrollRef.current;
    if (!el || !hasMore || loadingMore) return;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 320) {
      onLoadMore();
    }
  };

  // Auto-fitted value-column widths, measured from column metadata plus a
  // sample of the first loaded rows. Keyed on the first row's identity so
  // load-more appends never shift the layout.
  const firstRow = rows[0];
  const colWidths = useMemo(() => {
    const sample = rows.slice(0, 30);
    return columns.map((column, index) => fitColumnWidth(column, sample, index));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [columns, firstRow]);
  const colOffsets = useMemo(() => {
    const offsets = [0];
    for (const width of colWidths) offsets.push(offsets[offsets.length - 1] + width);
    return offsets;
  }, [colWidths]);
  // Part-info columns auto-fit too (an empty SBIN Name column shouldn't sit at
  // a fixed 160px while long test names get squeezed).
  const leftWidths = useMemo(() => {
    const sample = rows.slice(0, 100);
    return LEFT_COLS.map((col) => fitLeftColumnWidth(col, sample));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [firstRow]);
  const leftWidth = useMemo(() => leftWidths.reduce((sum, w) => sum + w, 0), [leftWidths]);

  // Window of rows/columns actually mounted; the rest is spacer height/width.
  const win = computeMatrixWindow({
    scrollTop: view.top,
    scrollLeft: view.left,
    viewportHeight: view.height,
    viewportWidth: view.width,
    headerHeight: view.headerH,
    leftWidth,
    rowHeight: TI_ROW_H,
    colWidth: TEST_COL_WIDTH,
    rowCount: rows.length,
    colCount: columns.length,
    colOffsets
  });
  const visibleRows = rows.slice(win.rowStart, win.rowEnd);
  const visibleCols = columns.slice(win.colStart, win.colEnd);
  const spacerTop = win.rowStart * TI_ROW_H;
  const spacerBottom = (rows.length - win.rowEnd) * TI_ROW_H;
  const spacerLead = colOffsets[win.colStart] ?? 0;
  const spacerTrail = (colOffsets[columns.length] ?? 0) - (colOffsets[win.colEnd] ?? 0);
  // colgroup entries: left info cols + lead spacer + windowed value cols + trail spacer.
  const fullColSpan = LEFT_COLS.length + visibleCols.length + 2;

  return (
    <section className={`fade-rise flex min-h-0 flex-1 flex-col gap-3 overflow-hidden ${PAGE_PAD}`} aria-label="测试项">
      <div className="flex items-center justify-between gap-3">
        <div>
          <span className={EYEBROW}>Test Items</span>
          <strong className="block text-[15px] font-semibold text-foreground">
            {loaded
              ? `共 ${colTotal.toLocaleString()} 个测试项 · ${rowTotal.toLocaleString()} 个 Part/Site 行`
              : session.status === "complete"
                ? "正在汇总测试项…"
                : "等待解析完成"}
          </strong>
        </div>
      </div>
      {!loaded ? (
        session.status === "complete" ? (
          <EmptyState
            icon={Loader2}
            spin
            title="正在汇总测试项…"
            body="按 Part × 测试列展开整份文件，大文件首次打开需要几秒；期间其他页面可以正常使用。"
          />
        ) : (
          <EmptyState
            icon={Hourglass}
            title="等待解析完成"
            body="测试项页在解析完成后开放，用于按 Part × 测试列查看 PTR / MPR / FTR 值。"
          />
        )
      ) : (
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-border/60 bg-card shadow-card">
          {/* Single toolbar: actions, then the column pager, with the loaded
              counter + PMR badge pushed right. Wraps only when space runs out. */}
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-border bg-muted px-3 py-1.5">
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
            <span aria-hidden="true" className="h-4 w-px shrink-0 bg-border-strong/70" />
            <ColumnPager
              page={colPage}
              size={colSize}
              total={colTotal}
              onPageChange={onColPageChange}
              onSizeChange={onColSizeChange}
            />
            <span className="ml-auto flex shrink-0 items-center gap-2 text-xs tabular-nums text-muted-foreground">
              {loadingMore && <Loader2 size={13} className="animate-spin" />}
              <span>
                已加载 {rows.length.toLocaleString()} / {rowTotal.toLocaleString()} 行
              </span>
              <span
                className="rounded-full border border-border bg-card px-2 py-0.5"
                title="文件中的 PMR（Pin Map Record，引脚映射记录）数量"
              >
                {pmrCount.toLocaleString()} PMR
              </span>
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
            <div className="flex min-h-0 flex-1 p-3">
              <EmptyState
                icon={SearchX}
                title="没有匹配筛选条件的测试项或 PART"
                body="换个筛选条件，或在筛选对话框里全选恢复全部测试项。"
              />
            </div>
          ) : (
            <div ref={scrollRef} onScroll={handleScroll} className="min-h-0 overflow-auto">
              {/* Virtualized matrix: only the scroll window of rows × columns is
                  mounted; spacer cells/rows keep the scrollbar geometry identical
                  to a full render. table-fixed + the colgroup make cell positions
                  pure arithmetic, so windowing never shifts the layout. */}
              <table
                className="w-max table-fixed border-separate border-spacing-0 text-[13px] [&_td]:border-b [&_td]:border-r [&_td]:border-border/80 [&_th]:border-b [&_th]:border-r [&_th]:border-border [&_thead_th]:sticky [&_thead_th]:top-0 [&_thead_th]:z-[2] [&_tbody_tr:hover]:bg-muted/30"
                aria-label="测试项矩阵"
              >
                <colgroup>
                  {LEFT_COLS.map((col, index) => (
                    <col key={col.key} style={{ width: leftWidths[index] }} />
                  ))}
                  <col style={{ width: spacerLead }} />
                  {visibleCols.map((column, index) => (
                    <col
                      key={`${column.record_type}:${column.test_num}`}
                      style={{ width: colWidths[win.colStart + index] }}
                    />
                  ))}
                  <col style={{ width: spacerTrail }} />
                </colgroup>
                <thead ref={theadRef}>
                  {/* Single unified header row: part-info labels on the left; each
                      test column shows its name plus a compact "type num · limits"
                      line. Full metadata opens in the click-to-open detail card. */}
                  <tr>
                    {LEFT_COLS.map((col) => (
                      <th
                        key={col.key}
                        className={`overflow-hidden whitespace-nowrap bg-muted px-2.5 py-2 align-middle text-[11px] font-semibold text-foreground ${ALIGN_CLASS[col.align]}`}
                      >
                        {col.label}
                      </th>
                    ))}
                    <th aria-hidden className="!border-r-0 bg-muted p-0" />
                    {visibleCols.map((column) => (
                      <th
                        key={`${column.record_type}:${column.test_num}`}
                        className="cursor-pointer bg-muted px-2 py-1.5 text-left align-bottom transition-colors hover:bg-primary-soft/60"
                        title="点击查看完整测试项信息"
                        onClick={(e) => setColInfo({ x: e.clientX, y: e.clientY, column })}
                      >
                        <span className="block truncate whitespace-nowrap text-[11px] font-medium leading-snug text-foreground">
                          {column.test_name || `#${column.test_num}`}
                        </span>
                        {/* Limits only — the record type and test number live in the
                            click-to-open detail card. Right-aligned to sit on the
                            same edge as the values below. */}
                        <span className="mt-0.5 block truncate text-right font-mono text-[11px] tabular-nums text-muted-foreground">
                          {columnMetaLine(column)}
                        </span>
                      </th>
                    ))}
                    <th aria-hidden className="!border-r-0 bg-muted p-0" />
                  </tr>
                </thead>
                <tbody>
                  {spacerTop > 0 && (
                    <tr aria-hidden style={{ height: spacerTop }}>
                      <td colSpan={fullColSpan} className="!border-0 p-0" />
                    </tr>
                  )}
                  {visibleRows.map((row) => (
                    <tr key={`${row.part_id}:${row.site_num}`} style={{ height: TI_ROW_H }}>
                      {LEFT_COLS.map((col) => (
                        <td
                          key={col.key}
                          className={`${MONO} overflow-hidden whitespace-nowrap px-2.5 py-0 align-middle text-foreground ${ALIGN_CLASS[col.align]}`}
                          title={col.title?.(row)}
                        >
                          {col.render ? (
                            col.render(row)
                          ) : (
                            <span className="block truncate">{col.get(row)}</span>
                          )}
                        </td>
                      ))}
                      <td aria-hidden className="!border-0 p-0" />
                      {visibleCols.map((column, index) => {
                        const cell = row.results[win.colStart + index];
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
                        // An MPR cell only previews its multi-pin array — clicking
                        // it opens the full per-pin dialog.
                        const expandable =
                          column.record_type === "MPR" && cell?.record_position !== undefined;
                        return (
                          <td
                            key={`${row.part_id}:${row.site_num}:${column.record_type}:${column.test_num}`}
                            className={`overflow-hidden px-2 py-0 align-middle ${status === "F" ? "bg-danger-soft" : ""} ${
                              expandable ? "cursor-pointer hover:bg-primary-soft/50" : ""
                            }`}
                            onClick={() => {
                              if (expandable) {
                                setPinDialog({
                                  column,
                                  row,
                                  recordPosition: cell.record_position as number
                                });
                              }
                            }}
                            onContextMenu={(e) => {
                              e.preventDefault();
                              setCellMenu({
                                x: e.clientX,
                                y: e.clientY,
                                value: cell?.value ?? "",
                                recordType: column.record_type,
                                recordPosition: cell?.record_position,
                                column,
                                row
                              });
                            }}
                          >
                            <span
                              title={
                                expandable
                                  ? `${cell?.value || ""}\n点击查看全部 pin 结果`
                                  : cell?.value || undefined
                              }
                              className={`block w-full truncate text-right font-mono text-xs tabular-nums ${textTone}`}
                            >
                              {display}
                            </span>
                          </td>
                        );
                      })}
                      <td aria-hidden className="!border-0 p-0" />
                    </tr>
                  ))}
                  {spacerBottom > 0 && (
                    <tr aria-hidden style={{ height: spacerBottom }}>
                      <td colSpan={fullColSpan} className="!border-0 p-0" />
                    </tr>
                  )}
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
      {/* Both popups render through a portal: position: fixed resolves against
          the nearest transformed ancestor, so leaving them inside the (animated)
          section risks offset coordinates whenever a transform is in effect. */}
      {colInfo &&
        createPortal(
        <div
          role="dialog"
          aria-label="测试项详情"
          className="pop-in fixed z-50 w-[320px] rounded-lg border border-border-strong bg-card p-3.5 shadow-overlay"
          style={{
            left: Math.max(8, Math.min(colInfo.x, window.innerWidth - 336)),
            top: Math.min(colInfo.y + 10, window.innerHeight - 220)
          }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <div className="flex items-center gap-2">
            <span className="rounded-sm bg-primary-soft px-1.5 py-px text-[11px] font-medium text-primary">
              {colInfo.column.record_type}
            </span>
            <span className="font-mono text-xs tabular-nums text-muted-foreground">
              {colInfo.column.test_num}
            </span>
          </div>
          <p className="mt-2 select-text break-all text-[13px] font-medium leading-relaxed text-foreground">
            {colInfo.column.test_name || "-"}
          </p>
          <div className="mt-2.5 grid grid-cols-3 gap-2 rounded-md bg-muted px-2.5 py-2 text-xs">
            <div>
              <div className="text-muted-foreground">Low</div>
              <div className="mt-0.5 select-text truncate font-mono text-foreground" title={colInfo.column.low_limit || undefined}>
                {colInfo.column.low_limit || "-"}
              </div>
            </div>
            <div>
              <div className="text-muted-foreground">High</div>
              <div className="mt-0.5 select-text truncate font-mono text-foreground" title={colInfo.column.high_limit || undefined}>
                {colInfo.column.high_limit || "-"}
              </div>
            </div>
            <div>
              <div className="text-muted-foreground">Unit</div>
              <div className="mt-0.5 select-text truncate text-foreground">
                {colInfo.column.record_type === "FTR" ? "P/F" : colInfo.column.unit || "-"}
              </div>
            </div>
          </div>
          <div className="mt-3 flex gap-2">
            <button
              type="button"
              className={PAGER_BTN}
              onClick={() => {
                void navigator.clipboard?.writeText(colInfo.column.test_name || "");
                setColInfo(null);
              }}
            >
              复制名称
            </button>
            <button
              type="button"
              className={PAGER_BTN}
              onClick={() => {
                void navigator.clipboard?.writeText(String(colInfo.column.test_num));
                setColInfo(null);
              }}
            >
              复制编号
            </button>
          </div>
        </div>,
        document.body
      )}
      {cellMenu &&
        createPortal(
        <div
          role="menu"
          className="pop-in fixed z-50 min-w-[160px] rounded-md border border-border-strong bg-card py-1 text-sm shadow-overlay"
          style={{
            left: Math.max(8, Math.min(cellMenu.x, window.innerWidth - 168)),
            top: Math.max(8, Math.min(cellMenu.y, window.innerHeight - 90))
          }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          {cellMenu.recordType === "MPR" && (
            <button
              type="button"
              role="menuitem"
              disabled={cellMenu.recordPosition === undefined}
              onClick={() => {
                if (cellMenu.recordPosition !== undefined) {
                  setPinDialog({
                    column: cellMenu.column,
                    row: cellMenu.row,
                    recordPosition: cellMenu.recordPosition
                  });
                }
                setCellMenu(null);
              }}
              className="block w-full px-3 py-1.5 text-left hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
            >
              查看全部 pin 结果
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            disabled={cellMenu.recordPosition === undefined}
            onClick={() => {
              if (cellMenu.recordPosition !== undefined) {
                onJumpToRecord(cellMenu.recordType, cellMenu.recordPosition);
              }
              setCellMenu(null);
            }}
            className="block w-full px-3 py-1.5 text-left hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
          >
            跳转到 {cellMenu.recordType} 记录
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={!cellMenu.value}
            onClick={() => {
              if (cellMenu.value) {
                void navigator.clipboard.writeText(cellMenu.value);
              }
              setCellMenu(null);
            }}
            className="block w-full px-3 py-1.5 text-left hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
          >
            复制值
          </button>
        </div>,
        document.body
      )}
      {pinDialog && (
        <MprPinDialog
          column={pinDialog.column}
          row={pinDialog.row}
          recordPosition={pinDialog.recordPosition}
          fetchPins={fetchMprPins}
          onExport={() =>
            exportMprPins(
              pinDialog.column.test_num,
              pinDialog.recordPosition,
              pinDialog.row.part_id,
              pinDialog.row.site_num
            )
          }
          onClose={() => setPinDialog(null)}
        />
      )}
    </section>
  );
}

// The pin table renders in chunks: an MPR can carry tens of thousands of
// pins, and mounting that many rows at once would freeze the dialog.
const PIN_DISPLAY_CAP = 1000;

// Full per-pin expansion of one MPR cell. Fetches on open — the backend
// re-reads the source file (decompressing if needed), so a spinner covers
// the sub-second-to-seconds wait on large files.
function MprPinDialog({
  column,
  row,
  recordPosition,
  fetchPins,
  onExport,
  onClose
}: {
  column: TestItemColumn;
  row: TestItemPartRow;
  recordPosition: number;
  fetchPins: (testNum: number, recordPosition: number) => Promise<MprPinDetails>;
  /** Resolves true when a file was written, false when the user cancelled. */
  onExport: () => Promise<boolean>;
  onClose: () => void;
}) {
  const [details, setDetails] = useState<MprPinDetails | null>(null);
  const [error, setError] = useState("");
  const [failOnly, setFailOnly] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exported, setExported] = useState(false);
  const [exportError, setExportError] = useState("");
  const [displayLimit, setDisplayLimit] = useState(PIN_DISPLAY_CAP);

  useEffect(() => {
    let cancelled = false;
    setDetails(null);
    setError("");
    fetchPins(column.test_num, recordPosition)
      .then((result) => {
        if (!cancelled) setDetails(result);
      })
      .catch((reason) => {
        if (!cancelled) setError(String(reason));
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [column.test_num, recordPosition]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const pins = details?.pins ?? [];
  const failCount = useMemo(() => pins.filter((pin) => pin.status === "F").length, [pins]);
  const stats = useMemo(() => {
    const values = pins
      .map((pin) => Number(pin.value))
      .filter((value) => Number.isFinite(value));
    if (values.length === 0) return null;
    let min = values[0];
    let max = values[0];
    let sum = 0;
    for (const value of values) {
      if (value < min) min = value;
      if (value > max) max = value;
      sum += value;
    }
    const round = (value: number) => String(Number(value.toPrecision(6)));
    return { min: round(min), max: round(max), avg: round(sum / values.length) };
  }, [pins]);
  // Keep each pin's 1-based position stable when the fail filter hides rows.
  const shown = useMemo(() => {
    const indexed = pins.map((pin, index) => ({ pin, index }));
    return (failOnly ? indexed.filter(({ pin }) => pin.status === "F") : indexed).slice(
      0,
      displayLimit
    );
  }, [pins, failOnly, displayLimit]);
  const hiddenCount = (failOnly ? failCount : pins.length) - shown.length;

  const exportXlsx = async () => {
    if (exporting) return;
    setExporting(true);
    setExportError("");
    try {
      const saved = await onExport();
      if (saved) {
        setExported(true);
        setTimeout(() => setExported(false), 2500);
      }
    } catch (reason) {
      setExportError(`导出 Excel 失败：${String(reason)}`);
    } finally {
      setExporting(false);
    }
  };

  return createPortal(
    <div
      className="overlay-fade fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
      onClick={onClose}
      role="presentation"
    >
      <div
        className="pop-in flex max-h-[85vh] w-[640px] max-w-full flex-col overflow-hidden rounded-xl border border-border bg-card shadow-overlay"
        onClick={(event) => event.stopPropagation()}
        onMouseDown={(event) => event.stopPropagation()}
        role="dialog"
        aria-label="MPR pin 结果"
      >
        <div className="border-b border-border px-4 py-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <span className="rounded-sm bg-primary-soft px-1.5 py-px text-[11px] font-medium text-primary">
                  MPR
                </span>
                <span className="font-mono text-xs tabular-nums text-muted-foreground">
                  {column.test_num}
                </span>
              </div>
              <p
                className="mt-1 select-text truncate text-[14px] font-semibold text-foreground"
                title={column.test_name || undefined}
              >
                {column.test_name || `#${column.test_num}`}
              </p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                Part {row.part_id || "-"} · Site {row.site_num || "-"}
              </p>
            </div>
            <button type="button" className={PAGER_BTN} onClick={onClose} aria-label="关闭">
              <X size={14} />
            </button>
          </div>
          {/* Limits split into labeled boxes — the inline "low ~ high" line was
              easy to misread with long numbers. Mirrors the column detail card. */}
          <div className="mt-2.5 grid grid-cols-3 gap-2 rounded-md bg-muted px-2.5 py-2 text-xs">
            <div>
              <div className="text-muted-foreground">下限 Low</div>
              <div
                className="mt-0.5 select-text truncate font-mono text-foreground"
                title={column.low_limit || undefined}
              >
                {column.low_limit || "-"}
              </div>
            </div>
            <div>
              <div className="text-muted-foreground">上限 High</div>
              <div
                className="mt-0.5 select-text truncate font-mono text-foreground"
                title={column.high_limit || undefined}
              >
                {column.high_limit || "-"}
              </div>
            </div>
            <div>
              <div className="text-muted-foreground">单位 Unit</div>
              <div className="mt-0.5 select-text truncate text-foreground">
                {column.unit || "-"}
              </div>
            </div>
          </div>
        </div>
        {!details && !error && (
          <div className="flex items-center justify-center gap-2 px-4 py-14 text-sm text-muted-foreground">
            <Loader2 size={16} className="animate-spin" />
            正在从源文件读取全部 pin 结果…
          </div>
        )}
        {error && (
          <div className="flex items-center justify-center gap-2 px-4 py-14 text-sm text-danger">
            <AlertCircle size={16} />
            <span className="select-text break-all">{error}</span>
          </div>
        )}
        {details && (
          <>
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-border px-4 py-2 text-xs text-muted-foreground">
              <span className="tabular-nums">
                共 {pins.length.toLocaleString()} pin
                {failCount > 0 ? (
                  <>
                    {" "}
                    · <span className="font-semibold text-danger">{failCount} fail</span>
                  </>
                ) : (
                  " · 全部通过"
                )}
              </span>
              {stats && (
                <span className="font-mono tabular-nums">
                  min {stats.min} / avg {stats.avg} / max {stats.max}
                  {details.unit ? ` ${details.unit}` : ""}
                </span>
              )}
              {failCount > 0 && (
                <label className="ml-auto flex cursor-pointer select-none items-center gap-1.5">
                  <input
                    type="checkbox"
                    checked={failOnly}
                    onChange={(event) => setFailOnly(event.target.checked)}
                    className="accent-primary"
                  />
                  仅看 Fail
                </label>
              )}
            </div>
            <div className="min-h-0 flex-1 overflow-auto">
              {shown.length === 0 ? (
                <div className="px-4 py-10 text-center text-xs text-muted-foreground">
                  没有匹配的 pin。
                </div>
              ) : (
                <table className="w-full border-collapse text-xs" aria-label="pin 结果列表">
                  <thead className="sticky top-0 bg-muted">
                    <tr className="text-left text-[11px] text-muted-foreground">
                      <th className="px-4 py-1.5 font-medium">#</th>
                      <th className="px-2 py-1.5 font-medium">PMR</th>
                      <th className="px-2 py-1.5 font-medium">Pin 名称</th>
                      <th className="px-2 py-1.5 text-right font-medium">
                        值{details.unit ? ` (${details.unit})` : ""}
                      </th>
                      <th className="px-4 py-1.5 text-right font-medium">P/F</th>
                    </tr>
                  </thead>
                  <tbody>
                    {shown.map(({ pin, index }) => (
                      <tr
                        key={index}
                        className={`border-t border-border ${pin.status === "F" ? "bg-danger-soft" : ""}`}
                      >
                        <td className="px-4 py-1 font-mono tabular-nums text-muted-foreground">
                          {index + 1}
                        </td>
                        <td className="px-2 py-1 font-mono tabular-nums text-muted-foreground">
                          {pin.pmr_index || "-"}
                        </td>
                        <td
                          className="max-w-[180px] select-text truncate px-2 py-1 text-foreground"
                          title={pin.pin_name || undefined}
                        >
                          {pin.pin_name || "-"}
                        </td>
                        <td
                          className={`select-text px-2 py-1 text-right font-mono tabular-nums ${
                            pin.status === "F" ? "font-semibold text-danger" : "text-foreground"
                          }`}
                        >
                          {pin.value || "-"}
                        </td>
                        <td
                          className={`px-4 py-1 text-right font-mono ${
                            pin.status === "F"
                              ? "font-semibold text-danger"
                              : pin.status === "P"
                                ? "text-success"
                                : "text-muted-foreground"
                          }`}
                        >
                          {pin.status || "-"}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              {hiddenCount > 0 && (
                <div className="px-2 py-2 text-center">
                  <button
                    type="button"
                    onClick={() => setDisplayLimit((limit) => limit + PIN_DISPLAY_CAP)}
                    className="rounded-md border border-border-strong bg-card px-3 py-1 text-[11px] text-muted-foreground transition hover:bg-muted hover:text-foreground"
                  >
                    继续加载 {PIN_DISPLAY_CAP.toLocaleString()} 个（剩余 {hiddenCount.toLocaleString()}）
                  </button>
                </div>
              )}
            </div>
            <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
              {exportError && (
                <span className="mr-auto select-text text-xs text-danger">{exportError}</span>
              )}
              <button
                type="button"
                className={BTN_SECONDARY}
                onClick={() => void exportXlsx()}
                disabled={exporting || pins.length === 0}
              >
                {exporting ? (
                  <>
                    <Loader2 size={14} className="animate-spin" />
                    导出中…
                  </>
                ) : exported ? (
                  "已导出 ✓"
                ) : (
                  <>
                    <Download size={14} />
                    导出 Excel
                  </>
                )}
              </button>
              <button type="button" className={BTN_PRIMARY} onClick={onClose}>
                关闭
              </button>
            </div>
          </>
        )}
      </div>
    </div>,
    document.body
  );
}

// Modal multi-select for choosing which test-item columns to show. Fuzzy search
// narrows the list; select/clear act on the matches without discarding selections
// from earlier searches. An empty selection means "show all".
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
  const [draft, setDraft] = useState<Set<string>>(() => new Set(applied));
  const [query, setQuery] = useState("");
  const [displayLimit, setDisplayLimit] = useState(FILTER_DISPLAY_CAP);

  // Re-seed the draft once the column list arrives (it may load after the dialog opens).
  // An empty applied selection stays visually empty because it means "no filter".
  useEffect(() => {
    setDraft(new Set(applied));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [columns]);

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
      className="overlay-fade fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
      onClick={onClose}
      role="presentation"
    >
      <div
        className="pop-in flex max-h-[80vh] w-[560px] max-w-full flex-col overflow-hidden rounded-xl border border-border bg-card shadow-overlay"
        onClick={(event) => event.stopPropagation()}
        role="dialog"
        aria-label="筛选测试项"
      >
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <strong className="text-[15px] font-semibold text-foreground">筛选测试项</strong>
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
              {draft.size === 0
                ? "未筛选，当前显示全部"
                : `已选 ${draft.size.toLocaleString()} / ${columns.length.toLocaleString()}`}
            </span>
            <span className="flex gap-3">
              <button type="button" className="text-primary hover:underline" onClick={selectMatches}>
                {needle ? "选择匹配项" : "选择全部"}
              </button>
              <button type="button" className="text-primary hover:underline" onClick={clearMatches}>
                {needle ? "取消匹配项" : "取消全部"}
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
            onClick={() =>
              onConfirm(
                columns.filter((column) => draft.has(column.key)).map((column) => column.key)
              )
            }
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
    return <EmptyState icon={SearchX} title="没有搜索结果" body="尝试搜索 record type、字段名或字段值。" />;
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
        icon={Inbox}
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
              <td className={`${TD} ${MONO}`}>
                {field.inherited_value !== undefined ? (
                  <span
                    className="italic text-muted-foreground"
                    title="STDF v4 允许该字段在同一 TEST_NUM 后续 PTR/MPR 中省略，值继承自首条记录"
                  >
                    {displayValue(field.name, field.inherited_value)}
                    <span className="ml-1.5 rounded-sm bg-primary-soft px-1 py-px text-[11px] font-medium not-italic text-primary">
                      继承
                    </span>
                  </span>
                ) : (
                  displayValue(field.name, field.value)
                )}
              </td>
              <td className={TD}>{field.description || "未提供"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function EmptyState({
  icon: Icon,
  spin = false,
  title,
  body
}: {
  icon?: LucideIcon;
  spin?: boolean;
  title: string;
  body: string;
}) {
  return (
    <div className="flex min-h-[160px] flex-1 flex-col items-center justify-center rounded-xl border border-dashed border-border-strong p-6 text-center text-muted-foreground">
      {Icon && (
        <div className="mb-3 flex h-11 w-11 items-center justify-center rounded-xl bg-muted">
          <Icon size={20} className={spin ? "animate-spin" : undefined} aria-hidden="true" />
        </div>
      )}
      <strong className="text-balance text-foreground">{title}</strong>
      <p className="mt-1.5 max-w-[340px] text-pretty text-[13px] leading-relaxed">{body}</p>
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
