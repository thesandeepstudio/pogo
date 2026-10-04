// System monitor poller — invokes the local `system_stats` command and hands the
// snapshot to State, which the System card renders. No network, no secrets.

import { Bridge } from "../core/bridge";
import { State } from "../core/state";

const POLL_MS = 2000;

export function startSystemMonitor() {
  const tick = async () => {
    if (State.paused) return;
    const stats = await Bridge.systemStats();
    if (!stats) return;
    State.integrations["system"] = {
      data: stats as unknown as Record<string, unknown>,
      error: null,
      loaded: true,
      configured: true,
    };
    State.notify();
  };
  void tick();
  window.setInterval(() => void tick(), POLL_MS);
}
