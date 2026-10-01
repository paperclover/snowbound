//! OneNote 2010's task panes beside the page: Page Search Results (Alt+O), listing the
//! search's results sorted and grouped, the Tags Summary, listing tagged paragraphs, and
//! Spelling (F7), walking the page's marked words.

use crate::search::{Scope, marked};
use crate::{State, Theme, art, page, platform};
use canvas::gpu::tag_sources;
use canvas::outline::TagIcon;
use canvas::search::{Found, Tagged};
use std::{error::Error, sync::atomic::Ordering};
use ui::{Anchor, Axis, Flags, Id, Spec, Ui, fill, fit, px};
use winit::keyboard::NamedKey;

/// The pane's width, and its lists' rows and group headings.
const WIDTH: f32 = 260.0;
const RESULT: f32 = 40.0;
const TAGGED: f32 = 24.0;
const HEADING: f32 = 22.0;
/// A control's width inside the pane.
const INSIDE: f32 = WIDTH - 2.0 * PAD;
const PAD: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pane {
    Search {
        sort: Sort,
        descending: bool,
    },
    Tags {
        group: Group,
        unchecked: bool,
        scope: TagScope,
    },
    /// The word is `State::correction`; `chosen` is the suggestion Change takes.
    Spelling {
        chosen: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Section,
    Title,
    Date,
}

impl Sort {
    const ALL: [Sort; 3] = [Sort::Section, Sort::Title, Sort::Date];

    fn name(self) -> &'static str {
        match self {
            Sort::Section => "Sort by Section",
            Sort::Title => "Sort by Title",
            Sort::Date => "Sort by Date Modified",
        }
    }
}

/// What the Tags Summary groups its paragraphs by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Tag,
    Section,
    Title,
    Date,
    Text,
}

impl Group {
    const ALL: [Group; 5] = [
        Group::Tag,
        Group::Section,
        Group::Title,
        Group::Date,
        Group::Text,
    ];

    fn name(self) -> &'static str {
        match self {
            Group::Tag => "Tag name",
            Group::Section => "Section",
            Group::Title => "Title",
            Group::Date => "Date",
            Group::Text => "Note text",
        }
    }
}

/// Where the Tags Summary looks: part of the notebooks, or tags applied in a span of days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagScope {
    PageGroup,
    Section,
    SectionGroup,
    Notebook,
    All,
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    Older,
}

impl TagScope {
    const ALL: [TagScope; 10] = [
        TagScope::PageGroup,
        TagScope::Section,
        TagScope::SectionGroup,
        TagScope::Notebook,
        TagScope::All,
        TagScope::Today,
        TagScope::Yesterday,
        TagScope::ThisWeek,
        TagScope::LastWeek,
        TagScope::Older,
    ];

    /// Its name, which also heads the paragraphs tagged in its span when grouped by date.
    fn name(self) -> &'static str {
        match self {
            TagScope::PageGroup => "This page group",
            TagScope::Section => "This section",
            TagScope::SectionGroup => "This section group",
            TagScope::Notebook => "This notebook",
            TagScope::All => "All notebooks",
            TagScope::Today => "Today's notes",
            TagScope::Yesterday => "Yesterday's notes",
            TagScope::ThisWeek => "This week's notes",
            TagScope::LastWeek => "Last week's notes",
            TagScope::Older => "Older notes",
        }
    }

    /// Whether a tag applied on local day `day` falls in the span, `today` being today.
    fn spans(self, day: i64, today: i64) -> bool {
        // OneNote's weeks start on Sunday; day 0 was a Thursday.
        let week = today - (today + 4).rem_euclid(7);
        match self {
            TagScope::Today => day >= today,
            TagScope::Yesterday => day == today - 1,
            TagScope::ThisWeek => day >= week,
            TagScope::LastWeek => (week - 7..week).contains(&day),
            TagScope::Older => day < week - 7,
            _ => true,
        }
    }

    /// The span heading a tag applied on `day` when grouped by date.
    fn of_day(day: i64, today: i64) -> TagScope {
        [
            TagScope::Today,
            TagScope::Yesterday,
            TagScope::ThisWeek,
            TagScope::LastWeek,
        ]
        .into_iter()
        .find(|span| span.spans(day, today))
        .unwrap_or(TagScope::Older)
    }
}

