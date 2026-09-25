//! An immediate-mode interface kit. Builder code declares boxes every frame from
//! application state; a cache keyed by stable ids keeps hover, press, focus, scroll and
//! animation state. Input is answered with the previous frame's layout, so a frame's
//! events are routed before building and its layout is solved after.

mod layout;
mod text;
mod theme;
mod widgets;

pub use theme::Theme;
pub use widgets::{button, scrollbar, text_field};

use draw::{Primitive, Stroke};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::BitOr,
    rc::Rc,
    time::Instant,
};
use text::{Label, Painted, Texts};
use winit::{
    event::{Ime, MouseButton},
    keyboard::{Key, ModifiersState},
    window::CursorIcon,
};

/// Seconds for an animated value to close half of its remaining distance.
const HALF_LIFE: f32 = 0.03;

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
    /// The fill under the pointer, blended in as hover animates.
    pub hover_fill: Option<[f32; 4]>,
    pub border: Option<[f32; 4]>,
    pub hover_border: Option<[f32; 4]>,
    pub radius: f32,
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
    hover_fill: Option<[f32; 4]>,
    border: Option<[f32; 4]>,
    hover_border: Option<[f32; 4]>,
    radius: f32,
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
    active: f32,
    scroll: f32,
    scroll_target: f32,
    /// Height of a scrolling box's children last frame.
    content: f32,
    rect: [f32; 4],
    /// A text field's caret and selection anchor, in bytes.
    caret: usize,
    mark: usize,
    /// Where a dragged scrollbar thumb was taken, from its start.
    grab: f32,
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
        border: Option<[f32; 4]>,
        radius: f32,
    },
    Text {
        painted: usize,
        origin: [f32; 2],
        clip: [f32; 4],
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
    display: Vec<Display>,
    painted: Vec<Painted>,
    animating: bool,
}

impl Ui {
    pub fn new(theme: Theme) -> Self {
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
            display: Vec::new(),
            painted: Vec::new(),
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
        self.animate(dt);
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
                ..
            } => {
                let Some(point) = self.pointer else {
                    return;
                };
                let target = self.hit(point, Flags::CLICKABLE | Flags::CUSTOM);
                if let Some(id) = target
                    && button == MouseButton::Left
                {
                    self.active = Some(id);
                    self.signals.entry(id).or_default().pressed = true;
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

    fn animate(&mut self, dt: f32) {
        let rate = 1.0 - 0.5_f32.powf(dt / HALF_LIFE);
        let mut animating = false;
        for (id, state) in &mut self.states {
            let hot = self.hover == Some(*id) && self.active.is_none_or(|active| active == *id);
            let targets = [
                (&mut state.hot, f32::from(u8::from(hot)), 0.002),
                (
                    &mut state.active,
                    f32::from(u8::from(self.active == Some(*id))),
                    0.002,
                ),
                (&mut state.scroll, state.scroll_target, 0.25),
            ];
            for (value, target, close) in targets {
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
        self.painted.clear();
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
        let blend = |base: Option<[f32; 4]>, hover: Option<[f32; 4]>| match (base, hover) {
            (base, Some(hover)) => Some(mix(base.unwrap_or([0.0; 4]), hover, state.hot)),
            (base, None) => base,
        };
        let fill = blend(node.fill, node.hover_fill);
        let border = blend(node.border, node.hover_border);
        if fill.is_some() || border.is_some() {
            self.display.push(Display::Rect {
                rect,
                fill: fill.unwrap_or([0.0; 4]),
                border,
                radius: node.radius,
            });
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
                border: None,
                radius: 0.0,
            });
        }
        if let Some(label) = &node.label {
            let inner = [
                rect[0] + node.pad[0],
                rect[1],
                rect[2] - node.pad[0],
                rect[3],
            ];
            let x = if node.center {
                (inner[0] + inner[2] - label.size[0]) / 2.0
            } else {
                inner[0]
            };
            let y = (rect[1] + rect[3] - label.size[1]) / 2.0;
            self.display.push(Display::Text {
                painted: self.painted.len(),
                origin: [x, y],
                clip: inner,
            });
            self.painted.push(Painted {
                label: label.clone(),
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
                    border,
                    radius,
                } => {
                    if fill[3] > 0.0 {
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
                Display::Text {
                    painted,
                    origin,
                    clip,
                } => primitives.push(Primitive::Text {
                    text: &self.painted[*painted],
                    origin: *origin,
                    clip: Some(*clip),
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

    pub(crate) fn caret(&mut self, id: Id) -> (&mut usize, &mut usize) {
        let state = self.states.entry(id).or_default();
        (&mut state.caret, &mut state.mark)
    }

    pub(crate) fn grab(&mut self, id: Id) -> &mut f32 {
        &mut self.states.entry(id).or_default().grab
    }
}

impl Built {
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
            hover_fill: spec.hover_fill,
            border: spec.border,
            hover_border: spec.hover_border,
            radius: spec.radius,
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

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
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
