// Now-playing poller — asks the OS media session what is playing and hands it to
// State, which the Media card renders. Local only: no account, no network.

import { Bridge } from "../core/bridge";
import { State } from "../core/state";

const POLL_MS = 1000;

export function startMediaMonitor() {
  const tick = async () => {
    if (State.paused) return;
    const media = await Bridge.mediaNowPlaying();
    // Only re-render when something actually changed: the card is on screen
    // most of the time and this runs every second.
    const before = State.media;
    const same =
      before === media ||
      (before !== null &&
        media !== null &&
        before.app === media.app &&
        before.title === media.title &&
        before.artist === media.artist &&
        before.album === media.album &&
        before.playing === media.playing &&
        Math.abs(before.position - media.position) < 1);
    if (same) return;
    State.media = media;
    State.notify();
  };
  void tick();
  window.setInterval(() => void tick(), POLL_MS);
}