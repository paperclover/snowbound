//! The interface as assistive technology sees it: an AccessKit tree of the latest frame's
//! boxes that have a role, and the keyboard's way among its controls, as `arc/ui.md`
//! describes them.

use crate::{Axis, Event, Flags, Id, Ui};
use accesskit::{
    Action, ActionData, ActionRequest, Node, NodeId, Orientation, Rect, Role, TreeId, TreeInfo,
    TreeUpdate,
};
use std::collections::{HashMap, HashSet};
use winit::{event::Ime, keyboard::NamedKey};

/// A control the keyboard can focus.
pub(crate) struct Stop {
    id: Id,
    rect: [f32; 4],
    /// The toolbar or tab list it moves within by arrows, with its role and their axis.
    group: Option<(Id, Role, Axis)>,
    popup: Option<Id>,
    selected: bool,
    /// Takes keys of its own: a text field, or a box the host handles.
    typing: bool,
    custom: bool,
}

impl Stop {
    /// What Tab steps over as one: its group, or itself.
    fn step(&self) -> Id {
        self.group.map_or(self.id, |(group, ..)| group)
    }
}

/// Roles that hold controls rather than being one, and whose text is their own.
pub(crate) fn holds(role: Role) -> bool {
    matches!(
        role,
        Role::GenericContainer
            | Role::Window
            | Role::Group
            | Role::Toolbar
            | Role::Dialog
            | Role::AlertDialog
            | Role::Menu
            | Role::MenuBar
            | Role::ListBox
            | Role::List
            | Role::TabList
            | Role::Tree
            | Role::Grid
            | Role::Pane
            | Role::ScrollView
            | Role::RadioGroup
            | Role::Form
            | Role::Region
    )
}

/// Roles whose text is their value rather than their name.
fn valued(role: Role) -> bool {
    matches!(
        role,
        Role::TextInput
            | Role::SearchInput
            | Role::MultilineTextInput
            | Role::ComboBox
            | Role::EditableComboBox
            | Role::SpinButton
    )
}

/// The axis a group's arrows move along, where it is one Tab steps over whole.
fn composite(node: &Node) -> Option<Axis> {
    let along = match node.role() {
        Role::Toolbar | Role::TabList => Axis::X,
        Role::Tree | Role::RadioGroup => Axis::Y,
        _ => return None,
    };
    Some(match node.orientation() {
        Some(Orientation::Horizontal) => Axis::X,
        Some(Orientation::Vertical) => Axis::Y,
        None => along,
    })
}

/// Lists and menus move their own selection by the keys of the box that owns them.
fn navigates_itself(role: Role) -> bool {
    matches!(role, Role::Menu | Role::ListBox)
}

/// Whether a box shows: not folded away, nor a clip shut to nothing, as a closed panel is.
fn shown(built: &crate::Built) -> bool {
    let [left, top, right, bottom] = built.rect;
    !built.hidden && !(built.flags.contains(Flags::CLIP) && (left >= right || top >= bottom))
}

fn bounds(rect: [f32; 4], scale: f64) -> Rect {
    let [x0, y0, x1, y1] = rect.map(|side| f64::from(side) * scale);
    Rect::new(x0, y0, x1, y1)
}

impl Ui {
    /// Box `id` as built this frame, as assistive technology sees it, a generic container
    /// where it has no `Spec::role`. Bounds, children and the actions its flags allow are
    /// filled in; where a control has no label, its text and its children's name it.
    pub fn access(&mut self, id: Id) -> Option<&mut Node> {
        let index = self.nodes.iter().rposition(|node| node.id == id)?;
        Some(
            self.nodes[index]
                .access
                .get_or_insert_with(|| Node::new(Role::GenericContainer)),
        )
    }

    /// The changes to the interface's tree since the last update, with the window as its root
    /// named `label`; `scale` is the window's pixels per logical pixel. Call after `end`.
    pub fn accessibility(&mut self, label: &str, scale: f64) -> TreeUpdate {
        let (nodes, focus) = self.tree_nodes(label, scale);
        let fresh = self.sent.is_empty();
        let mut changed = Vec::new();
        let mut sent = HashMap::with_capacity(nodes.len());
        for (id, node) in nodes {
            if self.sent.get(&id) != Some(&node) {
                changed.push((id.node(), node.clone()));
            }
            sent.insert(id, node);
        }
        self.sent = sent;
        TreeUpdate {
            nodes: changed,
            tree: fresh.then(|| TreeInfo::new(Id::ROOT.node())),
            tree_id: TreeId::ROOT,
            focus: focus.node(),
        }
    }

