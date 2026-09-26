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
pub const CLOCK: &[&str] = art!("icons/clock");
pub const CLOSE: &[&str] = art!("icons/close");
pub const EQUATION: &[&str] = art!("icons/equation");
pub const FONT_COLOR: &[&str] = art!("icons/font-color");
pub const HIGHLIGHTER: &[&str] = art!("icons/highlighter");
pub const INDENT: &[&str] = art!("icons/indent");
pub const ITALIC: &[&str] = art!("icons/italic");
pub const LINK: &[&str] = art!("icons/link");
pub const NEW_PAGE: &[&str] = art!("icons/new-page");
pub const NUMBERING: &[&str] = art!("icons/numbering");
pub const OUTDENT: &[&str] = art!("icons/outdent");
pub const PICTURE: &[&str] = art!("icons/picture");
pub const REDO: &[&str] = art!("icons/redo");
pub const SEARCH: &[&str] = art!("icons/search");
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

pub const TAG_STAR: &[&str] = art!("tags/star");
pub const TAG_REMEMBER: &[&str] = art!("tags/remember");
pub const TAG_DEFINITION: &[&str] = art!("tags/definition");
pub const TAG_HIGHLIGHT: &[&str] = art!("tags/highlight");
pub const TAG_CONTACT: &[&str] = art!("tags/contact");
pub const TAG_ADDRESS: &[&str] = art!("tags/address");
pub const TAG_PHONE: &[&str] = art!("tags/phone");
