//! Now-playing info and playback control.
//!
//! On Windows this uses the system media session API (the same one behind the
//! volume flyout), so it works with Spotify, browsers, and most media apps.
//! The work happens on a background thread; the UI sends commands over a channel
//! and receives state updates through Slint's event loop.

use std::sync::mpsc::{self, Receiver, Sender};

use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};

use crate::{AppWindow, Hub};

#[derive(Clone, Copy, Debug)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
    ToggleShuffle,
    ToggleRepeat,
    /// Jump to this fraction of the track, 0.0 to 1.0.
    Seek(f32),
    /// Control the player at this index in the list on screen, or -1 for whichever
    /// Windows picks (the one in the volume flyout).
    ChoosePlayer(i32),
}

/// The apps with a media session, for choosing which one the controls act on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Players {
    pub labels: Vec<String>,
    /// The one being controlled, or -1 if none.
    pub current: i32,
    /// Following Windows' choice rather than one picked on the hub.
    pub automatic: bool,
}

/// A short name from a media session's app id: "Spotify.exe" -> "Spotify",
/// "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic" -> "ZuneMusic", "MSEdge" -> "Edge".
pub fn player_name(app_id: &str) -> String {
    let name = app_id.rsplit(['!', '\\']).next().unwrap_or(app_id);
    let name = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
    let name = name.rsplit('.').next().unwrap_or(name);
    match name.to_lowercase().as_str() {
        "msedge" => "Edge".into(),
        "zunemusic" => "Media Player".into(),
        "vlc" => "VLC".into(),
        _ => {
            let mut chars = name.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

fn publish_players(ui: &slint::Weak<AppWindow>, players: Players) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let hub = ui.global::<Hub>();
        let labels: Vec<slint::SharedString> = players.labels.iter().map(|l| l.as_str().into()).collect();
        hub.set_players(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(labels))));
        hub.set_current_player(players.current);
        hub.set_player_automatic(players.automatic);
    });
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaState {
    pub available: bool,
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub position_secs: u64,
    pub duration_secs: u64,
    pub shuffle: bool,
    pub repeat: bool,
    pub can_shuffle: bool,
    pub can_repeat: bool,
    pub can_seek: bool,
}

pub fn spawn(ui: slint::Weak<AppWindow>) -> Sender<MediaCommand> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("media".into())
        .spawn(move || backend::run(rx, ui))
        .expect("failed to start the media thread");
    tx
}

fn publish(ui: &slint::Weak<AppWindow>, state: MediaState) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let hub = ui.global::<Hub>();
        hub.set_media_available(state.available);
        hub.set_media_playing(state.playing);
        hub.set_track_title(state.title.into());
        let subtitle = match (state.artist.is_empty(), state.album.is_empty()) {
            (false, false) => format!("{} · {}", state.artist, state.album),
            (false, true) => state.artist,
            (true, false) => state.album,
            (true, true) => String::new(),
        };
        hub.set_track_subtitle(subtitle.into());
        hub.set_media_position(format_time(state.position_secs).into());
        hub.set_media_duration(format_time(state.duration_secs).into());
        hub.set_media_progress(if state.duration_secs > 0 {
            (state.position_secs as f32 / state.duration_secs as f32).min(1.0)
        } else {
            0.0
        });
        hub.set_shuffle(state.shuffle);
        hub.set_repeat(state.repeat);
        hub.set_can_shuffle(state.can_shuffle);
        hub.set_can_repeat(state.can_repeat);
        hub.set_can_seek(state.can_seek);
    });
}

fn publish_art(ui: &slint::Weak<AppWindow>, art: Option<SharedPixelBuffer<Rgba8Pixel>>) {
    let color = art.as_ref().and_then(|pixels| main_color(pixels.as_slice()));
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let hub = ui.global::<Hub>();
        match art {
            Some(pixels) => {
                hub.set_album_art(slint::Image::from_rgba8(pixels));
                hub.set_has_art(true);
            }
            None => {
                hub.set_has_art(false);
                hub.set_album_art(slint::Image::default());
            }
        }
        let palette = ui.global::<crate::Palette>();
        palette.set_has_art_color(color.is_some());
        if let Some([r, g, b]) = color {
            palette.set_art(slint::Color::from_rgb_u8(r, g, b));
        }
    });
}