    /// The interface's whole tree, as `accessibility` first sends it, leaving what it last
    /// sent alone.
    pub fn accessibility_tree(&self, label: &str, scale: f64) -> TreeUpdate {
        let (nodes, focus) = self.tree_nodes(label, scale);
        TreeUpdate {
            nodes: nodes
                .into_iter()
                .map(|(id, node)| (id.node(), node))
                .collect(),
            tree: Some(TreeInfo::new(Id::ROOT.node())),
            tree_id: TreeId::ROOT,
            focus: focus.node(),
        }
    }

    /// Every node of the tree, and the focused one.
    fn tree_nodes(&self, label: &str, scale: f64) -> (Vec<(Id, Node)>, Id) {
        let mut nodes = Vec::new();
        let mut children = Vec::new();
        let mut seen = HashSet::from([Id::ROOT]);
        self.describe(0, scale, None, &mut children, &mut nodes, &mut seen);
        let mut root = Node::new(Role::Window);
        root.set_label(label);
        root.set_bounds(bounds(self.nodes[0].rect, scale));
        root.set_children(children.into_iter().map(|(id, _)| id).collect::<Vec<_>>());
        nodes.push((Id::ROOT, root));
        let focus = self.focus.filter(|focus| seen.contains(focus));
        (nodes, focus.unwrap_or(Id::ROOT))
    }

    /// Forgets what was sent, so the next update sends the whole tree.
    pub fn deactivate_accessibility(&mut self) {
        self.sent.clear();
    }

    /// Adds the nodes of box `index`'s children to `out` and their ids and rectangles to
    /// `children`. Inside a control, text without a role goes to `texts` to name it.
    fn describe(
        &self,
        index: usize,
        scale: f64,
        mut texts: Option<&mut Vec<String>>,
        children: &mut Vec<(NodeId, [f32; 4])>,
        out: &mut Vec<(Id, Node)>,
        seen: &mut HashSet<Id>,
    ) {
        let tooltip = self.tip.map(|tip| tip.id.child("tooltip"));
        for &child in &self.nodes[index].children {
            let built = &self.nodes[child];
            // A box built twice in a frame would give its node two parents.
            if !shown(built) || Some(built.id) == tooltip || !seen.insert(built.id) {
                continue;
            }
            let text = built
                .label
                .as_ref()
                .map(|label| label.key.0.as_str())
                .filter(|text| !text.is_empty());
            let Some(access) = &built.access else {
                match (text, texts.as_deref_mut()) {
                    (Some(text), Some(texts)) => texts.push(text.to_owned()),
                    (Some(text), None) => {
                        let mut node = Node::new(Role::Label);
                        node.set_value(text);
                        node.set_bounds(bounds(built.rect, scale));
                        children.push((built.id.node(), built.rect));
                        out.push((built.id, node));
                    }
                    (None, _) => {}
                }
                self.describe(child, scale, texts.as_deref_mut(), children, out, seen);
                continue;
            };
            let mut node = access.clone();
            let role = node.role();
            node.set_bounds(bounds(built.rect, scale));
            children.push((built.id.node(), built.rect));
            if node.tree_id().is_some() {
                // A graft holds only the tree it names, so what is built inside it shows
                // beside it.
                out.push((built.id, node));
                self.describe(child, scale, texts.as_deref_mut(), children, out, seen);
                continue;
            }
            let control = !holds(role);
            let mut own = Vec::new();
            let mut inner = Vec::new();
            self.describe(
                child,
                scale,
                control.then_some(&mut own),
                &mut inner,
                out,
                seen,
            );
            if node.label().is_none() && !valued(role) {
                let name: Vec<_> = text
                    .map(str::to_owned)
                    .into_iter()
                    .chain(own)
                    .collect::<Vec<_>>();
                if !name.is_empty() {
                    node.set_label(name.join(" "));
                }
            }
            if let Some(axis) = composite(&node) {
                // Built in paint order, as the open tab over the rest, but read in place.
                let along = usize::from(axis == Axis::Y);
                inner.sort_by(|a, b| a.1[along].total_cmp(&b.1[along]));
            }
            node.set_children(inner.into_iter().map(|(id, _)| id).collect::<Vec<_>>());
            if control && !node.is_disabled() {
                // A text field takes the focus where the pointer would click it.
                if built.flags.contains(Flags::FOCUSABLE) {
                    node.add_action(Action::Focus);
                    if valued(role) {
                        node.add_action(Action::SetValue);
                    }
                } else if built.flags.contains(Flags::CLICKABLE) {
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                }
            }
            out.push((built.id, node));
        }
    }

