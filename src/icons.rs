//! Line icons for shortcut tiles, as SVG path commands on a 24x24 grid.
//! hub.toml picks one by name with `icon = "..."`.

/// Every icon name, in the order the tile editor shows them.
pub const NAMES: [&str; 26] = [
    "mic", "mic-off", "speaker", "speaker-off", "music", "screenshot", "replay", "record", "game", "chat",
    "globe", "terminal", "folder", "settings", "activity", "lock", "moon", "sun", "power", "wifi", "bluetooth",
    "bell", "bell-off", "calendar", "heart", "app",
];

pub fn glyph(name: Option<&str>) -> &'static str {
    match name.unwrap_or("app") {
        "mic" => "M9 5a3 3 0 0 1 6 0v6a3 3 0 0 1 -6 0z M19 10v1a7 7 0 0 1-14 0v-1 M12 18v4",
        "mic-off" => "M9 5a3 3 0 0 1 6 0v6a3 3 0 0 1 -6 0z M19 10v1a7 7 0 0 1-14 0v-1 M12 18v4 M3 3l18 18",
        "screenshot" => "M3 7V5a2 2 0 0 1 2-2h2 M17 3h2a2 2 0 0 1 2 2v2 M21 17v2a2 2 0 0 1-2 2h-2 M7 21H5a2 2 0 0 1-2-2v-2 M9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0",
        "replay" => "M11 19L2 12l9-7z M22 19l-9-7 9-7z",
        "record" => "M3 12a9 9 0 1 0 18 0a9 9 0 1 0 -18 0 M8 12a4 4 0 1 0 8 0a4 4 0 1 0 -8 0",
        "lock" => "M6 11h12a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2z M8 11V7a4 4 0 0 1 8 0v4",
        "activity" => "M22 12h-4l-3 9L9 3l-3 9H2",
        "globe" => "M3 12a9 9 0 1 0 18 0a9 9 0 1 0 -18 0 M3 12h18 M12 3a14 14 0 0 1 0 18a14 14 0 0 1 0-18",
        "terminal" => "M4 17l6-5-6-5 M12 19h8",
        "moon" => "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z",
        "music" => "M9 18V5l12-2v13 M3 18a3 3 0 1 0 6 0a3 3 0 1 0 -6 0 M15 16a3 3 0 1 0 6 0a3 3 0 1 0 -6 0",
        "chat" => "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z",
        "game" => "M6 12h4 M8 10v4 M15 13h.01 M18 11h.01 M17.3 5H6.7a4 4 0 0 0-3.98 3.6L2 15.5A3 3 0 0 0 7.2 17.6L9 15h6l1.8 2.6a3 3 0 0 0 5.2-2.1l-.7-6.9A4 4 0 0 0 17.3 5z",
        "folder" => "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
        "settings" => "M9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0 M12 2v3 M12 19v3 M4.9 4.9l2.1 2.1 M17 17l2.1 2.1 M2 12h3 M19 12h3 M4.9 19.1L7 17 M17 7l2.1-2.1",
        "speaker" => "M11 5L6 9H2v6h4l5 4z M15.5 8.5a5 5 0 0 1 0 7 M19 5a10 10 0 0 1 0 14",
        "speaker-off" => "M11 5L6 9H2v6h4l5 4z M22 9l-6 6 M16 9l6 6",
        "power" => "M12 2v10 M18.4 6.6a9 9 0 1 1-12.8 0",
        "wifi" => "M5 12.55a11 11 0 0 1 14.08 0 M1.42 9a16 16 0 0 1 21.16 0 M8.53 16.11a6 6 0 0 1 6.95 0 M12 20h.01",
        "bluetooth" => "M7 7l10 10-5 5V2l5 5L7 17",
        "sun" => "M8 12a4 4 0 1 0 8 0a4 4 0 1 0 -8 0 M12 2v2 M12 20v2 M4.9 4.9l1.4 1.4 M17.7 17.7l1.4 1.4 M2 12h2 M20 12h2 M6.3 17.7l-1.4 1.4 M19.1 4.9l-1.4 1.4",
        "bell" => "M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9 M13.73 21a2 2 0 0 1-3.46 0",
        "bell-off" => "M13.73 21a2 2 0 0 1-3.46 0 M18.63 13A17.9 17.9 0 0 1 18 8 M6.26 6.26A5.9 5.9 0 0 0 6 8c0 7-3 9-3 9h14 M18 8a6 6 0 0 0-9.33-5 M2 2l20 20",
        "calendar" => "M5 4h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z M16 2v4 M8 2v4 M3 10h18",
        "heart" => "M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8l1 1.1L12 21l7.8-7.5 1-1.1a5.5 5.5 0 0 0 0-7.8z",
        _ => "M4 4h6v6H4z M14 4h6v6h-6z M4 14h6v6H4z M14 14h6v6h-6z",
    }
}

/// The icon for a mute tile while muted: the crossed-out version where there is one.
pub fn glyph_on(name: Option<&str>) -> &'static str {
    match name {
        Some("mic") => glyph(Some("mic-off")),
        Some("speaker") => glyph(Some("speaker-off")),
        Some("bell") => glyph(Some("bell-off")),
        other => glyph(other),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_listed_icon_has_its_own_glyph() {
        let generic = super::glyph(Some("app"));
        for name in super::NAMES.iter().filter(|n| **n != "app") {
            assert_ne!(super::glyph(Some(name)), generic, "{name} has no glyph");
        }
    }
}
