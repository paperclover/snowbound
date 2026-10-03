//! An immediate-mode interface kit. Builder code declares boxes every frame from
//! application state; a cache keyed by stable ids keeps hover, press, focus, scroll and
//! animation state. Input is answered with the previous frame's layout, so a frame's
//! events are routed before building and its layout is solved after.

mod access;
mod layout;
mod list;
pub mod popup;
pub mod shell;
mod text;
mod theme;
mod widgets;

pub use list::{List, Row, Rows, list};
pub use theme::{Menu, PopupMotion, Section, Shades, Shadow, Theme};
pub use widgets::{
    PaintedScroller, Scroller, ScrollerPart, Scrollers, Segments, badge, badge_width, button,
    check_box, dialog_buttons, edit_key, edit_modifiers, password_field, scrollbar, segmented,
    text_field,
};

use draw::{
    PathStyle, Primitive, RasterImage, Stroke,
    edit::{Clicks, SelectionUnit},
};
use parley::editing::Selection;
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::BitOr,
    rc::Rc,
    time::Duration,
};
use text::{Label, Texts};
use web_time::Instant;
use winit::{
    event::{Ime, MouseButton},
    keyboard::{Key, ModifiersState, NamedKey},
    window::CursorIcon,
};

/// Seconds for an animated value to close half of its remaining distance.
const HALF_LIFE: f32 = 0.03;
/// How far a box's shadow spreads, in logical pixels, and how far below the box it
/// falls; a popup's, floating higher, spreads and falls further.
pub const SHADOW: [f32; 2] = [3.0, 0.0];
const POPUP_SHADOW: [f32; 2] = [12.0, 4.0];
/// Logical size of a box's icon, and its distance from the label.
const ICON: f32 = 16.0;
const ICON_GAP: f32 = 6.0;
/// Seconds a dialog and other popups take to open and to close.
const DIALOG: [f32; 2] = [0.24, 0.16];
const POPUP: [f32; 2] = [0.16, 0.12];
/// Seconds the pointer rests on a control before its tooltip shows, as Windows' tooltips
/// wait a double-click interval; after one shows, others show at once for `TIP_WARM`
/// seconds, and one showing cold fades in over `TIP_FADE`.
const TIP_DELAY: f32 = 0.5;
const TIP_WARM: f32 = 0.5;
const TIP_FADE: f32 = 0.1;
/// How far outside a control the ring the keyboard's focus draws round it lies, and its width.
const FOCUS_RING: f32 = 2.0;

/// A box's identity across frames: its parent's id combined with a builder-chosen part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Id(u64);

impl Id {
    pub const ROOT: Id = Id(0);

    pub fn child(self, part: impl Hash) -> Id {
        let mut hasher = DefaultHasher::new();
        self.0.hash(&mut hasher);
        part.hash(&mut hasher);
        Id(hasher.finish())
    }

    /// The box's node in the interface's accessibility tree.
    pub fn node(self) -> accesskit::NodeId {
        accesskit::NodeId(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    /// Logical pixels.
    Pixels(f32),
    /// The label and padding.
    Text,
    /// A fraction of the nearest ancestor not sized by its children; across a parent sized by
    /// its children, a fraction of what the parent's other children make it.
    Fraction(f32),
    /// The children laid out along this axis, and padding.
    Children,
}

/// A size on one axis and the share of it a box keeps when its siblings overflow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub size: Size,
    pub strictness: f32,
}

impl Default for Extent {
    fn default() -> Self {
        children()
    }
}

/// Logical pixels, as `px`.
impl From<f32> for Extent {
    fn from(pixels: f32) -> Self {
        px(pixels)
    }
}

pub fn px(pixels: f32) -> Extent {
    Extent {
        size: Size::Pixels(pixels),
        strictness: 1.0,
    }
}

pub fn fit() -> Extent {
    Extent {
        size: Size::Text,
        strictness: 1.0,
    }
}

/// All the room the parent has, yielding to stricter siblings.
pub fn fill() -> Extent {
    Extent {
        size: Size::Fraction(1.0),
        strictness: 0.0,
    }
}

pub fn children() -> Extent {
    Extent {
        size: Size::Children,
        strictness: 1.0,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    #[default]
    X,
    Y,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags(u32);

impl Flags {
    /// Takes pointer presses, clicks and drags, and blocks boxes beneath it.
    pub const CLICKABLE: Flags = Flags(1);
    /// A press gives it keyboard focus.
    pub const FOCUSABLE: Flags = Flags(2);
    /// The wheel scrolls its children vertically; they may overflow it.
    pub const SCROLL: Flags = Flags(4);
    /// Children paint only within it.
    pub const CLIP: Flags = Flags(8);
    /// Placed at its `position` in the parent instead of in the parent's flow.
    pub const FLOAT: Flags = Flags(16);
    /// The host paints it and receives the events routed to it.
    pub const CUSTOM: Flags = Flags(32);
    /// Paints in place while the popup around it opens, as a field standing in for the one
    /// it opened from, and as tall as that box.
    pub const STILL: Flags = Flags(64);

    pub(crate) fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    fn intersects(self, other: Flags) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for Flags {
    type Output = Flags;

    fn bitor(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }
}

/// How a box's fill and border are outlined; `radius` rounds the named corners.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Shape {
    #[default]
    Rounded,
    /// A section tab: the top leading corner rounded and the top trailing corner `lean`
    /// pixels inside the box's width, from where the trailing edge leans out at 45°, so
    /// neighbours overlap and a taller tab only reaches further along the bottom.
    Tab { lean: f32 },
}

/// What a label does where it is wider than its box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    /// Runs on as one line, cut at the box's edge.
    #[default]
    Clip,
    /// Breaks into lines across the box, which grows to their height when sized by its text.
    Wrap,
    /// Stays one line, ending in an ellipsis where it is cut.
    Ellipsis,
}

/// The least a popup keeps between itself and each of the window's edges.
const POPUP_MARGIN: f32 = 4.0;

/// The longest a popup may be in a window `room` long; one longer scrolls or squeezes.
fn fitting(room: f32) -> f32 {
    (room - 2.0 * POPUP_MARGIN).max(0.0)
}

/// Where a popup opens, flipping to the far side of its anchor where the window ends first.
/// The box it opens beside is placed where layout puts it in the same frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// Under the box, from its leading edge, as a drop-down opens; at least as wide as it.
    Below(Id),
    /// Past the trailing edge of the box, or of the popup holding it, level with the box's
    /// top, as a submenu opens.
    Right(Id),
    /// Over the box from its corner, the popup's contents covering it, as a combo box opens
    /// into its own list; at least as wide as it.
    Over(Id),
    /// At a point, as a context menu opens.
    Point([f32; 2]),
    /// Centred across the window near its top, over the interface, which dims, as a dialog
    /// opens.
    Dialog,
    /// Where a dialog opens, swinging in as one does but leaving the interface undimmed, as a
    /// command palette opens.
    Top,
}

impl Anchor {
    /// Where a popup `size` long on `axis` starts in a window `room` long: past `around`, the
    /// rectangle it opens beside, on the axis it opens along, level with it otherwise. Shown
    /// only `shown` long as it opens, it keeps the edge it would have at full size.
    fn place(self, around: [f32; 4], axis: usize, size: f32, shown: f32, room: f32) -> f32 {
        let size = size.min(fitting(room));
        let shown = shown.min(size);
        let most = (room - POPUP_MARGIN - size).max(POPUP_MARGIN);
        let along = match self {
            Anchor::Below(_) | Anchor::Point(_) => Some(1),
            Anchor::Right(_) => Some(0),
            Anchor::Over(_) => None,
            Anchor::Dialog | Anchor::Top if axis == 0 => {
                return ((room - size) / 2.0).max(POPUP_MARGIN);
            }
            Anchor::Dialog | Anchor::Top => return (room / 8.0).clamp(POPUP_MARGIN, most),
        };
        let [low, high] = [around[axis], around[axis + 2]];
        let (first, second) = if along == Some(axis) {
            (high, low - size)
        } else {
            (low, high - size)
        };
        if first + size <= room - POPUP_MARGIN {
            first.max(POPUP_MARGIN)
        } else if second >= POPUP_MARGIN {
            second.min(most) + size - shown
        } else {
            first.clamp(POPUP_MARGIN, most) + size - shown
        }
    }

    /// Seconds the popup takes to open and to close.
    fn durations(self) -> [f32; 2] {
        if matches!(self, Anchor::Dialog | Anchor::Top) {
            DIALOG
        } else {
            POPUP
        }
    }
}

/// A box and what it holds scaled by `zoom` about `pivot`, leaned back `tilt` radians about
/// the line through it, and faded; logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Motion {
    zoom: f32,
    pivot: [f32; 2],
    tilt: f32,
    opacity: f32,
}

impl Motion {
    const NONE: Motion = Motion {
        zoom: 1.0,
        pivot: [0.0; 2],
        tilt: 0.0,
        opacity: 1.0,
    };
}

