//! Artwork for the window's chrome: tinted interface icons and the toolbar's tag buttons.

macro_rules! art {
    ($path:literal) => {
        &[include_str!(concat!("../assets/", $path, ".svg"))]
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
pub const ITALIC: &[&str] = art!("icons/italic");
pub const LINK: &[&str] = art!("icons/link");
pub const NOTEBOOK: &[&str] = art!("icons/notebook");
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
pub const STRIKETHROUGH: &[&str] = art!("icons/strikethrough");
pub const STYLES: &[&str] = art!("icons/styles");
pub const SUBSCRIPT: &[&str] = art!("icons/subscript");
pub const SUPERSCRIPT: &[&str] = art!("icons/superscript");
pub const TABLE: &[&str] = art!("icons/table");
pub const UNDERLINE: &[&str] = art!("icons/underline");
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
