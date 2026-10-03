//! Master volume and the default output and input devices, on a background thread like media.

use std::sync::mpsc::{self, Receiver, Sender};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::config::DeviceConfig;
use crate::{AppWindow, AudioDevice, Hub};

#[derive(Clone, Copy, Debug)]
pub enum AudioCommand {
    /// 0.0 to 1.0
    SetVolume(f32),
    /// Index into the outputs shown on screen.
    SelectOutput(usize),
    /// Index into the inputs shown on screen.
    SelectInput(usize),
}

/// Which devices to offer as buttons. Without any, every connected device is shown.
pub struct DeviceChoices {
    pub outputs: Vec<DeviceConfig>,
    pub inputs: Vec<DeviceConfig>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct AudioState {
    available: bool,
    volume: f32,
    /// Button label and whether it's the current default.
    outputs: Vec<(String, bool)>,
    inputs: Vec<(String, bool)>,
}

pub fn spawn(ui: slint::Weak<AppWindow>, choices: DeviceChoices) -> Sender<AudioCommand> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || backend::run(rx, ui, choices))
        .expect("failed to start the audio thread");
    tx
}

fn publish(ui: &slint::Weak<AppWindow>, state: AudioState, volume_changed: bool) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let hub = ui.global::<Hub>();
        hub.set_audio_available(state.available);
        // Only move the slider when the volume changed outside the hub, so it doesn't fight a drag.
        if volume_changed {
            hub.set_volume(state.volume);
        }
        let model = |devices: Vec<(String, bool)>| {
            let items: Vec<AudioDevice> = devices
                .into_iter()
                .map(|(label, active)| AudioDevice { label: label.into(), active })
                .collect();
            ModelRc::from(std::rc::Rc::new(VecModel::from(items)))
        };
        hub.set_outputs(model(state.outputs));
        hub.set_inputs(model(state.inputs));
    });
}

// The COM interface below keeps Windows' method names and declares methods it never calls.
#[cfg(windows)]
#[allow(non_snake_case, dead_code)]
mod backend {
    use std::ffi::c_void;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    use windows::core::{interface, IUnknown, IUnknown_Vtbl, GUID, HRESULT, PCWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IMMDevice,
        IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
    };

    use super::*;

    const POLL_INTERVAL: Duration = Duration::from_millis(500);
    /// More buttons than this won't fit on a row of the Music page.
    const MAX_DEVICES: usize = 4;

    /// Undocumented, but stable since Windows 7: the interface the Sound control panel
    /// uses to change the default device. Only SetDefaultEndpoint is called.
    #[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    unsafe trait IPolicyConfig: IUnknown {
        fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
        fn GetDeviceFormat(&self, device: PCWSTR, default: i32, format: *mut *mut c_void) -> HRESULT;
        fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(&self, device: PCWSTR, endpoint: *mut c_void, mix: *mut c_void) -> HRESULT;
        fn GetProcessingPeriod(&self, device: PCWSTR, default: i32, period: *mut i64, min: *mut i64) -> HRESULT;
        fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn GetPropertyValue(&self, device: PCWSTR, store: i32, key: *const c_void, value: *mut c_void) -> HRESULT;
        fn SetPropertyValue(&self, device: PCWSTR, store: i32, key: *const c_void, value: *mut c_void) -> HRESULT;
        fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    struct Device {
        id: String,
        name: String,
    }

