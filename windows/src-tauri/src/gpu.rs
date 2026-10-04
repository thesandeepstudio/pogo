// GPU monitor — total GPU utilisation and dedicated VRAM usage.
//
// Windows exposes both through the Performance Data Helper (PDH) counters that
// Task Manager itself reads:
//   \GPU Engine(*)\Utilization Percentage
//   \GPU Adapter Memory(*)\Dedicated Usage
// A single PDH query (and its handles) is kept alive for the life of the
// process: the utilisation counter is a ratio, so PDH needs two samples and the
// gap between polls becomes the measurement window.
//
// Total dedicated VRAM is not a counter, so DXGI is asked for each adapter's
// `DedicatedVideoMemory` instead. Everything here is best-effort: a machine
// without GPU counters simply gets `None`s and the widget hides the row.

/// One reading: GPU percent (0–100), dedicated VRAM used, dedicated VRAM total.
pub struct GpuStats {
    pub percent: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
}

#[cfg(not(windows))]
pub fn sample() -> GpuStats {
    GpuStats {
        percent: None,
        vram_used: None,
        vram_total: None,
    }
}

#[cfg(windows)]
pub fn sample() -> GpuStats {
    imp::sample()
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    use windows::core::{w, PCWSTR};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    };

    use super::GpuStats;

    struct Counters {
        query: PDH_HQUERY,
        engines: PDH_HCOUNTER,
        memory: PDH_HCOUNTER,
    }

    // The raw handles are only ever touched behind QUERY's mutex.
    unsafe impl Send for Counters {}

    static QUERY: LazyLock<Mutex<Option<Counters>>> = LazyLock::new(|| Mutex::new(open()));

    fn add(query: PDH_HQUERY, path: PCWSTR) -> Option<PDH_HCOUNTER> {
        let mut counter = PDH_HCOUNTER::default();
        let status = unsafe { PdhAddEnglishCounterW(query, path, 0, &mut counter) };
        (status == 0).then_some(counter)
    }

    fn open() -> Option<Counters> {
        let mut query = PDH_HQUERY::default();
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            return None;
        }
        let engines = add(query, w!("\\GPU Engine(*)\\Utilization Percentage"));
        let memory = add(query, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"));
        match (engines, memory) {
            (Some(engines), Some(memory)) => Some(Counters {
                query,
                engines,
                memory,
            }),
            _ => {
                let _ = unsafe { PdhCloseQuery(query) };
                None
            }
        }
    }

    /// Formatted values for every instance of a counter, e.g. one per GPU.
    fn read(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        let mut size = 0u32;
        let mut count = 0u32;
        unsafe {
            // First call just reports the buffer size.
            let _ = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                None,
            );
            if size == 0 || count == 0 {
                return out;
            }
            let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let cap = (size as usize + item - 1) / item;
            // A typed buffer, so the item pointers stay aligned for the doubles.
            let mut buf = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); cap];
            let status = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(buf.as_mut_ptr()),
            );
            if status != 0 {
                return out;
            }
            for i in 0..count as usize {
                let entry = &*buf.as_ptr().add(i);
                // PDH_CSTATUS_VALID_DATA: the first sample of a ratio is invalid.
                if entry.FmtValue.CStatus != 0 {
                    continue;
                }
                out.push((
                    entry.szName.to_string().unwrap_or_default(),
                    entry.FmtValue.Anonymous.doubleValue,
                ));
            }
        }
        out
    }

    /// `pid_1234_luid_0x0_0xABCD_phys_0_eng_0_engtype_3D` → one key per GPU
    /// engine, so concurrent engines are summed but hardware stays separate.
    fn engine_key(name: &str) -> Option<String> {
        let luid_and_after = &name[name.find("luid_")?..];
        let luid_end = luid_and_after
            .find("_phys_")
            .unwrap_or(luid_and_after.len());
        let engtype = name.split("engtype_").nth(1)?;
        Some(format!("{}|{}", &luid_and_after[..luid_end], engtype))
    }

    fn busiest_engine(values: &[(String, f64)]) -> Option<f32> {
        if values.is_empty() {
            return None;
        }
        let mut per_engine: HashMap<String, f64> = HashMap::new();
        for (name, value) in values {
            if let Some(key) = engine_key(name) {
                *per_engine.entry(key).or_insert(0.0) += value;
            }
        }
        let peak = per_engine.values().copied().fold(0.0f64, f64::max);
        Some(peak.clamp(0.0, 100.0) as f32)
    }

    fn vram_total() -> Option<u64> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut total = 0u64;
            let mut index = 0u32;
            while let Ok(adapter) = factory.EnumAdapters1(index) {
                if let Ok(desc) = adapter.GetDesc1() {
                    total += desc.DedicatedVideoMemory as u64;
                }
                index += 1;
            }
            (total > 0).then_some(total)
        }
    }

    pub fn sample() -> GpuStats {
        let mut guard = QUERY.lock().unwrap();
        let Some(counters) = guard.as_mut() else {
            return GpuStats {
                percent: None,
                vram_used: None,
                vram_total: None,
            };
        };
        unsafe {
            let _ = PdhCollectQueryData(counters.query);
        }

        let engines = read(counters.engines);
        let memory = read(counters.memory);

        let percent = busiest_engine(&engines);
        let used = (!memory.is_empty()).then(|| {
            memory
                .iter()
                .map(|(_, v)| v.max(0.0))
                .sum::<f64>() as u64
        });
        GpuStats {
            percent,
            vram_used: used,
            vram_total: vram_total(),
        }
    }
}
