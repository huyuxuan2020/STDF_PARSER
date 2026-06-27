import { useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { Download, Loader2, RefreshCw, X } from "lucide-react";

/* ------------------------------------------------------------------ *
 * Auto-update: silently checks GitHub Releases on launch and exposes  *
 * a manual "检查更新" rail button. The updater plugin throws outside   *
 * a Tauri runtime (vite dev in a plain browser, vitest), so every     *
 * call is guarded and the component renders nothing when unavailable. *
 * ------------------------------------------------------------------ */

const RAIL_ITEM =
  "flex h-14 w-14 flex-col items-center justify-center gap-1 rounded-xl text-[10px] font-medium transition duration-100 active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
const RAIL_ITEM_IDLE = "text-muted-foreground hover:bg-border/40 hover:text-foreground";
const BTN_BASE =
  "inline-flex min-h-[40px] items-center justify-center gap-2 rounded-md px-3.5 text-sm font-medium transition duration-100 active:scale-[0.98] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";
const BTN_PRIMARY = `${BTN_BASE} bg-primary text-primary-foreground hover:bg-primary-hover`;
const BTN_SECONDARY = `${BTN_BASE} whitespace-nowrap border border-border-strong bg-card text-muted-foreground hover:bg-muted hover:text-foreground`;

// Only run inside the Tauri webview; the global is injected by the runtime.
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type Phase =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "available"; update: Update }
  | { kind: "downloading"; pct: number | null }
  | { kind: "installing" }
  | { kind: "ready" }
  | { kind: "uptodate" }
  | { kind: "error"; message: string };