/// The cover's most common lively color, made light enough to read dark text on, for the
/// accent while music plays. None for covers that are all grey.
fn main_color(pixels: &[Rgba8Pixel]) -> Option<[u8; 3]> {
    // Count colors in coarse buckets (4 bits a channel), weighted by how vivid they are.
    let mut buckets = vec![(0f32, [0f32; 3]); 4096];
    for pixel in pixels.iter().step_by(7) {
        let [r, g, b] = [pixel.r, pixel.g, pixel.b].map(f32::from);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        // Near-black, near-white and grey pixels say little about the cover's color.
        if max < 40.0 || saturation < 0.25 {
            continue;
        }
        let index = ((pixel.r as usize >> 4) << 8) | ((pixel.g as usize >> 4) << 4) | (pixel.b as usize >> 4);
        let weight = saturation * saturation;
        let bucket = &mut buckets[index];
        bucket.0 += weight;
        for (sum, value) in bucket.1.iter_mut().zip([r, g, b]) {
            *sum += value * weight;
        }
    }
    let (weight, sums) = buckets.into_iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
    if weight <= 0.0 {
        return None;
    }
    Some(readable(sums.map(|s| s / weight)))
}

/// Lifts a color's lightness and saturation so it works as an accent on the dark theme,
/// with dark text on it.
fn readable([r, g, b]: [f32; 3]) -> [u8; 3] {
    let [r, g, b] = [r / 255.0, g / 255.0, b / 255.0];
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    let mut saturation = if delta == 0.0 { 0.0 } else { delta / (1.0 - (2.0 * lightness - 1.0).abs()) };
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    saturation = saturation.clamp(0.45, 0.9);
    let lightness = lightness.clamp(0.62, 0.78);
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hue as u32 / 60 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    [r, g, b].map(|c| ((c + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

fn format_time(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Decodes cover art and shrinks very large images so they don't waste memory.
fn decode_art(bytes: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    const MAX_SIZE: u32 = 640;
    let mut image = image::load_from_memory(bytes).ok()?.to_rgba8();
    if image.width() > MAX_SIZE || image.height() > MAX_SIZE {
        let scale = MAX_SIZE as f32 / image.width().max(image.height()) as f32;
        image = image::imageops::resize(
            &image,
            (image.width() as f32 * scale) as u32,
            (image.height() as f32 * scale) as u32,
            image::imageops::FilterType::Triangle,
        );
    }
    Some(SharedPixelBuffer::clone_from_slice(
        image.as_raw(),
        image.width(),
        image.height(),
    ))
}

#[cfg(windows)]
mod backend {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as SessionManager,
        GlobalSystemMediaTransportControlsSessionMediaProperties as MediaProperties,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    };
    use windows::Media::MediaPlaybackAutoRepeatMode as RepeatMode;
    use windows::Storage::Streams::DataReader;

    use super::*;

    /// How often to check for track changes when nothing was pressed.
    const POLL_INTERVAL: Duration = Duration::from_millis(500);
    /// Players often publish the cover a moment after the title, so keep asking for a while.
    const ART_ATTEMPTS: u32 = 8;
    /// Windows timestamps count 100 ns ticks from 1601; Unix time starts 11644473600 s later.
    const TICKS_PER_SEC: i64 = 10_000_000;
    const UNIX_EPOCH_TICKS: i64 = 11_644_473_600 * TICKS_PER_SEC;

    pub fn run(rx: Receiver<MediaCommand>, ui: slint::Weak<AppWindow>) {
        let manager = match SessionManager::RequestAsync().and_then(|op| op.join()) {
            Ok(manager) => manager,
            Err(err) => {
                eprintln!("media: could not connect to Windows media sessions: {err}");
                publish(&ui, MediaState::default());
                return;
            }
        };

        let mut last: Option<MediaState> = None;
        // The track the current cover belongs to, and how many more times to look for one.
        let mut art_track: Option<(String, String, String)> = None;
        let mut art_attempts_left = 0;
        // The app picked on the hub, by app id; None follows Windows' choice.
        let mut chosen: Option<String> = None;
        let mut app_ids: Vec<String> = Vec::new();
        let mut last_players: Option<Players> = None;
        loop {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(MediaCommand::ChoosePlayer(index)) => {
                    chosen = usize::try_from(index).ok().and_then(|i| app_ids.get(i).cloned());
                }
                Ok(command) => {
                    let result = pick(&manager, chosen.as_deref())
                        .ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))
                        .and_then(|session| send(&session, command));
                    if let Err(err) = result {
                        eprintln!("media: {command:?} failed: {err}");
                    }
                    // Give the player a moment to react before reading its state back.
                    std::thread::sleep(Duration::from_millis(150));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            let session = pick(&manager, chosen.as_deref());
            app_ids = all_sessions(&manager)
                .iter()
                .filter_map(|s| s.SourceAppUserModelId().ok().map(|id| id.to_string()))
                .collect();
            let current_id = session.as_ref().and_then(|s| s.SourceAppUserModelId().ok()).map(|id| id.to_string());
            let players = Players {
                labels: app_ids.iter().map(|id| player_name(id)).collect(),
                current: current_id
                    .and_then(|id| app_ids.iter().position(|a| *a == id))
                    .map_or(-1, |i| i as i32),
                automatic: chosen.as_ref().is_none_or(|c| !app_ids.contains(c)),
            };
            if last_players.as_ref() != Some(&players) {
                publish_players(&ui, players.clone());
                last_players = Some(players);
            }

            let props = session
                .as_ref()
                .and_then(|s| s.TryGetMediaPropertiesAsync().and_then(|op| op.join()).ok());
            let state = match &session {
                Some(session) => read(session, props.as_ref()),
                None => MediaState::default(),
            };

            let track = (state.title.clone(), state.artist.clone(), state.album.clone());
            if art_track.as_ref() != Some(&track) {
                art_track = Some(track);
                art_attempts_left = ART_ATTEMPTS;
                publish_art(&ui, None);
            }
            if art_attempts_left > 0 {
                art_attempts_left -= 1;
                if let Some(art) = props.as_ref().and_then(read_art) {
                    art_attempts_left = 0;
                    publish_art(&ui, Some(art));
                }
            }

            if last.as_ref() != Some(&state) {
                publish(&ui, state.clone());
                last = Some(state);
            }
        }
    }

    fn all_sessions(manager: &SessionManager) -> Vec<Session> {
        manager.GetSessions().map(|list| list.into_iter().collect()).unwrap_or_default()
    }

    /// The session picked on the hub while it's still open, or else Windows' current one.
    fn pick(manager: &SessionManager, chosen: Option<&str>) -> Option<Session> {
        chosen
            .and_then(|id| {
                all_sessions(manager)
                    .into_iter()
                    .find(|s| s.SourceAppUserModelId().is_ok_and(|a| a == id))
            })
            .or_else(|| manager.GetCurrentSession().ok())
    }

    fn send(session: &Session, command: MediaCommand) -> windows::core::Result<()> {
        let operation = match command {
            MediaCommand::PlayPause => session.TryTogglePlayPauseAsync()?,
            MediaCommand::Next => session.TrySkipNextAsync()?,
            MediaCommand::Previous => session.TrySkipPreviousAsync()?,
            MediaCommand::ToggleShuffle => {
                let info = session.GetPlaybackInfo()?;
                let on = info.IsShuffleActive().and_then(|v| v.Value()).unwrap_or(false);
                session.TryChangeShuffleActiveAsync(!on)?
            }
            MediaCommand::ToggleRepeat => {
                let info = session.GetPlaybackInfo()?;
                let mode = info.AutoRepeatMode().and_then(|v| v.Value()).unwrap_or(RepeatMode::None);
                let next = if mode == RepeatMode::None { RepeatMode::List } else { RepeatMode::None };
                session.TryChangeAutoRepeatModeAsync(next)?
            }
            MediaCommand::Seek(fraction) => {
                let timeline = session.GetTimelineProperties()?;
                let start = timeline.StartTime()?.Duration;
                let end = timeline.EndTime()?.Duration;
                let target = start + ((end - start).max(0) as f64 * fraction.clamp(0.0, 1.0) as f64) as i64;
                session.TryChangePlaybackPositionAsync(target)?
            }
            MediaCommand::ChoosePlayer(_) => return Ok(()),
        };
        operation.join()?;
        Ok(())
    }

    fn read(session: &Session, props: Option<&MediaProperties>) -> MediaState {
        let text = |get: fn(&MediaProperties) -> windows::core::Result<windows::core::HSTRING>| {
            props.and_then(|p| get(p).ok()).map(|s| s.to_string()).unwrap_or_default()
        };
        let info = session.GetPlaybackInfo().ok();
        let playing = info
            .as_ref()
            .and_then(|i| i.PlaybackStatus().ok())
            .is_some_and(|status| status == PlaybackStatus::Playing);
        let controls = info.as_ref().and_then(|i| i.Controls().ok());
        let (position_secs, duration_secs) = timeline(session, playing);

        MediaState {
            available: true,
            playing,
            title: text(MediaProperties::Title),
            artist: text(MediaProperties::Artist),
            album: text(MediaProperties::AlbumTitle),
            position_secs,
            duration_secs,
            shuffle: info
                .as_ref()
                .and_then(|i| i.IsShuffleActive().and_then(|v| v.Value()).ok())
                .unwrap_or(false),
            repeat: info
                .as_ref()
                .and_then(|i| i.AutoRepeatMode().and_then(|v| v.Value()).ok())
                .is_some_and(|mode| mode != RepeatMode::None),
            can_shuffle: controls.as_ref().and_then(|c| c.IsShuffleEnabled().ok()).unwrap_or(false),
            can_repeat: controls.as_ref().and_then(|c| c.IsRepeatEnabled().ok()).unwrap_or(false),
            can_seek: controls.as_ref().and_then(|c| c.IsPlaybackPositionEnabled().ok()).unwrap_or(false),
        }
    }

    /// Position and length in seconds. Players only report the position now and then,
    /// so while playing, add the time since their last report.
    fn timeline(session: &Session, playing: bool) -> (u64, u64) {
        let Ok(timeline) = session.GetTimelineProperties() else {
            return (0, 0);
        };
        let start = timeline.StartTime().map(|t| t.Duration).unwrap_or(0);
        let end = timeline.EndTime().map(|t| t.Duration).unwrap_or(0);
        let mut position = timeline.Position().map(|t| t.Duration).unwrap_or(0) - start;
        let duration = (end - start).max(0);

        if playing {
            let updated = timeline.LastUpdatedTime().map(|t| t.UniversalTime).unwrap_or(0);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64 / 100 + UNIX_EPOCH_TICKS)
                .unwrap_or(0);
            if updated > 0 && now > updated {
                position += now - updated;
            }
        }
        if duration > 0 {
            position = position.min(duration);
        }
        (
            (position.max(0) / TICKS_PER_SEC) as u64,
            (duration / TICKS_PER_SEC) as u64,
        )
    }

    fn read_art(props: &MediaProperties) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
        let size = u32::try_from(stream.Size().ok()?).ok().filter(|&s| s > 0)?;
        let reader = DataReader::CreateDataReader(&stream).ok()?;
        reader.LoadAsync(size).ok()?.join().ok()?;
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes).ok()?;
        decode_art(&bytes)
    }
}