/// What a box is this frame. Colours are linear RGBA.
#[derive(Clone, Default)]
pub struct Spec<'a> {
    pub flags: Flags,
    pub size: [Extent; 2],
    /// The axis its children flow along.
    pub axis: Axis,
    pub text: Option<&'a str>,
    /// The label's size in logical pixels; the theme's otherwise.
    pub font_size: Option<f32>,
    /// Shapes the label semibold.
    pub bold: bool,
    /// Shapes the label italic.
    pub italic: bool,
    /// The family the label is shaped in, where it has the glyphs; the interface's otherwise.
    pub font: Option<&'a str>,
    pub overflow: Overflow,
    /// The label's colour; the theme's text colour otherwise.
    pub color: Option<[f32; 4]>,
    pub fill: Option<[f32; 4]>,
    /// The fill's colour at the bottom, fading from `fill` at the top.
    pub gradient: Option<[f32; 4]>,
    /// A soft shadow of the box's outline, painted beneath it; a popup other than a dialog
    /// casts the theme's menu shadows instead.
    pub shadow: Option<[f32; 4]>,
    /// How far the box draws inside its leading, top, trailing and bottom edges: fill,
    /// border, shadow, icon and label; the pointer still finds the whole box.
    pub inset: [f32; 4],
    /// The fill under the pointer, blended in as hover animates.
    pub hover_fill: Option<[f32; 4]>,
    pub border: Option<[f32; 4]>,
    pub hover_border: Option<[f32; 4]>,
    pub radius: f32,
    pub shape: Shape,
    /// 16 px artwork before the label, tinted with `tint` or else the label's colour.
    pub icon: Option<&'static [&'static str]>,
    pub tint: Option<[f32; 4]>,
    /// A 16 px picture in the icon's place beside a label; without one, the picture fills
    /// the box inside its padding.
    pub image: Option<&'a RasterImage>,
    /// Inset of the label and children on each axis.
    pub pad: [f32; 2],
    /// Space between children along the flow.
    pub gap: f32,
    pub center: bool,
    /// For floating boxes, the offset from the parent's corner.
    pub position: [f32; 2],
    /// How far the box and its children draw and take the pointer from where they are laid
    /// out, the boxes around them keeping their places, as a row sliding aside does.
    pub offset: [f32; 2],
    pub cursor: Option<CursorIcon>,
    /// Floats the box over all others beside a rectangle in the window, as a popup
    /// `Ui::open_popup` opened; boxes beneath take no input while one is open.
    pub anchor: Option<Anchor>,
    /// Makes the box a group of a row in two forms, its first two children, both built every
    /// frame: its full form, and the one it folds to where the row lacks room. The row folds
    /// its groups, and those in rows of its own sized by their children, by ascending
    /// priority until it fits, once the space sized by ancestors has
    /// yielded and before any other box gives up room. The form not shown takes no room,
    /// paint or input, and its boxes take the group's rectangle, so what opens from them
    /// opens from the form shown.
    pub fold: Option<u32>,
    /// How far the children fade out into the box's fill towards its start and end along
    /// its axis, as a row or list of them cut there does.
    pub fade: [f32; 2],
    /// What the box is to assistive technology; without one, its text shows as a label and
    /// its children as its parent's. `Ui::access` adds the rest of what it says.
    pub role: Option<accesskit::Role>,
}

/// Input the host forwards; positions and wheel distances are logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    PointerMoved([f32; 2]),
    /// A pen's pressure from 0 to 1 for the pointer events that follow; none for a mouse.
    Pressure(Option<f32>),
    PointerLeft,
    Button {
        button: MouseButton,
        pressed: bool,
        at: Instant,
    },
    Wheel([f32; 2]),
    Key {
        key: Key,
        text: Option<String>,
    },
    Ime(Ime),
    Modifiers(ModifiersState),
    /// Assistive technology acting on a box of the tree `Ui::accessibility` built.
    Access(accesskit::ActionRequest),
}

/// How the user acted on a box this frame.
#[derive(Clone, Debug, Default)]
pub struct Signal {
    pub hovered: bool,
    /// The primary button went down on the box.
    pub pressed: bool,
    /// What the press selects text by: a single, double or triple click.
    pub unit: SelectionUnit,
    /// The primary button came up over the box it went down on.
    pub clicked: bool,
    /// The primary button is held after pressing the box.
    pub dragging: bool,
    /// The secondary button went down on the box, here: where its context menu opens.
    pub context: Option<[f32; 2]>,
    pub focused: bool,
    /// For custom boxes: every event routed to the box, in order. For focused boxes:
    /// keys, text and composition. For scrolling boxes: the wheel.
    pub events: Vec<Event>,
}

/// A frame's painting, in order: interface primitives, or a host-painted box.
pub enum Layer<'a> {
    Primitives(Primitives<'a>),
    Custom {
        id: Id,
        /// The box's visible logical rectangle.
        rect: [f32; 4],
    },
}

/// Interface primitives under a clip, in logical pixels, perhaps part of the way through
/// opening or closing.
pub struct Primitives<'a> {
    pub clip: Option<[f32; 4]>,
    /// A rounded rectangle and its corner radius the primitives paint only inside.
    round: Option<([f32; 4], f32)>,
    motion: Option<Motion>,
    pub primitives: Vec<Primitive<'a>>,
}

impl Primitives<'_> {
    /// The layer the renderer paints at `scale` device pixels per logical pixel.
    pub fn layer(&self, scale: f32) -> draw::Layer<'_> {
        let Motion {
            zoom,
            pivot,
            tilt,
            opacity,
        } = self.motion.unwrap_or(Motion::NONE);
        let device = |point: [f32; 2]| point.map(|value| value * scale);
        let moved = |point: [f32; 2]| {
            device(std::array::from_fn(|axis| {
                pivot[axis] + (point[axis] - pivot[axis]) * zoom
            }))
        };
        let moved_rect = |[left, top, right, bottom]: [f32; 4]| {
            let [[left, top], [right, bottom]] = [moved([left, top]), moved([right, bottom])];
            [left, top, right, bottom]
        };
        draw::Layer {
            scale: scale * zoom,
            origin: moved([0.0; 2]),
            clip: self.clip.map(moved_rect),
            backdrop: None,
            round: self
                .round
                .map(|(rect, radius)| (moved_rect(rect), radius * scale * zoom)),
            motion: self.motion.map(|_| draw::Motion {
                opacity,
                tilt,
                pivot: device(pivot),
            }),
            primitives: &self.primitives,
        }
    }
}

struct Built {
    id: Id,
    parent: usize,
    children: Vec<usize>,
    flags: Flags,
    size: [Extent; 2],
    axis: Axis,
    label: Option<Rc<Label>>,
    overflow: Overflow,
    color: [f32; 4],
    fill: Option<[f32; 4]>,
    gradient: Option<[f32; 4]>,
    shadow: Option<[f32; 4]>,
    inset: [f32; 4],
    hover_fill: Option<[f32; 4]>,
    border: Option<[f32; 4]>,
    hover_border: Option<[f32; 4]>,
    radius: f32,
    shape: Shape,
    icon: Option<&'static [&'static str]>,
    tint: [f32; 4],
    image: Option<RasterImage>,
    pad: [f32; 2],
    gap: f32,
    center: bool,
    position: [f32; 2],
    offset: [f32; 2],
    cursor: Option<CursorIcon>,
    anchor: Option<Anchor>,
    /// The rectangle a popup opens beside, as layout resolves its anchor.
    around: [f32; 4],
    /// How far open a popup over a box is, as it widens out of the box.
    open: f32,
    motion: Option<Motion>,
    /// Rectangles relative to the box, painted over its fill.
    marks: Vec<([f32; 4], [f32; 4], f32)>,
    fold: Option<u32>,
    fade: [f32; 2],
    access: Option<accesskit::Node>,
    /// Folded away with the form it belongs to: it takes no room, paint or input, and takes
    /// its group's rectangle.
    hidden: bool,
    computed: [f32; 2],
    relative: [f32; 2],
    rect: [f32; 4],
    content: f32,
}

#[derive(Default)]
struct State {
    touched: u64,
    hot: f32,
    /// The box paints differently under the pointer, so its hover is worth easing.
    hovers: bool,
    scroll: f32,
    scroll_target: f32,
    /// Height of a scrolling box's children last frame.
    content: f32,
    /// As last laid out; none for a box built this frame for the first time.
    rect: Option<[f32; 4]>,
    /// A text field's selection, and the selection and unit of the press a drag extends.
    selection: Selection,
    press: (Selection, SelectionUnit),
    /// The text a field selects when next built, from and to byte offsets clamped to its end.
    select: Option<[usize; 2]>,
    /// Where a dragged scrollbar thumb was taken, from its start.
    grab: f32,
    /// A system scroller's part held down, and when a held arrow or track next repeats.
    held: Option<(ScrollerPart, Instant)>,
    /// An animated value and its target, for `Ui::animate`.
    tween: Option<[f32; 2]>,
    /// When the condition `Ui::lasted` follows last became true.
    since: Option<Instant>,
}

