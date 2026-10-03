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
    });
}

fn publish_art(ui: &slint::Weak<AppWindow>, art: Option<SharedPixelBuffer<Rgba8Pixel>>) {
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
    });
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
        loop {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(command) => {
                    if let Err(err) = send(&manager, command) {
                        eprintln!("media: {command:?} failed: {err}");
                    }
                    // Give the player a moment to react before reading its state back.
                    std::thread::sleep(Duration::from_millis(150));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            let session = manager.GetCurrentSession().ok();
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

    fn send(manager: &SessionManager, command: MediaCommand) -> windows::core::Result<()> {
        let session = manager.GetCurrentSession()?;
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
        let _ = (publish_art, decode_art);
        // Drain commands until the UI closes so button presses don't error.
        while let Ok(command) = rx.recv() {
            eprintln!("media: {command:?} ignored (only supported on Windows)");
        }
    }
}