    /// The controls the keyboard can focus, in the order built.
    pub(crate) fn stops(&self) -> Vec<Stop> {
        let mut stops = Vec::new();
        self.collect_stops(0, None, None, &mut stops);
        stops
    }

    fn collect_stops(
        &self,
        index: usize,
        group: Option<(Id, Role, Axis)>,
        popup: Option<Id>,
        stops: &mut Vec<Stop>,
    ) {
        for &child in &self.nodes[index].children {
            let built = &self.nodes[child];
            if !shown(built) {
                continue;
            }
            let popup = if built.anchor.is_some() {
                Some(built.id)
            } else {
                popup
            };
            let mut group = group;
            if let Some(access) = &built.access {
                let role = access.role();
                if navigates_itself(role) {
                    continue;
                }
                if let Some(axis) = composite(access) {
                    group = Some((built.id, role, axis));
                } else if built.flags.intersects(Flags::CLICKABLE | Flags::FOCUSABLE)
                    && !access.is_disabled()
                    && (!holds(role) || built.flags.contains(Flags::CUSTOM))
                {
                    stops.push(Stop {
                        id: built.id,
                        rect: built.rect,
                        group,
                        popup,
                        selected: access.is_selected() == Some(true),
                        typing: built.flags.intersects(Flags::FOCUSABLE | Flags::CUSTOM),
                        custom: built.flags.contains(Flags::CUSTOM),
                    });
                }
            }
            self.collect_stops(child, group, popup, stops);
        }
    }

    /// Moves the focus for a key among the controls, returning whether it took the key.
    pub(crate) fn traverse(&mut self, key: NamedKey) -> bool {
        let back = self.modifiers.shift_key();
        let scope = self.popups.last().map(|popup| popup.id);
        let at = self
            .focus
            .and_then(|focus| self.stops.iter().position(|stop| stop.id == focus));
        // The focused control, where it takes no keys of its own.
        let control = at.filter(|at| !self.stops[*at].typing);
        let target = match key {
            NamedKey::Tab => {
                let from = match (self.focus, at) {
                    (None, _) => None,
                    (focus, _) if focus == scope => None,
                    (_, Some(at)) if !self.stops[at].custom => Some(at),
                    _ => return false,
                };
                self.step(scope, from, back, |_| true)
            }
            NamedKey::F6 if scope.is_none() => {
                self.step(None, at, back, |stop| stop.group.is_some() || stop.custom)
            }
            NamedKey::F5
                if self.modifiers.control_key()
                    && draw::edit::Platform::CURRENT == draw::edit::Platform::MacOs
                    && scope.is_none() =>
            {
                self.entry(|stop| matches!(stop.group, Some((_, Role::Toolbar, _))))
            }
            NamedKey::ArrowLeft
            | NamedKey::ArrowRight
            | NamedKey::ArrowUp
            | NamedKey::ArrowDown
            | NamedKey::Home
            | NamedKey::End => {
                let Some(at) = control else {
                    return false;
                };
                let Some((group, _, axis)) = self.stops[at].group else {
                    return false;
                };
                let along = usize::from(axis == Axis::Y);
                let mut members: Vec<_> = self
                    .stops
                    .iter()
                    .filter(|stop| stop.group.is_some_and(|(id, ..)| id == group))
                    .collect();
                members.sort_by(|a, b| a.rect[along].total_cmp(&b.rect[along]));
                let place = members.iter().position(|stop| stop.id == self.stops[at].id);
                let (Some(place), count) = (place, members.len()) else {
                    return false;
                };
                let [before, after] = match axis {
                    Axis::X => [NamedKey::ArrowLeft, NamedKey::ArrowRight],
                    Axis::Y => [NamedKey::ArrowUp, NamedKey::ArrowDown],
                };
                let to = match key {
                    NamedKey::Home => 0,
                    NamedKey::End => count - 1,
                    key if key == before => (place + count - 1) % count,
                    key if key == after => (place + 1) % count,
                    _ => return false,
                };
                Some(members[to].id)
            }
            NamedKey::Space | NamedKey::Enter => {
                let Some(at) = control else {
                    return false;
                };
                self.press(self.stops[at].id);
                return true;
            }
            NamedKey::Escape if scope.is_none() && control.is_some() => {
                self.focus = self.resume.take();
                self.focus_ring = false;
                return true;
            }
            _ => return false,
        };
        let Some(target) = target else {
            return false;
        };
        if at.is_none_or(|at| self.stops[at].custom) {
            self.resume = self.focus;
        }
        self.focus = Some(target);
        self.focus_ring = true;
        true
    }

