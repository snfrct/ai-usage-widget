import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type ToolStatus = "ok" | "not_logged_in" | "auth_expired" | "error";
type DataSource = "live" | "cached" | "local_estimate" | "none";

interface UsageWindow {
  used_pct: number;
  resets_label: string;
  resets_at: string | null;
}

interface ToolUsage {
  tool: string;
  status: ToolStatus;
  five_hour: UsageWindow | null;
  weekly: UsageWindow | null;
  monthly: UsageWindow | null;
  note: string | null;
  source: DataSource;
  fetched_at: string;
  message: string | null;
}

interface AllUsage {
  claude: ToolUsage | null;
  codex: ToolUsage | null;
  cursor: ToolUsage | null;
}

interface ToolConfig {
  key: keyof AllUsage;
  label: string;
  dual: boolean;
  loginCommand?: string;
}

const TOOLS: ToolConfig[] = [
  { key: "claude", label: "Claude Code", dual: true, loginCommand: "claude_login" },
  { key: "codex", label: "Codex", dual: true, loginCommand: "codex_login" },
  { key: "cursor", label: "Cursor", dual: false },
];

const rowsEl = document.querySelector<HTMLDivElement>("#rows")!;

function pct(n: number): string {
  return `${Math.max(0, Math.min(100, Math.round(n)))}%`;
}

function statsLineDual(usage: ToolUsage): string {
  const fh = usage.five_hour;
  const wk = usage.weekly;
  const left = fh ? `${pct(fh.used_pct)} · ${fh.resets_label}` : "—";
  const right = wk ? `${pct(wk.used_pct)} · ${wk.resets_label}` : "—";
  return `${left}<span class="sep">/</span>${right}`;
}

function segmentTitle(windowLabel: string, w: UsageWindow | null | undefined): string {
  if (!w) return `${windowLabel}: no data`;
  const reset = w.resets_label ? `, resets ${w.resets_label}` : "";
  return `${windowLabel}: ${pct(w.used_pct)} used${reset}`;
}

function barDual(usage: ToolUsage | null): string {
  const fh = usage?.five_hour ?? null;
  const wk = usage?.weekly ?? null;
  return `
    <div class="bar dual">
      <div class="segment five-hour" title="${segmentTitle("5-hour window", fh)}"><div class="fill" style="width:${fh?.used_pct ?? 0}%"></div></div>
      <div class="segment weekly" title="${segmentTitle("Weekly window", wk)}"><div class="fill" style="width:${wk?.used_pct ?? 0}%"></div></div>
    </div>`;
}

type WindowKey = "five_hour" | "weekly" | "monthly";

function segmentClass(key: WindowKey): string {
  return key === "five_hour" ? "five-hour" : key;
}

function segmentLabel(key: WindowKey): string {
  if (key === "five_hour") return "5-hour window";
  if (key === "weekly") return "Weekly window";
  return "Monthly usage";
}

function statsLineSingleFor(usage: ToolUsage, key: WindowKey): string {
  const w = usage[key];
  if (!w) return "—";
  const prefix = key === "monthly" ? "resets " : "";
  return w.resets_label ? `${pct(w.used_pct)} · ${prefix}${w.resets_label}` : pct(w.used_pct);
}

function barSingleFor(usage: ToolUsage | null, key: WindowKey): string {
  const w = usage?.[key] ?? null;
  return `
    <div class="bar single">
      <div class="segment ${segmentClass(key)}" title="${segmentTitle(segmentLabel(key), w)}"><div class="fill" style="width:${w?.used_pct ?? 0}%"></div></div>
    </div>`;
}

type Layout = { mode: "dual" } | { mode: "single"; key: WindowKey };

