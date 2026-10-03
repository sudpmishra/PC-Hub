//! NVIDIA graphics card readings through NVML, the library that ships with the driver.
//! On other cards (or without the driver) every reading is simply missing.

pub struct GpuSample {
    /// 0.0 to 1.0
    pub load: f32,
    pub temp_c: Option<f32>,
    pub clock_mhz: Option<u32>,
    pub power_w: Option<f32>,
    /// Fan speed and whether it is in RPM (true) or percent (false).
    pub fan: Option<(u32, bool)>,
    pub vram: Option<(u64, u64)>,
}

#[cfg(windows)]
pub struct Gpu {
    nvml: Option<nvml_wrapper::Nvml>,
}

#[cfg(windows)]
impl Gpu {
    pub fn new() -> Self {
        let nvml = match nvml_wrapper::Nvml::init() {
            Ok(nvml) => Some(nvml),
            Err(err) => {
                eprintln!("gpu: NVIDIA readings unavailable: {err}");
                None
            }
        };
        Self { nvml }
    }

    /// Short card name, e.g. "RTX 5070".
    pub fn name(&self) -> Option<String> {
        let name = self.nvml.as_ref()?.device_by_index(0).ok()?.name().ok()?;
        Some(name.replace("NVIDIA ", "").replace("GeForce ", ""))
    }

    pub fn sample(&self) -> Option<GpuSample> {
        use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};

        let device = self.nvml.as_ref()?.device_by_index(0).ok()?;
        let fan = device
            .fan_speed_rpm(0)
            .map(|rpm| (rpm, true))
            .or_else(|_| device.fan_speed(0).map(|pct| (pct, false)))
            .ok();
        Some(GpuSample {
            load: device.utilization_rates().map(|u| u.gpu as f32 / 100.0).unwrap_or(0.0),
            temp_c: device.temperature(TemperatureSensor::Gpu).ok().map(|t| t as f32),
            clock_mhz: device.clock_info(Clock::Graphics).ok(),
            power_w: device.power_usage().ok().map(|mw| mw as f32 / 1000.0),
            fan,
            vram: device.memory_info().ok().map(|m| (m.used, m.total)),
        })
    }
}

#[cfg(not(windows))]
pub struct Gpu;

#[cfg(not(windows))]
impl Gpu {
    pub fn new() -> Self {
        Self
    }

    pub fn name(&self) -> Option<String> {
        None
    }

    pub fn sample(&self) -> Option<GpuSample> {
        None
    }
}
