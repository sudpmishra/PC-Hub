//! Discord voice: who is in your voice channel and who is talking, with mute and deafen.
//!
//! This talks to the Discord app on this PC over its local connection (RPC). Discord only
//! allows that for apps registered on its developer site, and voice access only for the
//! app's own developers, so you register a free app of your own: see README > Discord.
//! The first time, Discord asks you to allow the hub; after that it signs in by itself
//! with a token kept in discord-token.json beside hub.toml.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::DiscordConfig;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub speaking: bool,
    /// Muted or deafened, by themselves or the server.
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// What's going on: "Connected", "Open Discord", "Allow PC Hub in Discord"...
    pub status: String,
    pub connected: bool,
    /// The voice channel you're in, or empty.
    pub channel: String,
    pub members: Vec<Member>,
    pub mute: bool,
    pub deaf: bool,
}

pub enum Command {
    ToggleMute,
    ToggleDeaf,
    /// hub.toml's [discord] changed.
    Config(DiscordConfig),
}

pub type Shared = Arc<Mutex<State>>;

pub fn start(config: DiscordConfig, token_file: PathBuf) -> (Sender<Command>, Shared) {
    let (commands, received) = mpsc::channel();
    let shared: Shared = Arc::new(Mutex::new(State::default()));
    let state = shared.clone();
    std::thread::Builder::new()
        .name("discord".into())
        .spawn(move || run(config, token_file, received, state))
        .expect("failed to start the Discord thread");
    (commands, shared)
}

fn set_status(shared: &Shared, status: &str) {
    if let Ok(mut state) = shared.lock() {
        *state = State {
            status: status.to_string(),
            ..State::default()
        };
    }
}

