// System events for the notch — the half that needs no new Rust.
//
// The lock keys, screenshots and downloads arrive from Rust as `system-event`
// (see src-tauri/src/events.rs). The battery needs nothing there at all: the
// System card already polls charge and charger state every two seconds, so this
// watches those samples for an edge and speaks only when one actually happens.

import { Bridge, onEvent } from "../core/bridge";
import { State, type SystemNotice } from "../core/state";
import type { Island } from "./island";

/** How long a notice sits in the bar before the next one takes over. */
const NOTICE_MS = 3000;

/** Re-check period while something is queued or on screen. */
const STEP_MS = 250;

/** Newest few only — a burst should not replay itself for a minute. */
const MAX_QUEUE = 4;

/** Below this the battery counts as low. */
const LOW_BATTERY = 20;

const queue: SystemNotice[] = [];
let current: SystemNotice | null = null;
let currentUntil = 0;
let running = false;

function push(notice: SystemNotice) {
  if (State.paused) return;
  if (queue.length >= MAX_QUEUE) queue.shift();
  queue.push(notice);
  if (!running) {
    running = true;
    window.setTimeout(step, STEP_MS);
  }
}

/**
 * Hands the bar one notice at a time. A notice that lands while the island is
 * open waits rather than disappearing unseen: it goes up as soon as the island
 * collapses back to the bar.
 */
function step() {
  if (State.paused) {
    State.notice = null;
    current = null;
    queue.length = 0;
    running = false;
    return;
  }

  if (!current) {
    current = queue.shift() ?? null;
    if (!current) {
      running = false;
      State.notice = null;
      State.notify();
      return;
    }
    currentUntil = performance.now() + NOTICE_MS;
    State.notice = current;
    State.notify();
    window.setTimeout(step, STEP_MS);
    return;
  }

  if (State.mode === "expanded") {
    window.setTimeout(step, STEP_MS);
    return;
  }

  if (performance.now() >= currentUntil) {
    State.notice = null;
    current = null;
    State.notify();
  }
  if (running) window.setTimeout(step, STEP_MS);
}

/** A hidden island has nothing to show a notice on, so it is woken first. */
function deliver(notice: SystemNotice, island: Island) {
  if (State.mode === "hidden") island.reveal();
  push(notice);
}

// ── Battery ────────────────────────────────────────────────────────────────

interface BatterySample {
  battery: number;
  charging: boolean;
}

function batterySample(): BatterySample | null {
  const data = State.integrations["system"]?.data;
  if (!data) return null;
  const battery = typeof data.battery === "number" ? (data.battery as number) : null;
  const charging = data.charging === true;
  // No battery at all (a desktop) says nothing worth interrupting for.
  if (battery == null) return null;
  return { battery, charging };
}

/** Fires at most once per edge, and never on the first sample. */
function watchBattery() {
  let last: BatterySample | null = null;

  return () => {
    const now = batterySample();
    if (!now) return;
    const before = last;
    last = now;
    if (!before) return;

    let notice: SystemNotice | null = null;
    if (!before.charging && now.charging) {
      notice = { kind: "battery", title: `Charger connected — ${now.battery}%` };
    } else if (before.charging && !now.charging) {
      notice = { kind: "battery", title: `Charger removed — ${now.battery}% left` };
    } else if (!now.charging && before.battery > LOW_BATTERY && now.battery <= LOW_BATTERY) {
      notice = { kind: "battery", title: `Battery low — ${now.battery}%` };
    } else if (now.charging && now.battery >= 100 && before.battery < 100) {
      notice = { kind: "battery", title: "Fully charged" };
    }
    if (notice) push(notice);
  };
}

export function startEventWatcher(island: Island) {
  const onBatteryEdge = watchBattery();
  let lastBatteryTick = 0;

  // The battery rides along on the System poll rather than asking for its own.
  State.subscribe(() => {
    const now = performance.now();
    if (now - lastBatteryTick < 1500) return;
    lastBatteryTick = now;
    onBatteryEdge();
  });

  void onEvent<SystemNotice>("system-event", (event) => {
    if (!event?.title) return;
    void Bridge.log(`event ${event.kind}: ${event.title}`);
    deliver(event, island);
  });
}