//! An immediate-mode interface kit. Builder code declares boxes every frame from
//! application state; a cache keyed by stable ids keeps hover, press, focus, scroll and
//! animation state. Input is answered with the previous frame's layout, so a frame's
//! events are routed before building and its layout is solved after.

mod layout;
pub mod shell;
mod text;
mod theme;
mod widgets;

pub use theme::{Section, Shades, Theme};
pub use widgets::{button, edit_key, edit_modifiers, scrollbar, text_field};

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
    time::{Duration, Instant},
};
use text::{Label, Texts};
use winit::{
    event::{Ime, MouseButton},
    keyboard::{Key, ModifiersState},
    window::CursorIcon,
};

/// Seconds for an animated value to close half of its remaining distance.
const HALF_LIFE: f32 = 0.03;
/// How far a box's shadow spreads, in logical pixels.
const SHADOW: f32 = 3.0;
/// Logical size of a box's icon, and its distance from the label.
const ICON: f32 = 16.0;
const ICON_GAP: f32 = 6.0;

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
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    /// Logical pixels.
    Pixels(f32),
    /// The label and padding.
    Text,
    /// A fraction of the nearest ancestor not sized by its children.
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
    /// A section tab: the top leading corner rounded and the trailing edge leaning out
    /// `slant` pixels from top to bottom, centred on the box's edge so neighbours overlap.
    Tab { slant: f32 },
    /// Only the trailing corners rounded, so the leading edge joins what it sits against.
    Trailing,
}

/// What a box is this frame. Colours are linear RGBA.
#[derive(Clone, Default)]
pub struct Spec<'a> {
    pub flags: Flags,
    pub size: [Extent; 2],
    /// The axis its children flow along.
    pub axis: Axis,
    pub text: Option<&'a str>,
    /// The label's colour; the theme's text colour otherwise.
    pub color: Option<[f32; 4]>,
    pub fill: Option<[f32; 4]>,
    /// The fill's colour at the bottom, fading from `fill` at the top.
    pub gradient: Option<[f32; 4]>,
    /// A soft shadow of the box's outline, painted beneath it.
    pub shadow: Option<[f32; 4]>,
    /// The fill under the pointer, blended in as hover animates.
    pub hover_fill: Option<[f32; 4]>,
    pub border: Option<[f32; 4]>,
    pub hover_border: Option<[f32; 4]>,
    pub radius: f32,
    pub shape: Shape,
    /// 16 px artwork before the label, tinted with the label's colour.
    pub icon: Option<&'static [&'static str]>,
    /// A 16 px picture in the icon's place.
    pub image: Option<&'a RasterImage>,
    /// Inset of the label and children on each axis.
    pub pad: [f32; 2],
    /// Space between children along the flow.
    pub gap: f32,
    pub center: bool,
    /// For floating boxes, the offset from the parent's corner.
    pub position: [f32; 2],
    pub cursor: Option<CursorIcon>,
}

/// Input the host forwards; positions and wheel distances are logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    PointerMoved([f32; 2]),
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
    pub focused: bool,
    /// For custom boxes: every event routed to the box, in order. For focused boxes:
    /// keys, text and composition.
    pub events: Vec<Event>,
}

/// A frame's painting, in order: interface primitives under a clip, or a host-painted box.
pub enum Layer<'a> {
    Primitives {
        /// Logical pixels.
        clip: Option<[f32; 4]>,
        primitives: Vec<Primitive<'a>>,
    },
    Custom {
        id: Id,
        /// The box's visible logical rectangle.
        rect: [f32; 4],
    },
}