/// Media control is Windows-only for now; elsewhere the page shows its empty state.
#[cfg(not(windows))]
mod backend {
    use super::*;

    pub fn run(rx: Receiver<MediaCommand>, ui: slint::Weak<AppWindow>) {
        publish(&ui, MediaState::default());
        let _ = (publish_art, decode_art, publish_players);
        // Drain commands until the UI closes so button presses don't error.
        while let Ok(command) = rx.recv() {
            eprintln!("media: {command:?} ignored (only supported on Windows)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::player_name;

    #[test]
    fn player_names() {
        assert_eq!(player_name("Spotify.exe"), "Spotify");
        assert_eq!(player_name("Chrome"), "Chrome");
        assert_eq!(player_name("MSEdge"), "Edge");
        assert_eq!(player_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Media Player");
        assert_eq!(player_name(r"C:\Program Files\VideoLAN\VLC\vlc.exe"), "VLC");
        assert_eq!(player_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
    }
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn cover_colors() {
        let red = Rgba8Pixel { r: 200, g: 30, b: 30, a: 255 };
        let grey = Rgba8Pixel { r: 120, g: 120, b: 120, a: 255 };
        let mut cover = vec![grey; 700];
        cover.extend(vec![red; 300]);
        let [r, g, b] = main_color(&cover).unwrap();
        assert!(r > 200 && g < 160 && b < 160, "{r} {g} {b}");
        assert_eq!(main_color(&[grey; 100]), None);
    }
}