/// The local day, counted from 1970, of Time32 `time`.
fn local_day(time: u64) -> i64 {
    let unix = time as i64 + 315_532_800;
    // SAFETY: `localtime_r` writes only the `tm` it is given.
    #[cfg(unix)]
    let offset = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&unix, &mut tm);
        tm.tm_gmtoff as i64
    };
    #[cfg(any(windows, target_arch = "wasm32"))]
    let offset = crate::platform::utc_offset(unix);
    (unix + offset).div_euclid(86_400)
}

/// Day `day` counted from 1970 as a year, month and day.
fn civil(day: i64) -> (i64, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Rows under group headings, each heading above the row starting its group.
struct Grouped {
    count: usize,
    /// The rows starting groups, in order.
    starts: Vec<usize>,
}

impl ui::Rows for Grouped {
    fn count(&self) -> usize {
        self.count
    }

    fn key(&self, index: usize) -> u64 {
        index as u64
    }

    fn find(&self, key: u64) -> Option<usize> {
        ((key as usize) < self.count).then_some(key as usize)
    }

    fn space_before(&self, index: usize) -> f32 {
        self.starts.partition_point(|start| *start <= index) as f32 * HEADING
    }
}

/// `items` in `order`, and each group's first row with its heading.
fn grouped(order: &[usize], heading: impl Fn(usize) -> String) -> (Grouped, Vec<String>) {
    let mut starts = Vec::new();
    let mut headings: Vec<String> = Vec::new();
    for (row, &item) in order.iter().enumerate() {
        let name = heading(item);
        if headings.last() != Some(&name) {
            starts.push(row);
            headings.push(name);
        }
    }
    (
        Grouped {
            count: order.len(),
            starts,
        },
        headings,
    )
}

/// A section's name from its `Library::key`.
fn section_name(key: &str) -> String {
    let path = key.split_once('\n').map_or(key, |(_, path)| path);
    crate::library::section_name(path, &None)
}

fn shown_title(title: &str) -> &str {
    if title.trim().is_empty() {
        "Untitled page"
    } else {
        title
    }
}

/// The search's results in `sort`'s order, and their groups: sections, first letters of
/// titles, or months.
fn arrange(found: &[Found], sort: Sort, descending: bool) -> (Vec<usize>, Grouped, Vec<String>) {
    let mut order: Vec<usize> = (0..found.len()).collect();
    match sort {
        Sort::Section => order.sort_by_cached_key(|&at| {
            let found = &found[at];
            (
                section_name(&found.section).to_lowercase(),
                found.section.clone(),
                found.order,
            )
        }),
        Sort::Title => {
            order.sort_by_cached_key(|&at| shown_title(&found[at].title).to_lowercase());
        }
        Sort::Date => order.sort_by_key(|&at| found[at].modified),
    }
    if descending {
        order.reverse();
    }
    let (rows, headings) = grouped(&order, |at| {
        let found = &found[at];
        match sort {
            Sort::Section => section_name(&found.section),
            Sort::Title => shown_title(&found.title)
                .chars()
                .next()
                .map_or_else(String::new, |first| first.to_uppercase().collect()),
            Sort::Date => {
                let (year, month, _) = civil(local_day(found.modified));
                format!("{}, {year}", MONTHS[month as usize - 1])
            }
        }
    });
    (order, rows, headings)
}

/// Tagged paragraphs in `group`'s order and their groups, each group's paragraphs in page
/// order.
fn arrange_tags(tagged: &[Tagged], group: Group) -> (Vec<usize>, Grouped, Vec<String>) {
    let today = local_day(crate::search::now());
    let heading = |at: usize| -> String {
        let tagged = &tagged[at];
        match group {
            Group::Tag => tagged.name.clone(),
            Group::Section => section_name(&tagged.section),
            Group::Title => shown_title(&tagged.title).to_owned(),
            Group::Date => TagScope::of_day(local_day(tagged.created), today)
                .name()
                .to_owned(),
            Group::Text => Group::Text.name().to_owned(),
        }
    };
    let mut order: Vec<usize> = (0..tagged.len()).collect();
    match group {
        Group::Date => order.sort_by_key(|&at| {
            TagScope::ALL
                .iter()
                .position(|span| *span == TagScope::of_day(local_day(tagged[at].created), today))
        }),
        Group::Text => order.sort_by_cached_key(|&at| tagged[at].text.to_lowercase()),
        _ => order.sort_by_cached_key(|&at| heading(at).to_lowercase()),
    }
    let (rows, headings) = grouped(&order, heading);
    (order, rows, headings)
}

/// The field of the Search Results pane.
pub(crate) fn field() -> Id {
    Id::ROOT.child("pane-field")
}

fn popup(name: &str) -> Id {
    Id::ROOT.child(("pane popup", name))
}

/// A combo box `label`led, `width` wide, showing `current` of `names`, returning the one
/// chosen.
fn choice(
    ui: &mut Ui,
    name: &str,
    label: &str,
    current: usize,
    names: &[&str],
    width: f32,
) -> Option<usize> {
    let combo = ui.id(name);
    ui::shell::combo(ui, name, label, names[current], width, popup(name), true);
    let items: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(index, text)| ui::popup::Item {
            text,
            checked: Some(index == current),
            current: index == current,
            ..Default::default()
        })
        .collect();
    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
    ui::popup::menu(ui, popup(name), anchor, &items, None)
}

