//! Artwork for the window's chrome: tinted interface icons and the toolbar's tag buttons.
//!
//! Neutral parts drawn over the row, as arrows and a page's or window's outer edge, paint
//! `currentColor` at an opacity, so they follow the label's colour on any row or highlight;
//! coloured parts, and what lies on a sheet, keep their colours.

/// Artwork of one or more files under `assets/`, painted in order, as a badge over its icon.
macro_rules! art {
    ($($path:literal),+) => {
        &[$(include_str!(concat!("../assets/", $path, ".svg"))),+]
    };
}

pub const ALIGN_CENTER: &[&str] = art!("icons/align-center");
pub const ALIGN_RIGHT: &[&str] = art!("icons/align-right");
pub const ALIGN_LEFT: &[&str] = art!("icons/align-left");
pub const ATTACHMENT: &[&str] = art!("icons/attachment");
pub const BOLD: &[&str] = art!("icons/bold");
pub const BULLETS: &[&str] = art!("icons/bullets");
pub const CALENDAR: &[&str] = art!("icons/calendar");
pub const CLEAR_FORMATTING: &[&str] = art!("icons/clear-formatting");
pub const CHEVRON_UP: &[&str] = art!("icons/chevron-up");
pub const CLOCK: &[&str] = art!("icons/clock");
pub const CLOSE: &[&str] = art!("icons/close");
pub const CONFLICT: &[&str] = art!("icons/conflict");
pub const EQUATION: &[&str] = art!("icons/equation");
pub const FOLDER: &[&str] = art!("icons/folder");
pub const ICLOUD: &[&str] = art!("icons/icloud");
pub const FONT_COLOR: &[&str] = art!("icons/font-color");
pub const HIGHLIGHTER: &[&str] = art!("icons/highlighter");
pub const INDENT: &[&str] = art!("icons/indent");
pub const INFO: &[&str] = art!("icons/info");
pub const ITALIC: &[&str] = art!("icons/italic");
pub const LINK: &[&str] = art!("icons/link");
/// A notebook: its back and tabs, the cover `currentColor` paints, and what lies on the cover.
/// iOS layers the same files.
pub const NOTEBOOK: &[&str] = &[
    include_str!("../assets/icons/notebook-back.svg"),
    include_str!("../assets/icons/notebook-cover.svg"),
    include_str!("../assets/icons/notebook.svg"),
];
/// A notebook iCloud Drive keeps: its glyph with a cloud at the corner.
pub const NOTEBOOK_ICLOUD: &[&str] = &[
    NOTEBOOK[0],
    NOTEBOOK[1],
    NOTEBOOK[2],
    include_str!("../assets/icons/icloud-badge.svg"),
];
pub const NUMBERING: &[&str] = art!("icons/numbering");
pub const OPTIONS: &[&str] = art!("icons/options");
pub const OUTDENT: &[&str] = art!("icons/outdent");
pub const PAGE: &[&str] = art!("icons/page");
pub const PICTURE: &[&str] = art!("icons/picture");
pub const PLUS: &[&str] = art!("icons/plus");
pub const REDO: &[&str] = art!("icons/redo");
pub const SEARCH: &[&str] = art!("icons/search");
pub const SECTION: &[&str] = art!("icons/section");
pub const SECTION_GROUP: &[&str] = art!("icons/section-group");
pub const SERVER: &[&str] = art!("icons/server");
pub const SIDEBAR_COLLAPSE: &[&str] = art!("icons/sidebar-collapse");
pub const SIDEBAR_EXPAND: &[&str] = art!("icons/sidebar-expand");
pub const SORT_ASCENDING: &[&str] = art!("icons/sort-ascending");
pub const SORT_DESCENDING: &[&str] = art!("icons/sort-descending");
pub const STRIKETHROUGH: &[&str] = art!("icons/strikethrough");
pub const STYLES: &[&str] = art!("icons/styles");
pub const SUBSCRIPT: &[&str] = art!("icons/subscript");
pub const SUPERSCRIPT: &[&str] = art!("icons/superscript");
pub const TABLE: &[&str] = art!("icons/table");
pub const UNDERLINE: &[&str] = art!("icons/underline");
pub const WARNING: &[&str] = art!("icons/warning");
pub const UNDO: &[&str] = art!("icons/undo");
pub const ZOOM_IN: &[&str] = art!("icons/zoom-in");
pub const ZOOM_OUT: &[&str] = art!("icons/zoom-out");