/// An open popup, above the one opened before it.
struct Popup {
    id: Id,
    /// The focus before it opened, restored when it closes.
    focus: Option<Id>,
    /// Its filter field's text, and the key of the row the keyboard or pointer last chose.
    query: String,
    highlight: Option<u64>,
    /// A colour picker's hue in degrees, saturation and lightness, once it has one.
    picked: Option<[f32; 3]>,
    opened: Instant,
    /// As a submenu, the row of the menu below it that it opens beside, whatever its anchor.
    beside: Option<Id>,
    /// A menu's row whose submenu opens, and when; None once it has.
    submenu: Option<(u64, Option<Instant>)>,
}

/// The tooltip of the box under the pointer, the last frame it was built, and when it
/// shows: never while None, after a press or the wheel, until the pointer leaves the box.
#[derive(Clone, Copy)]
struct Tip {
    id: Id,
    frame: u64,
    due: Option<Instant>,
}

/// A popup painted: its anchor, the rectangle it opens beside, and its own.
#[derive(Clone, Copy)]
struct Placed {
    id: Id,
    anchor: Anchor,
    around: [f32; 4],
    rect: [f32; 4],
}

impl Placed {
    /// How the popup shows `open` of the way open: where it `grows`, it swings out of its
    /// anchor's edge as it grows and fades in, and a dialog swings up into place.
    fn motion(self, open: f32, grows: bool) -> Motion {
        let Placed {
            anchor,
            around,
            rect,
            ..
        } = self;
        let pivot = match anchor {
            Anchor::Below(_) | Anchor::Point(_) => [around[0], around[3]],
            Anchor::Right(_) => [around[2], around[1]],
            Anchor::Over(_) => [around[0], around[1]],
            Anchor::Dialog | Anchor::Top => [(rect[0] + rect[2]) / 2.0, rect[1]],
        };
        let (from, tilt) = match anchor {
            _ if !grows => (1.0, 0.0),
            Anchor::Dialog | Anchor::Top => (0.95, 0.2),
            // Over a box, the popup widens out of it in layout instead of growing.
            Anchor::Over(_) => (1.0, 0.2),
            _ => (0.94, 0.2),
        };
        let pivot = [
            pivot[0].clamp(rect[0], rect[2]),
            pivot[1].clamp(rect[1], rect[3]),
        ];
        // A popup flipped above its anchor swings out of its bottom edge, its top leaning away.
        let tilt = if pivot[1] > (rect[1] + rect[3]) / 2.0 {
            -tilt
        } else {
            tilt
        };
        Motion {
            zoom: from + (1.0 - from) * open,
            pivot,
            tilt: tilt * (1.0 - open),
            opacity: open,
        }
    }
}

/// A popup's painting as it last showed, fading out since it closed.
struct Closing {
    placed: Placed,
    display: Vec<Display>,
    closed: Instant,
}

struct Hit {
    id: Id,
    rect: [f32; 4],
    flags: Flags,
    cursor: Option<CursorIcon>,
}

#[derive(Clone)]
enum Display {
    Clip(Option<[f32; 4]>),
    /// A rounded rectangle and its corner radius, clipping what follows.
    Round(Option<([f32; 4], f32)>),
    Motion(Option<Motion>),
    Rect {
        rect: [f32; 4],
        fill: [f32; 4],
        /// The fill's colour at the bottom.
        shade: Option<[f32; 4]>,
        border: Option<[f32; 4]>,
        radius: f32,
    },
    /// A hairline 1 px wide.
    Segment {
        from: [f32; 2],
        to: [f32; 2],
        color: [f32; 4],
    },
    Image {
        image: RasterImage,
        rect: [f32; 4],
    },
    /// A shaped outline, filled or stroked.
    Path {
        data: String,
        origin: [f32; 2],
        style: PathStyle,
        /// Top and bottom.
        colors: [[f32; 4]; 2],
    },
    Shadow {
        rect: [f32; 4],
        radius: f32,
        blur: f32,
        color: [f32; 4],
    },
    Icon {
        sources: &'static [&'static str],
        origin: [f32; 2],
        tint: [f32; 4],
    },
    Text {
        label: Rc<Label>,
        origin: [f32; 2],
        clip: [f32; 4],
        color: [f32; 4],
    },
    Custom {
        id: Id,
        rect: [f32; 4],
    },
}

pub struct Ui {
    pub theme: Theme,
    /// The colours icons' slots take.
    pub icon_palette: draw::Palette,
    /// Whether the window has keyboard focus; without it a field hides its caret and dims
    /// its selection.
    pub window_focused: bool,
    /// The platform's scrollers, where it draws fixed ones beside the content; scrollbars
    /// overlay the content otherwise.
    pub scrollers: Option<Scrollers>,
    pub segments: Segments,
    frame: u64,
    now: Instant,
    scale: f32,
    nodes: Vec<Built>,
    stack: Vec<usize>,
    states: HashMap<Id, State>,
    texts: Texts,
    queue: Vec<Event>,
    signals: HashMap<Id, Signal>,
    /// The previous frame's interactive boxes in paint order.
    hits: Vec<Hit>,
    /// Hits before this one lie beneath an open popup.
    modal: usize,
    popups: Vec<Popup>,
    pointer: Option<[f32; 2]>,
    /// The pointer moved this frame.
    moved: bool,
    /// Seconds since the previous frame.
    dt: f32,
    lists: HashMap<Id, list::State>,
    hover: Option<Id>,
    active: Option<Id>,
    focus: Option<Id>,
    modifiers: ModifiersState,
    clicks: Clicks,
    display: Vec<Display>,
    /// The painting of what holds still in the popup being painted, which goes over it.
    still: Vec<Display>,
    /// Where the popups' painting starts in `display`, for edges drawn beneath them.
    popups_painted: usize,
    /// Each popup painted, and its painting's place after `popups_painted`.
    painted: Vec<(Placed, std::ops::Range<usize>)>,
    closing: Vec<Closing>,
    tip: Option<Tip>,
    /// When a tooltip last showed.
    warm: Option<Instant>,
    animating: bool,
    /// This frame routed input, whose effects the builder may only have seen after boxes
    /// built before it read their state; one more frame shows them.
    routed: bool,
    /// The field showing a caret, when its blink started and the frame it last showed.
    caret: Option<(Id, Instant, u64)>,
    /// When the next timed change is due, such as a caret's blink.
    wake: Option<Instant>,
    /// The controls the keyboard moves among, in the latest layout's order.
    stops: Vec<access::Stop>,
    /// The keyboard last moved the focus, so the focused control shows a ring.
    focus_ring: bool,
    /// Where Escape returns the focus from the controls the keyboard moved it among.
    resume: Option<Id>,
    /// The nodes last sent to assistive technology.
    sent: HashMap<Id, accesskit::Node>,
    /// Scrolling boxes to scroll to a box within each, once laid out.
    scrolls: Vec<(Id, Id)>,
}

impl Ui {
    /// `double_click` is the platform's double-click interval.
    pub fn new(theme: Theme, double_click: Duration) -> Self {
        Self {
            theme,
            icon_palette: draw::Palette::default(),
            window_focused: true,
            scrollers: None,
            segments: Segments::default(),
            frame: 0,
            now: Instant::now(),
            scale: 1.0,
            nodes: Vec::new(),
            stack: Vec::new(),
            states: HashMap::new(),
            texts: Texts::default(),
            queue: Vec::new(),
            signals: HashMap::new(),
            hits: Vec::new(),
            modal: 0,
            popups: Vec::new(),
            pointer: None,
            moved: false,
            dt: 0.0,
            lists: HashMap::new(),
            hover: None,
            active: None,
            focus: None,
            modifiers: ModifiersState::empty(),
            clicks: Clicks::new(double_click),
            display: Vec::new(),
            still: Vec::new(),
            popups_painted: 0,
            painted: Vec::new(),
            closing: Vec::new(),
            tip: None,
            warm: None,
            animating: false,
            routed: false,
            caret: None,
            wake: None,
            stops: Vec::new(),
            focus_ring: false,
            resume: None,
            sent: HashMap::new(),
            scrolls: Vec::new(),
        }
    }

    /// Queues input for the next frame.
    pub fn event(&mut self, event: Event) {
        // A chord the host looks up before the next frame reads the modifiers held now.
        if let Event::Modifiers(modifiers) = event {
            self.modifiers = modifiers;
        }
        self.queue.push(event);
    }

    /// Whether queued input or animation needs another frame.
    pub fn wants_frame(&self) -> bool {
        self.animating || self.routed || !self.queue.is_empty()
    }

    /// When a timed change, such as a caret's blink, next needs a frame.
    pub fn wake_at(&self) -> Option<Instant> {
        self.wake
    }

    /// Asks for a frame `after` this one, as a running clock does.
    pub fn wake_after(&mut self, after: Duration) {
        self.wake_by(self.now + after);
    }

