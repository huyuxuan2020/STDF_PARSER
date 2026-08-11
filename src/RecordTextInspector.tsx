import {
  AlertCircle,
  CheckCircle2,
  Download,
  FileText,
  Hourglass,
  Loader2,
  ScanText
} from "lucide-react";
import type { DtrPreview, GdrPreview } from "./types";

export interface TextParseState<TPreview> {
  phase: "idle" | "parsing" | "done" | "error";
  count: number;
  previews: TPreview[];
  message: string;
  saving: boolean;
  saved: boolean;
}

type RecordTextInspectorProps = {
  onParse(): void;
  onDownload(): void;
  parseComplete: boolean;
} & (
  | { kind: "DTR"; state: TextParseState<DtrPreview> }
  | { kind: "GDR"; state: TextParseState<GdrPreview> }
);

const ACTION_PRIMARY =
  "inline-flex h-8 items-center gap-1.5 rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] hover:bg-primary-hover active:scale-[0.98] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";
const ACTION_SECONDARY =
  "inline-flex h-8 items-center gap-1.5 rounded-md border border-border-strong bg-card px-3 text-xs font-medium text-muted-foreground transition duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] hover:bg-muted hover:text-foreground active:scale-[0.98] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";

export function RecordTextInspector(props: RecordTextInspectorProps) {
  const { kind, state, onParse, onDownload, parseComplete } = props;
  const parsing = state.phase === "parsing";
  const done = state.phase === "done";
  const failed = state.phase === "error";
  const empty = done && state.count === 0;
  const label = kind === "DTR" ? "DTR 文本" : "GDR 数据";
  const chipTone = failed
    ? "bg-danger-soft text-danger"
    : empty
      ? "bg-muted text-muted-foreground"
      : done
        ? "bg-success-soft text-success"
        : "bg-primary-soft text-primary";
  const ChipIcon = failed ? AlertCircle : empty ? FileText : done ? CheckCircle2 : ScanText;
  const statusText = done
    ? `已解析，共 ${state.count.toLocaleString()} 条 ${label}`
    : parsing
      ? `正在扫描 ${label}…`
      : failed
        ? `${kind} 解析失败`
        : `${label}未解析`;
  const hintText = done
    ? empty
      ? `整份文件没有扫描到 ${kind} 记录。`
      : "TXT 下载内容已按 Part/Site 上下文分段。"
    : parsing
      ? "正在重新扫描整份文件。"
      : parseComplete
        ? `解析全部 ${kind} 后可预览和下载。`
        : "等待 STDF 主解析完成。";

  return (
    <div className="fade-rise flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-border/70 pb-3">
        <div key={state.phase} className="fade-rise flex min-w-0 items-center gap-3">
          <div className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-md ${chipTone}`}>
            {parsing ? (
              <Loader2 size={16} className="animate-spin" aria-hidden="true" />
            ) : (
              <ChipIcon size={16} aria-hidden="true" />
            )}
          </div>
          <div className="min-w-0">
            <strong className="block text-[13px] font-semibold text-foreground">{statusText}</strong>
            <p className="mt-0.5 text-xs leading-relaxed text-muted-foreground">{hintText}</p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {done ? (
            <>
              <button
                type="button"
                className={ACTION_PRIMARY}
                onClick={onDownload}
                disabled={state.saving || state.count === 0}
              >
                {state.saving ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
                {state.saving ? "保存中…" : state.saved ? "已保存 ✓" : "下载 TXT"}
              </button>
              <button
                type="button"
                className={ACTION_SECONDARY}
                onClick={onParse}
                disabled={!parseComplete}
              >
                <ScanText size={14} />
                重新解析
              </button>
            </>
          ) : (
            <button
              type="button"
              className={ACTION_PRIMARY}
              onClick={onParse}
              disabled={!parseComplete || parsing}
            >
              {parsing ? (
                <Loader2 size={14} className="animate-spin" />
              ) : parseComplete ? (
                <ScanText size={14} />
              ) : (
                <Hourglass size={14} />
              )}
              {parsing ? "解析中…" : `解析 ${label}`}
            </button>
          )}
        </div>
      </div>

      {parsing && (
        <div className="relative mt-3 h-1 shrink-0 overflow-hidden rounded-full bg-muted">
          <div className="indeterminate-sweep absolute inset-y-0 left-0 w-1/3 rounded-full bg-primary" />
        </div>
      )}

      {state.message && (
        <p
          className="mt-3 shrink-0 border-l-2 border-danger bg-danger-soft/70 px-3 py-2 text-left text-xs leading-relaxed text-danger [overflow-wrap:anywhere]"
          role="alert"
        >
          {state.message}
        </p>
      )}

      {done && !empty ? (
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
          <div className="flex shrink-0 items-center justify-between gap-3 border-b border-border/70 py-3">
            <strong className="text-xs font-semibold text-foreground">文件前 3 条预览</strong>
            <span className="text-[11px] tabular-nums text-muted-foreground">
              已显示 {state.previews.length.toLocaleString()} 条
            </span>
          </div>
          <div className="min-h-0 flex-1 overflow-auto overscroll-contain pr-1">
            {props.kind === "DTR" ? (
              <DtrPreviewList previews={props.state.previews} />
            ) : (
              <GdrPreviewList previews={props.state.previews} />
            )}
          </div>
        </div>
      ) : (
        !parsing && (
          <div className="flex min-h-[160px] flex-1 items-center justify-center text-center text-xs text-muted-foreground">
            {empty
              ? `没有可预览的 ${kind} 记录`
              : parseComplete
                ? `解析后显示文件前 3 条 ${kind}`
                : "主解析完成后可继续"}
          </div>
        )
      )}
    </div>
  );
}

function PreviewContext({ scope, parts }: Pick<DtrPreview, "scope" | "parts">) {
  const scopeLabel = scope === "part" ? "PART" : scope === "shared" ? "SHARED" : "UNASSIGNED";
  const scopeTone =
    scope === "part"
      ? "bg-success-soft text-success"
      : scope === "shared"
        ? "bg-primary-soft text-primary"
        : "bg-muted text-muted-foreground";
  return (
    <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] leading-relaxed text-muted-foreground">
      <span className={`inline-flex h-5 items-center rounded-sm px-1.5 font-semibold ${scopeTone}`}>{scopeLabel}</span>
      {parts.map((part, index) => (
        <span
          key={`${part.part_id}:${part.head_num}:${part.site_num}:${index}`}
          className="font-mono [overflow-wrap:anywhere]"
        >
          PART_ID={part.part_id} · HEAD_NUM={part.head_num} · SITE_NUM={part.site_num}
        </span>
      ))}
    </div>
  );
}

function DtrPreviewList({ previews }: { previews: DtrPreview[] }) {
  return (
    <div>
      {previews.map((preview) => (
        <section
          key={`${preview.index}:${preview.offset}`}
          className="border-b border-border/70 py-4 last:border-b-0"
        >
          <div className="flex items-baseline justify-between gap-3">
            <strong className="text-xs font-semibold text-foreground">DTR #{preview.index}</strong>
            <span className="font-mono text-[11px] tabular-nums text-muted-foreground">offset {preview.offset}</span>
          </div>
          <PreviewContext scope={preview.scope} parts={preview.parts} />
          <pre className="mt-3 whitespace-pre-wrap break-words border-l-2 border-primary/35 bg-muted/35 px-3 py-2.5 font-mono text-xs leading-relaxed text-foreground">
            {preview.text || "(空文本)"}
          </pre>
        </section>
      ))}
    </div>
  );
}

function GdrPreviewList({ previews }: { previews: GdrPreview[] }) {
  return (
    <div>
      {previews.map((preview) => (
        <section
          key={`${preview.index}:${preview.offset}`}
          className="border-b border-border/70 py-4 last:border-b-0"
        >
          <div className="flex items-baseline justify-between gap-3">
            <strong className="text-xs font-semibold text-foreground">
              GDR #{preview.index} · FLD_CNT {preview.field_count}
            </strong>
            <span className="font-mono text-[11px] tabular-nums text-muted-foreground">offset {preview.offset}</span>
          </div>
          <PreviewContext scope={preview.scope} parts={preview.parts} />
          <div className="mt-3 border-y border-border/70">
            {preview.fields.map((field) => (
              <div
                key={field.index}
                className="grid min-h-[34px] grid-cols-[minmax(112px,0.45fr)_70px_minmax(0,1.4fr)] items-start border-b border-border/70 px-2 py-2 text-xs transition-colors duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] last:border-b-0 hover:bg-muted/40"
              >
                <span className="font-mono text-muted-foreground">GEN_DATA[{field.index}]</span>
                <span className="font-mono text-muted-foreground">{field.field_type}</span>
                <span className="min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere] text-foreground">
                  {field.value || "(空值)"}
                </span>
              </div>
            ))}
          </div>
          {preview.omitted_field_count > 0 && (
            <p className="mt-2 text-[11px] text-muted-foreground">
              其余 {preview.omitted_field_count.toLocaleString()} 项请下载完整 TXT
            </p>
          )}
        </section>
      ))}
    </div>
  );
}
