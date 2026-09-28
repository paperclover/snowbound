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
pub const SIDEBAR_COLLAPSE: &[&str] = art!("icons/sidebar-collapse");
pub const SIDEBAR_EXPAND: &[&str] = art!("icons/sidebar-expand");
pub const STRIKETHROUGH: &[&str] = art!("icons/strikethrough");
pub const SUBSCRIPT: &[&str] = art!("icons/subscript");
pub const SUPERSCRIPT: &[&str] = art!("icons/superscript");
pub const TABLE: &[&str] = art!("icons/table");
pub const UNDERLINE: &[&str] = art!("icons/underline");
pub const UNDO: &[&str] = art!("icons/undo");
pub const ZOOM_IN: &[&str] = art!("icons/zoom-in");
pub const ZOOM_OUT: &[&str] = art!("icons/zoom-out");

// The toolbar layout's new buttons (resources/toolbar-spec.md); each loses its `expect`
// when its button lands.
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const BACK: &[&str] = art!("icons/back");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const COPY: &[&str] = art!("icons/copy");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const CUT: &[&str] = art!("icons/cut");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const DATE_TIME: &[&str] = art!("icons/date-time");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const FIND_TAGS: &[&str] = art!("icons/find-tags");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const FORMAT_PAINTER: &[&str] = art!("icons/format-painter");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const FORWARD: &[&str] = art!("icons/forward");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const FULL_PAGE_VIEW: &[&str] = art!("icons/full-page-view");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const INSERT_SPACE: &[&str] = art!("icons/insert-space");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const PAGE_COLOR: &[&str] = art!("icons/page-color");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const PASTE: &[&str] = art!("icons/paste");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const RECORD_AUDIO: &[&str] = art!("icons/record-audio");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const RECORD_VIDEO: &[&str] = art!("icons/record-video");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const RULE_LINES: &[&str] = art!("icons/rule-lines");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const SCREEN_CLIPPING: &[&str] = art!("icons/screen-clipping");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const SPELLING: &[&str] = art!("icons/spelling");
#[expect(dead_code, reason = "Its toolbar button has not landed")]
pub const SYMBOL: &[&str] = art!("icons/symbol");

pub const TAG_REMEMBER: &[&str] = art!("tags/remember");
pub const TAG_DEFINITION: &[&str] = art!("tags/definition");