    fn wake_by(&mut self, due: Instant) {
        self.wake = Some(self.wake.map_or(due, |wake| wake.min(due)));
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn modifiers(&self) -> ModifiersState {
        self.modifiers
    }

    /// Where the pointer is, in logical pixels, while it is over the window.
    pub fn pointer(&self) -> Option<[f32; 2]> {
        self.pointer
    }

    pub fn focused(&self) -> Option<Id> {
        self.focus
    }

    /// The focus where it is a text field, which the keyboard's edits act on.
    pub fn focused_field(&self) -> Option<Id> {
        self.focus.filter(|focus| {
            let flags = self.hit_flags(*focus);
            flags.contains(Flags::FOCUSABLE) && !flags.contains(Flags::CUSTOM)
        })
    }

    pub fn set_focus(&mut self, id: Option<Id>) {
        self.focus = id;
    }

    /// Focuses text field `id` with all its text selected once it is built, as a field
    /// opening to rename something does.
    pub fn focus_all(&mut self, id: Id) {
        let state = self.states.entry(id).or_default();
        state.touched = self.frame;
        state.select = Some([0, usize::MAX]);
        self.focus = Some(id);
    }

    /// The clickable or custom box at `point` in the latest layout.
    pub fn box_at(&self, point: [f32; 2]) -> Option<Id> {
        self.hit(point, Flags::CLICKABLE | Flags::CUSTOM)
    }

    /// The least width the box's children fit in, with its padding, as built this frame or
    /// last: each row's groups folded, and each box yielding all its strictness lets it.
    pub fn narrowest(&self, id: Id) -> Option<f32> {
        let index = self.nodes.iter().position(|node| node.id == id)?;
        Some(layout::narrowest(&self.nodes, index))
    }

    /// Where box `id` lies in the latest layout: this frame's once `end` has run, otherwise
    /// the previous frame's, which input is routed against; none for one not laid out yet.
    /// For hosts painting, hit-testing or mapping input after layout: a box placed beside
    /// another takes an `Anchor`, which layout resolves in the same frame.
    pub fn laid_out(&self, id: Id) -> Option<[f32; 4]> {
        self.states.get(&id).and_then(|state| state.rect)
    }

    /// The window's size in logical pixels this frame.
    pub fn size(&self) -> [f32; 2] {
        let root = self.nodes.first().map(|root| root.size);
        root.unwrap_or_default().map(|extent| match extent.size {
            Size::Pixels(pixels) => pixels,
            _ => 0.0,
        })
    }

    /// Scrolls `scroller`, a scrolling box, until `target` within it starts at its top, as
    /// this frame lays them out, easing there.
    pub fn scroll_to(&mut self, scroller: Id, target: Id) {
        self.scrolls.push((scroller, target));
    }

    /// The pointer's cursor, or None over a custom box, whose host chooses.
    pub fn cursor(&self) -> Option<CursorIcon> {
        let hit = self
            .hover
            .and_then(|id| self.hits.iter().find(|hit| hit.id == id));
        match hit {
            Some(hit) if hit.flags.contains(Flags::CUSTOM) => None,
            Some(hit) => Some(hit.cursor.unwrap_or_default()),
            None => Some(CursorIcon::Default),
        }
    }

    /// Starts a frame of `size` logical pixels drawn at `scale` device pixels each, and
    /// routes the queued input through the previous frame's layout.
    pub fn begin(&mut self, size: [f32; 2], scale: f32, now: Instant) {
        self.frame += 1;
        let dt = now
            .saturating_duration_since(self.now)
            .as_secs_f32()
            .min(0.1);
        self.now = now;
        self.scale = scale;
        self.nodes.clear();
        self.nodes.push(Built::new(
            Id::ROOT,
            0,
            Spec {
                size: [px(size[0]), px(size[1])],
                axis: Axis::Y,
                ..Spec::default()
            },
            None,
            self.theme.text,
        ));
        self.stack.clear();
        self.stack.push(0);
        self.signals.clear();
        self.moved = false;
        self.wake = None;
        self.routed = !self.queue.is_empty();
        for event in std::mem::take(&mut self.queue) {
            self.route(event);
        }
        // Boxes that move under a still pointer, as reordered tabs or a scrolled list, take
        // the hover from it.
        if !self.moved
            && self.active.is_none()
            && let Some(point) = self.pointer
        {
            let hover = self.hit(point, Flags::CLICKABLE | Flags::CUSTOM);
            if hover != self.hover {
                self.custom_event(self.hover, Event::PointerLeft);
                self.hover = hover;
            }
        }
        self.ease(dt);
    }

    fn route(&mut self, event: Event) {
        if matches!(event, Event::Button { pressed: true, .. } | Event::Wheel(_)) {
            if let Some(tip) = &mut self.tip {
                tip.due = None;
            }
            self.warm = None;
        }
        match event {
            Event::PointerMoved(point) => {
                self.pointer = Some(point);
                self.moved = true;
                let hover = self.hit(point, Flags::CLICKABLE | Flags::CUSTOM);
                if hover != self.hover
                    && self.active.is_none_or(|active| Some(active) != self.hover)
                {
                    self.custom_event(self.hover, Event::PointerLeft);
                }
                self.hover = hover;
                self.custom_event(self.active.or(hover), event);
            }
            Event::Pressure(_) => self.custom_event(self.active.or(self.hover), event),
            Event::PointerLeft => {
                self.pointer = None;
                if self.active.is_none() {
                    self.custom_event(self.hover, Event::PointerLeft);
                    self.hover = None;
                }
            }
            Event::Button {
                button,
                pressed: true,
                at,
            } => {
                let Some(point) = self.pointer else {
                    return;
                };
                self.focus_ring = false;
                if !self.popups.is_empty() {
                    let under = self.popups.iter().rposition(|popup| {
                        self.laid_out(popup.id)
                            .is_some_and(|rect| contains(rect, point))
                    });
                    // A press outside every popup only dismisses them.
                    let Some(under) = under else {
                        self.close_from(0);
                        return;
                    };
                    self.close_from(under + 1);
                }
                let target = self.hit(point, Flags::CLICKABLE | Flags::CUSTOM);
                if let Some(id) = target
                    && button == MouseButton::Right
                {
                    self.signals.entry(id).or_default().context = Some(point);
                }
                if let Some(id) = target
                    && button == MouseButton::Left
                {
                    self.active = Some(id);
                    let signal = self.signals.entry(id).or_default();
                    signal.pressed = true;
                    signal.unit = self.clicks.press(at, point, 4.0);
                    if self.hit_flags(id).contains(Flags::FOCUSABLE) {
                        self.focus = Some(id);
                    }
                }
                self.custom_event(target, event);
            }
            Event::Button { pressed: false, .. } => {
                let Some(active) = self.active else {
                    self.custom_event(self.hover, event);
                    return;
                };
                if self
                    .pointer
                    .and_then(|point| self.hit(point, Flags::CLICKABLE | Flags::CUSTOM))
                    == Some(active)
                {
                    self.signals.entry(active).or_default().clicked = true;
                }
                self.custom_event(Some(active), event);
                self.active = None;
                self.hover = self
                    .pointer
                    .and_then(|point| self.hit(point, Flags::CLICKABLE | Flags::CUSTOM));
                if self.hover != Some(active) {
                    self.custom_event(Some(active), Event::PointerLeft);
                }
            }
            Event::Wheel(delta) => {
                let Some(point) = self.pointer else {
                    return;
                };
                let target = self.hit(point, Flags::SCROLL | Flags::CUSTOM);
                match target {
                    Some(id) if self.hit_flags(id).contains(Flags::CUSTOM) => {
                        self.custom_event(Some(id), event)
                    }
                    Some(id) => {
                        let state = self.states.entry(id).or_default();
                        let shown = state.rect.map_or(0.0, |rect| rect[3] - rect[1]);
                        let most = (state.content - shown).max(0.0);
                        // The wheel moves the content at once, as on the page; only jumps ease.
                        state.scroll = (state.scroll - delta[1]).clamp(0.0, most);
                        state.scroll_target = state.scroll;
                        self.signals.entry(id).or_default().events.push(event);
                    }
                    None => {}
                }
            }
            Event::Key {
                key: Key::Named(NamedKey::Escape),
                ..
            } if !self.popups.is_empty() => self.close_from(self.popups.len() - 1),
            Event::Key {
                key: Key::Named(key),
                ..
            } if self.traverse(key) => {}
            Event::Access(request) => self.act(request),
            Event::Key { .. } | Event::Ime(_) => {
                if let Some(focus) = self.focus {
                    self.signals.entry(focus).or_default().events.push(event);
                }
            }
            Event::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                let custom: Vec<_> = self
                    .hits
                    .iter()
                    .filter(|hit| hit.flags.contains(Flags::CUSTOM))
                    .map(|hit| hit.id)
                    .collect();
                for id in custom {
                    self.signals
                        .entry(id)
                        .or_default()
                        .events
                        .push(event.clone());
                }
            }
        }
    }

    fn custom_event(&mut self, target: Option<Id>, event: Event) {
        if let Some(id) = target
            && self.hit_flags(id).contains(Flags::CUSTOM)
        {
            self.signals.entry(id).or_default().events.push(event);
        }
    }