pub const BACK: &[&str] = art!("icons/back");
pub const COPY: &[&str] = art!("icons/copy");
pub const CUT: &[&str] = art!("icons/cut");
pub const DATE_TIME: &[&str] = art!("icons/date-time");
pub const FIND_TAGS: &[&str] = art!("icons/find-tags");
pub const FORMAT_PAINTER: &[&str] = art!("icons/format-painter");
pub const FORWARD: &[&str] = art!("icons/forward");
pub const INSERT_SPACE: &[&str] = art!("icons/insert-space");
pub const PAGE_COLOR: &[&str] = art!("icons/page-color");
pub const CUSTOM_COLOR: &[&str] = art!("icons/custom-color");
pub const PASTE: &[&str] = art!("icons/paste");
pub const RECORD_AUDIO: &[&str] = art!("icons/record-audio");
pub const RECORD_VIDEO: &[&str] = art!("icons/record-video");
pub const SCREEN_CLIPPING: &[&str] = art!("icons/screen-clipping");
pub const SPELLING: &[&str] = art!("icons/spelling");
pub const SWATCH: &[&str] = art!("icons/swatch");
pub const SYMBOL: &[&str] = art!("icons/symbol");
pub const SYNC_BUSY: &[&str] = art!("icons/sync-busy");
pub const SYNC_DONE: &[&str] = art!("icons/sync-done");
pub const SYNC_ERROR: &[&str] = art!("icons/sync-error");
pub const SYNC_OFFLINE: &[&str] = art!("icons/sync-offline");
pub const SYNC_WARNING: &[&str] = art!("icons/sync-warning");

pub const ERASER: &[&str] = art!("icons/eraser");
pub const LASSO: &[&str] = art!("icons/lasso");
pub const PEN: &[&str] = art!("icons/pen");
pub const SELECT: &[&str] = art!("icons/select");
pub const SHAPES: &[&str] = art!("icons/shapes");
pub const SHAPE_ARROW: &[&str] = art!("icons/shape-arrow");
pub const SHAPE_LINE: &[&str] = art!("icons/shape-line");
pub const SHAPE_OVAL: &[&str] = art!("icons/shape-oval");
pub const SHAPE_RECTANGLE: &[&str] = art!("icons/shape-rectangle");

