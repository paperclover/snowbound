//! Sections and pages made ready before they are shown, and pages kept once left, so
//! switching to one shows it at once. A section kept open is its notebook's to hand out
//! (`Library::open`); pages are kept as scenes, whose pictures are what takes long to
//! draw, and every page shown is read and laid out afresh around them.

use super::*;
use std::collections::VecDeque;

/// Most recently used first, at most `most` of them.
pub struct Recent<K, V> {
    entries: VecDeque<(K, V)>,
    most: usize,
}

impl<K: PartialEq, V> Recent<K, V> {
    pub const fn new(most: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            most,
        }
    }

    /// Keeps `value` as the most recent, returning what `key` held before and what no
    /// longer fits.
    pub fn put(&mut self, key: K, value: V) -> Vec<V> {
        let mut gone: Vec<V> = self.take(&key).into_iter().collect();
        self.entries.push_front((key, value));
        gone.extend(self.trim(u64::MAX, |_| 0));
        gone
    }

    pub fn take(&mut self, key: &K) -> Option<V> {
        let at = self.entries.iter().position(|(held, _)| held == key)?;
        self.entries.remove(at).map(|(_, value)| value)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.iter().any(|(held, _)| held == key)
    }

    /// Lets go of the entries `keep` refuses.
    pub fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        self.entries.retain(|(key, _)| keep(key));
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, value)| value)
    }

    /// Lets go of the least recent past `most`, and past `budget` as `weigh` counts them.
    pub fn trim(&mut self, budget: u64, weigh: impl Fn(&V) -> u64) -> Vec<V> {
        let mut kept = 0u64;
        let fits = self
            .entries
            .iter()
            .take(self.most)
            .take_while(|(_, value)| {
                kept = kept.saturating_add(weigh(value));
                kept <= budget
            })
            .count();
        self.entries.drain(fits..).map(|(_, value)| value).collect()
    }
}

/// Pages kept: a section's `Library::key` and the page's space.
type Key = (String, ExGuid);

/// How many pages are kept laid out, and the decoded pictures they may hold between them,
/// besides the page shown.
const SCENES: usize = 32;
const SCENE_BYTES: u64 = 128 << 20;

/// A page kept for showing again: its scene, at the offset `PageView::open` takes, and, while
/// its pictures are still drawing, the editor that places them.
pub struct Kept {
    scene: (PageScene, [f32; 2]),
    editor: Option<CanvasEditor>,
}

pub type Scenes = Arc<Mutex<Recent<Key, Kept>>>;

/// Work done ahead on its own thread, one piece at a time.
enum Job {
    /// Reads a page of the open section.
    Page(
        Key,
        Box<dyn FnOnce() -> Result<Page, Box<dyn Error>> + Send>,
    ),
    /// Opens a section for its notebook to keep, then reads the page it would show.
    Section {
        library: Arc<Library>,
        path: String,
        last: Option<ExGuid>,
        notify: Box<dyn Fn() + Send>,
    },
}

pub struct Prefetch {
    pub scenes: Scenes,
    jobs: mpsc::Sender<Job>,
    /// What was asked for lately, by section key and page (none for a section), so a hover
    /// held over many frames asks once.
    asked: Recent<(String, Option<ExGuid>), ()>,
}

impl Prefetch {
    pub fn new(layouts: Arc<Mutex<TextEngine>>, redraw: std::task::Waker) -> Self {
        let scenes = Scenes::new(Mutex::new(Recent::new(SCENES)));
        let (jobs, queue) = mpsc::channel();
        let kept = Arc::clone(&scenes);
        std::thread::Builder::new()
            .name("prefetch".into())
            .spawn(move || {
                for job in queue {
                    // What fails ahead fails again, and says so, once it is asked for.
                    if prepare(job, &layouts, &kept).is_ok() {
                        redraw.wake_by_ref();
                    }
                }
            })
            .expect("The prefetch thread starts");
        Self {
            scenes,
            jobs,
            asked: Recent::new(32),
        }
    }

    /// The scene kept for page `space` of the section `section` names, if any.
    pub fn take(scenes: &Scenes, section: &str, space: ExGuid) -> Option<PageScene> {
        let kept = scenes.lock().ok()?.take(&(section.to_owned(), space))?;
        Some(kept.scene.0)
    }

