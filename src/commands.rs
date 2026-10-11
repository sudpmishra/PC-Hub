//! Tiles that work in the background and show the result: a PowerShell command (the first
//! line it prints), a web request (the first line of the answer), or a macro of steps.

use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::audio::AudioCommand;
use crate::config::{Step, Work};

/// Does a tile's work and returns what to show on it. Blocks, so call it from a
/// background thread. `audio` handles the mic-mute and mute actions in macros.
pub fn run_work(work: &Work, audio: &Sender<AudioCommand>) -> Result<String, String> {
    match work {
        Work::Command(command) => run(command),
        Work::Web(request) => crate::web::send(request),
        Work::Steps(steps) => run_steps(steps, audio),
    }
}

/// Runs macro steps in order, stopping at the first that fails.
fn run_steps(steps: &[Step], audio: &Sender<AudioCommand>) -> Result<String, String> {
    for (number, step) in steps.iter().enumerate() {
        run_step(step, audio).map_err(|err| format!("step {}: {err}", number + 1))?;
    }
    Ok(format!("Done · {} steps", steps.len()))
}

fn run_step(step: &Step, audio: &Sender<AudioCommand>) -> Result<(), String> {
    if let Some(keys) = &step.keys {
        crate::actions::send_keys(keys)?;
        // Key presses are held briefly on their own thread; let them finish first.
        std::thread::sleep(Duration::from_millis(120));
    } else if let Some(action) = &step.action {
        match action.as_str() {
            "mic-mute" => audio.send(AudioCommand::ToggleMicMute).map_err(|e| e.to_string())?,
            "mute" => audio.send(AudioCommand::ToggleMute).map_err(|e| e.to_string())?,
            action => crate::actions::system(action)?,
        }
    } else if let Some(run) = step.run.as_deref().filter(|r| !r.is_empty()) {
        crate::config::spawn(run, &step.args).map_err(|e| e.to_string())?;
    } else if let Some(command) = &step.command {
        self::run(command)?;
    } else if let Some(request) = step.web() {
        crate::web::send(&request)?;
    } else if let Some(seconds) = step.wait {
        std::thread::sleep(Duration::from_secs_f32(seconds.clamp(0.0, 600.0)));
    } else {
        return Err("nothing to do: set keys, action, run, command, url or wait".into());
    }
    Ok(())
}

/// Longest output shown on a tile; the rest is cut off with an ellipsis.
const MAX_CHARS: usize = 60;

/// Runs `command` in PowerShell, hidden, and returns the first line it printed. Blocks
/// until the command ends, so call it from a background thread.
pub fn run(command: &str) -> Result<String, String> {
    let mut process = Command::new(if cfg!(windows) { "powershell" } else { "pwsh" });
    process
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    let output = process.output().map_err(|e| format!("could not start PowerShell: {e}"))?;
    let stdout = first_line(&String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(stdout.unwrap_or_else(|| "Done".into()))
    } else {
        Err(first_line(&String::from_utf8_lossy(&output.stderr))
            .or(stdout)
            .unwrap_or_else(|| format!("failed ({})", output.status)))
    }
}

/// The first line with something on it, trimmed and shortened to fit a tile.
pub fn first_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(if line.chars().count() > MAX_CHARS {
        format!("{}…", line.chars().take(MAX_CHARS - 1).collect::<String>())
    } else {
        line.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_skips_blanks_and_shortens() {
        assert_eq!(first_line("\r\n  \n hello \nworld"), Some("hello".into()));
        assert_eq!(first_line("  \n"), None);
        let long = "x".repeat(100);
        let shown = first_line(&long).unwrap();
        assert_eq!(shown.chars().count(), MAX_CHARS);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn macros_run_in_order_and_stop_at_a_failure() {
        let (audio, received) = std::sync::mpsc::channel();
        let steps = vec![
            Step { action: Some("mute".into()), ..Default::default() },
            Step { wait: Some(0.01), ..Default::default() },
            Step { action: Some("mic-mute".into()), ..Default::default() },
        ];
        assert_eq!(run_steps(&steps, &audio), Ok("Done · 3 steps".into()));
        assert!(matches!(received.try_recv(), Ok(AudioCommand::ToggleMute)));
        assert!(matches!(received.try_recv(), Ok(AudioCommand::ToggleMicMute)));

        let failing = vec![Step::default(), Step { action: Some("mute".into()), ..Default::default() }];
        assert!(run_steps(&failing, &audio).unwrap_err().starts_with("step 1:"));
        assert!(received.try_recv().is_err());
    }

    #[cfg(windows)]
    #[test]
    fn runs_powershell() {
        assert_eq!(run("Write-Output 'hi there'"), Ok("hi there".into()));
        assert!(run("throw 'nope'").unwrap_err().contains("nope"));
    }
}
