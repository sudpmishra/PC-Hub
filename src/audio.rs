//! Master volume and the default output and input devices, on a background thread like media.

use std::sync::mpsc::{self, Receiver, Sender};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::config::DeviceConfig;
use crate::{AppVolume, AppWindow, AudioDevice, Hub};

#[derive(Clone, Debug)]
pub enum AudioCommand {
    /// 0.0 to 1.0
    SetVolume(f32),
    /// Index into the outputs shown on screen.
    SelectOutput(usize),
    /// Index into the inputs shown on screen.
    SelectInput(usize),
    /// Mute or unmute the default output.
    ToggleMute,
    /// Mute or unmute the default microphone.
    ToggleMicMute,
    /// hub.toml changed which devices get buttons.
    SetChoices(DeviceChoices),
    /// Index into the apps in the mixer, and 0.0 to 1.0.
    SetAppVolume(usize, f32),
    ToggleAppMute(usize),
}

/// Which devices to offer as buttons. Without any, every connected device is shown.
#[derive(Clone, Debug)]
pub struct DeviceChoices {
    pub outputs: Vec<DeviceConfig>,
    pub inputs: Vec<DeviceConfig>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct AudioState {
    available: bool,
    volume: f32,
    muted: bool,
    mic_available: bool,
    mic_muted: bool,
    /// Button label and whether it's the current default.
    outputs: Vec<(String, bool)>,
    inputs: Vec<(String, bool)>,
    /// Apps playing through the default output, for the mixer.
    apps: Vec<AppLevel>,
}

#[derive(Clone, Debug, PartialEq)]
struct AppLevel {
    label: String,
    volume: f32,
    muted: bool,
}

/// Whether the mixer needs redrawing. Small volume differences don't count, so a slider
/// being dragged isn't replaced under the finger.
fn apps_changed(old: &[AppLevel], new: &[AppLevel]) -> bool {
    old.len() != new.len()
        || old
            .iter()
            .zip(new)
            .any(|(a, b)| a.label != b.label || a.muted != b.muted || (a.volume - b.volume).abs() > 0.004)
}

/// A name for the mixer from an app's .exe path: "C:\...\Discord.exe" -> "Discord".
fn app_label(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    match stem.to_lowercase().as_str() {
        "msedge" => "Edge".into(),
        "msedgewebview2" => "Edge WebView".into(),
        "explorer" => "Windows".into(),
        _ => {
            let mut chars = stem.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

pub fn spawn(ui: slint::Weak<AppWindow>, choices: DeviceChoices) -> Sender<AudioCommand> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || backend::run(rx, ui, choices))
        .expect("failed to start the audio thread");
    tx
}

/// Starts the microphone level meter's thread; it measures while `enabled` is set.
pub fn spawn_meter(ui: slint::Weak<AppWindow>, enabled: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    std::thread::Builder::new()
        .name("mic meter".into())
        .spawn(move || backend::meter(ui, enabled))
        .expect("failed to start the mic meter thread");
}

fn publish(ui: &slint::Weak<AppWindow>, state: AudioState, volume_changed: bool, apps_changed: bool) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let hub = ui.global::<Hub>();
        hub.set_audio_available(state.available);
        if apps_changed {
            let apps: Vec<AppVolume> = state
                .apps
                .iter()
                .map(|app| AppVolume {
                    label: app.label.as_str().into(),
                    volume: app.volume,
                    muted: app.muted,
                })
                .collect();
            hub.set_apps(ModelRc::from(std::rc::Rc::new(VecModel::from(apps))));
        }
        // Only move the slider when the volume changed outside the hub, so it doesn't fight a drag.
        if volume_changed {
            hub.set_volume(state.volume);
        }
        hub.set_muted(state.muted);
        hub.set_mic_available(state.mic_available);
        hub.set_mic_muted(state.mic_muted);
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use windows::core::{interface, Interface, IUnknown, IUnknown_Vtbl, GUID, HRESULT, PCWSTR, PWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, eConsole, eMultimedia, eRender, AudioSessionStateExpired, EDataFlow, ERole,
        AUDCLNT_SHAREMODE_SHARED, IAudioClient, IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume,
        MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
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

    pub fn run(rx: Receiver<AudioCommand>, ui: slint::Weak<AppWindow>, mut choices: DeviceChoices) {
        let enumerator: IMMDeviceEnumerator = match unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .and_then(|_| CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL))
        } {
            Ok(enumerator) => enumerator,
            Err(err) => {
                eprintln!("audio: could not reach Windows audio: {err}");
                publish(&ui, AudioState::default(), true, true);
                return;
            }
        };

        let mut last: Option<AudioState> = None;
        // Device ids behind the buttons currently on screen.
        let mut output_ids: Vec<String> = Vec::new();
        let mut input_ids: Vec<String> = Vec::new();
        // The sessions behind each app in the mixer.
        let mut app_sessions: Vec<Vec<ISimpleAudioVolume>> = Vec::new();
        loop {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(AudioCommand::SetVolume(level)) => {
                    let result = endpoint_volume(&enumerator, eRender).and_then(|v| unsafe {
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
                Ok(command @ (AudioCommand::ToggleMute | AudioCommand::ToggleMicMute)) => {
                    let flow = if matches!(command, AudioCommand::ToggleMute) { eRender } else { eCapture };
                    let result = endpoint_volume(&enumerator, flow).and_then(|v| unsafe {
                        let muted = v.GetMute()?.as_bool();
                        v.SetMute(!muted, std::ptr::null())
                    });
                    if let Err(err) = result {
                        eprintln!("audio: {command:?} failed: {err}");
                    }
                }
                Ok(AudioCommand::SetChoices(new)) => choices = new,
                Ok(AudioCommand::SetAppVolume(index, level)) => {
                    let level = level.clamp(0.0, 1.0);
                    for session in app_sessions.get(index).into_iter().flatten() {
                        if let Err(err) = unsafe { session.SetMasterVolume(level, std::ptr::null()) } {
                            eprintln!("audio: could not set an app's volume: {err}");
                        }
                    }
                    if let Some(app) = last.as_mut().and_then(|l| l.apps.get_mut(index)) {
                        app.volume = level;
                    }
                }
                Ok(AudioCommand::ToggleAppMute(index)) => {
                    if let Some(sessions) = app_sessions.get(index) {
                        let muted = sessions.first().and_then(|s| unsafe { s.GetMute() }.ok()).is_some_and(|m| m.as_bool());
                        for session in sessions {
                            let _ = unsafe { session.SetMute(!muted, std::ptr::null()) };
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            let output = endpoint_volume(&enumerator, eRender).ok();
            let volume = output.as_ref().and_then(|v| unsafe { v.GetMasterVolumeLevelScalar() }.ok());
            let muted = output.as_ref().and_then(|v| unsafe { v.GetMute() }.ok()).is_some_and(|m| m.as_bool());
            let mic_muted = endpoint_volume(&enumerator, eCapture).and_then(|v| unsafe { v.GetMute() });
            let (outputs, ids) = buttons(&enumerator, eRender, &choices.outputs);
            output_ids = ids;
            let (inputs, ids) = buttons(&enumerator, eCapture, &choices.inputs);
            input_ids = ids;
            let apps = list_apps(&enumerator).unwrap_or_default();
            let levels = apps
                .iter()
                .map(|(label, sessions)| {
                    let first = sessions.first();
                    AppLevel {
                        label: label.clone(),
                        volume: first.and_then(|s| unsafe { s.GetMasterVolume() }.ok()).unwrap_or(0.0),
                        muted: first.and_then(|s| unsafe { s.GetMute() }.ok()).is_some_and(|m| m.as_bool()),
                    }
                })
                .collect();
            app_sessions = apps.into_iter().map(|(_, sessions)| sessions).collect();
            let state = AudioState {
                available: volume.is_some(),
                volume: volume.unwrap_or(0.0),
                muted,
                mic_available: mic_muted.is_ok(),
                mic_muted: mic_muted.is_ok_and(|m| m.as_bool()),
                outputs,
                inputs,
                apps: levels,
            };

            if last.as_ref() != Some(&state) {
                let volume_changed = last.as_ref().is_none_or(|l| (l.volume - state.volume).abs() > 0.004);
                let apps_changed = last.as_ref().is_none_or(|l| apps_changed(&l.apps, &state.apps));
                publish(&ui, state.clone(), volume_changed, apps_changed);
                last = Some(state);
            }
        }
    }

    /// The apps playing through the default output, one entry per app with all of its
    /// sessions (browsers open several), so one slider moves them together.
    fn list_apps(enumerator: &IMMDeviceEnumerator) -> windows::core::Result<Vec<(String, Vec<ISimpleAudioVolume>)>> {
        const SYSTEM_SOUNDS: &str = "System sounds";
        let mut apps: Vec<(String, Vec<ISimpleAudioVolume>)> = Vec::new();
        unsafe {
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
            let sessions = manager.GetSessionEnumerator()?;
            for i in 0..sessions.GetCount()? {
                let control = sessions.GetSession(i)?;
                if control.GetState()? == AudioSessionStateExpired {
                    continue;
                }
                let control2: IAudioSessionControl2 = control.cast()?;
                // S_OK (0) means it is the system sounds session; S_FALSE means it isn't.
                let label = if control2.IsSystemSoundsSession().0 == 0 {
                    SYSTEM_SOUNDS.to_string()
                } else {
                    match control2.GetProcessId().ok().and_then(process_path) {
                        Some(path) => app_label(&path),
                        None => continue,
                    }
                };
                let volume: ISimpleAudioVolume = control.cast()?;
                match apps.iter_mut().find(|(l, _)| *l == label) {
                    Some((_, volumes)) => volumes.push(volume),
                    None => apps.push((label, vec![volume])),
                }
            }
        }
        // Sorted by name, with system sounds last.
        apps.sort_by_key(|(label, _)| (label == SYSTEM_SOUNDS, label.to_lowercase()));
        Ok(apps)
    }

    fn process_path(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = [0u16; 1024];
            let mut size = buffer.len() as u32;
            let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut size);
            let _ = CloseHandle(handle);
            result.ok()?;
            Some(String::from_utf16_lossy(&buffer[..size as usize]))
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

    /// Volume and mute of the default output (eRender) or microphone (eCapture).
    fn endpoint_volume(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> windows::core::Result<IAudioEndpointVolume> {
        unsafe {
            enumerator
                .GetDefaultAudioEndpoint(flow, eConsole)?
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


    /// Reads the default microphone's level while `enabled`, for the meter under Input.
    /// Windows only measures a microphone that something is recording from, so a
    /// recording is opened (and its sound thrown away) while the meter shows.
    pub fn meter(ui: slint::Weak<AppWindow>, enabled: Arc<AtomicBool>) {
        let enumerator: IMMDeviceEnumerator = match unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .and_then(|_| CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL))
        } {
            Ok(enumerator) => enumerator,
            Err(_) => return,
        };
        let mut open: Option<(String, IAudioClient, IAudioMeterInformation)> = None;
        let mut checked = Instant::now();
        let mut shown = -1.0f32;
        loop {
            if !enabled.load(Ordering::Relaxed) {
                if let Some((_, client, _)) = open.take() {
                    let _ = unsafe { client.Stop() };
                    publish_level(&ui, 0.0);
                    shown = 0.0;
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            // Follow a change of default microphone.
            if open.is_none() || checked.elapsed() > Duration::from_secs(2) {
                checked = Instant::now();
                let current = unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }.and_then(|d| device_id(&d)).ok();
                if current.is_none() || current.as_deref() != open.as_ref().map(|(id, _, _)| id.as_str()) {
                    if let Some((_, client, _)) = open.take() {
                        let _ = unsafe { client.Stop() };
                    }
                    open = open_meter(&enumerator).ok();
                }
            }
            let level = open.as_ref().and_then(|(_, _, meter)| unsafe { meter.GetPeakValue() }.ok()).unwrap_or(0.0);
            if (level - shown).abs() > 0.01 {
                publish_level(&ui, level);
                shown = level;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
    }

    fn open_meter(enumerator: &IMMDeviceEnumerator) -> windows::core::Result<(String, IAudioClient, IAudioMeterInformation)> {
        unsafe {
            let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            let result = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None);
            CoTaskMemFree(Some(format as *const c_void));
            result?;
            client.Start()?;
            let meter: IAudioMeterInformation = device.Activate(CLSCTX_ALL, None)?;
            Ok((device_id(&device)?, client, meter))
        }
    }

    fn publish_level(ui: &slint::Weak<AppWindow>, level: f32) {
        let _ = ui.upgrade_in_event_loop(move |ui| ui.global::<Hub>().set_mic_level(level));
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
        publish(&ui, AudioState::default(), true, true);
        while rx.recv().is_ok() {}
    }

    pub fn meter(_ui: slint::Weak<AppWindow>, _enabled: std::sync::Arc<std::sync::atomic::AtomicBool>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_labels() {
        assert_eq!(app_label(r"C:\Users\me\AppData\Local\Discord\app-1.0\Discord.exe"), "Discord");
        assert_eq!(app_label(r"C:\Program Files\Google\Chrome\Application\chrome.exe"), "Chrome");
        assert_eq!(app_label(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"), "Edge");
        assert_eq!(app_label("spotify"), "Spotify");
    }

    #[test]
    fn small_volume_changes_dont_redraw_the_mixer() {
        let app = |volume: f32, muted: bool| AppLevel { label: "Discord".into(), volume, muted };
        assert!(!apps_changed(&[app(0.5, false)], &[app(0.501, false)]));
        assert!(apps_changed(&[app(0.5, false)], &[app(0.6, false)]));
        assert!(apps_changed(&[app(0.5, false)], &[app(0.5, true)]));
        assert!(apps_changed(&[], &[app(0.5, false)]));
    }
}