    /// Keeps the scene of a page left, its pictures drawn.
    pub fn keep(&mut self, section: String, space: ExGuid, scene: (PageScene, [f32; 2])) {
        self.asked.take(&(section.clone(), Some(space)));
        if let Ok(mut scenes) = self.scenes.lock() {
            scenes.put(
                (section, space),
                Kept {
                    scene,
                    editor: None,
                },
            );
            scenes.trim(SCENE_BYTES, weigh);
        }
    }

    /// Lets go of the scenes kept of the section `section` names, as a locked one's.
    pub fn forget(&mut self, section: &str) {
        if let Ok(mut scenes) = self.scenes.lock() {
            scenes.retain(|(kept, _)| kept != section);
        }
        self.asked.retain(|(asked, _)| asked != section);
    }

    /// Whether `target` is new since it was last asked for, noting it.
    fn ask(&mut self, target: (String, Option<ExGuid>)) -> bool {
        let new = !self.asked.contains(&target);
        self.asked.put(target, ());
        new
    }

    /// Draws the pictures kept pages show first, as `view` would open them, letting go of
    /// each editor once they are drawn.
    pub fn draw(&self, view: &PageView, paper: canvas::gpu::Paper, waker: &std::task::Waker) {
        let Ok(mut scenes) = self.scenes.lock() else {
            return;
        };
        for kept in scenes.values_mut() {
            if let Some(editor) = &kept.editor
                && view.prepare(
                    &mut kept.scene,
                    editor,
                    paper.colored(editor.page_color()),
                    waker,
                )
            {
                kept.editor = None;
            }
        }
        scenes.trim(SCENE_BYTES, weigh);
    }
}

fn weigh(kept: &Kept) -> u64 {
    kept.scene.0.raster_bytes()
}

fn prepare(job: Job, layouts: &Mutex<TextEngine>, scenes: &Scenes) -> Result<(), Box<dyn Error>> {
    let (key, page) = match job {
        Job::Page(key, read) => {
            if scenes.lock().map_err(|_| "Prefetch failed")?.contains(&key) {
                return Ok(());
            }
            let page = read()?;
            (key, page)
        }
        Job::Section {
            library,
            path,
            last,
            notify,
        } => {
            let replica = library.prefetch(&path, notify)?;
            let pages = replica.pages()?;
            let space = last
                .filter(|space| pages.iter().any(|(listed, ..)| listed == space))
                .or(pages.first().map(|(space, ..)| *space))
                .ok_or("The section has no pages")?;
            let key = (library.key(&path), space);
            if scenes.lock().map_err(|_| "Prefetch failed")?.contains(&key) {
                return Ok(());
            }
            (key, replica.page(space)?)
        }
    };
    let (scene, editor) = {
        let mut engine = layouts.lock().map_err(|_| "Page layout failed")?;
        PageScene::from_page(page, &mut engine)?
    };
    let mut scenes = scenes.lock().map_err(|_| "Prefetch failed")?;
    scenes.put(
        key,
        Kept {
            scene: (scene, [0.0; 2]),
            editor: Some(editor),
        },
    );
    scenes.trim(SCENE_BYTES, weigh);
    Ok(())
}

impl Session {
    /// The section's `Library::key`.
    pub fn key(&self) -> String {
        self.library.key(&self.tabs[self.tab].path)
    }
}

/// How long a page may take to open before its area shows that it is opening, so a page
/// ready in a frame or two never flashes the placeholder.
const PATIENCE: std::time::Duration = std::time::Duration::from_millis(80);

/// A page's outline while it opens, in points from the page area's corner: its title, its
/// date and some lines of text, breathing in `paper`'s ink.
pub fn skeleton(paper: canvas::gpu::Paper, since: Instant) -> Vec<draw::Primitive<'static>> {
    let breath = 0.5 - 0.5 * (since.elapsed().as_secs_f32() * std::f32::consts::TAU / 1.6).cos();
    let [red, green, blue, _] = paper.ink;
    let color = [red, green, blue, 0.06 + 0.06 * breath];
    let bar = |x: f32, y: f32, width: f32, height: f32| draw::Primitive::RoundedRect {
        rect: [x, y, x + width, y + height],
        radius: [height / 3.0; 2],
        stroke: None,
        color,
    };
    let mut bars = vec![bar(48.0, 40.0, 280.0, 24.0), bar(48.0, 74.0, 150.0, 10.0)];
    let lines = [420.0, 380.0, 440.0, 300.0, 0.0, 400.0, 360.0, 220.0];
    for (line, width) in lines.into_iter().enumerate() {
        if width > 0.0 {
            bars.push(bar(64.0, 120.0 + 24.0 * line as f32, width, 11.0));
        }
    }
    bars
}