    fn hit_flags(&self, id: Id) -> Flags {
        self.hits
            .iter()
            .find(|hit| hit.id == id)
            .map_or(Flags::default(), |hit| hit.flags)
    }

    /// The topmost box under `point` with any of `flags`.
    fn hit(&self, point: [f32; 2], flags: Flags) -> Option<Id> {
        self.hits[self.modal..]
            .iter()
            .rev()
            .find(|hit| hit.flags.intersects(flags) && contains(hit.rect, point))
            .map(|hit| hit.id)
    }

    /// Opens popup `id`, which shows while built with its `Spec::anchor` each frame, and
    /// takes the keyboard. Popups open from within another stay above it; any other
    /// closes those open.
    pub fn open_popup(&mut self, id: Id) {
        if self.popup_open(id) {
            return;
        }
        let within = self
            .stack
            .iter()
            .filter_map(|index| {
                let id = self.nodes[*index].id;
                self.popups.iter().position(|popup| popup.id == id)
            })
            .max();
        self.push_popup(id, within.map_or(0, |within| within + 1), None);
    }

    /// Opens popup `id` as the submenu of open popup `from`, beside its row `row`, in place of
    /// any other opened from it.
    pub fn open_submenu(&mut self, id: Id, from: Id, row: Id) {
        let Some(from) = self.popups.iter().position(|popup| popup.id == from) else {
            return;
        };
        if self.popups.get(from + 1).is_none_or(|open| open.id != id) {
            self.push_popup(id, from + 1, Some(row));
        }
    }

    /// Closes the popups from `at` on and opens `id` above the rest.
    fn push_popup(&mut self, id: Id, at: usize, beside: Option<Id>) {
        self.close_from(at);
        self.popups.push(Popup {
            id,
            focus: self.focus,
            query: String::new(),
            highlight: None,
            picked: None,
            opened: self.now,
            beside,
            submenu: None,
        });
        self.focus = Some(id);
        self.states.entry(id).or_default().touched = self.frame;
    }

    pub fn popup_open(&self, id: Id) -> bool {
        self.popups.iter().any(|popup| popup.id == id)
    }

    /// Whether any menu, dialog or other popup is open.
    pub fn popups_open(&self) -> bool {
        !self.popups.is_empty()
    }

    /// Closes popup `id` and those opened from it, returning the focus it took; a submenu
    /// closes the menus it opened from with it, as choosing from it ends them.
    pub fn close_popup(&mut self, id: Id) {
        if let Some(mut index) = self.popups.iter().position(|popup| popup.id == id) {
            while index > 0 && self.popups[index].beside.is_some() {
                index -= 1;
            }
            self.close_from(index);
        }
    }

    fn close_from(&mut self, index: usize) {
        if let Some(popup) = self.popups.get(index) {
            self.focus = popup.focus;
            self.popups.truncate(index);
        }
    }

    fn ease(&mut self, dt: f32) {
        self.dt = dt;
        let rate = 1.0 - 0.5_f32.powf(dt / HALF_LIFE);
        let mut animating = false;
        for (id, state) in &mut self.states {
            let hot = self.hover == Some(*id) && self.active.is_none_or(|active| active == *id);
            if !state.hovers {
                state.hot = f32::from(u8::from(hot));
            }
            let targets = [
                (&mut state.hot, f32::from(u8::from(hot)), 0.002),
                (&mut state.scroll, state.scroll_target, 0.25),
            ];
            let tween = state
                .tween
                .as_mut()
                .map(|[value, target]| (value, *target, 0.005));
            for (value, target, close) in targets.into_iter().chain(tween) {
                let delta = target - *value;
                if delta.abs() < close {
                    *value = target;
                } else {
                    *value += delta * rate;
                    animating = true;
                }
            }
        }
        self.animating = animating;
    }

    /// The box being built, which new boxes become children of.
    pub fn current(&self) -> Id {
        self.nodes[*self.stack.last().unwrap()].id
    }

    /// The id a box built next under the current parent would take.
    pub fn id(&self, part: impl Hash) -> Id {
        self.current().child(part)
    }

    /// Adds a box and makes it the parent of the boxes built until `close`.
    pub fn open(&mut self, part: impl Hash, spec: Spec<'_>) -> Id {
        self.open_as(self.id(part), spec)
    }

    /// Opens a box under an id chosen outside the hierarchy, so the host can name it
    /// wherever it is built.
    pub fn open_as(&mut self, id: Id, spec: Spec<'_>) -> Id {
        let label = spec.text.map(|text| {
            let size = spec.font_size.unwrap_or(self.theme.font_size);
            self.texts
                .styled(text, size, spec.bold, spec.italic, spec.font, self.frame)
        });
        // Popups hang from the root, outside the clips and flow of where they are built.
        let parent = if spec.anchor.is_some() {
            0
        } else {
            *self.stack.last().unwrap()
        };
        let open = match spec.anchor {
            Some(anchor @ Anchor::Over(_)) if self.popup_motion(anchor) == PopupMotion::Grow => {
                self.opening(id, anchor).unwrap_or(1.0)
            }
            _ => 1.0,
        };
        let index = self.nodes.len();
        self.states.entry(id).or_default().touched = self.frame;
        self.nodes.push(Built {
            open,
            ..Built::new(id, parent, spec, label, self.theme.text)
        });
        self.nodes[parent].children.push(index);
        self.stack.push(index);
        id
    }

    pub fn close(&mut self) {
        debug_assert!(self.stack.len() > 1, "close without open");
        self.stack.pop();
    }

    /// Adds a box without children and reports how it was used.
    pub fn leaf(&mut self, part: impl Hash, spec: Spec<'_>) -> Signal {
        let id = self.open(part, spec);
        self.close();
        self.signal(id)
    }

    /// Holds animated value `id` at `value` without easing, as a dragged box follows the
    /// pointer; it eases from there once animated again.
    pub fn hold(&mut self, id: Id, value: f32) -> f32 {
        let state = self.states.entry(id).or_default();
        state.touched = self.frame;
        state.tween = Some([value; 2]);
        value
    }

    /// Eases towards `target` over the next frames, starting there the first time `id` asks.
    pub fn animate(&mut self, id: Id, target: f32) -> f32 {
        let state = self.states.entry(id).or_default();
        state.touched = self.frame;
        let [value, goal] = state.tween.get_or_insert([target; 2]);
        *goal = target;
        self.animating |= value != goal;
        *value
    }

