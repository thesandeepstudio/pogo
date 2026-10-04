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
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemEvent {
    pub kind: String,
    pub title: String,
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
        "watching events — screenshots: {}, downloads: {}, lock keys: {}",
        shots.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "none".into()),
        downloads.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "none".into()),
        if lock_keys().is_some() { "yes" } else { "no" },
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
                            queue(SystemEvent {
                                kind: "capsLock".into(),
                                title: if caps { "Caps Lock on".into() } else { "Caps Lock off".into() },
                            });
                        }
                        if num != num0 {
                            queue(SystemEvent {
                                kind: "numLock".into(),
                                title: if num { "Num Lock on".into() } else { "Num Lock off".into() },
                            });
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
                    queue(SystemEvent {
                        kind: "screenshot".into(),
                        title: "Screenshot taken".into(),
                    });
                }
            }
            if let Some(w) = downloads.as_mut() {
                let fresh = w.poll();
                if !fresh.is_empty() {
                    queue(SystemEvent {
                        kind: "download".into(),
                        title: format!("Downloaded {}", Watcher::describe(fresh)),
                    });
                }
            }
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