impl State {
    /// The section tab shown open: one being opened, from the moment it is asked for.
    pub fn open_tab(&self) -> Option<usize> {
        let session = self.session.as_ref()?;
        let opening = self.switching.and_then(|(tab, _)| tab);
        Some(
            opening
                .filter(|tab| *tab < session.tabs.len())
                .unwrap_or(session.tab),
        )
    }

    /// Since when the page area shows that a page is opening, once it has taken a while.
    pub fn loading(&self) -> Option<Instant> {
        let (_, since) = self.switching?;
        (since.elapsed() >= PATIENCE).then_some(since)
    }

    /// Leaves the page shown as it is, dropping whatever was opening in its place.
    pub fn stop_loading(&mut self) {
        self.loading += 1;
        self.opening = None;
        self.switching = None;
    }

    /// Readies the pages beside the open one and the sections beside the open section.
    pub fn prefetch_around(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let at = session
            .pages
            .iter()
            .position(|(space, ..)| *space == session.space);
        let pages: Vec<ExGuid> = at
            .into_iter()
            .flat_map(|at| [at + 1, at.wrapping_sub(1)])
            .filter_map(|at| session.pages.get(at).map(|(space, ..)| *space))
            .collect();
        let tabs: Vec<usize> = [session.tab + 1, session.tab.wrapping_sub(1)]
            .into_iter()
            .filter(|tab| *tab < session.tabs.len())
            .collect();
        for space in pages {
            self.prefetch_page(space);
        }
        for tab in tabs {
            self.prefetch_section(tab);
        }
    }

    /// Readies page `space` of the open section.
    pub fn prefetch_page(&mut self, space: ExGuid) {
        let Some(session) = &self.session else {
            return;
        };
        let key = session.key();
        if space == session.space || !self.prefetch.ask((key.clone(), Some(space))) {
            return;
        }
        let read = session.reader(space);
        let _ = self
            .prefetch
            .jobs
            .send(Job::Page((key, space), Box::new(read)));
    }

    /// Readies section tab `tab` of the open section's folder.
    pub fn prefetch_section(&mut self, tab: usize) {
        let Some(session) = &self.session else {
            return;
        };
        let path = session.tabs[tab].path.clone();
        let key = session.library.key(&path);
        if tab == session.tab || !self.prefetch.ask((key.clone(), None)) {
            return;
        }
        let _ = self.prefetch.jobs.send(Job::Section {
            library: Arc::clone(&session.library),
            path,
            last: self.last_pages.get(&key).copied(),
            notify: Box::new(notify(self.proxy.clone())),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_least_recent_go_first_past_the_count() {
        let mut recent = Recent::new(2);
        assert!(recent.put("a", 1).is_empty());
        assert!(recent.put("b", 2).is_empty());
        // Putting again makes it the most recent and gives back what it held.
        assert_eq!(recent.put("a", 3), vec![1]);
        assert_eq!(recent.put("c", 4), vec![2]);
        assert!(!recent.contains(&"b"));
        assert_eq!(recent.take(&"a"), Some(3));
        assert_eq!(recent.take(&"a"), None);
        assert_eq!(
            recent.values_mut().map(|value| *value).collect::<Vec<_>>(),
            [4]
        );
    }

    #[test]
    fn the_least_recent_go_first_past_the_budget() {
        let mut recent = Recent::new(8);
        for (key, bytes) in [("a", 40), ("b", 30), ("c", 20), ("d", 10)] {
            recent.put(key, bytes);
        }
        // Most recent first: d, c, b fit in 60; a does not.
        assert_eq!(recent.trim(60, |bytes| *bytes), vec![40]);
        assert_eq!(recent.trim(30, |bytes| *bytes), vec![30]);
        assert!(recent.contains(&"c") && recent.contains(&"d"));
        // One entry over the budget on its own goes too.
        assert_eq!(recent.trim(5, |bytes| *bytes), vec![10, 20]);
    }
}
