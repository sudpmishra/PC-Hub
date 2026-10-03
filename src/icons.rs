//! Line icons for shortcut tiles, as SVG path commands on a 24x24 grid.
//! hub.toml picks one by name with `icon = "..."`.

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
        "power" => "M12 2v10 M18.4 6.6a9 9 0 1 1-12.8 0",
        _ => "M4 4h6v6H4z M14 4h6v6h-6z M4 14h6v6H4z M14 14h6v6h-6z",
    }
}