fn label(ui: &mut Ui, theme: &Theme, part: &str, text: &str) {
    ui.leaf(
        part,
        Spec {
            size: [fill(), px(20.0)],
            text: Some(text),
            color: Some(theme.text_dim),
            ..Spec::default()
        },
    );
}

/// A group's heading, floating above the row starting it.
fn heading(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.leaf(
        "heading",
        Spec {
            flags: Flags::FLOAT,
            size: [fill(), px(HEADING)],
            position: [0.0, -HEADING],
            text: Some(text),
            bold: true,
            color: Some(theme.text),
            pad: [4.0, 0.0],
            ..Spec::default()
        },
    );
}

impl State {
    /// Opens the Search Results pane on the search, as Alt+O does.
    pub(crate) fn open_search_pane(&mut self) {
        if !matches!(self.search.pane, Some(Pane::Search { .. })) {
            self.search.pane = Some(Pane::Search {
                sort: Sort::Date,
                descending: true,
            });
        }
        self.search.open = false;
        self.search.finding = false;
        self.search.claim = true;
        self.ui.close_popup(crate::search::results());
        self.sync_index(true, Vec::new());
    }

    /// Opens or closes a pane, as its command does.
    pub(crate) fn toggle_pane(&mut self, tags: bool) {
        match self.search.pane {
            Some(Pane::Tags { .. }) if tags => self.close_pane(),
            Some(Pane::Search { .. }) if !tags => self.close_pane(),
            _ if tags => {
                self.close_pane();
                self.search.pane = Some(Pane::Tags {
                    group: Group::Tag,
                    unchecked: false,
                    scope: TagScope::Notebook,
                });
                self.sync_index(true, Vec::new());
            }
            _ => self.open_search_pane(),
        }
    }

    /// Closes the pane; the Search Results pane takes its search, and the marks, with it.
    fn close_pane(&mut self) {
        self.correction = None;
        if matches!(self.search.pane.take(), Some(Pane::Search { .. })) {
            self.end_search();
        }
    }

    /// Opens the Spelling pane on the next marked word, as F7 does.
    pub(crate) fn open_spelling_pane(&mut self) {
        if !matches!(self.search.pane, Some(Pane::Spelling { .. })) {
            self.close_pane();
        }
        self.next_word();
    }

    /// Shows the next marked word in the Spelling pane; past the last, the check is complete.
    fn next_word(&mut self) {
        match self.view.next_correction() {
            Ok(Some((response, correction))) => {
                self.respond(response);
                self.correction = Some(correction);
                self.search.pane = Some(Pane::Spelling { chosen: 0 });
            }
            Ok(None) => {
                self.close_pane();
                platform::alert("The spelling check is complete.", "");
            }
            Err(error) => eprintln!("{error}"),
        }
    }

