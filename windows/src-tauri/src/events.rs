// Cheap system events for the notch: Caps/Num Lock, a screenshot landing in the
// Screenshots folder, a download finishing in Downloads.
//
// Every one of these is a poll of something the OS already exposes — no message
// loop, no window subclass, no new dependency. The battery half of the same idea
// needs nothing here: `system::system_stats` already reports charge and charger
// state every two seconds, so the island edge-detects that one itself.
//
// Each watcher takes its first reading as a baseline and stays quiet about it, so
// nothing fires on launch just because the machine happens to be in some state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::island::WINDOW_LABEL;

/// A one-line event for the notch bar. `kind` picks the tone, `title` is read.
/// The volume notices carry the level so the bar can draw it rather than only say it.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemEvent {
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
}

impl SystemEvent {
    /// Most events are a sentence and nothing else.
    fn plain(kind: &str, title: impl Into<String>) -> Self {
        Self { kind: kind.into(), title: title.into(), level: None, muted: None }
    }
}

/// Caps Lock is VK_CAPITAL, Num Lock is VK_NUMLOCK.
#[cfg(windows)]
const VK_CAPITAL: i32 = 0x14;
#[cfg(windows)]
const VK_NUMLOCK: i32 = 0x90;

/// GetKeyState on the low-order bit is a trivial call, and 120 ms is well under
/// the time it takes a person to notice a missing indicator.
const LOCK_POLL_MS: u64 = 120;

/// Directory listings four times a second is a lot of stat() calls for something
/// this rare, and 1.5 s still feels instant next to a human.
const FOLDER_POLL_MS: u64 = 1500;

/// The volume slider has to keep up with a hand dragging it, which is a much
/// tighter budget than a folder listing.
const VOLUME_POLL_MS: u64 = 250;

/// Below this the level counts as still: the scalar comes back as a float, and
/// rounding noise must not read as somebody turning the volume down.
const VOLUME_EPSILON: f32 = 0.005;

/// How often queued events are handed to the island.
const EMIT_POLL_MS: u64 = 200;

/// Still-being-written extensions. A browser download only takes its real name
/// once it has finished, so skipping these is what makes the notice mean
/// "downloaded" rather than "started".
const PARTIAL_EXTS: &[&str] =
    &["crdownload", "part", "partial", "tmp", "download", "opdownload", "!ut"];

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp", "gif"];

/// A file older than this when it is first noticed was already there before the
/// watcher got to it — a suspended laptop, a long sleep.
const MAX_AGE_S: f64 = 30.0;

/// Toggle state of both lock keys, or None where the OS cannot say.
#[cfg(windows)]
fn lock_keys() -> Option<(bool, bool)> {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    // SAFETY: reads the key state table, takes no pointers, cannot fail.
    unsafe {
        Some((
            GetKeyState(VK_CAPITAL) & 1 != 0,
            GetKeyState(VK_NUMLOCK) & 1 != 0,
        ))
    }
}

#[cfg(not(windows))]
fn lock_keys() -> Option<(bool, bool)> {
    None
}

fn age_s(mtime: SystemTime) -> f64 {
    SystemTime::now()
        .duration_since(mtime)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn screenshots_dir() -> Option<PathBuf> {
    // Windows keeps these in the profile's Pictures folder, but OneDrive
    // redirection moves that wholesale, so both spellings are worth a look.
    let mut dirs = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        dirs.push(PathBuf::from(profile).join("Pictures").join("Screenshots"));
    }
    if let Ok(one) = std::env::var("OneDrive") {
        if !one.is_empty() {
            dirs.push(PathBuf::from(one).join("Pictures").join("Screenshots"));
        }
    }
    dirs.into_iter().find(|d| d.is_dir())
}

fn downloads_dir() -> Option<PathBuf> {
    // This machine keeps Downloads on D:, which is where the browsers put things.
    // The env var keeps that from being baked in forever.
    if let Ok(dir) = std::env::var("COUCOU_DOWNLOADS_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    let preferred = PathBuf::from(r"D:\Downloads");
    if preferred.is_dir() {
        return Some(preferred);
    }
    std::env::var("USERPROFILE")
        .ok()
        .map(|p| PathBuf::from(p).join("Downloads"))
        .filter(|d| d.is_dir())
}

/// A directory watched for new files: name -> age when last seen.
struct Watcher {
    dir: PathBuf,
    exts: Option<&'static [&'static str]>,
    seen: HashMap<String, f64>,
    primed: bool,
}

impl Watcher {
    fn new(dir: PathBuf, exts: Option<&'static [&'static str]>) -> Self {
        Self { dir, exts, seen: HashMap::new(), primed: false }
    }

    fn listing(&self) -> HashMap<String, f64> {
        let mut out = HashMap::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(ext) = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()) else {
                continue;
            };
            if PARTIAL_EXTS.contains(&ext.as_str()) {
                continue;
            }
            if let Some(allowed) = self.exts {
                if !allowed.contains(&ext.as_str()) {
                    continue;
                }
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let Ok(modified) = meta.modified() else { continue };
            out.insert(name, age_s(modified));
        }
        out
    }

    /// Files that appeared since the last poll, newest first. The first poll only
    /// records the baseline and reports nothing.
    fn poll(&mut self) -> Vec<String> {
        let now = self.listing();
        let mut fresh: Vec<String> = now
            .iter()
            .filter(|(name, age)| !self.seen.contains_key(*name) && **age < MAX_AGE_S)
            .map(|(name, _)| name.clone())
            .collect();
        self.seen = now;
        if !self.primed {
            self.primed = true;
            return Vec::new();
        }
        fresh.sort();
        fresh
    }

    /// A file name trimmed to what fits a 288 px bar, or a count for a burst.
    fn describe(files: Vec<String>) -> String {
        if files.len() > 1 {
            return format!("{} new files", files.len());
        }
        let Some(name) = files.into_iter().next() else {
            return String::new();
        };
        let trimmed = name.trim();
        if trimmed.chars().count() > 28 {
            let head: String = trimmed.chars().take(25).collect();
            format!("{head}…")
        } else {
            trimmed.to_string()
        }
    }
}

