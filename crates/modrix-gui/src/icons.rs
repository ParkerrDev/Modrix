// SPDX-License-Identifier: GPL-2.0-only
//! The icon set: simple 24-unit stroked outlines drawn in the current colour.

use rune::Icon;

macro_rules! stroked {
    ($id:ident, $name:literal, $body:literal) => {
        #[doc = concat!("The ", $name, " icon.")]
        pub static $id: Icon = Icon {
            name: concat!("modrix-", $name),
            view: 24.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        };
    };
}

stroked!(
    HOME,
    "home",
    r#"<path d="M3.5 10.5 12 3.5l8.5 7"/><path d="M5.5 9v11h13V9"/><path d="M10 20v-6h4v6"/>"#
);
stroked!(
    LAYERS,
    "layers",
    r#"<path d="M12 3.5 21 8l-9 4.5L3 8z"/><path d="m3 12 9 4.5 9-4.5"/><path d="m3 16 9 4.5 9-4.5"/>"#
);
stroked!(
    ORDER,
    "order",
    r#"<path d="M10 6h10M10 12h10M10 18h10"/><path d="M4 5h2v3M4 8h2.5M4 11.5h2.4L4 15h2.6"/><path d="M4 18h2.5l-1 1.2a1 1 0 1 1-1.5 1"/>"#
);
stroked!(
    CONFLICT,
    "conflict",
    r#"<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="12" r="2.5"/><path d="M6 8.5v7"/><path d="M8.3 7.2c3.5 1 5.5 2.5 7.4 3.8"/><path d="M8.3 16.8c3.5-1 5.5-2.5 7.4-3.8"/>"#
);
stroked!(
    DOWNLOAD,
    "download",
    r#"<path d="M12 4v11"/><path d="m7 10.5 5 5 5-5"/><path d="M4.5 19.5h15"/>"#
);
stroked!(
    GAMEPAD,
    "gamepad",
    r#"<path d="M7 7.5h10a4.5 4.5 0 0 1 4.4 5.4l-.8 4a2.6 2.6 0 0 1-4.6 1L14.5 16h-5L8 17.9a2.6 2.6 0 0 1-4.6-1l-.8-4A4.5 4.5 0 0 1 7 7.5z"/><path d="M8 10.5v3M6.5 12h3"/><path d="M15.5 11h.01M17.5 13h.01"/>"#
);
stroked!(
    SETTINGS,
    "settings",
    r#"<circle cx="12" cy="12" r="3"/><path d="M12 2.8v2.4M12 18.8v2.4M2.8 12h2.4M18.8 12h2.4M5.5 5.5l1.7 1.7M16.8 16.8l1.7 1.7M5.5 18.5l1.7-1.7M16.8 7.2l1.7-1.7"/>"#
);
stroked!(
    SEARCH,
    "search",
    r#"<circle cx="11" cy="11" r="6.5"/><path d="m20 20-4.4-4.4"/>"#
);
stroked!(PLUS, "plus", r#"<path d="M12 5v14M5 12h14"/>"#);
stroked!(
    BELL,
    "bell",
    r#"<path d="M6 10a6 6 0 0 1 12 0c0 5 2 6.5 2 6.5H4S6 15 6 10z"/><path d="M10 19.5a2 2 0 0 0 4 0"/>"#
);
stroked!(PLAY, "play", r#"<path d="M7 4.5v15l12.5-7.5z"/>"#);
stroked!(CHECK, "check", r#"<path d="m5 12.5 4.5 4.5L19 7.5"/>"#);
stroked!(CLOSE, "close", r#"<path d="M6 6l12 12M18 6 6 18"/>"#);
stroked!(CHEVRON_DOWN, "chevron-down", r#"<path d="m6 9 6 6 6-6"/>"#);
stroked!(
    CHEVRON_RIGHT,
    "chevron-right",
    r#"<path d="m9 6 6 6-6 6"/>"#
);
stroked!(
    CHEVRON_UP_DOWN,
    "chevron-up-down",
    r#"<path d="m8 9.5 4-4 4 4M8 14.5l4 4 4-4"/>"#
);
stroked!(
    GRIP,
    "grip",
    r#"<circle cx="9" cy="6" r=".9"/><circle cx="15" cy="6" r=".9"/><circle cx="9" cy="12" r=".9"/><circle cx="15" cy="12" r=".9"/><circle cx="9" cy="18" r=".9"/><circle cx="15" cy="18" r=".9"/>"#
);
stroked!(ARROW_UP, "arrow-up", r#"<path d="M12 19V5M6 11l6-6 6 6"/>"#);
stroked!(
    ARROW_DOWN,
    "arrow-down",
    r#"<path d="M12 5v14M6 13l6 6 6-6"/>"#
);
stroked!(
    TRASH,
    "trash",
    r#"<path d="M4 7h16"/><path d="M9.5 7V4.5h5V7"/><path d="M6 7l1 13h10l1-13"/><path d="M10 11v5M14 11v5"/>"#
);
stroked!(
    REFRESH,
    "refresh",
    r#"<path d="M20 11.5A8 8 0 0 0 6 6.6L4 8.5"/><path d="M4 4v4.5h4.5"/><path d="M4 12.5A8 8 0 0 0 18 17.4l2-1.9"/><path d="M20 20v-4.5h-4.5"/>"#
);
stroked!(
    DEPLOY,
    "deploy",
    r#"<path d="M12 15.5V3.5"/><path d="m7 8.5 5-5 5 5"/><path d="M5 13.5v5a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-5"/>"#
);
stroked!(
    SHIELD,
    "shield",
    r#"<path d="M12 3 19.5 6v5.5c0 4.6-3.2 8.2-7.5 9.5-4.3-1.3-7.5-4.9-7.5-9.5V6z"/><path d="m8.8 12 2.2 2.2 4.3-4.4"/>"#
);
stroked!(
    ERASE,
    "erase",
    r#"<path d="m14.5 4.5 5 5L11 18H6l-2.5-2.5z"/><path d="m9.5 9.5 5 5"/><path d="M11 20h9"/>"#
);
stroked!(
    FOLDER,
    "folder",
    r#"<path d="M3.5 6.5a2 2 0 0 1 2-2h4l2 2.5h7a2 2 0 0 1 2 2v8.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/>"#
);
stroked!(
    COPY,
    "copy",
    r#"<rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2"/><path d="M15.5 8.5V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v7.5a2 2 0 0 0 2 2h2.5"/>"#
);
stroked!(
    SPARKLES,
    "sparkles",
    r#"<path d="M11 3.5 12.6 8 17 9.5l-4.4 1.6L11 15.5 9.4 11.1 5 9.5 9.4 8z"/><path d="M18 14.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z"/>"#
);
stroked!(
    PUZZLE,
    "puzzle",
    r#"<path d="M9 4.5h3.5V6a1.8 1.8 0 1 0 3.5 0V4.5H19v5h-1.5a1.8 1.8 0 1 0 0 3.5H19v6.5h-5V18a1.8 1.8 0 1 0-3.5 0v1.5H5v-5h1.5a1.8 1.8 0 1 0 0-3.5H5V4.5z"/>"#
);
stroked!(
    WARNING,
    "warning",
    r#"<path d="M12 4 21.5 20h-19z"/><path d="M12 10v4.5M12 17.5h.01"/>"#
);
stroked!(
    INFO,
    "info",
    r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5.5M12 7.8h.01"/>"#
);
stroked!(
    LINK,
    "link",
    r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#
);
stroked!(
    PACKAGE,
    "package",
    r#"<path d="M12 3 20 7.5v9L12 21l-8-4.5v-9z"/><path d="m4 7.5 8 4.5 8-4.5M12 12v9"/>"#
);
stroked!(
    SLIDERS,
    "sliders",
    r#"<path d="M5 20v-6M5 10V4M12 20v-8M12 8V4M19 20v-4M19 12V4"/><path d="M3 14h4M10 8h4M17 16h4"/>"#
);
stroked!(
    UPDATE,
    "update",
    r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 16V8M8.5 11.5 12 8l3.5 3.5"/>"#
);
stroked!(
    PIN,
    "pin",
    r#"<path d="M9 4h6l-1 6 3 3H7l3-3z"/><path d="M12 13v7"/>"#
);
