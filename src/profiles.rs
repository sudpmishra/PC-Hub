//! Profiles: named sets of shortcuts and settings (hub.toml and hub-settings.toml), kept
//! in a "profiles" folder beside them, one folder each, such as "profiles/Gaming".
//! Switching saves the current files into the profile in use first, so changes made while
//! a profile is in use stay with it.

use std::path::{Path, PathBuf};

const FILES: [&str; 2] = ["hub.toml", "hub-settings.toml"];
/// Names the profile in use.
const ACTIVE: &str = "active.txt";

pub fn folder(config_dir: &Path) -> PathBuf {
    config_dir.join("profiles")
}

/// A name that's safe as a folder name, or why not.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the profile needs a name".into());
    }
    if name.chars().count() > 40 || name.chars().any(|c| "<>:\"/\\|?*".contains(c) || c.is_control()) || name.trim_matches('.').is_empty() {
        return Err("use letters, numbers and spaces for the profile's name".into());
    }
    Ok(name.to_string())
}

/// Profile names, sorted.
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
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// The profile in use, if it still exists.
pub fn active(config_dir: &Path) -> Option<String> {
    let name = std::fs::read_to_string(folder(config_dir).join(ACTIVE)).ok()?;
    let name = name.trim().to_string();
    folder(config_dir).join(&name).is_dir().then_some(name)
}

fn set_active(config_dir: &Path, name: &str) -> Result<(), String> {
    std::fs::write(folder(config_dir).join(ACTIVE), name).map_err(|e| e.to_string())
}

fn copy_files(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for file in FILES {
        if from.join(file).is_file() {
            std::fs::copy(from.join(file), to.join(file)).map_err(|e| format!("{file}: {e}"))?;
        }
    }
    Ok(())
}

/// Saves the current shortcuts and settings as a profile (replacing one of that name) and
/// makes it the one in use.
pub fn save(config_dir: &Path, name: &str) -> Result<String, String> {
    let name = check_name(name)?;
    copy_files(config_dir, &folder(config_dir).join(&name))?;
    set_active(config_dir, &name)?;
    Ok(name)
}

/// Switches to a profile: the current files go into the profile in use (or a backup, when
/// none is), then the profile's files replace them.
pub fn switch(config_dir: &Path, name: &str) -> Result<(), String> {
    let name = check_name(name)?;
    let source = folder(config_dir).join(&name);
    if !source.is_dir() {
        return Err(format!("there's no profile called \"{name}\""));
    }
    match active(config_dir) {
        Some(current) if current == name => return Ok(()),
        Some(current) => copy_files(config_dir, &folder(config_dir).join(current))?,
        None => {
            crate::backup::create(config_dir)?;
        }
    }
    for file in FILES {
        if source.join(file).is_file() {
            std::fs::copy(source.join(file), config_dir.join(file)).map_err(|e| format!("{file}: {e}"))?;
        }
    }
    set_active(config_dir, &name)
}

pub fn delete(config_dir: &Path, name: &str) -> Result<(), String> {
    let name = check_name(name)?;
    std::fs::remove_dir_all(folder(config_dir).join(&name)).map_err(|e| e.to_string())?;
    if active(config_dir).is_none() {
        let _ = std::fs::remove_file(folder(config_dir).join(ACTIVE));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_switch_and_delete() {
        let dir = std::env::temp_dir().join(format!("pc-hub-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let read = || std::fs::read_to_string(dir.join("hub.toml")).unwrap();

        std::fs::write(dir.join("hub.toml"), "columns = 4\n").unwrap();
        assert_eq!(save(&dir, " Work ").unwrap(), "Work");
        std::fs::write(dir.join("hub.toml"), "columns = 6\n").unwrap();
        save(&dir, "Gaming").unwrap();
        assert_eq!(list(&dir), ["Gaming", "Work"]);
        assert_eq!(active(&dir).as_deref(), Some("Gaming"));

        // Changes made while Gaming is in use go with it.
        std::fs::write(dir.join("hub.toml"), "columns = 7\n").unwrap();
        switch(&dir, "Work").unwrap();
        assert_eq!(read(), "columns = 4\n");
        switch(&dir, "Gaming").unwrap();
        assert_eq!(read(), "columns = 7\n");

        delete(&dir, "Gaming").unwrap();
        assert_eq!(list(&dir), ["Work"]);
        assert_eq!(active(&dir), None);
        assert!(switch(&dir, "Nope").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name("..").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
