// Now-playing — reads (and controls) whatever is playing media on Windows via
// the Global System Media Transport Controls (GSMTC), the same source as the
// OS media flyout. Works for Spotify, browsers and other players with no
// account and no API key.
//
// WinRT async calls block while waiting for their result, which would deadlock
// on the STA main thread (the callback has to marshal back to the same pump),
// so every call is marshalled to one dedicated MTA worker thread.

use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    /// Friendly source label, e.g. "Spotify" or "Chrome".
    pub app: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    /// Seconds elapsed / total, 0 when the player reports no timeline.
    pub position: f64,
    pub duration: f64,
}

#[derive(Clone, Copy)]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
}

pub fn now_playing() -> Option<NowPlaying> {
    imp::now_playing()
}

pub fn control(action: MediaAction) {
    imp::control(action)
}

#[cfg(not(windows))]
mod imp {
    use super::{MediaAction, NowPlaying};

    pub fn now_playing() -> Option<NowPlaying> {
        None
    }

    pub fn control(_action: MediaAction) {}
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::LazyLock;
    use std::time::Duration;

    use windows::Foundation::TimeSpan;
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as Playback;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    use super::{MediaAction, NowPlaying};

    enum Job {
        Query(Sender<Option<NowPlaying>>),
        Control(MediaAction),
    }

    // If the worker cannot be spawned its receiver drops with it, every send
    // then fails and the widget simply reports nothing playing.
    static TX: LazyLock<Sender<Job>> = LazyLock::new(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let _ = std::thread::Builder::new()
            .name("media".into())
            .spawn(move || worker(rx));
        tx
    });

    pub fn now_playing() -> Option<NowPlaying> {
        let (reply_tx, reply_rx) = mpsc::channel();
        TX.send(Job::Query(reply_tx)).ok()?;
        reply_rx.recv_timeout(Duration::from_millis(1500)).ok().flatten()
    }

    pub fn control(action: MediaAction) {
        let _ = TX.send(Job::Control(action));
    }

    fn worker(rx: Receiver<Job>) {
        // MTA is what lets the blocking `.get()` calls below complete.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        while let Ok(job) = rx.recv() {
            match job {
                Job::Query(reply) => {
                    let _ = reply.send(query());
                }
                Job::Control(action) => perform(action),
            }
        }
    }

    fn manager() -> Option<Manager> {
        Manager::RequestAsync().ok()?.get().ok()
    }

    fn query() -> Option<NowPlaying> {
        let session = manager()?.GetCurrentSession().ok()?;
        let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
        let playing = match session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()) {
            Ok(status) => status == Playback::Playing,
            Err(_) => false,
        };
        let mut position = 0.0;
        let mut duration = 0.0;
        if let Ok(t) = session.GetTimelineProperties() {
            position = t.Position().map(secs).unwrap_or(0.0);
            duration = t.EndTime().map(secs).unwrap_or(0.0);
        }
        let app_id = session
            .SourceAppUserModelId()
            .map(|s| s.to_string())
            .unwrap_or_default();
        Some(NowPlaying {
            app: label(&app_id),
            title: props.Title().map(|s| s.to_string()).unwrap_or_default(),
            artist: props.Artist().map(|s| s.to_string()).unwrap_or_default(),
            album: props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default(),
            playing,
            position,
            duration,
        })
    }

    fn perform(action: MediaAction) {
        let Some(session) = manager().and_then(|m| m.GetCurrentSession().ok()) else {
            return;
        };
        let op = match action {
            MediaAction::PlayPause => session.TryTogglePlayPauseAsync(),
            MediaAction::Next => session.TrySkipNextAsync(),
            MediaAction::Previous => session.TrySkipPreviousAsync(),
        };
        if let Ok(op) = op {
            let _ = op.get();
        }
    }

    /// TimeSpan counts 100-nanosecond ticks.
    fn secs(t: TimeSpan) -> f64 {
        t.Duration as f64 / 10_000_000.0
    }

    /// `SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify` → `Spotify`.
    fn label(id: &str) -> String {
        let lower = id.to_ascii_lowercase();
        if lower.contains("spotify") {
            return "Spotify".into();
        }
        if lower.contains("msedge") || lower.contains("edge") {
            return "Edge".into();
        }
        if lower.contains("chrome") {
            return "Chrome".into();
        }
        if lower.contains("firefox") {
            return "Firefox".into();
        }
        if id.is_empty() {
            return "Media".into();
        }
        let tail = id.rsplit('!').next().unwrap_or(id);
        tail.rsplit('.').next().unwrap_or(tail).to_string()
    }
}