/// Codex in particular can return several shapes depending on plan and
/// current backend rollout — free-tier accounts get a single ~30-day
/// (monthly) window; some paid accounts have only a weekly window active
/// with no 5-hour window at all (secondary_window: null); the "classic"
/// paid shape has both. The layout follows whatever data actually came
/// back — dual only when *both* five_hour and weekly are present, a single
/// full-width bar for whichever one window exists otherwise — rather than
/// assuming a fixed shape per tool. `config.dual` is only the fallback
/// shape used before any data has arrived yet (or for error/logged-out
/// states with no window data at all).
function resolveLayout(config: ToolConfig, usage: ToolUsage | null): Layout {
  if (usage) {
    const present: WindowKey[] = [];
    if (usage.five_hour) present.push("five_hour");
    if (usage.weekly) present.push("weekly");
    if (usage.monthly) present.push("monthly");

    if (present.includes("five_hour") && present.includes("weekly")) return { mode: "dual" };
    if (present.length === 1) return { mode: "single", key: present[0] };
  }
  return config.dual ? { mode: "dual" } : { mode: "single", key: "monthly" };
}

function renderRow(config: ToolConfig, usage: ToolUsage | null): string {
  const muted = !usage || usage.status !== "ok";
  const isError = usage?.status === "error" || usage?.status === "auth_expired" || usage?.status === "not_logged_in";
  const layout = resolveLayout(config, usage);

  let statsHtml: string;
  if (!usage) {
    statsHtml = "loading…";
  } else if (usage.status === "ok") {
    statsHtml = layout.mode === "dual" ? statsLineDual(usage) : statsLineSingleFor(usage, layout.key);
  } else if (usage.status === "not_logged_in" && config.loginCommand) {
    statsHtml = `<span class="login-link" data-login="${config.loginCommand}">${usage.message ?? "Not logged in"}</span>`;
  } else {
    statsHtml = usage.message ?? "Unavailable";
  }

  const bar = layout.mode === "dual" ? barDual(usage) : barSingleFor(usage, layout.key);
  const note = usage?.note ? `<div class="tool-note">${usage.note}</div>` : "";
  const estimateBadge = usage?.source === "local_estimate" ? " (est.)" : "";

  return `
    <div class="tool-row ${muted ? "is-muted" : ""} ${isError ? "is-error" : ""}" data-tool="${config.key}">
      <div class="tool-header">
        <span class="tool-name">${config.label}</span>
        <span class="tool-stats">${statsHtml}${estimateBadge}</span>
      </div>
      ${bar}
      ${note}
    </div>`;
}

const updatedAtEl = document.querySelector<HTMLDivElement>("#updated-at")!;
let lastUsage: AllUsage | null = null;

function formatRelativeTime(iso: string): string {
  const seconds = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  return `${hours}h ago`;
}

function renderUpdatedAt(usage: AllUsage) {
  const timestamps = TOOLS.map((c) => usage[c.key]?.fetched_at).filter((t): t is string => !!t);
  if (timestamps.length === 0) {
    updatedAtEl.textContent = "";
    return;
  }
  const latest = timestamps.reduce((a, b) => (new Date(a) > new Date(b) ? a : b));
  updatedAtEl.textContent = `Updated ${formatRelativeTime(latest)}`;
}

function render(usage: AllUsage) {
  lastUsage = usage;
  rowsEl.innerHTML = TOOLS.map((config) => renderRow(config, usage[config.key])).join("");

  rowsEl.querySelectorAll<HTMLElement>("[data-login]").forEach((el) => {
    el.addEventListener("click", () => {
      const command = el.dataset.login;
      if (command) void invoke(command);
    });
  });

  renderUpdatedAt(usage);
}

async function bootstrap() {
  document.querySelector("#quit-btn")?.addEventListener("click", () => {
    void invoke("quit_app");
  });

  const cached = await invoke<AllUsage>("get_usage");
  render(cached);

  await listen<AllUsage>("usage-updated", (event) => {
    render(event.payload);
  });

  void invoke<AllUsage>("refresh_now").then(render);

  // Keeps the "X ago" text live between actual data refreshes.
  setInterval(() => {
    if (lastUsage) renderUpdatedAt(lastUsage);
  }, 30_000);
}

window.addEventListener("DOMContentLoaded", () => {
  void bootstrap();
});
