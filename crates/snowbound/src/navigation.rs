//! Back and Forward through the pages visited, across sections and notebooks, as OneNote
//! 2010's Quick Access Toolbar offers them, and the pages shown lately, which the palette
//! lists first.

use super::*;
use canvas::search::Index;
use std::{collections::HashSet, time::Duration};
use winit::event::TouchPhase;

/// A page shown: its notebook's location, its section's catalog path and the page.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Place {
    pub notebook: String,
    pub section: String,
    pub page: ExGuid,
}

/// Pages visited before and after the one shown, and those shown lately.
#[derive(Default)]
pub struct Trail {
    back: Vec<Place>,
    here: Option<Place>,
    forward: Vec<Place>,
    /// Each page shown lately once, latest first, kept between launches.
    pub recent: Vec<Place>,
}

/// Places Back keeps, at most.
const KEPT: usize = 100;
/// Pages `Trail::recent` keeps, at most.
const RECENT: usize = 8;

impl Trail {
    /// Follows the notebook at `from` to `to`, where it moved.
    pub fn moved(&mut self, from: &str, to: &str) {
        let places = (self.back.iter_mut())
            .chain(&mut self.here)
            .chain(&mut self.forward)
            .chain(&mut self.recent);
        for place in places.filter(|place| place.notebook == from) {
            place.notebook = to.to_owned();
        }
    }

    /// Notes `place` shown. Arriving anywhere but where Back or Forward went drops the pages
    /// ahead, as a browser does.
    pub fn visit(&mut self, place: Place) {
        self.recent.retain(|recent| *recent != place);
        self.recent.insert(0, place.clone());
        self.recent.truncate(RECENT);
        if self.here.as_ref() == Some(&place) {
            return;
        }
        if let Some(here) = self.here.replace(place) {
            self.back.push(here);
            if self.back.len() > KEPT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// Steps back, or `forward`, to the nearest place `exists` accepts, dropping those it
    /// refuses; the place to show.
    pub fn step(&mut self, forward: bool, exists: impl Fn(&Place) -> bool) -> Option<Place> {
        let (from, to) = if forward {
            (&mut self.forward, &mut self.back)
        } else {
            (&mut self.back, &mut self.forward)
        };
        let place = std::iter::from_fn(|| from.pop()).find(|place| exists(place))?;
        if let Some(here) = self.here.replace(place.clone()) {
            to.push(here);
        }
        Some(place)
    }

    /// Whether Back and Forward have somewhere to go.
    pub fn open(&self) -> [bool; 2] {
        [!self.back.is_empty(), !self.forward.is_empty()]
    }

    /// Drops the recent pages whose section `listed` refuses, and those whose section `index`
    /// has read without them. Returns whether any went.
    pub fn prune(&mut self, index: &Index, listed: impl Fn(&Place) -> bool) -> bool {
        let read: HashSet<&str> = (index.entries().iter())
            .map(|entry| entry.section.as_str())
            .collect();
        let before = self.recent.len();
        self.recent.retain(|place| {
            let key = crate::library::key(&place.notebook, &place.section);
            listed(place)
                && (!read.contains(key.as_str())
                    || (index.entries().iter())
                        .any(|entry| entry.space == place.page && entry.section == key))
        });
        self.recent.len() != before
    }
}

impl State {
    /// Notes the page just shown for Back and Forward, and among the recent pages.
    pub(crate) fn visited(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let place = Place {
            notebook: session.library.location.clone(),
            section: session.tabs[session.tab].path.clone(),
            page: session.space,
        };
        let moved = self.trail.recent.first() != Some(&place);
        self.trail.visit(place);
        if moved {
            self.save_settings();
        }
    }

    /// Whether `library`'s section at `path` is the one open.
    pub(crate) fn open(&self, library: &Library, path: &str) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session.library.location == library.location && session.tabs[session.tab].path == path
        })
    }

    /// Shows page `space` of `library`'s section at `path`, opening the section on it where
    /// it is not the one open.
    pub(crate) fn go(&mut self, library: Arc<Library>, path: String, space: ExGuid) {
        if self.open(&library, &path) {
            self.commands.push(Command::OpenPage(space));
        } else {
            self.last_pages.insert(library.key(&path), space);
            self.commands.push(Command::OpenSection(library, path));
        }
    }

    /// Shows the page visited before the one shown, or after it going `forward`. A page
    /// deleted from the open section since is passed over.
    pub(crate) fn travel(&mut self, forward: bool) {
        let session = self.session.as_ref();
        let exists = |place: &Place| {
            session.is_none_or(|session| {
                session.library.location != place.notebook
                    || session.tabs[session.tab].path != place.section
                    || session.pages.iter().any(|(page, ..)| *page == place.page)
            })
        };
        let Some(place) = self.trail.step(forward, exists) else {
            return;
        };
        let Some(library) = self
            .notebooks
            .iter()
            .find(|library| library.location == place.notebook)
        else {
            return;
        };
        self.go(Arc::clone(library), place.section, place.page);
    }
}

/// A two-finger horizontal trackpad swipe, which travels Back or Forward as macOS's
/// "swipe between pages" does.
#[derive(Default)]
pub struct Swipe {
    travelled: Option<[f32; 2]>,
    ended: Option<Instant>,
}

