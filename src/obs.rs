//! OBS Studio: switch scenes, start and stop recording and streaming, and show how long
//! each has run. Talks to OBS's built-in WebSocket server (Tools > WebSocket Server
//! Settings in OBS 28 and later), on a thread of its own.

use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::config::ObsConfig;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub status: String,
    pub connected: bool,
    /// In the order OBS lists them, top first.
    pub scenes: Vec<String>,
    pub current: String,
    pub recording: bool,
    pub record_paused: bool,
    /// "0:12:05", or empty when not recording.
    pub record_time: String,
    pub streaming: bool,
    pub stream_time: String,
}

pub enum Command {
    Scene(String),
    ToggleRecord,
    ToggleStream,
    Config(ObsConfig),
}

pub type Shared = Arc<Mutex<State>>;

pub fn start(config: ObsConfig) -> (Sender<Command>, Shared) {
    let (commands, received) = mpsc::channel();
    let shared: Shared = Arc::new(Mutex::new(State::default()));
    let state = shared.clone();
    std::thread::Builder::new()
        .name("obs".into())
        .spawn(move || run(config, received, state))
        .expect("failed to start the OBS thread");
    (commands, shared)
}

fn run(mut config: ObsConfig, commands: Receiver<Command>, shared: Shared) {
    loop {
        let result = if config.enabled {
            Connection::open(&config).and_then(|mut obs| obs.serve(&commands, &shared))
        } else {
            Err("Not set up: add [obs] to hub.toml".into())
        };
        match result {
            Ok(Some(new)) => {
                config = new;
                continue;
            }
            Ok(None) => return,
            Err(err) => {
                if let Ok(mut state) = shared.lock() {
                    *state = State { status: err, ..State::default() };
                }
            }
        }
        let until = Instant::now() + Duration::from_secs(5);
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

/// obs-websocket's sign-in answer: base64(sha256(base64(sha256(password + salt)) + challenge)).
fn auth(password: &str, salt: &str, challenge: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = b64.encode(Sha256::digest(format!("{password}{salt}")));
    b64.encode(Sha256::digest(format!("{secret}{challenge}")))
}

/// "00:12:05.123" -> "0:12:05"
fn timecode(text: &str) -> String {
    let text = text.split('.').next().unwrap_or(text);
    text.strip_prefix('0').filter(|t| t.len() == 7).unwrap_or(text).to_string()
}

struct Connection {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl Connection {
    fn open(config: &ObsConfig) -> Result<Self, String> {
        let url = format!("ws://{}:{}", config.host, config.port);
        let (mut socket, _) = tungstenite::connect(&url).map_err(|_| "Start OBS with its WebSocket server on".to_string())?;
        if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
        }
        let mut obs = Connection { socket, next_id: 0 };
        let hello = obs.receive(Duration::from_secs(5))?.ok_or("OBS didn't say hello")?;
        let mut identify = json!({ "rpcVersion": 1, "eventSubscriptions": 0 });
        if let Some(challenge) = hello["d"]["authentication"].as_object() {
            if config.password.is_empty() {
                return Err("OBS wants a password: set password under [obs] in hub.toml".into());
            }
            identify["authentication"] = json!(auth(
                &config.password,
                challenge["salt"].as_str().unwrap_or_default(),
                challenge["challenge"].as_str().unwrap_or_default()
            ));
        }
        obs.send(json!({ "op": 1, "d": identify }))?;
        let identified = obs.receive(Duration::from_secs(5))?;
        if identified.as_ref().and_then(|m| m["op"].as_u64()) != Some(2) {
            return Err("OBS didn't accept the password".into());
        }
        Ok(obs)
    }

    fn send(&mut self, message: Value) -> Result<(), String> {
        self.socket.send(Message::text(message.to_string())).map_err(|e| e.to_string())
    }

    /// The next message, or None when nothing came within `timeout`.
    fn receive(&mut self, timeout: Duration) -> Result<Option<Value>, String> {
        let until = Instant::now() + timeout;
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => return Ok(serde_json::from_str(&text).ok()),
                Ok(Message::Close(_)) => return Err("OBS closed the connection".into()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(err))
                    if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    if Instant::now() >= until {
                        return Ok(None);
                    }
                }
                Err(_) => return Err("OBS closed the connection".into()),
            }
        }
    }

    fn request(&mut self, kind: &str, data: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        self.send(json!({ "op": 6, "d": { "requestType": kind, "requestId": id, "requestData": data } }))?;
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let left = until.checked_duration_since(Instant::now()).ok_or("OBS didn't answer")?;
            if let Some(message) = self.receive(left)? {
                if message["op"].as_u64() == Some(7) && message["d"]["requestId"].as_str() == Some(id.as_str()) {
                    let status = &message["d"]["requestStatus"];
                    if status["result"].as_bool() != Some(true) {
                        return Err(format!("OBS: {}", status["comment"].as_str().unwrap_or("request failed")));
                    }
                    return Ok(message["d"]["responseData"].clone());
                }
            }
        }
    }

    fn read_state(&mut self) -> Result<State, String> {
        let scenes = self.request("GetSceneList", json!({}))?;
        let record = self.request("GetRecordStatus", json!({}))?;
        let stream = self.request("GetStreamStatus", json!({}))?;
        let active = |v: &Value| v["outputActive"].as_bool().unwrap_or(false);
        let time = |v: &Value| if active(v) { timecode(v["outputTimecode"].as_str().unwrap_or_default()) } else { String::new() };
        Ok(State {
            status: "Connected".into(),
            connected: true,
            // OBS lists scenes bottom first.
            scenes: scenes["scenes"]
                .as_array()
                .map(|list| list.iter().rev().filter_map(|s| s["sceneName"].as_str().map(String::from)).collect())
                .unwrap_or_default(),
            current: scenes["currentProgramSceneName"].as_str().unwrap_or_default().to_string(),
            recording: active(&record),
            record_paused: record["outputPaused"].as_bool().unwrap_or(false),
            record_time: time(&record),
            streaming: active(&stream),
            stream_time: time(&stream),
        })
    }

    fn serve(&mut self, commands: &Receiver<Command>, shared: &Shared) -> Result<Option<ObsConfig>, String> {
        loop {
            let state = self.read_state()?;
            if let Ok(mut shared) = shared.lock() {
                *shared = state;
            }
            match commands.recv_timeout(Duration::from_secs(1)) {
                Ok(Command::Scene(name)) => {
                    self.request("SetCurrentProgramScene", json!({ "sceneName": name }))?;
                }
                Ok(Command::ToggleRecord) => {
                    self.request("ToggleRecord", json!({}))?;
                }
                Ok(Command::ToggleStream) => {
                    self.request("ToggleStream", json!({}))?;
                }
                Ok(Command::Config(new)) => return Ok(Some(new)),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_answer() {
        // From obs-websocket's protocol documentation.
        assert_eq!(
            auth("supersecretpassword", "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=", "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY="),
            "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4="
        );
    }

    #[test]
    fn timecodes() {
        assert_eq!(timecode("00:12:05.123"), "0:12:05");
        assert_eq!(timecode("01:02:03.000"), "1:02:03");
        assert_eq!(timecode("12:00:00.000"), "12:00:00");
    }
}
