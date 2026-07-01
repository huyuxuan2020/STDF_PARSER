import { useEffect, useRef, useState, type ReactNode } from "react";
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

const RELEASES_API = "https://api.github.com/repos/huyuxuan2020/STDF_PARSER/releases?per_page=30";

/**
 * Parse a semver tag ("v0.1.2", "0.1.2") to a comparable tuple. Non-numeric
 * segments read as 0, so exotic tags sort at the low end rather than throw.
 */
function parseVersion(raw: string): number[] {
  return raw
    .replace(/^v/, "")
    .split(".")
    .map((n) => {
      const v = parseInt(n, 10);
      return Number.isFinite(v) ? v : 0;
    });
}

function compareVersion(a: string, b: string): number {
  const pa = parseVersion(a);
  const pb = parseVersion(b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

type GhRelease = {
  tag_name: string;
  body: string | null;
  draft: boolean;
  prerelease: boolean;
};

/**
 * Combine release notes for every published version in (fromVersion, toVersion].
 * Used to catch the user up when they skipped several releases in a row.
 * Falls back to null on any fetch/parse failure — the caller can then use the
 * single-version body from the updater plugin.
 */
async function fetchCombinedNotes(
  fromVersion: string,
  toVersion: string
): Promise<string | null> {
  try {
    const res = await fetch(RELEASES_API);
    if (!res.ok) return null;
    const releases = (await res.json()) as GhRelease[];
    const relevant = releases
      .filter((r) => !r.draft && !r.prerelease && r.body && r.body.trim() !== "")
      .filter(
        (r) =>
          compareVersion(r.tag_name, fromVersion) > 0 &&
          compareVersion(r.tag_name, toVersion) <= 0
      )
      .sort((a, b) => compareVersion(b.tag_name, a.tag_name));
    if (relevant.length === 0) return null;
    // A single-release window matches the plugin's own body — no gain, and
    // avoids a redundant "## v0.1.2" header on top.
    if (relevant.length === 1) return relevant[0].body!.trim();
    return relevant
      .map((r) => `## v${r.tag_name.replace(/^v/, "")}\n\n${r.body!.trim()}`)
      .join("\n\n---\n\n");
  } catch {
    return null;
  }
}

/**
 * Best-effort classifier for update-check failures caused by no network
 * (offline, DNS, firewall, timeout). We can't reliably distinguish from
 * a string message across Tauri/reqwest/OS variants, so err on the side
 * of "looks like a connection problem" — the friendly copy is safe even
 * when we're wrong, and never masks a real bug in the updater config.
 */
function isLikelyOfflineError(raw: string): boolean {
  const s = raw.toLowerCase();
  return [
    "network",
    "connection",
    "connect ",
    "connect(",
    "dns",
    "resolve",
    "timeout",
    "timed out",
    "offline",
    "unreachable",
    "no such host",
    "reqwest",
    "io error",
    "could not connect",
    "failed to send",
    "sending request",
    "error sending",
    "certificate",
    "tls",
    "socket",
    "nodename",
  ].some((needle) => s.includes(needle));
}

/* ------------------------------------------------------------------ *
 * Release notes are Markdown (the GitHub release body). They ship a   *
 * small, controlled subset — headings, bullet lists, **bold** and     *
 * `code` — so we render that subset inline rather than pulling in a    *
 * full Markdown dependency. Anything unrecognized falls through as     *
 * plain text.                                                          *
 * ------------------------------------------------------------------ */

// Render inline **bold** and `code` spans within a single line of text.
function renderInline(text: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = /\*\*(.+?)\*\*|`([^`]+)`/g;
  let last = 0;
  let token = 0;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > last) {
      nodes.push(<span key={`${keyPrefix}-t${token}`}>{text.slice(last, match.index)}</span>);
    }
    if (match[1] !== undefined) {
      nodes.push(
        <strong key={`${keyPrefix}-b${token}`} className="font-semibold text-foreground">
          {match[1]}
        </strong>
      );
    } else if (match[2] !== undefined) {
      nodes.push(
        <code
          key={`${keyPrefix}-c${token}`}
          className="rounded bg-border/60 px-1 py-0.5 font-mono text-[11px] text-foreground"
        >
          {match[2]}
        </code>
      );
    }
    last = match.index + match[0].length;
    token += 1;
  }
  if (last < text.length) {
    nodes.push(<span key={`${keyPrefix}-t${token}`}>{text.slice(last)}</span>);
  }
  return nodes;
}

function ReleaseNotes({ body }: { body: string }) {
  const lines = body.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let bullets: string[] = [];
  let key = 0;

  const flushBullets = () => {
    if (bullets.length === 0) return;
    const items = bullets;
    bullets = [];
    blocks.push(
      <ul key={`ul-${key++}`} className="flex list-none flex-col gap-1">
        {items.map((item, i) => (
          <li key={i} className="flex gap-2 text-foreground">
            <span aria-hidden="true" className="select-none text-primary">
              •
            </span>
            <span className="min-w-0">{renderInline(item, `li-${key}-${i}`)}</span>
          </li>
        ))}
      </ul>
    );
  };

  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (line === "") {
      flushBullets();
      continue;
    }
    // Horizontal rule: `---` or `***`. Used to separate versions when the
    // notes body was combined from multiple releases.
    if (/^(-{3,}|\*{3,})$/.test(line)) {
      flushBullets();
      blocks.push(<hr key={`hr-${key++}`} className="my-1 border-border" />);
      continue;
    }
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    if (heading) {
      flushBullets();
      blocks.push(
        <p key={`h-${key++}`} className="mt-1 font-semibold text-foreground first:mt-0">
          {renderInline(heading[1], `h-${key}`)}
        </p>
      );
      continue;
    }
    const bullet = /^[-*]\s+(.*)$/.exec(line);
    if (bullet) {
      bullets.push(bullet[1]);
      continue;
    }
    flushBullets();
    blocks.push(
      <p key={`p-${key++}`} className="text-muted-foreground">
        {renderInline(line, `p-${key}`)}
      </p>
    );
  }
  flushBullets();

  return <div className="flex flex-col gap-2 text-xs leading-relaxed text-foreground">{blocks}</div>;
}

type Phase =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "available"; update: Update; notes: string }
  | { kind: "downloading"; pct: number | null }
  | { kind: "installing" }
  | { kind: "ready" }
  | { kind: "uptodate" }
  | { kind: "error"; message: string };

type PendingUpdate = { update: Update; notes: string };

export function UpdateChecker() {
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  // Tracks whether the user triggered the check, so the silent startup check
  // can stay quiet when already up to date but a manual click still reports it.
  const [manual, setManual] = useState(false);
  // Stashed update from the silent startup check. When set, the rail button
  // shows a red dot — no dialog, no auto-download. Clicking the button then
  // opens the dialog with this update instead of running a fresh check. The
  // `notes` field is a combined body across every skipped version (empty
  // string if the enrichment fetch failed, in which case update.body is used).
  const [pendingUpdate, setPendingUpdate] = useState<PendingUpdate | null>(null);
  // Once the user clicks the rail button, skip the pending silent check —
  // otherwise it races the click and flips `manual` back to false mid-check,
  // hiding the dialog while the button spins on the second in-flight check.
  const manualTaken = useRef(false);

  async function runCheck(isManual: boolean) {
    if (!inTauri) return;
    if (isManual) {
      manualTaken.current = true;
    } else if (manualTaken.current) {
      return;
    }
    setManual(isManual);
    setPhase({ kind: "checking" });
    try {
      const update = await check();
      // The user's manual click may have arrived while check() was in flight;
      // let that run own the visible state instead of this silent one.
      if (!isManual && manualTaken.current) return;
      if (update) {
        // Enrich the notes with every version between the installed one and
        // the target — the user may have skipped several releases. If this
        // call fails (rate limit, offline), fall back to the plugin's own
        // single-version body from latest.json.
        const combined = update.currentVersion
          ? await fetchCombinedNotes(update.currentVersion, update.version)
          : null;
        // Same guard as above for a click that landed during the API call.
        if (!isManual && manualTaken.current) return;
        const notes = combined ?? update.body ?? "";
        if (isManual) {
          setPhase({ kind: "available", update, notes });
        } else {
          // Silent: surface the update via the red dot on the rail button
          // and stay out of the way until the user clicks.
          setPendingUpdate({ update, notes });
          setPhase({ kind: "idle" });
        }
      } else if (isManual) {
        setPhase({ kind: "uptodate" });
      } else {
        // Silent + no update: stay quiet.
        setPhase({ kind: "idle" });
      }
    } catch (err) {
      if (isManual) {
        setPhase({ kind: "error", message: String(err) });
      } else {
        // Silent errors stay silent (no network, offline, etc.).
        setPhase({ kind: "idle" });
      }
    }
  }

  // Silent check shortly after launch.
  useEffect(() => {
    if (!inTauri) return;
    const timer = window.setTimeout(() => void runCheck(false), 1500);
    return () => window.clearTimeout(timer);
  }, []);

  // Clicking the rail button: if a silent check already found an update,
  // just open the dialog with that update; otherwise kick off a fresh check.
  function handleManualClick() {
    if (pendingUpdate) {
      manualTaken.current = true;
      setManual(true);
      setPhase({ kind: "available", update: pendingUpdate.update, notes: pendingUpdate.notes });
      return;
    }
    void runCheck(true);
  }

  async function downloadAndInstall(update: Update) {
    let total = 0;
    let received = 0;
    // User committed to installing; clear the red-dot indicator so it
    // doesn't linger after they pick "稍后重启".
    setPendingUpdate(null);
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
        aria-label={pendingUpdate ? "有新版本可用" : "检查更新"}
        title={pendingUpdate ? `有新版本 v${pendingUpdate.update.version}` : "检查更新"}
        disabled={checking || phase.kind === "downloading" || phase.kind === "installing"}
        onClick={handleManualClick}
        className={`relative ${RAIL_ITEM} ${RAIL_ITEM_IDLE} disabled:opacity-50`}
      >
        {checking ? (
          <Loader2 size={19} className="animate-spin" aria-hidden="true" />
        ) : (
          <RefreshCw size={19} aria-hidden="true" />
        )}
        <span>更新</span>
        {pendingUpdate && !checking && (
          <span
            aria-hidden="true"
            className="absolute top-1.5 right-2 h-2.5 w-2.5 rounded-full bg-danger"
          />
        )}
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
                  {phase.notes && (
                    <div className="max-h-48 overflow-auto rounded-md border border-border bg-muted/40 px-3 py-2.5">
                      <ReleaseNotes body={phase.notes} />
                    </div>
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

              {phase.kind === "error" &&
                (isLikelyOfflineError(phase.message) ? (
                  <div className="flex flex-col gap-1.5">
                    <p className="text-muted-foreground">
                      当前无法连接到更新服务器，请检查网络后重试。
                    </p>
                    <p className="text-xs text-muted-foreground">
                      离线也不影响 STDF 解析等本地功能，可以正常继续使用。
                    </p>
                  </div>
                ) : (
                  <p className="text-danger">检查更新失败：{phase.message}</p>
                ))}
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