    fn spelling_pane(&mut self, theme: &Theme, mut chosen: usize) {
        let Some(correction) = &self.correction else {
            return;
        };
        let ui = &mut self.ui;
        label(
            ui,
            theme,
            "kind",
            if correction.repeated {
                "Repeated word:"
            } else {
                "Current spelling:"
            },
        );
        ui.leaf(
            "word",
            Spec {
                size: [fill(), px(ui::shell::TOOL)],
                text: Some(&correction.word),
                bold: true,
                fill: Some(theme.base),
                border: Some(theme.chip),
                radius: 4.0,
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
        ui.open(
            "actions",
            Spec {
                size: [fill(), px(theme.font_size * 2.0)],
                gap: 4.0,
                ..Spec::default()
            },
        );
        let [first, second] = if correction.repeated {
            ["Delete", "Ignore"]
        } else {
            ["Ignore", "Add to Dictionary"]
        };
        let pressed = [first, second].map(|text| ui::button(ui, text, text).clicked);
        ui.close();
        let mut change = false;
        if !correction.repeated {
            label(ui, theme, "suggestions label", "Suggestions:");
            ui.open(
                "suggestions",
                Spec {
                    axis: Axis::Y,
                    size: [fill(), px(TAGGED * 5.0 + 2.0)],
                    fill: Some(theme.base),
                    border: Some(theme.chip),
                    radius: 4.0,
                    pad: [1.0, 1.0],
                    role: Some(accesskit::Role::List),
                    ..Spec::default()
                },
            );
            for (index, suggestion) in correction.suggestions.iter().enumerate() {
                let row = ui.leaf(
                    index,
                    Spec {
                        flags: Flags::CLICKABLE,
                        size: [fill(), px(TAGGED)],
                        text: Some(suggestion),
                        fill: (index == chosen).then(|| theme.hover()),
                        hover_fill: Some(theme.hover()),
                        pad: [6.0, 0.0],
                        role: Some(accesskit::Role::ListItem),
                        ..Spec::default()
                    },
                );
                if row.clicked {
                    chosen = index;
                    change = row.unit != draw::edit::SelectionUnit::Grapheme;
                }
            }
            if correction.suggestions.is_empty() {
                label(ui, theme, "none", "(No Spelling Suggestions)");
            }
            ui.close();
            if !correction.suggestions.is_empty() {
                change |= ui::button(ui, "change", "Change").clicked;
            }
        }
        self.search.pane = Some(Pane::Spelling { chosen });
        let Some(correction) = self
            .correction
            .take_if(|_| change || pressed.contains(&true))
        else {
            return;
        };
        let spelling = self.view.spelling.clone();
        let text = match (correction.repeated, pressed) {
            (_, _) if change => correction.suggestions.get(chosen).map(String::as_str),
            (true, [true, _]) => Some(""),
            (false, [false, true]) => {
                spelling.inspect(|spelling| spelling.learn(&correction.word));
                None
            }
            _ => {
                spelling.inspect(|spelling| spelling.ignore(&correction.word));
                None
            }
        };
        if let Some(text) = text {
            match self.view.correct(&correction, text) {
                Ok(response) => self.respond(response),
                Err(error) => eprintln!("{error}"),
            }
        }
        self.next_word();
    }

    /// The open pane at the window's right, easing open and closed.
    pub(crate) fn task_pane(&mut self, theme: &Theme) {
        let id = self.ui.id("task pane");
        let width = self.ui.animate(
            id,
            if self.search.pane.is_some() {
                WIDTH
            } else {
                0.0
            },
        );
        self.ui.open(
            "task pane",
            Spec {
                flags: Flags::CLIP,
                size: [px(width), fill()],
                fill: Some(theme.strip),
                ..Spec::default()
            },
        );
        // The content keeps its width as the pane eases, so its labels never reflow.
        self.ui.open(
            "content",
            Spec {
                axis: Axis::Y,
                size: [px(WIDTH), fill()],
                pad: [PAD, 4.0],
                gap: 4.0,
                ..Spec::default()
            },
        );
        if let Some(pane) = self.search.pane {
            self.ui.open(
                "header",
                Spec {
                    size: [fill(), px(ui::shell::TOOL)],
                    ..Spec::default()
                },
            );
            self.ui.leaf(
                "title",
                Spec {
                    size: [fill(), px(ui::shell::TOOL)],
                    text: Some(match pane {
                        Pane::Search { .. } => "Search Results",
                        Pane::Tags { .. } => "Tags Summary",
                        Pane::Spelling { .. } => "Spelling",
                    }),
                    bold: true,
                    ..Spec::default()
                },
            );
            if ui::shell::tool_button(&mut self.ui, "close", art::CLOSE, theme.text_dim, None)
                .clicked
            {
                self.close_pane();
            }
            let close = self.ui.id("close");
            crate::name(&mut self.ui, close, "Close");
            self.ui.close();
            match self.search.pane {
                Some(Pane::Search { sort, descending }) => {
                    if let Err(error) = self.search_pane(theme, sort, descending) {
                        eprintln!("{error}");
                    }
                }
                Some(Pane::Tags {
                    group,
                    unchecked,
                    scope,
                }) => self.tags_pane(theme, group, unchecked, scope),
                Some(Pane::Spelling { chosen }) => self.spelling_pane(theme, chosen),
                None => {}
            }
        }
        self.ui.close();
        self.ui.close();
    }

    fn search_pane(
        &mut self,
        theme: &Theme,
        mut sort: Sort,
        mut descending: bool,
    ) -> Result<(), Box<dyn Error>> {
        self.refresh_results()?;
        let keys = ui::popup::navigation(
            &mut self.ui,
            &[field()],
            &[
                NamedKey::ArrowUp,
                NamedKey::ArrowDown,
                NamedKey::PageUp,
                NamedKey::PageDown,
                NamedKey::Enter,
                NamedKey::Escape,
            ],
        );
        if keys.contains(&NamedKey::Escape) {
            self.ui.set_focus(Some(page()));
        }
        let border = if self.ui.focused() == Some(field()) {
            theme.accent
        } else {
            theme.chip
        };
        ui::text_field(
            &mut self.ui,
            field(),
            &mut self.search.query,
            "Search",
            Spec {
                size: [fill(), px(ui::shell::TOOL)],
                fill: Some(theme.base),
                border: Some(border),
                radius: 4.0,
                pad: [6.0, 0.0],
                role: Some(accesskit::Role::SearchInput),
                ..Spec::default()
            },
        );
        crate::name(&mut self.ui, field(), "Search");
        if std::mem::take(&mut self.search.claim) {
            self.ui.set_focus(Some(field()));
        }
        let scopes = Scope::ALL.map(|scope| format!("Search {}", scope.name()));
        let names = scopes.each_ref().map(String::as_str);
        let current = Scope::ALL
            .iter()
            .position(|scope| *scope == self.search.scope)
            .unwrap_or_default();
        if let Some(index) = choice(
            &mut self.ui,
            "scope",
            "Search scope",
            current,
            &names,
            INSIDE,
        ) {
            self.search.scope = Scope::ALL[index];
        }
        self.ui.open(
            "sort",
            Spec {
                size: [fill(), px(ui::shell::TOOL)],
                gap: 2.0,
                ..Spec::default()
            },
        );
        let names = Sort::ALL.map(Sort::name);
        let current = Sort::ALL
            .iter()
            .position(|listed| *listed == sort)
            .unwrap_or_default();
        let width = INSIDE - ui::shell::TOOL - 2.0;
        if let Some(index) = choice(&mut self.ui, "by", "Sort by", current, &names, width) {
            sort = Sort::ALL[index];
            // Dates start newest first, names from the start of the alphabet.
            descending = sort == Sort::Date;
        }
        let arrow = if descending {
            ui::shell::CHEVRON
        } else {
            art::CHEVRON_UP
        };
        if ui::shell::tool_button(&mut self.ui, "direction", arrow, theme.text, None).clicked {
            descending = !descending;
        }
        let tip = if descending {
            "Sort ascending"
        } else {
            "Sort descending"
        };
        ui::popup::tooltip(&mut self.ui, tip, "", None);
        self.ui.close();
        self.search.pane = Some(Pane::Search { sort, descending });
        let found = &self.search.found;
        if found.is_empty() {
            let status = if self.search.query.trim().is_empty() {
                ""
            } else if self.search.busy.load(Ordering::Relaxed) {
                "Searching…"
            } else {
                "No matches"
            };
            label(&mut self.ui, theme, "status", status);
            return Ok(());
        }
        let (order, rows, headings) = arrange(found, sort, descending);
        let dates: Vec<String> = order
            .iter()
            .map(|&at| {
                let (year, month, day) = civil(local_day(found[at].modified));
                format!("{month}/{day}/{year}")
            })
            .collect();
        let before = self.search.selected;
        let list = Id::ROOT.child("pane results");
        let clicked = ui::list(
            &mut self.ui,
            list,
            Spec {
                size: [fill(), fill()],
                fill: Some(theme.base),
                border: Some(theme.chip),
                radius: 4.0,
                role: Some(accesskit::Role::ListBox),
                ..Spec::default()
            },
            ui::List {
                rows: &rows,
                row: RESULT,
                keys: &keys,
                hover_selects: false,
            },
            &mut self.search.selected,
            |ui, row| {
                if let Some(node) = ui.access(list.child(row.key)) {
                    node.set_role(accesskit::Role::ListBoxOption);
                    node.set_selected(row.selected);
                }
                if let Ok(group) = rows.starts.binary_search(&row.index) {
                    heading(ui, theme, &headings[group]);
                }
                result_row(
                    ui,
                    theme,
                    &found[order[row.index]],
                    &dates[row.index],
                    row.selected,
                );
            },
        );
        if let Some(key) = self.search.selected
            && let Some(node) = self.ui.access(field())
        {
            node.set_active_descendant(list.child(key).node());
        }
        let moved = before != self.search.selected && !keys.contains(&NamedKey::Enter);
        let chosen = clicked.or_else(|| {
            self.search
                .selected
                .map(|key| key as usize)
                .filter(|_| moved || keys.contains(&NamedKey::Enter))
        });
        if let Some(row) = chosen {
            self.search.selected = Some(row as u64);
            let found = &self.search.found[order[row]];
            self.reveal(found.section.clone(), found.space, None)?;
            if clicked.is_some() || keys.contains(&NamedKey::Enter) {
                self.ui.set_focus(Some(page()));
            }
        }
        Ok(())
    }

    fn tags_pane(
        &mut self,
        theme: &Theme,
        mut group: Group,
        mut unchecked: bool,
        mut scope: TagScope,
    ) {
        let status = if self.search.busy.load(Ordering::Relaxed) {
            "Searching…"
        } else {
            "Search completed"
        };
        label(&mut self.ui, theme, "status", status);
        label(&mut self.ui, theme, "group by", "Group tags by:");
        let names = Group::ALL.map(Group::name);
        let current = Group::ALL
            .iter()
            .position(|listed| *listed == group)
            .unwrap_or_default();
        if let Some(index) = choice(
            &mut self.ui,
            "group",
            "Group tags by",
            current,
            &names,
            INSIDE,
        ) {
            group = Group::ALL[index];
        }
        if ui::check_box(
            &mut self.ui,
            "unchecked",
            "Show only unchecked items",
            unchecked,
        )
        .clicked
        {
            unchecked = !unchecked;
        }
        let tagged = self.tagged(scope, unchecked);
        let (order, rows, headings) = arrange_tags(&tagged, group);
        let mut selected = None;
        let list = Id::ROOT.child("pane tags");
        let clicked = ui::list(
            &mut self.ui,
            list,
            Spec {
                size: [fill(), fill()],
                fill: Some(theme.base),
                border: Some(theme.chip),
                radius: 4.0,
                role: Some(accesskit::Role::List),
                ..Spec::default()
            },
            ui::List {
                rows: &rows,
                row: TAGGED,
                keys: &[],
                hover_selects: false,
            },
            &mut selected,
            |ui, row| {
                if let Some(node) = ui.access(list.child(row.key)) {
                    node.set_role(accesskit::Role::ListItem);
                }
                if let Ok(at) = rows.starts.binary_search(&row.index) {
                    heading(ui, theme, &headings[at]);
                }
                tagged_row(ui, theme, &tagged[order[row.index]]);
            },
        );
        label(&mut self.ui, theme, "search", "Search:");
        let names = TagScope::ALL.map(TagScope::name);
        let current = TagScope::ALL
            .iter()
            .position(|listed| *listed == scope)
            .unwrap_or_default();
        if let Some(index) = choice(&mut self.ui, "tag scope", "Search", current, &names, INSIDE) {
            scope = TagScope::ALL[index];
        }
        self.search.pane = Some(Pane::Tags {
            group,
            unchecked,
            scope,
        });
        if let Some(row) = clicked {
            let tagged = &tagged[order[row]];
            if let Err(error) =
                self.reveal(tagged.section.clone(), tagged.space, Some(tagged.paragraph))
            {
                eprintln!("{error}");
            }
            self.ui.set_focus(Some(page()));
        }
    }

    /// The tagged paragraphs `scope` takes in, only those not checked off when `unchecked`.
    fn tagged(&self, scope: TagScope, unchecked: bool) -> Vec<Tagged> {
        let session = self.session.as_ref();
        let open = session.map(crate::Session::key);
        // The open page with its subpages, or the page it is a subpage of with its others.
        let group: Vec<_> = session
            .and_then(|session| {
                let pages = &session.pages;
                let at = pages
                    .iter()
                    .position(|(space, ..)| *space == session.space)?;
                let base = pages.iter().map(|(.., level)| *level).min()?;
                let start = pages[..=at]
                    .iter()
                    .rposition(|(.., level)| *level == base)?;
                let end = pages[start + 1..]
                    .iter()
                    .position(|(.., level)| *level == base)
                    .map_or(pages.len(), |after| start + 1 + after);
                Some(pages[start..end].iter().map(|(space, ..)| *space).collect())
            })
            .unwrap_or_default();
        let place = match scope {
            TagScope::Section => Scope::Section,
            TagScope::SectionGroup => Scope::Group,
            TagScope::Notebook => Scope::Notebook,
            _ => Scope::All,
        };
        let today = local_day(crate::search::now());
        let index = self
            .search
            .index
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        index
            .tagged(|entry| match scope {
                TagScope::PageGroup => {
                    Some(&entry.section) == open.as_ref() && group.contains(&entry.space)
                }
                _ => self.in_scope(place, &entry.section),
            })
            .into_iter()
            .filter(|tagged| scope.spans(local_day(tagged.created), today))
            .filter(|tagged| !(unchecked && tagged.checked))
            .collect()
    }
}

/// A result: its page's icon, title and date, and the snippet around its first match.
fn result_row(ui: &mut Ui, theme: &Theme, found: &Found, date: &str, selected: bool) {
    ui.open(
        "result",
        Spec {
            axis: Axis::Y,
            size: [fill(), fill()],
            fill: selected.then(|| theme.hover()),
            hover_fill: Some(theme.hover()),
            radius: 4.0,
            pad: [4.0, 2.0],
            ..Spec::default()
        },
    );
    ui.open(
        "top",
        Spec {
            size: [fill(), px(18.0)],
            gap: 4.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "icon",
        Spec {
            size: [px(16.0), px(18.0)],
            icon: Some(art::PAGE),
            color: Some(theme.text_dim),
            ..Spec::default()
        },
    );
    marked(
        ui,
        "title",
        shown_title(&found.title),
        &found.title_hits,
        Spec {
            size: [fill(), px(18.0)],
            color: Some(theme.text),
            ..Spec::default()
        },
    );
    ui.leaf(
        "date",
        Spec {
            size: [fit(), px(18.0)],
            text: Some(date),
            color: Some(theme.text_dim),
            ..Spec::default()
        },
    );
    ui.close();
    marked(
        ui,
        "snippet",
        &found.snippet,
        &found.snippet_hits,
        Spec {
            size: [fill(), px(18.0)],
            color: Some(theme.text_dim),
            pad: [20.0, 0.0],
            ..Spec::default()
        },
    );
    ui.close();
}

/// A tagged paragraph: its tag's symbol and its text.
fn tagged_row(ui: &mut Ui, theme: &Theme, tagged: &Tagged) {
    ui.open(
        "tagged",
        Spec {
            size: [fill(), fill()],
            hover_fill: Some(theme.hover()),
            radius: 4.0,
            pad: [4.0, 0.0],
            gap: 6.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "icon",
        Spec {
            size: [px(16.0), px(TAGGED)],
            icon: TagIcon::of(tagged.shape, tagged.checked).map(tag_sources),
            color: Some([1.0; 4]),
            ..Spec::default()
        },
    );
    ui.leaf(
        "text",
        Spec {
            size: [fill(), px(TAGGED)],
            text: Some(&tagged.text),
            overflow: ui::Overflow::Ellipsis,
            ..Spec::default()
        },
    );
    ui.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_read_as_the_calendar_and_weeks_start_on_sunday() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_725), (2026, 9, 29));
        assert_eq!(civil(-1), (1969, 12, 31));
        // Tuesday, September 29, 2026: its week began on Sunday the 27th.
        let today = 20_725;
        assert_eq!(TagScope::of_day(today, today), TagScope::Today);
        assert_eq!(TagScope::of_day(today - 1, today), TagScope::Yesterday);
        assert_eq!(TagScope::of_day(today - 2, today), TagScope::ThisWeek);
        assert_eq!(TagScope::of_day(today - 3, today), TagScope::LastWeek);
        assert_eq!(TagScope::of_day(today - 9, today), TagScope::LastWeek);
        assert_eq!(TagScope::of_day(today - 10, today), TagScope::Older);
        assert!(TagScope::ThisWeek.spans(today, today));
    }
}
