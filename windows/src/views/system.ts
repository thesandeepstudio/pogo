// System monitor card — CPU / memory / GPU from the Rust `system_stats`
// command. Disk, network and battery are collected but not shown; the left
// column stays a compact CPU / memory / GPU read-out. Rendered through the
// same card chrome as the old integration cards so it sits naturally in the
// island's left column.

import { h, dot } from "./dom";
import { State } from "../core/state";
import type { SystemStats } from "../core/bridge";

function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

function fmtGB(n: number): string {
  return (n / 1073741824).toFixed(1);
}

function fmtUptime(sec: number): string {
  const d = Math.floor(sec / 86400);
  const hrs = Math.floor((sec % 86400) / 3600);
  const min = Math.floor((sec % 3600) / 60);
  if (d > 0) return `${d}d ${hrs}h`;
  if (hrs > 0) return `${hrs}h ${min}m`;
  return `${min}m`;
}

function loadColor(frac: number): string {
  if (frac < 0.6) return "#22C55E";
  if (frac < 0.85) return "#F5A524";
  return "#F4505E";
}

function meter(frac: number, color: string): HTMLElement {
  const fill = h("div", { class: "sys-meter-fill" });
  fill.style.width = `${Math.round(Math.max(0, Math.min(1, frac)) * 100)}%`;
  fill.style.background = color;
  return h("div", { class: "sys-meter" }, fill);
}

function meterRow(label: string, value: string, frac: number, color: string): HTMLElement {
  return h(
    "div",
    { class: "int-row" },
    h("span", { class: "int-name", text: label }),
    meter(frac, color),
    h("span", { class: "int-amount", style: "color:#c5c8cd", text: value }),
  );
}

function stats(): SystemStats | null {
  const data = State.integrations["system"]?.data;
  return (data as unknown as SystemStats | undefined) ?? null;
}

export function systemCard(): HTMLElement {
  const s = stats();
  const head = h(
    "div",
    { class: "int-head" },
    dot("#38BDF8", 7),
    h("b", { text: "System" }),
    h("span", { text: s?.host || "This PC" }),
  );

  if (!s) {
    return h(
      "div",
      { class: "int-card" },
      head,
      h("div", { class: "int-status" }, dot("#F5A524", 5), h("span", { text: "Reading…" })),
    );
  }

  const cpuFrac = s.cpu / 100;
  const memFrac = s.memTotal ? s.memUsed / s.memTotal : 0;
  const rows = h("div", { class: "int-rows" });
  rows.append(meterRow("CPU", `${Math.round(s.cpu)}%`, cpuFrac, loadColor(cpuFrac)));
  rows.append(
    meterRow(
      "Memory",
      `${fmtBytes(s.memUsed)} / ${fmtBytes(s.memTotal)}`,
      memFrac,
      loadColor(memFrac),
    ),
  );
  if (s.gpu != null) {
    const gpuFrac = s.gpu / 100;
    const vram =
      s.vramUsed != null && s.vramTotal
        ? ` · ${fmtGB(s.vramUsed)}/${fmtGB(s.vramTotal)} GB`
        : "";
    rows.append(meterRow("GPU", `${Math.round(s.gpu)}%${vram}`, gpuFrac, "#7C5CFF"));
  }

  const status = h(
    "div",
    { class: "int-status" },
    dot("#22C55E", 5),
    h("span", { text: `${s.cores} cores · up ${fmtUptime(s.uptime)} · ${s.processes} processes` }),
  );

  return h("div", { class: "int-card" }, head, rows, status);
}
