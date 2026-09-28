//! Back and Forward through the pages visited, across sections and notebooks, as OneNote
//! 2010's Quick Access Toolbar offers them.

use super::*;

/// A page shown: its notebook's location, its section's catalog path and the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub notebook: String,
    pub section: String,
    pub page: ExGuid,
}

/// Pages visited before and after the one shown.
#[derive(Default)]
pub struct Trail {
    back: Vec<Place>,
    here: Option<Place>,
    forward: Vec<Place>,
}

/// Places Back keeps, at most.
const KEPT: usize = 100;

impl Trail {
    /// Notes `place` shown. Arriving anywhere but where Back or Forward went drops the pages
    /// ahead, as a browser does.
    pub fn visit(&mut self, place: Place) {
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
}

impl State {
    /// Notes the page just shown for Back and Forward.
    pub(crate) fn visited(&mut self) {
        if let Some(session) = &self.session {
            self.trail.visit(Place {
                notebook: session.library.location.clone(),
                section: session.tabs[session.tab].path.clone(),
                page: session.space,
            });
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
        if let Some(session) = &self.session
            && session.library.location == place.notebook
            && session.tabs[session.tab].path == place.section
        {
            self.commands.push(Command::OpenPage(place.page));
            return;
        }
        let Some(library) = self
            .notebooks
            .iter()
            .find(|library| library.location == place.notebook)
        else {
            return;
        };
        let library = Arc::clone(library);
        self.last_pages
            .insert(library.key(&place.section), place.page);
        self.commands
            .push(Command::OpenSection(library, place.section));
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