impl Swipe {
    /// Feeds one trackpad scroll in logical pixels; `armed` when the page cannot scroll
    /// sideways. Returns whether to go forward once a swipe completes.
    pub fn scroll(
        &mut self,
        phase: TouchPhase,
        delta: [f32; 2],
        armed: bool,
        at: Instant,
    ) -> Option<bool> {
        match phase {
            // Momentum arrives as a second gesture right after the fingers lift.
            TouchPhase::Started => {
                let momentum = self
                    .ended
                    .is_some_and(|ended| at - ended < Duration::from_millis(150));
                self.travelled = (armed && !momentum).then_some([0.0; 2]);
                None
            }
            TouchPhase::Moved => {
                if let Some(travelled) = &mut self.travelled {
                    travelled[0] += delta[0];
                    travelled[1] += delta[1];
                }
                None
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.ended = Some(at);
                let [x, y] = self.travelled.take()?;
                (phase == TouchPhase::Ended && x.abs() > 120.0 && x.abs() > 2.0 * y.abs())
                    .then_some(x < 0.0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(section: &str, page: u32) -> Place {
        Place {
            notebook: "/notebooks/Personal".into(),
            section: section.into(),
            page: ExGuid {
                guid: [7; 16],
                n: page,
            },
        }
    }

    #[test]
    fn swipe_travels_on_a_long_sideways_stroke_only() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let stroke = |swipe: &mut Swipe, dx: f32, dy: f32, armed: bool, t: u64| {
            swipe.scroll(TouchPhase::Started, [0.0; 2], armed, at(t));
            for _ in 0..10 {
                swipe.scroll(TouchPhase::Moved, [dx / 10.0, dy / 10.0], armed, at(t));
            }
            swipe.scroll(TouchPhase::Ended, [0.0; 2], armed, at(t))
        };
        let mut swipe = Swipe::default();
        // Fingers right go Back, left go Forward.
        assert_eq!(stroke(&mut swipe, 200.0, 10.0, true, 0), Some(false));
        assert_eq!(stroke(&mut swipe, -200.0, 10.0, true, 1000), Some(true));
        // The momentum right after a stroke is not another.
        assert_eq!(stroke(&mut swipe, -200.0, 0.0, true, 1050), None);
        assert_eq!(stroke(&mut swipe, 60.0, 0.0, true, 2000), None);
        assert_eq!(stroke(&mut swipe, 200.0, 150.0, true, 3000), None);
        assert_eq!(stroke(&mut swipe, 200.0, 0.0, false, 4000), None);
    }

    #[test]
    fn back_and_forward_retrace_visits_across_sections() {
        let mut trail = Trail::default();
        assert_eq!(trail.open(), [false, false]);
        for (section, page) in [("A.one", 1), ("A.one", 2), ("B.one", 1)] {
            trail.visit(place(section, page));
        }
        // Showing the same page again, as a reload does, is no visit.
        trail.visit(place("B.one", 1));
        let all = |_: &Place| true;
        assert_eq!(trail.step(false, all), Some(place("A.one", 2)));
        // Arriving where Back went is not a new visit.
        trail.visit(place("A.one", 2));
        assert_eq!(trail.open(), [true, true]);
        assert_eq!(trail.step(false, all), Some(place("A.one", 1)));
        assert_eq!(trail.step(false, all), None);
        assert_eq!(trail.step(true, all), Some(place("A.one", 2)));
        assert_eq!(trail.step(true, all), Some(place("B.one", 1)));
        assert_eq!(trail.step(true, all), None);
        // A page opened after going back drops the pages ahead.
        trail.step(false, all);
        trail.visit(place("C.one", 5));
        assert_eq!(trail.open(), [true, false]);
        assert_eq!(trail.step(false, all), Some(place("A.one", 2)));
    }

    #[test]
    fn back_passes_over_pages_since_deleted() {
        let mut trail = Trail::default();
        for page in 1..=3 {
            trail.visit(place("A.one", page));
        }
        let kept = |place: &Place| place.page.n != 2;
        assert_eq!(trail.step(false, kept), Some(place("A.one", 1)));
        assert_eq!(trail.step(true, kept), Some(place("A.one", 3)));
        assert_eq!(trail.open(), [true, false]);
    }

    #[test]
    fn recent_pages_are_kept_once_latest_first_and_bounded() {
        let mut trail = Trail::default();
        for page in [1, 2, 1] {
            trail.visit(place("A.one", page));
        }
        assert_eq!(trail.recent, [place("A.one", 1), place("A.one", 2)]);
        for page in 0..20 {
            trail.visit(place("B.one", page));
        }
        assert_eq!(trail.recent.len(), RECENT);
        assert_eq!(trail.recent[0], place("B.one", 19));
    }

    #[test]
    fn recent_pages_go_with_their_section_or_once_the_index_lacks_them() {
        let page = |title: &str| onestore::page::Page {
            title: title.into(),
            identity: None,
            created: None,
            margin_origin: [0.0; 2],
            rtl: false,
            color: None,
            rule_lines: None,
            objects: Vec::new(),
            definitions: Default::default(),
        };
        let mut index = Index::default();
        let section = crate::library::key("/notebooks/Personal", "A.one");
        let kept = place("A.one", 1);
        index.set(canvas::search::Entry::new(
            &section,
            kept.page,
            &page("Kept"),
            0,
        ));
        let mut trail = Trail::default();
        // Visited oldest first: the index has read A without page 2, not yet B, and C is gone.
        for gone in [
            place("C.one", 1),
            place("B.one", 1),
            place("A.one", 2),
            kept.clone(),
        ] {
            trail.visit(gone);
        }
        let listed = |place: &Place| place.section != "C.one";
        assert!(trail.prune(&index, listed));
        assert_eq!(trail.recent, [kept, place("B.one", 1)]);
        assert!(!trail.prune(&index, listed), "nothing more goes");
    }

    #[test]
    fn back_keeps_its_last_hundred_places() {
        let mut trail = Trail::default();
        for page in 0..150 {
            trail.visit(place("A.one", page));
        }
        let mut steps = 0;
        while trail.step(false, |_| true).is_some() {
            steps += 1;
        }
        assert_eq!(steps, KEPT);
    }
}
