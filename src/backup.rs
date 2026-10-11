//! Copies of hub.toml and hub-settings.toml, kept in a "backups" folder beside them, one
//! folder per backup named by its date and time, such as "2026-10-11 14-30-05".

use std::path::{Path, PathBuf};

/// The files a backup holds.
const FILES: [&str; 2] = ["hub.toml", "hub-settings.toml"];
/// Older backups than the newest this many are deleted.
const KEEP: usize = 10;

/// The backups folder beside hub.toml.
pub fn folder(config_dir: &Path) -> PathBuf {
    config_dir.join("backups")
}

/// Copies the files into a new backup, deletes the oldest past KEEP, and returns its name.
pub fn create(config_dir: &Path) -> Result<String, String> {
    // Two backups in the same second (say, one just before a restore) get " 2", " 3"...
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
    let name = (1..)
        .map(|n| if n == 1 { stamp.clone() } else { format!("{stamp} {n}") })
        .find(|name| !folder(config_dir).join(name).exists())
        .unwrap_or(stamp);
    let target = folder(config_dir).join(&name);
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut copied = 0;
    for file in FILES {
        let source = config_dir.join(file);
        if source.is_file() {
            std::fs::copy(&source, target.join(file)).map_err(|e| format!("{file}: {e}"))?;
            copied += 1;
        }
    }
    if copied == 0 {
        let _ = std::fs::remove_dir(&target);
        return Err("there's no hub.toml or hub-settings.toml to back up".into());
    }
    for old in list(config_dir).into_iter().skip(KEEP) {
        let _ = std::fs::remove_dir_all(folder(config_dir).join(old));
    }
    Ok(name)
}

/// Backup names, newest first.
pub fn list(config_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder(config_dir))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    // The names are dates, so sorting them sorts by time.
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
}

/// Puts a backup's files back. The current ones are backed up first, so a restore can be
/// undone by restoring that backup.
pub fn restore(config_dir: &Path, name: &str) -> Result<(), String> {
    let source = folder(config_dir).join(name);
    if name.contains(['/', '\\']) || !source.is_dir() {
        return Err(format!("there's no backup called \"{name}\""));
    }
    create(config_dir)?;
    for file in FILES {
        let saved = source.join(file);
        if saved.is_file() {
            std::fs::copy(&saved, config_dir.join(file)).map_err(|e| format!("{file}: {e}"))?;
        }
    }
    Ok(())
}

/// "2026-10-11 14-30-05" -> "11 Oct 2026, 14:30"
pub fn describe(name: &str) -> String {
    chrono::NaiveDateTime::parse_from_str(name.get(..19).unwrap_or(name), "%Y-%m-%d %H-%M-%S")
        .map(|time| time.format("%-d %b %Y, %H:%M").to_string())
        .unwrap_or_else(|_| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pc-hub-backup-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn back_up_and_restore() {
        let dir = temp_dir("restore");
        std::fs::write(dir.join("hub.toml"), "columns = 4\n").unwrap();
        let name = create(&dir).unwrap();
        assert_eq!(list(&dir), std::slice::from_ref(&name));

        std::fs::write(dir.join("hub.toml"), "columns = 6\n").unwrap();
        restore(&dir, &name).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("hub.toml")).unwrap(), "columns = 4\n");
        // The file from before the restore was backed up too.
        assert_eq!(list(&dir).len(), 2);
        let undo = &list(&dir)[0];
        assert_eq!(std::fs::read_to_string(folder(&dir).join(undo).join("hub.toml")).unwrap(), "columns = 6\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_to_back_up_and_bad_names() {
        let dir = temp_dir("empty");
        assert!(create(&dir).is_err());
        assert!(list(&dir).is_empty());
        assert!(restore(&dir, r"..\elsewhere").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_read_as_dates() {
        assert_eq!(describe("2026-10-11 14-30-05"), "11 Oct 2026, 14:30");
        assert_eq!(describe("odd"), "odd");
    }
}