pub const ADD_TO_DICTIONARY: &[&str] = art!("icons/spelling", "icons/badge-add");
pub const CITATION: &[&str] = art!("icons/citation");
pub const CLOSE_NOTEBOOK: &[&str] = art!(
    "icons/notebook-back",
    "icons/notebook-accent",
    "icons/notebook",
    "icons/badge-close"
);
pub const CODE: &[&str] = art!("icons/code");
pub const COMMAND_PALETTE: &[&str] = art!("icons/palette", "icons/badge-command");
pub const COPY_LINK: &[&str] = art!("icons/page", "icons/badge-link");
pub const CUSTOMIZE_TAGS: &[&str] = art!("icons/tag", "icons/badge-edit");
pub const DELETE: &[&str] = art!("icons/delete");
pub const EMPTY_RECYCLE_BIN: &[&str] = art!("icons/recycle-bin", "icons/badge-remove");
pub const EXPORT_PDF: &[&str] = art!("icons/pdf");
pub const FAST_FORWARD: &[&str] = art!("icons/fast-forward");
pub const FIND: &[&str] = art!("icons/page", "icons/badge-search");
pub const FULL_PAGE: &[&str] = art!("icons/full-page");
pub const GO_TO: &[&str] = art!("icons/palette", "icons/badge-go");
/// Heading 1 to 6, in order.
pub const HEADINGS: [&[&str]; 6] = [
    art!("icons/heading", "icons/digit-1"),
    art!("icons/heading", "icons/digit-2"),
    art!("icons/heading", "icons/digit-3"),
    art!("icons/heading", "icons/digit-4"),
    art!("icons/heading", "icons/digit-5"),
    art!("icons/heading", "icons/digit-6"),
];
pub const HELP: &[&str] = art!("icons/help");
pub const KEEP_SOURCE_FORMATTING: &[&str] = art!("icons/paste", "icons/badge-brush");
pub const KEEP_TEXT_ONLY: &[&str] = art!("icons/paste", "icons/badge-text");
pub const LIVE_SHARE: &[&str] = art!(
    "icons/notebook-back",
    "icons/notebook-accent",
    "icons/notebook",
    "icons/badge-live"
);
pub const LOCK: &[&str] = art!("icons/lock");
pub const MARKDOWN: &[&str] = art!("icons/markdown");
pub const MARK_NOTEBOOK_READ: &[&str] = art!(
    "icons/notebook-back",
    "icons/notebook-accent",
    "icons/notebook",
    "icons/badge-check"
);
pub const MARK_READ: &[&str] = art!("icons/page", "icons/badge-check");
pub const MERGE_FORMATTING: &[&str] = art!("icons/paste", "icons/badge-merge");
pub const MOVE: &[&str] = art!("icons/page", "icons/badge-go");
pub const MOVE_DOWN: &[&str] = art!("icons/move-down");
pub const MOVE_UP: &[&str] = art!("icons/move-up");
pub const NEW_NOTEBOOK: &[&str] = art!(
    "icons/notebook-back",
    "icons/notebook-accent",
    "icons/notebook",
    "icons/badge-add"
);
pub const NEW_PAGE: &[&str] = art!("icons/page", "icons/badge-add");
pub const NEW_SECTION: &[&str] = art!("icons/tab", "icons/badge-add");
pub const NEW_SECTION_GROUP: &[&str] = art!("icons/section-group", "icons/badge-add");
pub const NEW_SUBPAGE: &[&str] = art!("icons/subpage", "icons/badge-add");
pub const NEXT_UNREAD: &[&str] = art!("icons/page", "icons/badge-unread");
pub const NORMAL: &[&str] = art!("icons/normal");
pub const OPEN: &[&str] = art!("icons/folder-open");
pub const OPEN_SHARED: &[&str] = art!("icons/folder-open", "icons/badge-live");
pub const PAGE_LIST: &[&str] = art!("icons/page-list");
pub const PAGE_TITLE: &[&str] = art!("icons/page-title");
pub const PAGE_VERSIONS: &[&str] = art!("icons/page", "icons/badge-history");
pub const PAGES_MATCH_THEME: &[&str] = art!("icons/pages-match-theme");
pub const PASSWORD: &[&str] = art!("icons/tab", "icons/badge-lock");
pub const PASTE_PICTURE: &[&str] = art!("icons/paste", "icons/badge-picture");
pub const PAUSE: &[&str] = art!("icons/pause");
pub const PRINT: &[&str] = art!("icons/print");
pub const PROPERTIES: &[&str] = art!("icons/properties");
pub const QUOTE: &[&str] = art!("icons/quote");
pub const RECYCLE_BIN: &[&str] = art!("icons/recycle-bin");
pub const REMOVE_LINK: &[&str] = art!("icons/link", "icons/badge-remove");
pub const REMOVE_TAG: &[&str] = art!("icons/tag", "icons/badge-remove");
pub const RENAME: &[&str] = art!("icons/rename");
pub const REWIND: &[&str] = art!("icons/rewind");
pub const SAVE: &[&str] = art!("icons/save");
pub const SEARCH_RESULTS: &[&str] = art!("icons/page-list", "icons/badge-search");
pub const SECTION_COLOR: &[&str] = art!("icons/tab", "icons/badge-drop");
pub const SEE_PLAYBACK: &[&str] = art!("icons/page", "icons/badge-play");
pub const SEEK: &[&str] = art!("icons/clock", "icons/badge-go");
pub const SELECT_ALL: &[&str] = art!("icons/select-all");
pub const SHOW_UNREAD: &[&str] = art!(
    "icons/notebook-back",
    "icons/notebook-accent",
    "icons/notebook",
    "icons/badge-unread"
);
pub const SNAP_TO_GRID: &[&str] = art!("icons/snap-to-grid");
pub const STOP: &[&str] = art!("icons/stop");
pub const SUBPAGE: &[&str] = art!("icons/subpage");
pub const SYNC_NOW: &[&str] = art!("icons/sync-now");
pub const TO_DO: &[&str] = art!("icons/to-do");
pub const UPDATE: &[&str] = art!("icons/update");
pub const ZOOM_ACTUAL: &[&str] = art!("icons/zoom-actual");