    /// The control Tab or F6 steps to in popup `scope` from stop `from`, over the steps
    /// `counts` keeps: a group's is its selected control, or its first.
    fn step(
        &self,
        scope: Option<Id>,
        from: Option<usize>,
        back: bool,
        counts: impl Fn(&Stop) -> bool,
    ) -> Option<Id> {
        let mut entries: Vec<usize> = Vec::new();
        for (index, stop) in self.stops.iter().enumerate() {
            if stop.popup != scope {
                continue;
            }
            match entries.last_mut() {
                Some(last) if self.stops[*last].step() == stop.step() => {
                    if stop.selected {
                        *last = index;
                    }
                }
                _ => entries.push(index),
            }
        }
        entries.retain(|entry| counts(&self.stops[*entry]));
        let count = entries.len();
        if count == 0 {
            return None;
        }
        let here = from.and_then(|from| {
            let step = self.stops[from].step();
            entries
                .iter()
                .position(|entry| self.stops[*entry].step() == step)
        });
        let to = match (here, from) {
            (Some(here), _) if back => (here + count - 1) % count,
            (Some(here), _) => (here + 1) % count,
            // From a control not counted, the step either side of it.
            (None, Some(from)) => {
                let after = entries
                    .iter()
                    .position(|entry| *entry > from)
                    .unwrap_or(count);
                (if back { after + count - 1 } else { after }) % count
            }
            (None, None) if back => count - 1,
            (None, None) => 0,
        };
        Some(self.stops[entries[to]].id)
    }

    /// The control a group `matching` is entered at.
    fn entry(&self, matching: impl Fn(&Stop) -> bool) -> Option<Id> {
        let first = self.stops.iter().position(&matching)?;
        let group = self.stops[first].step();
        let selected = self
            .stops
            .iter()
            .find(|stop| stop.step() == group && stop.selected);
        Some(selected.unwrap_or(&self.stops[first]).id)
    }

    /// Presses and clicks box `id` in one, as a key or assistive technology does.
    fn press(&mut self, id: Id) {
        let signal = self.signals.entry(id).or_default();
        signal.pressed = true;
        signal.clicked = true;
    }

    /// Carries out assistive technology's `request`.
    pub(crate) fn act(&mut self, request: ActionRequest) {
        let id = Id(request.target_node.0);
        match (request.action, request.data) {
            (Action::Click, _) => {
                // As a press there would, one on a box beneath the popups closes them.
                if self.hits[..self.modal].iter().any(|hit| hit.id == id) {
                    self.close_from(0);
                }
                self.press(id);
            }
            (Action::Focus, _) => self.focus = Some(id),
            (Action::SetValue, Some(ActionData::Value(value))) => {
                self.focus_all(id);
                self.signals
                    .entry(id)
                    .or_default()
                    .events
                    .push(Event::Ime(Ime::Commit(value.into())));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