struct Built {
    id: Id,
    parent: usize,
    children: Vec<usize>,
    flags: Flags,
    size: [Extent; 2],
    axis: Axis,
    label: Option<Rc<Label>>,
    color: [f32; 4],
    fill: Option<[f32; 4]>,
    gradient: Option<[f32; 4]>,
    shadow: Option<[f32; 4]>,
    hover_fill: Option<[f32; 4]>,
    border: Option<[f32; 4]>,
    hover_border: Option<[f32; 4]>,
    radius: f32,
    shape: Shape,
    icon: Option<&'static [&'static str]>,
    image: Option<RasterImage>,
    pad: [f32; 2],
    gap: f32,
    center: bool,
    position: [f32; 2],
    cursor: Option<CursorIcon>,
    /// Rectangles relative to the box, painted over its fill.
    marks: Vec<([f32; 4], [f32; 4])>,
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
    rect: [f32; 4],
    /// A text field's selection, and the selection and unit of the press a drag extends.
    selection: Selection,
    press: (Selection, SelectionUnit),
    /// Where a dragged scrollbar thumb was taken, from its start.
    grab: f32,
    /// An animated value and its target, for `Ui::animate`.
    tween: Option<[f32; 2]>,
}

struct Hit {
    id: Id,
    rect: [f32; 4],
    flags: Flags,
    cursor: Option<CursorIcon>,
}

enum Display {
    Clip(Option<[f32; 4]>),
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
    pointer: Option<[f32; 2]>,
    hover: Option<Id>,
    active: Option<Id>,
    focus: Option<Id>,
    modifiers: ModifiersState,
    clicks: Clicks,
    display: Vec<Display>,
    animating: bool,
}

impl Ui {
    /// `double_click` is the platform's double-click interval.
    pub fn new(theme: Theme, double_click: Duration) -> Self {
        Self {
            theme,
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
            pointer: None,
            hover: None,
            active: None,
            focus: None,
            modifiers: ModifiersState::empty(),
            clicks: Clicks::new(double_click),
            display: Vec::new(),
            animating: false,
        }
    }

    /// Queues input for the next frame.
    pub fn event(&mut self, event: Event) {
        self.queue.push(event);
    }