    /// How long `held` has stayed true through the frames that ask of `id`; zero while
    /// false, as a status waits before showing a state that may pass at once.
    pub fn lasted(&mut self, id: Id, held: bool) -> Duration {
        let now = self.now;
        let state = self.states.entry(id).or_default();
        state.touched = self.frame;
        state.since = held.then(|| state.since.unwrap_or(now));
        state
            .since
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    /// Makes `family` the interface's font, where fontique doesn't know the system's, as
    /// with Mac OS X 10.6's Lucida Grande.
    pub fn set_system_font(&mut self, family: &str) {
        self.texts.set_system_font(family);
    }

    /// Previews `family` in the font `data` holds, as a font menu shows the substitute a
    /// missing family lays out in.
    pub fn preview_font(&mut self, data: parley::fontique::Blob<u8>, family: &str) {
        self.texts.preview_font(data, family);
    }

    /// Draws what the interface's fonts and their scripts' fallbacks lack with the first of
    /// `families` that has it; see `draw::fall_back_to`.
    pub fn fall_back_to(&mut self, families: &[String]) {
        self.texts.fall_back_to(families);
    }

    /// Whether the interface's fonts draw every glyph of `text`.
    pub fn shows(&mut self, text: &str) -> bool {
        self.texts.shows(text, self.frame)
    }

    /// The size of `text` as a label, in logical pixels.
    pub fn measure(&mut self, text: &str) -> [f32; 2] {
        self.texts
            .label(text, self.theme.font_size, self.frame)
            .size
    }

    /// Paints `color` over the current box at `rect`, relative to its corner, with corners
    /// of `radius`.
    pub fn mark(&mut self, rect: [f32; 4], color: [f32; 4], radius: f32) {
        let index = *self.stack.last().unwrap();
        self.nodes[index].marks.push((rect, color, radius));
    }

    /// The caret opacity of field `id` this frame, restarting its blink when `moved` or
    /// when it was not shown the frame before.
    pub(crate) fn blink(&mut self, id: Id, moved: bool) -> f32 {
        let start = match self.caret {
            Some((shown, start, frame)) if shown == id && frame + 1 == self.frame && !moved => {
                start
            }
            _ => self.now,
        };
        self.caret = Some((id, start, self.frame));
        let (opacity, hold) = draw::edit::caret_blink(self.now.saturating_duration_since(start));
        if let Some(hold) = hold {
            self.wake_after(hold);
        }
        opacity
    }

    /// How the user acted on the box this frame; a box's routed events are taken once.
    pub fn signal(&mut self, id: Id) -> Signal {
        let mut signal = self.signals.remove(&id).unwrap_or_default();
        signal.hovered = self.hover == Some(id);
        signal.dragging = self.active == Some(id);
        signal.focused = self.focus == Some(id);
        signal
    }

    /// Solves this frame's layout and prepares its painting.
    pub fn end(&mut self) {
        debug_assert_eq!(self.stack.len(), 1, "open without close");
        layout::solve(
            &mut self.nodes,
            &self.states,
            self.scale,
            &mut self.texts,
            self.frame,
        );
        for node in &self.nodes {
            let state = self.states.entry(node.id).or_default();
            state.rect = Some(node.rect);
            state.hovers = node.hover_fill.is_some() || node.hover_border.is_some();
            if node.flags.contains(Flags::SCROLL) {
                state.content = node.content;
                let most = (node.content - (node.rect[3] - node.rect[1])).max(0.0);
                state.scroll_target = state.scroll_target.clamp(0.0, most);
                state.scroll = state.scroll.clamp(0.0, most);
            }
        }
        for (scroller, target) in std::mem::take(&mut self.scrolls) {
            let (Some(top), Some(start)) = (self.laid_out(target), self.laid_out(scroller)) else {
                continue;
            };
            let state = self.states.entry(scroller).or_default();
            let most = (state.content - (start[3] - start[1])).max(0.0);
            state.scroll_target = (state.scroll + top[1] - start[1]).clamp(0.0, most);
            self.animating |= state.scroll != state.scroll_target;
        }
        self.states
            .retain(|id, state| state.touched == self.frame || *id == Id::ROOT);
        self.texts.prune(self.frame);
        self.lists.retain(|id, _| self.states.contains_key(id));
        if let Some(gone) = self
            .popups
            .iter()
            .position(|popup| !self.states.contains_key(&popup.id))
        {
            self.close_from(gone);
        }
        if let Some(tip) = self.tip.filter(|tip| tip.frame != self.frame) {
            if tip.due.is_some_and(|due| due <= self.now) {
                self.warm = Some(self.now);
            }
            self.tip = None;
        }
        let tip = self.tip.map(|tip| tip.id.child("tooltip"));
        let last = std::mem::take(&mut self.display);
        let last_popups = self.popups_painted;
        self.hits.clear();
        self.paint(0, None, None);
        self.popups_painted = self.display.len();
        let beneath = self.hits.len();
        let mut painted = Vec::new();
        for index in self.nodes[0].children.clone() {
            let node = &self.nodes[index];
            let Some(anchor) = node.anchor else {
                continue;
            };
            let placed = Placed {
                id: node.id,
                anchor,
                around: node.around,
                rect: node.rect,
            };
            if Some(placed.id) == tip {
                continue;
            }
            if let Some(open) = self.opening(placed.id, anchor) {
                self.scrim(anchor, open);
                let grows = self.popup_motion(anchor) == PopupMotion::Grow;
                self.nodes[index].motion = Some(placed.motion(open, grows));
            }
            let from = self.display.len() - self.popups_painted;
            self.paint(index, None, None);
            if !self.still.is_empty() {
                let still = std::mem::take(&mut self.still);
                self.display.extend(still);
                self.display.extend([
                    Display::Motion(None),
                    Display::Clip(None),
                    Display::Round(None),
                ]);
            }
            painted.push((placed, from..self.display.len() - self.popups_painted));
        }
        for (placed, range) in std::mem::replace(&mut self.painted, painted) {
            if self.painted.iter().any(|(shown, _)| shown.id == placed.id) {
                continue;
            }
            // What held still while it opened goes at once, uncovering what it stood in for.
            let mut still = false;
            let display = last[last_popups + range.start..last_popups + range.end]
                .iter()
                .filter(|item| match item {
                    Display::Motion(motion) => {
                        still = motion.is_none();
                        false
                    }
                    _ => !still,
                })
                .cloned()
                .collect();
            self.closing.push(Closing {
                placed,
                display,
                closed: self.now,
            });
        }
        for closing in std::mem::take(&mut self.closing) {
            let Placed { id, anchor, .. } = closing.placed;
            let motion = self.popup_motion(anchor);
            let open = match motion {
                PopupMotion::Grow => {
                    (1.0 - self.progress(closing.closed, anchor.durations()[1])).powi(3)
                }
                PopupMotion::Cut => 0.0,
                PopupMotion::Fade([_, close]) => {
                    (1.0 - self.progress(closing.closed, close)).powi(4)
                }
            };
            if open == 0.0 {
                continue;
            }
            self.scrim(anchor, open);
            let grows = motion == PopupMotion::Grow;
            let motion = closing.placed.motion(open, grows);
            self.display.push(Display::Motion(Some(motion)));
            self.display.extend(closing.display.iter().cloned());
            self.display.push(Display::Motion(None));
            if !self.painted.iter().any(|(shown, _)| shown.id == id) {
                self.closing.push(closing);
            }
        }
        // Over everything, closing popups included; it goes at once, without closing.
        if let Some(index) = self.nodes[0]
            .children
            .iter()
            .copied()
            .find(|index| Some(self.nodes[*index].id) == tip)
            && let Some(due) = self.tip.and_then(|tip| tip.due)
        {
            let opacity = self.progress(due, TIP_FADE);
            self.nodes[index].motion = Some(Motion {
                opacity,
                ..Motion::NONE
            });
            self.paint(index, None, None);
        }
        self.modal = if self.popups.is_empty() { 0 } else { beneath };
        for id in [
            &mut self.hover,
            &mut self.active,
            &mut self.focus,
            &mut self.resume,
        ] {
            if id.is_some_and(|id| !self.states.contains_key(&id)) {
                *id = None;
            }
        }
        self.stops = self.stops();
    }

    /// How far open popup `id` beside `anchor` shows this frame, from 0 to 1, while open.
    pub fn opening(&mut self, id: Id, anchor: Anchor) -> Option<f32> {
        let opened = self.popups.iter().find(|popup| popup.id == id)?.opened;
        Some(match self.popup_motion(anchor) {
            PopupMotion::Grow => 1.0 - (1.0 - self.progress(opened, anchor.durations()[0])).powi(3),
            PopupMotion::Cut => 1.0,
            PopupMotion::Fade([open, _]) => self.progress(opened, open),
        })
    }

    /// How popups beside `anchor` open and close; dialogs and the palette always swing as
    /// Snowbound's own.
    fn popup_motion(&self, anchor: Anchor) -> PopupMotion {
        if matches!(anchor, Anchor::Dialog | Anchor::Top) {
            PopupMotion::Grow
        } else {
            self.theme.menu().motion
        }
    }

    /// How far through an animation `duration` seconds long that began at `start` this
    /// frame is, asking for frames until it ends.
    fn progress(&mut self, start: Instant, duration: f32) -> f32 {
        let progress =
            (self.now.saturating_duration_since(start).as_secs_f32() / duration).min(1.0);
        self.animating |= progress < 1.0;
        progress
    }

    /// Dims the window beneath a dialog `open` of the way open.
    fn scrim(&mut self, anchor: Anchor, open: f32) {
        if anchor == Anchor::Dialog {
            self.display.push(Display::Rect {
                rect: self.nodes[0].rect,
                fill: [0.0, 0.0, 0.0, self.theme.shadow[3] * open],
                shade: None,
                border: None,
                radius: 0.0,
            });
        }
    }

    fn paint(&mut self, index: usize, clip: Option<[f32; 4]>, motion: Option<Motion>) {
        // Painted over the popup it holds still in, which appears as one picture beneath.
        if self.nodes[index].flags.contains(Flags::STILL) && motion.is_some() {
            let from = self.display.len();
            self.paint_box(index, clip, motion);
            let still = self.display.split_off(from);
            self.still.extend(still);
        } else {
            self.paint_box(index, clip, motion);
        }
    }

    fn paint_box(&mut self, index: usize, clip: Option<[f32; 4]>, motion: Option<Motion>) {
        let node = &self.nodes[index];
        // The motion the box and its children paint with, where it changes it.
        let own = if node.flags.contains(Flags::STILL) {
            Some(None)
        } else {
            node.motion.map(Some)
        };
        if let Some(own) = own {
            self.display.push(Display::Motion(own));
        }
        let state = &self.states[&node.id];
        let rect = node.rect;
        let visible = clip.map_or(Some(rect), |clip| intersect(rect, clip));
        let blend = |base: Option<[f32; 4]>, hover: Option<[f32; 4]>| match (base, hover) {
            (base, Some(hover)) => Some(mix(base.unwrap_or_default(), hover, state.hot)),
            (base, None) => base,
        };
        let fill = blend(node.fill, node.hover_fill);
        // The gradient keeps its difference from the fill as hover moves it.
        let shade = node.gradient.map(|shade| {
            let base = node.fill.unwrap_or(shade);
            std::array::from_fn(|channel| {
                shade[channel] + fill.unwrap_or(base)[channel] - base[channel]
            })
        });
        let border = blend(node.border, node.hover_border);
        let [left, top, right, bottom] = node.inset;
        // A box squeezed smaller than its inset, or than nothing as it eases shut, paints
        // empty rather than inverted.
        let [x, y] = [rect[0] + left, rect[1] + top];
        let painted = [x, y, (rect[2] - right).max(x), (rect[3] - bottom).max(y)];
        let size = [painted[2] - painted[0], painted[3] - painted[1]];
        if let Some(color) = node.shadow {
            let (own, menu);
            let shadows: &[Shadow] = match node.anchor {
                Some(Anchor::Dialog) | None => {
                    let [blur, drop] = if node.anchor.is_some() {
                        POPUP_SHADOW
                    } else {
                        SHADOW
                    };
                    own = [Shadow {
                        color,
                        blur,
                        drop,
                        spread: 0.0,
                    }];
                    &own
                }
                Some(_) => {
                    menu = self.theme.menu();
                    &menu.shadows
                }
            };
            for &Shadow {
                color,
                blur,
                drop,
                spread,
            } in shadows
            {
                let origin = [painted[0] - spread, painted[1] + drop - spread];
                let grown = size.map(|side| side + 2.0 * spread);
                self.display.push(match node.shape {
                    Shape::Rounded => Display::Shadow {
                        rect: [
                            origin[0],
                            origin[1],
                            origin[0] + grown[0],
                            origin[1] + grown[1],
                        ],
                        radius: node.radius + spread,
                        blur,
                        color,
                    },
                    Shape::Tab { .. } => Display::Path {
                        data: outline(node.shape, grown, node.radius + spread),
                        origin,
                        style: PathStyle::Shadow(blur),
                        colors: [color; 2],
                    },
                });
            }
        }
        if node.shape == Shape::Rounded {
            if fill.is_some() || border.is_some() {
                self.display.push(Display::Rect {
                    rect: painted,
                    fill: fill.unwrap_or([0.0; 4]),
                    shade,
                    border,
                    radius: node.radius,
                });
            }
        } else {
            let data = outline(node.shape, size, node.radius);
            let paints = [
                fill.map(|fill| (PathStyle::Fill, [fill, shade.unwrap_or(fill)])),
                border.map(|border| (PathStyle::Stroke(1.0), [border; 2])),
            ];
            for (style, colors) in paints.into_iter().flatten() {
                self.display.push(Display::Path {
                    data: data.clone(),
                    origin: [painted[0], painted[1]],
                    style,
                    colors,
                });
            }
        }
        for (mark, color, radius) in &node.marks {
            self.display.push(Display::Rect {
                rect: [
                    rect[0] + mark[0],
                    rect[1] + mark[1],
                    rect[0] + mark[2],
                    rect[1] + mark[3],
                ],
                fill: *color,
                shade: None,
                border: None,
                radius: *radius,
            });
        }
        // A box squeezed narrower than its padding keeps an empty inside, not an inverted one.
        let left = (painted[0] + node.pad[0]).min(painted[2]);
        let inner = [
            left,
            painted[1],
            (painted[2] - node.pad[0]).max(left),
            painted[3],
        ];
        let mut x = if node.center {
            (inner[0] + inner[2] - node.content_width()) / 2.0
        } else {
            inner[0]
        };
        let top = (painted[1] + painted[3] - ICON) / 2.0;
        if let Some(sources) = node.icon {
            self.display.push(Display::Icon {
                sources,
                origin: [x, top],
                tint: node.tint,
            });
            x += ICON + ICON_GAP;
        } else if let Some(image) = &node.image {
            let rect = if node.label.is_some() {
                [x, top, x + ICON, top + ICON]
            } else {
                let top = (painted[1] + node.pad[1]).min(painted[3]);
                [inner[0], top, inner[2], (painted[3] - node.pad[1]).max(top)]
            };
            self.display.push(Display::Image {
                image: image.clone(),
                rect,
            });
            x += ICON + ICON_GAP;
        }
        if let Some(label) = &node.label {
            let y = (painted[1] + painted[3] - label.size[1]) / 2.0;
            self.display.push(Display::Text {
                label: label.clone(),
                origin: [x, y],
                clip: inner,
                color: node.color,
            });
        }
        if let Some(visible) = visible
            && node
                .flags
                .intersects(Flags::CLICKABLE | Flags::CUSTOM | Flags::SCROLL)
        {
            self.hits.push(Hit {
                id: node.id,
                rect: visible,
                flags: node.flags,
                cursor: node.cursor,
            });
            if node.flags.contains(Flags::CUSTOM) {
                self.display.push(Display::Custom {
                    id: node.id,
                    rect: visible,
                });
            }
        }
        // A popup's contents show only inside its rounded outline.
        let round = node.anchor.map(|_| (rect, node.radius));
        let inner_clip = if node.flags.contains(Flags::CLIP) || round.is_some() {
            Some(visible.unwrap_or([rect[0], rect[1], rect[0], rect[1]]))
        } else {
            clip
        };
        if inner_clip != clip {
            self.display.push(Display::Clip(inner_clip));
        }
        if round.is_some() {
            self.display.push(Display::Round(round));
        }
        for child in node.children.clone() {
            if self.nodes[child].anchor.is_none() && !self.nodes[child].hidden {
                self.paint(child, inner_clip, own.unwrap_or(motion));
            }
        }
        if round.is_some() {
            self.display.push(Display::Round(None));
        }
        self.fade(index);
        if inner_clip != clip {
            self.display.push(Display::Clip(clip));
        }
        let node = &self.nodes[index];
        if self.focus_ring
            && self.focus == Some(node.id)
            && node.access.is_some()
            && node.anchor.is_none()
            && !node.flags.contains(Flags::CUSTOM)
        {
            let size = size.map(|side| side + 2.0 * FOCUS_RING);
            self.display.push(Display::Path {
                data: outline(Shape::Rounded, size, node.radius + FOCUS_RING),
                origin: [painted[0] - FOCUS_RING, painted[1] - FOCUS_RING],
                style: PathStyle::Stroke(FOCUS_RING),
                colors: [self.theme.accent; 2],
            });
        }
        if own.is_some() {
            self.display.push(Display::Motion(motion));
        }
    }

    /// Fades box `index`'s children out towards its ends along its axis by its `fade`, in
    /// steps of a point, into its fill however opaque that is: each step clears what lies
    /// beneath towards transparency and lays the fill over it.
    fn fade(&mut self, index: usize) {
        let node = &self.nodes[index];
        let along = usize::from(node.axis == Axis::Y);
        let rect = node.rect;
        let fill = node.fill.unwrap_or_default();
        let mut steps = Vec::new();
        for (side, width) in node.fade.into_iter().enumerate() {
            let count = width.round() as usize;
            for step in 0..count {
                // Through the step's middle, eased so the fade leaves the children softly.
                let alpha = 1.0 - (step as f32 + 0.5) / count as f32;
                let alpha = alpha * alpha * (3.0 - 2.0 * alpha);
                let at = if side == 0 {
                    rect[along] + step as f32
                } else {
                    rect[along + 2] - 1.0 - step as f32
                };
                let mut band = rect;
                [band[along], band[along + 2]] = [at, at + 1.0];
                steps.push((band, alpha));
            }
        }
        let [width, height] = [rect[2] - rect[0], rect[3] - rect[1]];
        let data = if along == 0 {
            format!("M0 0H1V{height}H0Z")
        } else {
            format!("M0 0H{width}V1H0Z")
        };
        for (rect, alpha) in steps {
            // Laid over what the erasing leaves, the fill's share makes up the rest of the
            // children's lost opacity: all of it where the fill is opaque, none where clear.
            let laid = alpha * fill[3];
            if laid < 1.0 {
                self.display.push(Display::Path {
                    data: data.clone(),
                    origin: [rect[0], rect[1]],
                    style: PathStyle::Erase,
                    colors: [[0.0, 0.0, 0.0, 1.0 - (1.0 - alpha) / (1.0 - laid)]; 2],
                });
            }
            self.display.push(Display::Rect {
                rect,
                fill: [fill[0], fill[1], fill[2], laid],
                shade: None,
                border: None,
                radius: 0.0,
            });
        }
    }

    /// This frame's painting in order, in logical pixels.
    pub fn layers(&self) -> Vec<Layer<'_>> {
        let mut layers = Vec::new();
        let mut clip = None;
        let mut round = None;
        let mut motion = None;
        let mut primitives = Vec::new();
        fn flush<'a>(
            layers: &mut Vec<Layer<'a>>,
            clip: Option<[f32; 4]>,
            round: Option<([f32; 4], f32)>,
            motion: Option<Motion>,
            primitives: &mut Vec<Primitive<'a>>,
        ) {
            if !primitives.is_empty() {
                layers.push(Layer::Primitives(Primitives {
                    clip,
                    round,
                    motion,
                    primitives: std::mem::take(primitives),
                }));
            }
        }
        for item in &self.display {
            match item {
                Display::Clip(next) => {
                    flush(&mut layers, clip, round, motion, &mut primitives);
                    clip = *next;
                }
                Display::Round(next) => {
                    flush(&mut layers, clip, round, motion, &mut primitives);
                    round = *next;
                }
                Display::Motion(next) => {
                    flush(&mut layers, clip, round, motion, &mut primitives);
                    motion = *next;
                }
                Display::Rect {
                    rect,
                    fill,
                    shade,
                    border,
                    radius,
                } => {
                    if let Some(shade) = shade {
                        primitives.push(Primitive::Gradient {
                            rect: *rect,
                            radius: [*radius; 2],
                            colors: [*fill, *shade],
                        });
                    } else if fill[3] > 0.0 {
                        primitives.push(if *radius > 0.0 {
                            Primitive::RoundedRect {
                                rect: *rect,
                                radius: [*radius; 2],
                                stroke: None,
                                color: *fill,
                            }
                        } else {
                            Primitive::Rect {
                                rect: *rect,
                                color: *fill,
                            }
                        });
                    }
                    if let Some(border) = border.filter(|border| border[3] > 0.0) {
                        primitives.push(Primitive::RoundedRect {
                            rect: *rect,
                            radius: [*radius; 2],
                            stroke: Some(Stroke::Solid(1.0 / self.scale)),
                            color: border,
                        });
                    }
                }
                Display::Path {
                    data,
                    origin,
                    style,
                    colors,
                } => primitives.push(Primitive::Path {
                    data,
                    origin: *origin,
                    style: *style,
                    colors: *colors,
                }),
                &Display::Shadow {
                    rect,
                    radius,
                    blur,
                    color,
                } => primitives.push(Primitive::Shadow {
                    rect,
                    radius,
                    blur,
                    color,
                }),
                Display::Segment { from, to, color } => primitives.push(Primitive::Segment {
                    from: *from,
                    to: *to,
                    width: 1.0,
                    round: false,
                    color: *color,
                }),
                Display::Image { image, rect } => {
                    primitives.push(Primitive::Image { image, rect: *rect })
                }
                Display::Icon {
                    sources,
                    origin,
                    tint,
                } => primitives.push(Primitive::Icon {
                    sources,
                    origin: *origin,
                    size: ICON,
                    tint: *tint,
                    palette: self.icon_palette,
                }),
                Display::Text {
                    label,
                    origin,
                    clip,
                    color,
                } => primitives.push(Primitive::Text {
                    text: &**label,
                    origin: *origin,
                    clip: Some(*clip),
                    ink: *color,
                }),
                Display::Custom { id, rect } => {
                    flush(&mut layers, clip, round, motion, &mut primitives);
                    layers.push(Layer::Custom {
                        id: *id,
                        rect: *rect,
                    });
                }
            }
        }
        flush(&mut layers, clip, round, motion, &mut primitives);
        layers
    }

    pub(crate) fn state(&mut self, id: Id) -> &mut State {
        self.states.entry(id).or_default()
    }
}