fn run(mut config: DiscordConfig, token_file: PathBuf, commands: Receiver<Command>, shared: Shared) {
    loop {
        let result = if config.is_set() {
            Session::open(&config, &token_file, &shared).and_then(|mut session| session.serve(&commands, &shared))
        } else {
            Err("Not set up: add [discord] to hub.toml".into())
        };
        match result {
            // hub.toml changed: start over with the new settings.
            Ok(Some(new)) => {
                config = new;
                continue;
            }
            Ok(None) => return,
            Err(err) => set_status(&shared, &err),
        }
        // Wait a while before trying again, still listening for new settings.
        let until = Instant::now() + Duration::from_secs(10);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match commands.recv_timeout(left) {
                Ok(Command::Config(new)) => {
                    config = new;
                    break;
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;

struct Session {
    pipe: std::fs::File,
    nonce: u64,
    channel_id: Option<String>,
    state: State,
}

/// Saved sign-in, so Discord doesn't ask every time.
#[derive(serde::Serialize, serde::Deserialize)]
struct Token {
    access_token: String,
    refresh_token: String,
}

impl Session {
    fn open(config: &DiscordConfig, token_file: &PathBuf, shared: &Shared) -> Result<Self, String> {
        let pipe = (0..10)
            .find_map(|n| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(format!(r"\\.\pipe\discord-ipc-{n}"))
                    .ok()
            })
            .ok_or("Open Discord to see your voice channel")?;
        let mut session = Session {
            pipe,
            nonce: 0,
            channel_id: None,
            state: State::default(),
        };
        session.send(HANDSHAKE, &json!({ "v": 1, "client_id": config.client_id }))?;
        let ready = session.read_frame(Duration::from_secs(10))?;
        if ready.get("evt").and_then(Value::as_str) != Some("READY") {
            return Err(format!("Discord said: {}", message(&ready)));
        }

        // Sign in with the saved token, a refreshed one, or by asking.
        let saved: Option<Token> =
            std::fs::read_to_string(token_file).ok().and_then(|t| serde_json::from_str(&t).ok());
        let mut signed_in = false;
        if let Some(token) = &saved {
            signed_in = session.authenticate(&token.access_token).is_ok();
            if !signed_in {
                if let Ok(new) = exchange(config, &[("grant_type", "refresh_token"), ("refresh_token", &token.refresh_token)]) {
                    signed_in = session.authenticate(&new.access_token).is_ok();
                    save_token(token_file, &new);
                }
            }
        }
        if !signed_in {
            set_status(shared, "Allow PC Hub in the Discord window");
            let answer = session.request(
                "AUTHORIZE",
                json!({ "client_id": config.client_id, "scopes": ["rpc", "rpc.voice.read", "rpc.voice.write"] }),
                Duration::from_secs(120),
            )?;
            let code = answer["data"]["code"].as_str().ok_or("Discord didn't allow the hub")?.to_string();
            let token = exchange(
                config,
                &[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", &config.redirect_uri)],
            )?;
            session.authenticate(&token.access_token)?;
            save_token(token_file, &token);
        }

        session.subscribe("VOICE_CHANNEL_SELECT", json!({}))?;
        session.subscribe("VOICE_SETTINGS_UPDATE", json!({}))?;
        let settings = session.request("GET_VOICE_SETTINGS", json!({}), Duration::from_secs(10))?;
        session.voice_settings(&settings["data"]);
        let channel = session.request("GET_SELECTED_VOICE_CHANNEL", json!({}), Duration::from_secs(10))?;
        session.show_channel(&channel["data"])?;
        session.state.connected = true;
        session.state.status = "Connected".into();
        Ok(session)
    }

    fn send(&mut self, op: u32, body: &Value) -> Result<(), String> {
        let bytes = body.to_string().into_bytes();
        let mut frame = Vec::with_capacity(bytes.len() + 8);
        frame.extend(op.to_le_bytes());
        frame.extend((bytes.len() as u32).to_le_bytes());
        frame.extend(bytes);
        self.pipe.write_all(&frame).map_err(|_| "Discord closed the connection".to_string())
    }

    /// Bytes waiting to be read, without blocking.
    fn waiting(&self) -> Result<u32, String> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::Pipes::PeekNamedPipe;
            let mut available = 0u32;
            unsafe { PeekNamedPipe(HANDLE(self.pipe.as_raw_handle()), None, 0, None, Some(&mut available), None) }
                .map_err(|_| "Discord closed the connection".to_string())?;
            Ok(available)
        }
        #[cfg(not(windows))]
        Ok(0)
    }

    /// The next message from Discord, waiting up to `timeout` for one to start.
    fn read_frame(&mut self, timeout: Duration) -> Result<Value, String> {
        let until = Instant::now() + timeout;
        loop {
            if self.waiting()? >= 8 {
                break;
            }
            if Instant::now() >= until {
                return Err("Discord didn't answer".into());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        let mut header = [0u8; 8];
        self.pipe.read_exact(&mut header).map_err(|e| e.to_string())?;
        let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let mut body = vec![0u8; length];
        self.pipe.read_exact(&mut body).map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        match op {
            CLOSE => Err(format!("Discord closed the connection: {}", message(&value))),
            PING => {
                self.send(PONG, &value)?;
                self.read_frame(timeout)
            }
            _ => Ok(value),
        }
    }

    /// Sends a command and waits for its answer, handling events that arrive meanwhile.
    fn request(&mut self, command: &str, args: Value, timeout: Duration) -> Result<Value, String> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        self.send(FRAME, &json!({ "cmd": command, "args": args, "nonce": nonce }))?;
        let until = Instant::now() + timeout;
        loop {
            let left = until.checked_duration_since(Instant::now()).ok_or("Discord didn't answer")?;
            let frame = self.read_frame(left)?;
            if frame.get("nonce").and_then(Value::as_str) == Some(nonce.as_str()) {
                if frame.get("evt").and_then(Value::as_str) == Some("ERROR") {
                    return Err(format!("Discord said: {}", message(&frame)));
                }
                return Ok(frame);
            }
            self.event(&frame)?;
        }
    }

    fn subscribe(&mut self, event: &str, args: Value) -> Result<(), String> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        self.send(FRAME, &json!({ "cmd": "SUBSCRIBE", "evt": event, "args": args, "nonce": nonce }))
    }

    fn unsubscribe(&mut self, event: &str, args: Value) -> Result<(), String> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        self.send(FRAME, &json!({ "cmd": "UNSUBSCRIBE", "evt": event, "args": args, "nonce": nonce }))
    }

    fn authenticate(&mut self, token: &str) -> Result<(), String> {
        self.request("AUTHENTICATE", json!({ "access_token": token }), Duration::from_secs(10)).map(drop)
    }

    fn voice_settings(&mut self, data: &Value) {
        if let Some(mute) = data["mute"].as_bool() {
            self.state.mute = mute;
        }
        if let Some(deaf) = data["deaf"].as_bool() {
            self.state.deaf = deaf;
        }
    }

    /// Shows a voice channel (or none) and follows who joins, leaves and talks in it.
    fn show_channel(&mut self, channel: &Value) -> Result<(), String> {
        const EVENTS: [&str; 5] = ["SPEAKING_START", "SPEAKING_STOP", "VOICE_STATE_CREATE", "VOICE_STATE_UPDATE", "VOICE_STATE_DELETE"];
        let id = channel["id"].as_str().map(String::from);
        if id != self.channel_id {
            if let Some(old) = self.channel_id.take() {
                for event in EVENTS {
                    self.unsubscribe(event, json!({ "channel_id": old }))?;
                }
            }
            if let Some(new) = &id {
                for event in EVENTS {
                    self.subscribe(event, json!({ "channel_id": new }))?;
                }
            }
            self.channel_id = id;
        }
        self.state.channel = channel["name"].as_str().unwrap_or_default().to_string();
        self.state.members = channel["voice_states"]
            .as_array()
            .map(|states| states.iter().filter_map(member).collect())
            .unwrap_or_default();
        Ok(())
    }

    fn event(&mut self, frame: &Value) -> Result<(), String> {
        let data = &frame["data"];
        match frame.get("evt").and_then(Value::as_str).unwrap_or_default() {
            "VOICE_SETTINGS_UPDATE" => self.voice_settings(data),
            "VOICE_CHANNEL_SELECT" => {
                // Fetch the channel's members; null means you left voice.
                if data["channel_id"].is_null() {
                    self.show_channel(&Value::Null)?;
                } else {
                    let channel = self.request("GET_CHANNEL", json!({ "channel_id": data["channel_id"] }), Duration::from_secs(10))?;
                    self.show_channel(&channel["data"])?;
                }
            }
            "SPEAKING_START" | "SPEAKING_STOP" => {
                let speaking = frame["evt"] == "SPEAKING_START";
                let user = data["user_id"].as_str().unwrap_or_default();
                for member in self.state.members.iter_mut().filter(|m| m.id == user) {
                    member.speaking = speaking;
                }
            }
            "VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE" => {
                if let Some(new) = member(data) {
                    match self.state.members.iter_mut().find(|m| m.id == new.id) {
                        Some(old) => {
                            old.name = new.name;
                            old.muted = new.muted;
                        }
                        None => self.state.members.push(new),
                    }
                }
            }
            "VOICE_STATE_DELETE" => {
                let user = data["user"]["id"].as_str().unwrap_or_default().to_string();
                self.state.members.retain(|m| m.id != user);
            }
            _ => {}
        }
        Ok(())
    }

    /// Answers the hub's buttons and follows Discord until the connection ends. Returns
    /// new settings when hub.toml changed, or None when the hub is closing.
    fn serve(&mut self, commands: &Receiver<Command>, shared: &Shared) -> Result<Option<DiscordConfig>, String> {
        loop {
            if let Ok(mut state) = shared.lock() {
                if *state != self.state {
                    *state = self.state.clone();
                }
            }
            match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(Command::ToggleMute) => {
                    let mute = !self.state.mute;
                    let answer = self.request("SET_VOICE_SETTINGS", json!({ "mute": mute }), Duration::from_secs(5))?;
                    self.voice_settings(&answer["data"]);
                }
                Ok(Command::ToggleDeaf) => {
                    let deaf = !self.state.deaf;
                    let answer = self.request("SET_VOICE_SETTINGS", json!({ "deaf": deaf }), Duration::from_secs(5))?;
                    self.voice_settings(&answer["data"]);
                }
                Ok(Command::Config(new)) => return Ok(Some(new)),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
            while self.waiting()? >= 8 {
                let frame = self.read_frame(Duration::from_secs(1))?;
                self.event(&frame)?;
            }
        }
    }
}

/// A voice state from Discord as a member of the channel.
fn member(voice: &Value) -> Option<Member> {
    let user = &voice["user"];
    let id = user["id"].as_str()?.to_string();
    let name = voice["nick"]
        .as_str()
        .or(user["global_name"].as_str())
        .or(user["username"].as_str())
        .unwrap_or("Someone")
        .to_string();
    let state = &voice["voice_state"];
    let flag = |key: &str| state[key].as_bool().unwrap_or(false);
    Some(Member {
        id,
        name,
        speaking: false,
        muted: flag("mute") || flag("self_mute") || flag("deaf") || flag("self_deaf"),
    })
}

fn message(frame: &Value) -> String {
    frame["data"]["message"].as_str().or(frame["message"].as_str()).unwrap_or("unknown error").to_string()
}

/// Gets a token from Discord's sign-in service.
fn exchange(config: &DiscordConfig, fields: &[(&str, &str)]) -> Result<Token, String> {
    let mut form: Vec<(&str, &str)> = vec![("client_id", &config.client_id), ("client_secret", &config.client_secret)];
    form.extend_from_slice(fields);
    let mut response = ureq::post("https://discord.com/api/oauth2/token")
        .send_form(form)
        .map_err(|e| format!("Discord sign-in failed: {e}"))?;
    let token: Value = response.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(Token {
        access_token: token["access_token"].as_str().ok_or("Discord sign-in failed")?.to_string(),
        refresh_token: token["refresh_token"].as_str().unwrap_or_default().to_string(),
    })
}

fn save_token(path: &PathBuf, token: &Token) {
    if let Ok(text) = serde_json::to_string(token) {
        if let Err(err) = std::fs::write(path, text) {
            eprintln!("discord: could not save the sign-in: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_from_voice_states() {
        let voice = json!({
            "nick": "Sam",
            "user": { "id": "1", "username": "sam_99", "global_name": "Samuel" },
            "voice_state": { "self_mute": true, "deaf": false }
        });
        let m = member(&voice).unwrap();
        assert_eq!((m.id.as_str(), m.name.as_str(), m.muted), ("1", "Sam", true));
        let plain = json!({ "user": { "id": "2", "username": "alex" }, "voice_state": {} });
        assert_eq!(member(&plain).unwrap().name, "alex");
        assert!(member(&json!({})).is_none());
    }
}