    pub fn run(rx: Receiver<AudioCommand>, ui: slint::Weak<AppWindow>, choices: DeviceChoices) {
        let enumerator: IMMDeviceEnumerator = match unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .and_then(|_| CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL))
        } {
            Ok(enumerator) => enumerator,
            Err(err) => {
                eprintln!("audio: could not reach Windows audio: {err}");
                publish(&ui, AudioState::default(), true);
                return;
            }
        };

        let mut last: Option<AudioState> = None;
        // Device ids behind the buttons currently on screen.
        let mut output_ids: Vec<String> = Vec::new();
        let mut input_ids: Vec<String> = Vec::new();
        loop {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(AudioCommand::SetVolume(level)) => {
                    let result = default_volume(&enumerator).and_then(|v| unsafe {
                        v.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null())
                    });
                    if let Err(err) = result {
                        eprintln!("audio: could not set the volume: {err}");
                    }
                    // Remember the level as already shown so the slider isn't nudged back.
                    if let Some(last) = last.as_mut() {
                        last.volume = level;
                    }
                }
                Ok(AudioCommand::SelectOutput(index)) => {
                    if let Some(id) = output_ids.get(index) {
                        if let Err(err) = set_default(id) {
                            eprintln!("audio: could not switch output: {err}");
                        }
                    }
                }
                Ok(AudioCommand::SelectInput(index)) => {
                    if let Some(id) = input_ids.get(index) {
                        if let Err(err) = set_default(id) {
                            eprintln!("audio: could not switch input: {err}");
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            let volume = default_volume(&enumerator).and_then(|v| unsafe { v.GetMasterVolumeLevelScalar() });
            let (outputs, ids) = buttons(&enumerator, eRender, &choices.outputs);
            output_ids = ids;
            let (inputs, ids) = buttons(&enumerator, eCapture, &choices.inputs);
            input_ids = ids;
            let state = AudioState {
                available: volume.is_ok(),
                volume: volume.unwrap_or(0.0),
                outputs,
                inputs,
            };

            if last.as_ref() != Some(&state) {
                let volume_changed = last.as_ref().is_none_or(|l| (l.volume - state.volume).abs() > 0.004);
                publish(&ui, state.clone(), volume_changed);
                last = Some(state);
            }
        }
    }

    /// Buttons for one direction: the configured devices that are plugged in, or else
    /// every device. Returns the labels with which one is the default, and their ids.
    fn buttons(
        enumerator: &IMMDeviceEnumerator,
        flow: EDataFlow,
        configured: &[DeviceConfig],
    ) -> (Vec<(String, bool)>, Vec<String>) {
        let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }
            .and_then(|d| device_id(&d))
            .ok();
        let devices = list_devices(enumerator, flow).unwrap_or_default();

        let shown: Vec<(String, &Device)> = if configured.is_empty() {
            devices.iter().map(|d| (short_name(&d.name), d)).take(MAX_DEVICES).collect()
        } else {
            configured
                .iter()
                .filter_map(|c| {
                    let wanted = c.device.to_lowercase();
                    devices
                        .iter()
                        .find(|d| d.name.to_lowercase().contains(&wanted))
                        .map(|d| (c.label.clone(), d))
                })
                .take(MAX_DEVICES)
                .collect()
        };

        let labels = shown
            .iter()
            .map(|(label, d)| (label.clone(), default_id.as_deref() == Some(d.id.as_str())))
            .collect();
        (labels, shown.into_iter().map(|(_, d)| d.id.clone()).collect())
    }

    /// Windows names devices "Kind (Adapter)", e.g. "Speakers (Realtek(R) Audio)" or
    /// "ZP2716 (NVIDIA High Definition Audio)". Keep whichever half tells devices apart.
    fn short_name(name: &str) -> String {
        const GENERIC: [&str; 10] = [
            "speakers",
            "speaker",
            "headphones",
            "headset",
            "headset earphone",
            "digital audio",
            "microphone",
            "headset microphone",
            "microphone array",
            "line in",
        ];
        let name = name.replace("(R)", "").replace("(TM)", "");
        let Some((kind, adapter)) = name.split_once(" (") else {
            return name;
        };
        let adapter = adapter.strip_suffix(')').unwrap_or(adapter);
        if GENERIC.contains(&kind.trim().to_lowercase().as_str()) {
            adapter.trim().to_string()
        } else {
            kind.trim().to_string()
        }
    }

    fn default_volume(enumerator: &IMMDeviceEnumerator) -> windows::core::Result<IAudioEndpointVolume> {
        unsafe {
            enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)?
                .Activate(CLSCTX_ALL, None)
        }
    }

    fn list_devices(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> windows::core::Result<Vec<Device>> {
        unsafe {
            let collection = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            let mut devices = Vec::new();
            for i in 0..collection.GetCount()? {
                let device = collection.Item(i)?;
                let name = device
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                devices.push(Device { id: device_id(&device)?, name });
            }
            Ok(devices)
        }
    }

    fn device_id(device: &IMMDevice) -> windows::core::Result<String> {
        unsafe {
            let raw = device.GetId()?;
            let id = raw.to_string();
            CoTaskMemFree(Some(raw.0 as *const c_void));
            id.map_err(|_| windows::core::Error::from_hresult(HRESULT(-1)))
        }
    }

    /// Makes the device the default for every role, so apps and calls all follow it.
    fn set_default(id: &str) -> windows::core::Result<()> {
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let policy: IPolicyConfig = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
            for role in [eConsole, eMultimedia, eCommunications] {
                policy.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role).ok()?;
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod backend {
    use super::*;

    pub fn run(rx: Receiver<AudioCommand>, ui: slint::Weak<AppWindow>, _choices: DeviceChoices) {
        publish(&ui, AudioState::default(), true);
        while rx.recv().is_ok() {}
    }
}