/// Watchers hand events here; the emitter thread drains it. Keeps the watcher
/// threads from touching Tauri at all.
static QUEUE: Mutex<Vec<SystemEvent>> = Mutex::new(Vec::new());

fn queue(event: SystemEvent) {
    crate::log::line(format!("event: {}", event.title));
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    // A burst (an extract, a sync) should not queue forty notices; the newest
    // few are what the user actually needs to see.
    if q.len() >= 4 {
        q.remove(0);
    }
    q.push(event);
}

/// Starts the watchers. Nothing here may be expensive — this runs for as long as
/// the app does.
pub fn start(app: AppHandle) {
    let shots = screenshots_dir();
    let downloads = downloads_dir();
    crate::log::line(format!(
        "watching events — screenshots: {}, downloads: {}, lock keys: {}, volume: {}",
        shots.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "none".into()),
        downloads.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "none".into()),
        if lock_keys().is_some() { "yes" } else { "no" },
        if crate::volume::Endpoint::open().is_some() { "yes" } else { "no" },
    ));

    // Lock keys get their own thread: they need a tighter period than a directory
    // listing and must not wait behind one.
    std::thread::spawn(move || {
        let mut last: Option<(bool, bool)> = None;
        loop {
            if let Some((caps, num)) = lock_keys() {
                match last {
                    None => last = Some((caps, num)),
                    Some((caps0, num0)) => {
                        if caps != caps0 {
                            queue(SystemEvent::plain(
                                "capsLock",
                                if caps { "Caps Lock on" } else { "Caps Lock off" },
                            ));
                        }
                        if num != num0 {
                            queue(SystemEvent::plain(
                                "numLock",
                                if num { "Num Lock on" } else { "Num Lock off" },
                            ));
                        }
                        last = Some((caps, num));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(LOCK_POLL_MS));
        }
    });

    // Both directory watchers share a thread: at 1.5 s apart they are never going
    // to compete, and one sleeping thread costs less than two.
    std::thread::spawn(move || {
        let mut shots = shots.map(|d| Watcher::new(d, Some(IMAGE_EXTS)));
        let mut downloads = downloads.map(|d| Watcher::new(d, None));
        // Prime both before the first sleep so a restart cannot announce whatever
        // is already on disk.
        if let Some(w) = shots.as_mut() {
            w.poll();
        }
        if let Some(w) = downloads.as_mut() {
            w.poll();
        }
        loop {
            std::thread::sleep(Duration::from_millis(FOLDER_POLL_MS));
            if let Some(w) = shots.as_mut() {
                let fresh = w.poll();
                if !fresh.is_empty() {
                    queue(SystemEvent::plain("screenshot", "Screenshot taken"));
                }
            }
            if let Some(w) = downloads.as_mut() {
                let fresh = w.poll();
                if !fresh.is_empty() {
                    queue(SystemEvent::plain("download", format!("Downloaded {}", Watcher::describe(fresh))));
                }
            }
        }
    });

    // The volume gets its own thread because it is the only watcher with to keep
    // up with a hand rather than wait for a person to notice.
    std::thread::spawn(|| {
        let mut endpoint = crate::volume::Endpoint::open();
        let mut last: Option<crate::volume::Volume> = None;
        loop {
            std::thread::sleep(Duration::from_millis(VOLUME_POLL_MS));

            // An endpoint can vanish mid-session — a USB headset being unplugged
            // takes the default device with it — so a failed read reopens rather
            // than ending the thread.
            if endpoint.is_none() {
                endpoint = crate::volume::Endpoint::open();
            }
            let Some(now) = endpoint.as_ref().and_then(|e| e.read()) else {
                continue;
            };

            let Some(before) = last else {
                last = Some(now);
                continue;
            };
            last = Some(now);

            let moved = (now.level - before.level).abs() > VOLUME_EPSILON;
            if !moved && now.muted == before.muted {
                continue;
            }

            let title = if now.muted != before.muted && !moved {
                if now.muted { "Muted".to_string() } else { "Unmuted".to_string() }
            } else {
                format!("Volume {}%", (now.level * 100.0).round() as i32)
            };
            queue(SystemEvent {
                kind: "volume".into(),
                title,
                level: Some(now.level),
                muted: Some(now.muted),
            });
        }
    });

    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(EMIT_POLL_MS));
        let drained: Vec<SystemEvent> = {
            let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if q.is_empty() {
                continue;
            }
            std::mem::take(&mut *q)
        };
        for event in drained {
            let _ = app.emit_to(WINDOW_LABEL, "system-event", event);
        }
    });
}