export function UpdateChecker() {
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  // Tracks whether the user triggered the check, so the silent startup check
  // can stay quiet when already up to date but a manual click still reports it.
  const [manual, setManual] = useState(false);

  async function runCheck(isManual: boolean) {
    if (!inTauri) return;
    setManual(isManual);
    setPhase({ kind: "checking" });
    try {
      const update = await check();
      if (update) {
        setPhase({ kind: "available", update });
      } else {
        setPhase({ kind: "uptodate" });
      }
    } catch (err) {
      setPhase({ kind: "error", message: String(err) });
    }
  }

  // Silent check shortly after launch.
  useEffect(() => {
    if (!inTauri) return;
    const timer = window.setTimeout(() => void runCheck(false), 1500);
    return () => window.clearTimeout(timer);
  }, []);

  async function downloadAndInstall(update: Update) {
    let total = 0;
    let received = 0;
    setPhase({ kind: "downloading", pct: null });
    try {
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? 0;
            break;
          case "Progress":
            received += event.data.chunkLength;
            setPhase({
              kind: "downloading",
              pct: total > 0 ? Math.round((received / total) * 100) : null
            });
            break;
          case "Finished":
            setPhase({ kind: "installing" });
            break;
        }
      });
      setPhase({ kind: "ready" });
    } catch (err) {
      setPhase({ kind: "error", message: String(err) });
    }
  }

  if (!inTauri) return null;

  // Modal is shown whenever there is something worth surfacing. A silent
  // startup check that finds nothing leaves phase === "uptodate" with
  // manual === false, which renders nothing.
  const showModal =
    phase.kind === "available" ||
    phase.kind === "downloading" ||
    phase.kind === "installing" ||
    phase.kind === "ready" ||
    phase.kind === "error" ||
    (phase.kind === "uptodate" && manual) ||
    (phase.kind === "checking" && manual);

  const checking = phase.kind === "checking";

  return (
    <>
      <button
        type="button"
        aria-label="检查更新"
        title="检查更新"
        disabled={checking || phase.kind === "downloading" || phase.kind === "installing"}
        onClick={() => void runCheck(true)}
        className={`${RAIL_ITEM} ${RAIL_ITEM_IDLE} disabled:opacity-50`}
      >
        {checking ? (
          <Loader2 size={19} className="animate-spin" aria-hidden="true" />
        ) : (
          <RefreshCw size={19} aria-hidden="true" />
        )}
        <span>更新</span>
      </button>

      {showModal && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
          onClick={() => {
            // Allow dismissing only when not mid-download.
            if (phase.kind !== "downloading" && phase.kind !== "installing") {
              setPhase({ kind: "idle" });
            }
          }}
          role="presentation"
        >
          <div
            className="flex w-[420px] max-w-full flex-col overflow-hidden rounded-xl border border-border bg-card shadow-xl"
            onClick={(event) => event.stopPropagation()}
            role="dialog"
            aria-label="软件更新"
          >
            <div className="flex items-center justify-between border-b border-border px-4 py-3">
              <strong className="text-sm font-semibold text-foreground">软件更新</strong>
              {phase.kind !== "downloading" && phase.kind !== "installing" && (
                <button
                  type="button"
                  onClick={() => setPhase({ kind: "idle" })}
                  aria-label="关闭"
                  className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
                >
                  <X size={14} />
                </button>
              )}
            </div>

            <div className="px-4 py-4 text-sm text-foreground">
              {phase.kind === "checking" && (
                <p className="flex items-center gap-2 text-muted-foreground">
                  <Loader2 size={16} className="animate-spin" /> 正在检查更新…
                </p>
              )}

              {phase.kind === "uptodate" && (
                <p className="text-muted-foreground">已经是最新版本。</p>
              )}

              {phase.kind === "available" && (
                <div className="flex flex-col gap-3">
                  <p>
                    发现新版本{" "}
                    <span className="font-semibold text-primary">v{phase.update.version}</span>
                    {phase.update.currentVersion && (
                      <span className="text-muted-foreground">
                        {" "}
                        （当前 v{phase.update.currentVersion}）
                      </span>
                    )}
                    。
                  </p>
                  {phase.update.body && (
                    <pre className="max-h-40 overflow-auto whitespace-pre-wrap rounded-md border border-border bg-muted px-3 py-2 text-xs text-muted-foreground">
                      {phase.update.body}
                    </pre>
                  )}
                </div>
              )}

              {phase.kind === "downloading" && (
                <div className="flex flex-col gap-2">
                  <p className="flex items-center gap-2 text-muted-foreground">
                    <Download size={16} /> 正在下载更新…
                  </p>
                  <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
                    <div
                      className="h-full bg-primary transition-[width] duration-150"
                      style={{ width: `${phase.pct ?? 10}%` }}
                    />
                  </div>
                  {phase.pct != null && (
                    <span className="text-right text-xs tabular-nums text-muted-foreground">
                      {phase.pct}%
                    </span>
                  )}
                </div>
              )}

              {phase.kind === "installing" && (
                <p className="flex items-center gap-2 text-muted-foreground">
                  <Loader2 size={16} className="animate-spin" /> 正在安装…
                </p>
              )}

              {phase.kind === "ready" && (
                <p>更新已安装,重启后生效。</p>
              )}

              {phase.kind === "error" && (
                <p className="text-danger">检查更新失败:{phase.message}</p>
              )}
            </div>

            <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
              {phase.kind === "available" && (
                <>
                  <button
                    type="button"
                    className={BTN_SECONDARY}
                    onClick={() => setPhase({ kind: "idle" })}
                  >
                    稍后
                  </button>
                  <button
                    type="button"
                    className={BTN_PRIMARY}
                    onClick={() => void downloadAndInstall(phase.update)}
                  >
                    下载并安装
                  </button>
                </>
              )}

              {phase.kind === "ready" && (
                <>
                  <button
                    type="button"
                    className={BTN_SECONDARY}
                    onClick={() => setPhase({ kind: "idle" })}
                  >
                    稍后重启
                  </button>
                  <button type="button" className={BTN_PRIMARY} onClick={() => void relaunch()}>
                    立即重启
                  </button>
                </>
              )}

              {(phase.kind === "uptodate" || phase.kind === "error") && (
                <button
                  type="button"
                  className={BTN_SECONDARY}
                  onClick={() => setPhase({ kind: "idle" })}
                >
                  关闭
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </>
  );
}
