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

/// A page visited, as Back and Forward find it again after its section is renamed or moved:
/// its notebook's location, its section's file identity, and the page within it, by space
/// and by the identity it keeps moving to another section.
#[derive(Clone, Debug, PartialEq)]
pub struct Visit {
    notebook: String,
    section: [u8; 16],
    space: ExGuid,
    page: Option<[u8; 16]>,
    /// Whether the section was in the recycle bin, where a deleted section keeps its identity.
    binned: bool,
}

impl Visit {
    /// Whether `other` is this page, wherever it has moved since.
    fn same(&self, other: &Visit) -> bool {
        self.notebook == other.notebook
            && match (self.page, other.page) {
                (Some(page), Some(other)) => page == other,
                _ => self.section == other.section && self.space == other.space,
            }
    }
}

/// Pages visited before and after the one shown, and those shown lately.
#[derive(Default)]
pub struct Trail {
    back: Vec<Visit>,
    here: Option<Visit>,
    forward: Vec<Visit>,
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
        let visits = (self.back.iter_mut())
            .chain(&mut self.here)
            .chain(&mut self.forward)
            .map(|visit| &mut visit.notebook);
        let places = self.recent.iter_mut().map(|place| &mut place.notebook);
        for notebook in visits.chain(places).filter(|notebook| *notebook == from) {
            *notebook = to.to_owned();
        }
    }

    /// Notes `place` shown, as the recent pages list it.
    pub fn remember(&mut self, place: Place) {
        self.recent.retain(|recent| *recent != place);
        self.recent.insert(0, place);
        self.recent.truncate(RECENT);
    }

    /// Notes `visit` shown. Arriving anywhere but where Back or Forward went drops the pages
    /// ahead, as a browser does.
    pub fn visit(&mut self, visit: Visit) {
        if let Some(here) = &mut self.here
            && here.same(&visit)
        {
            *here = visit;
            return;
        }
        if let Some(here) = self.here.replace(visit) {
            self.back.push(here);
            if self.back.len() > KEPT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// Steps back, or `forward`, to the nearest place `exists` accepts; the place to show.
    /// Places it refuses, on either side, are dropped.
    pub fn step(&mut self, forward: bool, exists: impl Fn(&Visit) -> bool) -> Option<Visit> {
        self.back.retain(&exists);
        self.forward.retain(&exists);
        let (from, to) = if forward {
            (&mut self.forward, &mut self.back)
        } else {
            (&mut self.back, &mut self.forward)
        };
        let visit = from.pop()?;
        if let Some(here) = self.here.replace(visit.clone()) {
            to.push(here);
        }
        Some(visit)
    }

    /// Whether Back and Forward have somewhere `exists` accepts to go.
    pub fn open(&self, exists: impl Fn(&Visit) -> bool) -> [bool; 2] {
        [&self.back, &self.forward].map(|side| side.iter().rev().any(&exists))
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

/// Where `visit`'s page is in `notebooks`, as `index` has read them and `open` tells whether
/// the section open, by key, lists a page: the notebook, the section's catalog path and the
/// page. A page gone from its section is followed by its identity; none once it is nowhere.
fn find(
    visit: &Visit,
    notebooks: &[Arc<Library>],
    open: impl Fn(&str, ExGuid) -> Option<bool>,
    index: &Index,
) -> Option<(Arc<Library>, String, ExGuid)> {
    let library = (notebooks.iter()).find(|library| library.location == visit.notebook)?;
    let path = match &library.notebook {
        Ok(Some(notebook)) => (crate::library::folders(notebook.catalog(), |_| true).into_iter())
            .flat_map(|folder| &folder.sections)
            .find(|section| section.file_id == visit.section)
            .map(|section| section.path.clone()),
        Ok(None) => Some(library.location.clone()),
        Err(_) => None,
    }
    .filter(|path| crate::recycle::binned(path) == visit.binned);
    if let Some(path) = path {
        let key = library.key(&path);
        // A section the index has yet to read is taken to hold the page.
        let listed = open(&key, visit.space).unwrap_or_else(|| {
            index.get(&key, visit.space).is_some()
                || !index.entries().iter().any(|entry| entry.section == key)
        });
        if listed {
            return Some((Arc::clone(library), path, visit.space));
        }
    }
    let page = visit.page?;
    index.entries().iter().find_map(|entry| {
        let (location, path) = entry.section.split_once('\n')?;
        (entry.identity == Some(page) && location == visit.notebook)
            .then(|| (Arc::clone(library), path.to_owned(), entry.space))
    })
}

impl State {
    /// Notes the page just shown for Back and Forward, and among the recent pages.
    pub(crate) fn visited(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let path = &session.tabs[session.tab].path;
        let place = Place {
            notebook: session.library.location.clone(),
            section: path.clone(),
            page: session.space,
        };
        if let Ok(section) = session.section.identity() {
            self.trail.visit(Visit {
                notebook: place.notebook.clone(),
                section,
                space: session.space,
                page: self.view.editor.identity(),
                binned: crate::recycle::binned(path),
            });
        }
        let moved = self.trail.recent.first() != Some(&place);
        self.trail.remember(place);
        if moved {
            self.save_settings();
        }
    }

    /// Where `visit`'s page is now, as `find` follows it.
    fn find(&self, visit: &Visit) -> Option<(Arc<Library>, String, ExGuid)> {
        let lists = |key: &str, page| {
            let session = self
                .session
                .as_ref()
                .filter(|session| session.key() == key)?;
            Some(session.pages.iter().any(|(space, ..)| *space == page))
        };
        let index = (self.search.index.lock()).unwrap_or_else(|poison| poison.into_inner());
        find(visit, &self.notebooks, lists, &index)
    }

    /// Whether Back and Forward have a page to go to.
    pub(crate) fn can_travel(&self) -> [bool; 2] {
        self.trail.open(|visit| self.find(visit).is_some())
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

    /// Shows the page visited before the one shown, or after it going `forward`, wherever it
    /// went since. Pages since deleted, and those of notebooks since closed, are passed over.
    pub(crate) fn travel(&mut self, forward: bool) {
        let mut trail = std::mem::take(&mut self.trail);
        let visit = trail.step(forward, |visit| self.find(visit).is_some());
        self.trail = trail;
        if let Some((library, path, space)) = visit.and_then(|visit| self.find(&visit)) {
            self.go(library, path, space);
        }
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

    fn visit(section: u8, page: u32) -> Visit {
        let mut identity = [0; 16];
        identity[0] = section;
        identity[1..5].copy_from_slice(&page.to_le_bytes());
        Visit {
            notebook: "/notebooks/Personal".into(),
            section: [section; 16],
            space: ExGuid {
                guid: [7; 16],
                n: page,
            },
            page: Some(identity),
            binned: false,
        }
    }

    #[test]
    fn back_and_forward_retrace_visits_across_sections() {
        let mut trail = Trail::default();
        let all = |_: &Visit| true;
        assert_eq!(trail.open(all), [false, false]);
        for (section, page) in [(1, 1), (1, 2), (2, 1)] {
            trail.visit(visit(section, page));
        }
        // Showing the same page again, as a reload does, is no visit.
        trail.visit(visit(2, 1));
        assert_eq!(trail.step(false, all), Some(visit(1, 2)));
        // Arriving where Back went is not a new visit.
        trail.visit(visit(1, 2));
        assert_eq!(trail.open(all), [true, true]);
        assert_eq!(trail.step(false, all), Some(visit(1, 1)));
        assert_eq!(trail.step(false, all), None);
        assert_eq!(trail.step(true, all), Some(visit(1, 2)));
        assert_eq!(trail.step(true, all), Some(visit(2, 1)));
        assert_eq!(trail.step(true, all), None);
        // A page opened after going back drops the pages ahead.
        trail.step(false, all);
        trail.visit(visit(3, 5));
        assert_eq!(trail.open(all), [true, false]);
        assert_eq!(trail.step(false, all), Some(visit(1, 2)));
    }

    #[test]
    fn back_and_forward_pass_over_pages_gone_and_go_dark_without_any() {
        let mut trail = Trail::default();
        for page in 1..=4 {
            trail.visit(visit(1, page));
        }
        let all = |_: &Visit| true;
        trail.step(false, all);
        // Pages 1 and 2 behind, 4 ahead; 2 and 4 are gone since.
        let kept = |visit: &Visit| ![2, 4].contains(&visit.space.n);
        assert_eq!(trail.open(kept), [true, false]);
        assert_eq!(trail.step(true, kept), None);
        assert_eq!(trail.step(false, kept), Some(visit(1, 1)));
        // What was passed over is gone for good.
        assert_eq!(trail.step(true, all), Some(visit(1, 3)));
        assert_eq!(trail.open(all), [true, false]);
    }

    #[test]
    fn arriving_at_a_page_moved_since_is_no_new_visit() {
        let mut trail = Trail::default();
        for page in 1..=2 {
            trail.visit(visit(1, page));
        }
        let all = |_: &Visit| true;
        trail.step(false, all);
        // Page 1 now shows from section 2, under a new space, as Back found it.
        let moved = Visit {
            section: [2; 16],
            space: visit(2, 9).space,
            ..visit(1, 1)
        };
        trail.visit(moved.clone());
        assert_eq!(trail.open(all), [false, true]);
        assert_eq!(trail.step(true, all), Some(visit(1, 2)));
        assert_eq!(trail.step(false, all), Some(moved));
    }

    #[test]
    fn recent_pages_are_kept_once_latest_first_and_bounded() {
        let mut trail = Trail::default();
        for page in [1, 2, 1] {
            trail.remember(place("A.one", page));
        }
        assert_eq!(trail.recent, [place("A.one", 1), place("A.one", 2)]);
        for page in 0..20 {
            trail.remember(place("B.one", page));
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
            trail.remember(gone);
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
            trail.visit(visit(1, page));
        }
        let mut steps = 0;
        while trail.step(false, |_| true).is_some() {
            steps += 1;
        }
        assert_eq!(steps, KEPT);
    }

    /// Back and Forward find a page by its section's identity and its own, in the cases the
    /// path it was visited at no longer opens: the section renamed, moved into a group or
    /// deleted, the page moved or deleted, the notebook closed.
    #[test]
    fn visits_are_found_wherever_their_section_and_page_went() {
        use notebook::session::Notebook;
        use onestore::PageCreation;
        let temporary =
            std::env::temp_dir().join(format!("snowbound-visits-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&temporary);
        notebook::fs::create_dir_all(&temporary).unwrap();
        let root = temporary.join("Visited");
        let location = root.to_str().unwrap();
        let cache = temporary.join("cache");
        let dated = || PageCreation::new(None, Some(""), "Author").unwrap();
        let mut notebook =
            Notebook::create(location, &cache, Notebook::NEW_COLOR, &dated()).unwrap();
        notebook.create_section("", "A", &dated()).unwrap();
        notebook.create_section("", "B", &dated()).unwrap();
        let mut library = Arc::new(Library::created(location, notebook, &cache));
        let a = library.section_identity("A.one").unwrap();
        let page = |n| ExGuid { guid: [9; 16], n };
        let shown = Visit {
            notebook: location.to_owned(),
            section: a,
            space: page(1),
            page: Some([5; 16]),
            binned: false,
        };
        let mut index = Index::default();
        let found = |library: &Arc<Library>, index: &Index, open: Option<&str>| {
            let lists = |key: &str, space| (Some(key) == open).then_some(space == page(2));
            find(&shown, std::slice::from_ref(library), lists, index)
                .map(|(_, path, space)| (path, space.n))
        };
        assert_eq!(found(&library, &index, None), Some(("A.one".into(), 1)));

        let change = |library: &mut Arc<Library>, change: &dyn Fn(&mut Notebook)| {
            let mut notebook = library.reopen().unwrap();
            change(&mut notebook);
            *library = Arc::new(library.with(notebook));
        };
        change(&mut library, &|notebook| {
            drop(notebook.rename("A.one", "Renamed").unwrap())
        });
        assert_eq!(
            found(&library, &index, None),
            Some(("Renamed.one".into(), 1))
        );
        change(&mut library, &|notebook| {
            notebook.create_group("", "Group").unwrap();
            notebook.move_entry("Renamed.one", "Group").unwrap();
        });
        let moved = "Group/Renamed.one";
        assert_eq!(found(&library, &index, None), Some((moved.into(), 1)));

        // The open section's own list, and the index's of others, tell a page deleted.
        let key = library.key(moved);
        let open = Some(key.as_str());
        assert_eq!(found(&library, &index, open), None);
        let entry = |path: &str, space, identity| {
            let mut page = onestore::page::Page {
                title: String::new(),
                identity: None,
                created: None,
                margin_origin: [0.0; 2],
                rtl: false,
                color: None,
                rule_lines: None,
                objects: Vec::new(),
                definitions: Default::default(),
            };
            page.identity = identity;
            canvas::search::Entry::new(&library.key(path), space, &page, 0)
        };
        index.set(entry(moved, page(2), None));
        assert_eq!(found(&library, &index, None), None);
        // A page moved to another section is followed by its identity.
        index.set(entry("B.one", page(7), Some([5; 16])));
        assert_eq!(found(&library, &index, None), Some(("B.one".into(), 7)));
        assert_eq!(found(&library, &index, open), Some(("B.one".into(), 7)));

        // A section deleted to the recycle bin keeps its identity there.
        let index = Index::default();
        change(&mut library, &|notebook| notebook.delete(moved).unwrap());
        let notebooks = [library];
        assert!(
            find(&shown, &notebooks, |_, _| None, &index).is_none(),
            "the section deleted"
        );
        assert!(
            find(&shown, &[], |_, _| None, &index).is_none(),
            "the notebook closed"
        );
        notebook::fs::remove_dir_all(&temporary).unwrap();
    }
}
