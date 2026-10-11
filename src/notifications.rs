//! Windows notifications (the ones in the notification center) from the apps chosen in
//! hub.toml, read every few seconds on a background thread. Windows asks once whether the
//! hub may read them.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: String,
    /// "14:05", or the date for older ones.
    pub time: String,
}

pub enum Command {
    /// Removes one from the notification center.
    Dismiss(u32),
    DismissAll,
    /// Only these apps (part of their name, any case); empty for every app.
    Apps(Vec<String>),
}

pub type Latest = Arc<Mutex<Option<Result<Vec<Notice>, String>>>>;

/// How many are kept for the hub.
const KEEP: usize = 12;

pub fn start(apps: Vec<String>) -> (Sender<Command>, Latest) {
    let (commands, received) = mpsc::channel();
    let latest: Latest = Arc::new(Mutex::new(None));
    let writer = latest.clone();
    std::thread::Builder::new()
        .name("notifications".into())
        .spawn(move || {
            let mut apps = apps;
            loop {
                let result = imp::read(&apps).map(|mut notices| {
                    notices.truncate(KEEP);
                    notices
                });
                if let Ok(mut latest) = writer.lock() {
                    *latest = Some(result);
                }
                match received.recv_timeout(Duration::from_secs(3)) {
                    Ok(Command::Dismiss(id)) => imp::dismiss(Some(id)),
                    Ok(Command::DismissAll) => imp::dismiss(None),
                    Ok(Command::Apps(new)) => apps = new,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("failed to start the notifications thread");
    (commands, latest)
}

fn wanted(apps: &[String], app: &str) -> bool {
    let app = app.to_lowercase();
    apps.is_empty() || apps.iter().any(|a| !a.trim().is_empty() && app.contains(&a.trim().to_lowercase()))
}

#[cfg(windows)]
mod imp {
    use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
    use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

    use super::Notice;

    fn listener() -> Result<UserNotificationListener, String> {
        let listener = UserNotificationListener::Current().map_err(|e| e.to_string())?;
        let mut status = listener.GetAccessStatus().map_err(|e| e.to_string())?;
        if status == UserNotificationListenerAccessStatus::Unspecified {
            status = listener.RequestAccessAsync().and_then(|op| op.join()).map_err(|e| e.to_string())?;
        }
        if status != UserNotificationListenerAccessStatus::Allowed {
            return Err("Windows doesn't allow reading notifications (Settings > Privacy > Notifications)".into());
        }
        Ok(listener)
    }

    pub fn read(apps: &[String]) -> Result<Vec<Notice>, String> {
        let listener = listener()?;
        let list = listener
            .GetNotificationsAsync(NotificationKinds::Toast)
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        let mut notices: Vec<(i64, Notice)> = list
            .into_iter()
            .filter_map(|n| notice(&n).ok())
            .filter(|(_, n)| super::wanted(apps, &n.app))
            .collect();
        notices.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
        Ok(notices.into_iter().map(|(_, n)| n).collect())
    }

    fn notice(n: &UserNotification) -> windows::core::Result<(i64, Notice)> {
        let app = n.AppInfo()?.DisplayInfo()?.DisplayName()?.to_string();
        let binding = n.Notification()?.Visual()?.GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
        let texts: Vec<String> = binding
            .GetTextElements()?
            .into_iter()
            .filter_map(|t| t.Text().ok().map(|t| t.to_string()))
            .filter(|t| !t.trim().is_empty())
            .collect();
        let ticks = n.CreationTime()?.UniversalTime;
        Ok((
            ticks,
            Notice {
                id: n.Id()?,
                app,
                title: texts.first().cloned().unwrap_or_default(),
                body: texts.get(1..).map(|rest| rest.join(" ")).unwrap_or_default(),
                time: time_text(ticks),
            },
        ))
    }

    /// Windows time (100 ns ticks since 1601) as local "14:05", or "3 Oct" if not today.
    fn time_text(ticks: i64) -> String {
        const UNIX_EPOCH_TICKS: i64 = 11_644_473_600 * 10_000_000;
        let Some(utc) = chrono::DateTime::from_timestamp((ticks - UNIX_EPOCH_TICKS) / 10_000_000, 0) else {
            return String::new();
        };
        let local = utc.with_timezone(&chrono::Local);
        if local.date_naive() == chrono::Local::now().date_naive() {
            local.format("%H:%M").to_string()
        } else {
            local.format("%-d %b").to_string()
        }
    }

    pub fn dismiss(id: Option<u32>) {
        let Ok(listener) = listener() else { return };
        let result = match id {
            Some(id) => listener.RemoveNotification(id),
            None => listener.ClearNotifications(),
        };
        if let Err(err) = result {
            eprintln!("notifications: could not dismiss: {err}");
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Notice;

    pub fn read(_apps: &[String]) -> Result<Vec<Notice>, String> {
        Err("only supported on Windows".into())
    }

    pub fn dismiss(_id: Option<u32>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_filter() {
        assert!(wanted(&[], "Mail"));
        let apps = vec!["discord".to_string(), "Teams".to_string()];
        assert!(wanted(&apps, "Discord"));
        assert!(wanted(&apps, "Microsoft Teams"));
        assert!(!wanted(&apps, "Mail"));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads this PC's notifications"]
    fn reads_notifications() {
        println!("{:?}", imp::read(&[]));
    }
}
