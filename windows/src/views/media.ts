// Media card — what the OS is playing right now (Spotify, a browser, anything
// that registers a media session) plus transport controls. Local only.

import { h, svg } from "./dom";
import { ICONS } from "./icons";
import { Bridge, type MediaAction } from "../core/bridge";
import { State } from "../core/state";

const GREEN = "#1DB954";

function fmtTime(sec: number): string {
  if (!Number.isFinite(sec) || sec <= 0) return "0:00";
  const m = Math.floor(sec / 60);
  return `${m}:${String(Math.floor(sec % 60)).padStart(2, "0")}`;
}

function transport(icon: string, label: string, action: MediaAction, big = false): HTMLElement {
  return h(
    "button",
    {
      class: big ? "np-btn big" : "np-btn",
      title: label,
      "aria-label": label,
      onclick: () => void Bridge.mediaControl(action),
    },
    svg(icon, big ? 15 : 13, { fill: "currentColor" }),
  );
}

export function mediaCard(): HTMLElement {
  const m = State.media;
  const head = h(
    "div",
    { class: "int-head" },
    svg(ICONS.note, 11, { fill: GREEN }),
    h("b", { text: m?.app || "Media" }),
    h("span", { text: m ? (m.playing ? "Playing" : "Paused") : "Nothing playing" }),
  );

  if (!m) {
    return h(
      "div",
      { class: "int-card" },
      head,
      h("div", { class: "int-empty", text: "Play something and it shows up here" }),
    );
  }

  const frac = m.duration > 0 ? Math.max(0, Math.min(1, m.position / m.duration)) : 0;
  const track = h(
    "div",
    { class: "np-track" },
    h("div", {
      class: "np-fill",
      style: `width:${Math.round(frac * 100)}%;background:${m.playing ? GREEN : "#6b7280"}`,
    }),
  );

  return h(
    "div",
    { class: "int-card" },
    head,
    h("div", { class: "np-title", text: m.title || "Unknown track" }),
    m.artist ? h("div", { class: "np-artist", text: m.artist }) : null,
    m.album ? h("div", { class: "np-album", text: m.album }) : null,
    track,
    h(
      "div",
      { class: "np-times" },
      h("span", { text: fmtTime(m.position) }),
      h("span", { text: fmtTime(m.duration) }),
    ),
    h(
      "div",
      { class: "np-controls" },
      transport(ICONS.prev, "Previous", "previous"),
      transport(m.playing ? ICONS.pause : ICONS.play, m.playing ? "Pause" : "Play", "playPause", true),
      transport(ICONS.next, "Next", "next"),
    ),
  );
}