    /// Whether queued input or animation needs another frame.
    pub fn wants_frame(&self) -> bool {
        self.animating || !self.queue.is_empty()
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub(crate) fn modifiers(&self) -> ModifiersState {
        self.modifiers
    }

    pub(crate) fn pointer(&self) -> Option<[f32; 2]> {
        self.pointer
    }

    pub fn focused(&self) -> Option<Id> {
        self.focus
    }

    pub fn set_focus(&mut self, id: Option<Id>) {
        self.focus = id;
    }

    /// The clickable or custom box at `point` in the latest layout.
    pub fn box_at(&self, point: [f32; 2]) -> Option<Id> {
        self.hit(point, Flags::CLICKABLE | Flags::CUSTOM)
    }

    /// The box's rectangle from the latest layout.
    pub fn rect(&self, id: Id) -> Option<[f32; 4]> {
        self.states.get(&id).map(|state| state.rect)
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
        for event in std::mem::take(&mut self.queue) {
            self.route(event);
        }
        self.ease(dt);
    }

    fn route(&mut self, event: Event) {
        match event {
            Event::PointerMoved(point) => {
                self.pointer = Some(point);
                let hover = self.hit(point, Flags::CLICKABLE | Flags::CUSTOM);
                if hover != self.hover
                    && self.active.is_none_or(|active| Some(active) != self.hover)
                {
                    self.custom_event(self.hover, Event::PointerLeft);
                }
                self.hover = hover;
                self.custom_event(self.active.or(hover), event);
            }
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
                let target = self.hit(point, Flags::CLICKABLE | Flags::CUSTOM);
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
                        let most = (state.content - (state.rect[3] - state.rect[1])).max(0.0);
                        state.scroll_target = (state.scroll_target - delta[1]).clamp(0.0, most);
                    }
                    None => {}
                }
            }
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
        self.hits
            .iter()
            .rev()
            .find(|hit| {
                hit.flags.intersects(flags)
                    && point[0] >= hit.rect[0]
                    && point[0] < hit.rect[2]
                    && point[1] >= hit.rect[1]
                    && point[1] < hit.rect[3]
            })
            .map(|hit| hit.id)
    }

    fn ease(&mut self, dt: f32) {
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
    pub(crate) fn current(&self) -> Id {
        self.nodes[*self.stack.last().unwrap()].id
    }

    /// The id a box built next under the current parent would take.
    pub fn id(&self, part: impl Hash) -> Id {
        self.nodes[*self.stack.last().unwrap()].id.child(part)
    }

    /// Adds a box and makes it the parent of the boxes built until `close`.
    pub fn open(&mut self, part: impl Hash, spec: Spec<'_>) -> Id {
        self.open_as(self.id(part), spec)
    }

    /// Opens a box under an id chosen outside the hierarchy, so the host can name it
    /// wherever it is built.
    pub fn open_as(&mut self, id: Id, spec: Spec<'_>) -> Id {
        let label = spec
            .text
            .map(|text| self.texts.label(text, self.theme.font_size, self.frame));
        let parent = *self.stack.last().unwrap();
        let index = self.nodes.len();
        self.nodes
            .push(Built::new(id, parent, spec, label, self.theme.text));
        self.nodes[parent].children.push(index);
        self.stack.push(index);
        self.states.entry(id).or_default().touched = self.frame;
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

    /// Eases towards `target` over the next frames, starting there the first time `id` asks.
    pub fn animate(&mut self, id: Id, target: f32) -> f32 {
        let state = self.states.entry(id).or_default();
        state.touched = self.frame;
        let [value, goal] = state.tween.get_or_insert([target; 2]);
        *goal = target;
        self.animating |= value != goal;
        *value
    }

    /// Shapes labels in the family the font `files` define, in place of the system's
    /// interface font. Returns the family's name, or None when the files hold no font.
    pub fn use_fonts(&mut self, files: impl IntoIterator<Item = Vec<u8>>) -> Option<String> {
        self.texts.use_fonts(files)
    }

    /// The size of `text` as a label, in logical pixels.
    pub fn measure(&mut self, text: &str) -> [f32; 2] {
        self.texts
            .label(text, self.theme.font_size, self.frame)
            .size
    }

    /// Paints `color` over the current box at `rect`, relative to its corner.
    pub fn mark(&mut self, rect: [f32; 4], color: [f32; 4]) {
        let index = *self.stack.last().unwrap();
        self.nodes[index].marks.push((rect, color));
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
        layout::solve(&mut self.nodes, &self.states, self.scale);
        for node in &self.nodes {
            let state = self.states.entry(node.id).or_default();
            state.rect = node.rect;
            state.hovers = node.hover_fill.is_some() || node.hover_border.is_some();
            if node.flags.contains(Flags::SCROLL) {
                state.content = node.content;
                let most = (node.content - (node.rect[3] - node.rect[1])).max(0.0);
                state.scroll_target = state.scroll_target.clamp(0.0, most);
                state.scroll = state.scroll.clamp(0.0, most);
            }
        }
        self.states
            .retain(|id, state| state.touched == self.frame || *id == Id::ROOT);
        self.texts.prune(self.frame);
        self.display.clear();
        self.hits.clear();
        self.paint(0, None);
        for id in [&mut self.hover, &mut self.active, &mut self.focus] {
            if id.is_some_and(|id| !self.states.contains_key(&id)) {
                *id = None;
            }
        }
    }

    fn paint(&mut self, index: usize, clip: Option<[f32; 4]>) {
        let node = &self.nodes[index];
        let state = &self.states[&node.id];
        let rect = node.rect;
        let visible = clip.map_or(Some(rect), |clip| intersect(rect, clip));
        // Without a colour of its own, a box fades the hover colour in rather than
        // blending from transparent black.
        let blend = |base: Option<[f32; 4]>, hover: Option<[f32; 4]>| match (base, hover) {
            (base, Some(hover)) => Some(mix(
                base.unwrap_or([hover[0], hover[1], hover[2], 0.0]),
                hover,
                state.hot,
            )),
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
        let size = [rect[2] - rect[0], rect[3] - rect[1]];
        if let Some(shadow) = node.shadow {
            self.display.push(Display::Path {
                data: outline(node.shape, size, node.radius),
                origin: [rect[0], rect[1]],
                style: PathStyle::Shadow(SHADOW),
                colors: [shadow; 2],
            });
        }
        if node.shape == Shape::Rounded {
            if fill.is_some() || border.is_some() {
                self.display.push(Display::Rect {
                    rect,
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
                    origin: [rect[0], rect[1]],
                    style,
                    colors,
                });
            }
        }
        for (mark, color) in &node.marks {
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
                radius: 0.0,
            });
        }
        let inner = [
            rect[0] + node.pad[0],
            rect[1],
            rect[2] - node.pad[0],
            rect[3],
        ];
        let mut x = if node.center {
            (inner[0] + inner[2] - node.content_width()) / 2.0
        } else {
            inner[0]
        };
        let top = (rect[1] + rect[3] - ICON) / 2.0;
        if let Some(sources) = node.icon {
            self.display.push(Display::Icon {
                sources,
                origin: [x, top],
                tint: node.color,
            });
            x += ICON + ICON_GAP;
        } else if let Some(image) = &node.image {
            self.display.push(Display::Image {
                image: image.clone(),
                rect: [x, top, x + ICON, top + ICON],
            });
            x += ICON + ICON_GAP;
        }
        if let Some(label) = &node.label {
            let y = (rect[1] + rect[3] - label.size[1]) / 2.0;
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
        let inner_clip = if node.flags.contains(Flags::CLIP) {
            Some(visible.unwrap_or([rect[0], rect[1], rect[0], rect[1]]))
        } else {
            clip
        };
        if inner_clip != clip {
            self.display.push(Display::Clip(inner_clip));
        }
        for child in node.children.clone() {
            self.paint(child, inner_clip);
        }
        if inner_clip != clip {
            self.display.push(Display::Clip(clip));
        }
    }

    /// This frame's painting in order, in logical pixels.
    pub fn layers(&self) -> Vec<Layer<'_>> {
        let mut layers = Vec::new();
        let mut clip = None;
        let mut primitives = Vec::new();
        for item in &self.display {
            match item {
                Display::Clip(next) => {
                    if !primitives.is_empty() {
                        layers.push(Layer::Primitives {
                            clip,
                            primitives: std::mem::take(&mut primitives),
                        });
                    }
                    clip = *next;
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
                    if !primitives.is_empty() {
                        layers.push(Layer::Primitives {
                            clip,
                            primitives: std::mem::take(&mut primitives),
                        });
                    }
                    layers.push(Layer::Custom {
                        id: *id,
                        rect: *rect,
                    });
                }
            }
        }
        if !primitives.is_empty() {
            layers.push(Layer::Primitives { clip, primitives });
        }
        layers
    }

    pub(crate) fn texts(&mut self) -> (&mut Texts, u64) {
        (&mut self.texts, self.frame)
    }

    pub(crate) fn field(&mut self, id: Id) -> (&mut Selection, &mut (Selection, SelectionUnit)) {
        let state = self.states.entry(id).or_default();
        (&mut state.selection, &mut state.press)
    }

    pub(crate) fn grab(&mut self, id: Id) -> &mut f32 {
        &mut self.states.entry(id).or_default().grab
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
            color: spec.color.unwrap_or(text),
            fill: spec.fill,
            gradient: spec.gradient,
            shadow: spec.shadow,
            hover_fill: spec.hover_fill,
            border: spec.border,
            hover_border: spec.hover_border,
            radius: spec.radius,
            shape: spec.shape,
            icon: spec.icon,
            image: spec.image.cloned(),
            pad: spec.pad,
            gap: spec.gap,
            center: spec.center,
            position: spec.position,
            cursor: spec.cursor,
            marks: Vec::new(),
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
        Shape::Tab { slant } => [
            ([0.0, height], 0.0),
            ([0.0, 0.0], radius),
            ([width - slant / 2.0, 0.0], radius / 2.0),
            ([width + slant / 2.0, height], 0.0),
        ],
        Shape::Trailing => [
            ([0.0, height], 0.0),
            ([0.0, 0.0], 0.0),
            ([width, 0.0], radius),
            ([width, height], radius),
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

/// `a` moved `t` of the way to `b`.
pub fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
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