impl Built {
    /// The width of the icon and label together.
    fn content_width(&self) -> f32 {
        let label = self.label.as_ref().map_or(0.0, |label| label.size[0]);
        match (self.icon.is_some() || self.image.is_some(), label > 0.0) {
            (true, true) => ICON + ICON_GAP + label,
            (true, false) => ICON,
            (false, _) => label,
        }
    }

    fn new(
        id: Id,
        parent: usize,
        spec: Spec<'_>,
        label: Option<Rc<Label>>,
        text: [f32; 4],
    ) -> Self {
        Self {
            id,
            parent,
            children: Vec::new(),
            flags: spec.flags,
            size: spec.size,
            axis: spec.axis,
            label,
            overflow: spec.overflow,
            color: spec.color.unwrap_or(text),
            fill: spec.fill,
            gradient: spec.gradient,
            shadow: spec.shadow,
            inset: spec.inset,
            hover_fill: spec.hover_fill,
            border: spec.border,
            hover_border: spec.hover_border,
            radius: spec.radius,
            shape: spec.shape,
            icon: spec.icon,
            tint: spec.tint.or(spec.color).unwrap_or(text),
            image: spec.image.cloned(),
            pad: spec.pad,
            gap: spec.gap,
            center: spec.center,
            position: spec.position,
            offset: spec.offset,
            cursor: spec.cursor,
            anchor: spec.anchor,
            around: [0.0; 4],
            open: 1.0,
            motion: None,
            marks: Vec::new(),
            fold: spec.fold,
            fade: spec.fade,
            access: spec.role.map(accesskit::Node::new),
            hidden: false,
            computed: [0.0; 2],
            relative: [0.0; 2],
            rect: [0.0; 4],
            content: 0.0,
        }
    }
}

