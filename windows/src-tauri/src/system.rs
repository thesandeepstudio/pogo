// Local system monitor — CPU / memory / GPU / disks / network / uptime.
//
// No network calls and no configuration: everything comes from the OS via
// sysinfo, plus GPU counters (see gpu.rs). The `System` is kept alive between
// calls so the CPU percentages are deltas between two samples rather than a
// cold zero.

use std::sync::{LazyLock, Mutex};

use serde::Serialize;
use sysinfo::{Disks, Networks, ProcessesToUpdate, System};

static SYS: LazyLock<Mutex<System>> = LazyLock::new(|| Mutex::new(System::new_all()));

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiskStat {
    pub name: String,
    pub mount: String,
    pub used: u64,
    pub total: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemStats {
    /// Global CPU load, 0–100.
    pub cpu: f32,
    /// 1-minute load average normalised by core count (0 where unsupported).
    pub load: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// Busiest GPU engine, 0–100 (None where the OS exposes no GPU counters).
    pub gpu: Option<f32>,
    /// Dedicated video memory in use / installed (None when unreported).
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    /// Charge left, 0–100 (None on a desktop or where the OS hides it).
    pub battery: Option<u8>,
    /// True while the charger is connected.
    pub charging: bool,
    pub disks: Vec<DiskStat>,
    pub net_rx: u64,
    pub net_tx: u64,
    /// Seconds since boot.
    pub uptime: u64,
    pub processes: usize,
    pub cores: usize,
    pub host: String,
    pub os: String,
    pub cpu_name: String,
}

/// Charge left and whether the charger is plugged in, straight from the OS.
/// `None` on a desktop: the machine reports "no battery" rather than 0%.
fn power_status() -> (Option<u8>, bool) {
    #[cfg(windows)]
    {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: the struct is a plain fixed-size value the OS fills in.
        if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
            return (None, false);
        }
        // 128 = no battery, 255 = unknown percentage.
        if status.BatteryFlag & 128 != 0 || status.BatteryLifePercent > 100 {
            return (None, false);
        }
        let charging = status.ACLineStatus == 1;
        (Some(status.BatteryLifePercent), charging)
    }
    #[cfg(not(windows))]
    {
        (None, false)
    }
}

#[tauri::command]
pub fn system_stats() -> SystemStats {
    let mut sys = SYS.lock().unwrap();
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let disks = Disks::new_with_refreshed_list()
        .list()
        .iter()
        .map(|d| {
            let total = d.total_space();
            DiskStat {
                name: d.name().to_string_lossy().trim().to_string(),
                mount: d.mount_point().to_string_lossy().to_string(),
                used: total.saturating_sub(d.available_space()),
                total,
            }
        })
        .collect();

    let (mut net_rx, mut net_tx) = (0u64, 0u64);
    for data in Networks::new_with_refreshed_list().list().values() {
        net_rx = net_rx.saturating_add(data.total_received());
        net_tx = net_tx.saturating_add(data.total_transmitted());
    }

    let cores = sys.cpus().len().max(1);
    let load = {
        let avg = System::load_average();
        (avg.one / cores as f64) as f32
    };
    let gpu = crate::gpu::sample();
    let (battery, charging) = power_status();

    SystemStats {
        cpu: sys.global_cpu_usage(),
        load,
        mem_used: sys.used_memory(),
        mem_total: sys.total_memory(),
        swap_used: sys.used_swap(),
        swap_total: sys.total_swap(),
        gpu: gpu.percent,
        vram_used: gpu.vram_used,
        vram_total: gpu.vram_total,
        battery,
        charging,
        disks,
        net_rx,
        net_tx,
        uptime: System::uptime(),
        processes: sys.processes().len(),
        cores,
        host: System::host_name().unwrap_or_default(),
        os: System::long_os_version().unwrap_or_default(),
        cpu_name: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default(),
    }
}