/// SVG path data outlining a box of `size` from its corner, rounding corners by `radius`.
/// A tab's outline stays open along its bottom, so a border leaves the edge it stands on.
fn outline(shape: Shape, [width, height]: [f32; 2], radius: f32) -> String {
    let corners = match shape {
        Shape::Tab { lean } => [
            ([0.0, height], 0.0),
            ([0.0, 0.0], radius),
            ([width - lean, 0.0], radius / 2.0),
            ([width - lean + height, height], 0.0),
        ],
        Shape::Rounded => [
            ([0.0, height], radius),
            ([0.0, 0.0], radius),
            ([width, 0.0], radius),
            ([width, height], radius),
        ],
    };
    let mut data = String::new();
    for index in 0..corners.len() {
        let corner = Corner::new(&corners, index);
        let verb = if index == 0 { 'M' } else { 'L' };
        data += &format!(
            "{verb}{} {}{}",
            corner.start[0],
            corner.start[1],
            corner.curve([0.0; 2])
        );
    }
    if !matches!(shape, Shape::Tab { .. }) {
        data.push('Z');
    }
    data
}

/// A polygon's corner rounded by a quarter-circle's cubic, from where it leaves the edge
/// before to where it joins the edge after.
struct Corner {
    point: [f32; 2],
    start: [f32; 2],
    end: [f32; 2],
    controls: [[f32; 2]; 2],
    /// It turns clockwise, so it bulges out of a shape listed clockwise.
    outward: bool,
}

impl Corner {
    /// The corner at `points[index]`, each point with its rounding radius.
    fn new(points: &[([f32; 2], f32)], index: usize) -> Self {
        let count = points.len();
        let (point, radius) = points[index];
        let [before, after] =
            [(index + count - 1) % count, (index + 1) % count].map(|index| points[index].0);
        let direction = |from: [f32; 2], to: [f32; 2]| {
            let length = (to[0] - from[0]).hypot(to[1] - from[1]).max(f32::EPSILON);
            [(to[0] - from[0]) / length, (to[1] - from[1]) / length]
        };
        let [incoming, outgoing] = [direction(before, point), direction(point, after)];
        let along = |from: [f32; 2], direction: [f32; 2], distance: f32| {
            [
                from[0] + direction[0] * distance,
                from[1] + direction[1] * distance,
            ]
        };
        let start = along(point, incoming, -radius);
        let end = along(point, outgoing, radius);
        // A quarter circle's cubic control distance.
        let handle = radius * 0.552_284_8;
        Self {
            point,
            start,
            end,
            controls: [
                along(start, incoming, handle),
                along(end, outgoing, -handle),
            ],
            outward: incoming[0] * outgoing[1] - incoming[1] * outgoing[0] > 0.0,
        }
    }

    /// The cubic from `start` to `end`, as path data relative to `origin`.
    fn curve(&self, origin: [f32; 2]) -> String {
        let [a, b, c] = [self.controls[0], self.controls[1], self.end]
            .map(|point| [point[0] - origin[0], point[1] - origin[1]]);
        format!("C{} {} {} {} {} {}", a[0], a[1], b[0], b[1], c[0], c[1])
    }
}

/// Where a box dragged from index `from` of a list laid out at `spans`, each a start and a
/// length along it, lands with its middle at `middle`: after every other box whose middle it
/// has passed, as browser tabs reorder. The index is in the list's new order.
pub fn drop_slot(spans: &[[f32; 2]], from: usize, middle: f32) -> usize {
    spans
        .iter()
        .enumerate()
        .filter(|(index, [start, length])| *index != from && start + length / 2.0 < middle)
        .count()
}

/// How far box `index` of a list slides aside while the box at `from`, `length` long, is
/// dragged to land at `to`.
pub fn slide(index: usize, from: usize, to: usize, length: f32) -> f32 {
    if from < index && index <= to {
        -length
    } else if to <= index && index < from {
        length
    } else {
        0.0
    }
}

/// `a` moved `t` of the way to `b`, mixed as premultiplied colours, so from transparent
/// `b` fades in with its own hue.
pub fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let alpha = a[3] + (b[3] - a[3]) * t;
    if alpha == 0.0 {
        return [0.0; 4];
    }
    let channel = |i: usize| (a[i] * a[3] + (b[i] * b[3] - a[i] * a[3]) * t) / alpha;
    [channel(0), channel(1), channel(2), alpha]
}

fn contains(rect: [f32; 4], point: [f32; 2]) -> bool {
    point[0] >= rect[0] && point[0] < rect[2] && point[1] >= rect[1] && point[1] < rect[3]
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let rect = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (rect[0] < rect[2] && rect[1] < rect[3]).then_some(rect)
}

#[cfg(test)]
mod tests;
