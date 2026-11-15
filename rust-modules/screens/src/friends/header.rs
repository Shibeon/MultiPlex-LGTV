//! The Friends page's head: the page's title block ([`page_header::draw_title_noted`] at the shared
//! [`page_header::TOP`]: "Friends" and the quiet lead line as its note), and under it, at [`ROW_Y`]
//! (the Library's chip row), the one row of controls, the shared [`PageHeader`]: the friend filter
//! on the left ("All" first, then the friends of the feed, the most active first; the chosen one
//! wears the selected plate, as the Library's current library does) and, on the right, the account's
//! "Share what I watch" toggle as a filled chip (the two-people mark while shared, the lock while
//! private, the state as its value).
//!
//! This file DECLARES the row; its layout, paint, motion and focus group are the ui's, the same ones
//! the Library and My list wear.
use plx_machine::machine::{EntryId, FocusKey, GroupId, Measure};
use plx_ui::icons::Icon;
use plx_ui::page_header::{self, Chip, HeaderFocus, HeaderItem, HeaderRow, HeaderSwap, PageHeader, PageHeaderSpec};
use plx_ui::widgets::{CtlPop, TabStrip};
use plx_ui::Painter;
use std::ffi::CString;

/// The title block's top: the shared row's [`page_header::TOP`], where the document starts.
pub const HEADER_TOP: f32 = page_header::TOP;
/// The row of controls: one title block and its air under [`HEADER_TOP`] (the Library's `ROW_Y`).
pub const ROW_Y: f32 = HEADER_TOP + page_header::TITLE_H + page_header::TITLE_ROW_GAP;
/// The head's height in the document: the title, the row and the air under it, so the first shelf's
/// heading stands where the Library's grid does under its chips.
pub const BLOCK_H: f32 = ROW_Y + page_header::BLOCK_H;
/// How many friends the filter row offers after "All" (the most active ones). The row is one line
/// of plates on a 1920 frame; the feed's other friends are still in the feed under "All".
pub const FILTER_FRIENDS: usize = 6;

/// The share toggle as the row shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShareFace {
    /// Not known yet, or being saved: "…", and the press does nothing.
    Pending,
    On,
    Off,
    /// The read or the last save failed.
    Failed,
}

/// The row laid out, and which laid-out item each stop is.
pub struct Row {
    row: HeaderRow,
    title: CString,
    lead: CString,
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Row {
    /// The row for the filter's `names` ("All" first is added here), the chosen one at `selected`
    /// (0: All), and the toggle's `share` face. Filter chip `i` is stop `chip_elem(i)`; the toggle
    /// is `share_elem`.
    pub fn new(measure: &dyn Measure, names: &[String], selected: usize, share: ShareFace, chip_elem: impl Fn(usize) -> u32, share_elem: u32) -> Self {
        use plx_platform::i18n::msg;
        let left: Vec<HeaderItem> = std::iter::once(msg::friends_filter_all().to_string())
            .chain(names.iter().cloned())
            .enumerate()
            .map(|(i, label)| HeaderItem { label, selected_plate: i == selected, focusable: true, ..Default::default() })
            .collect();
        let value = match share {
            ShareFace::Pending => msg::friends_share_pending(),
            ShareFace::On => msg::friends_share_on(),
            ShareFace::Off => msg::friends_share_off(),
            ShareFace::Failed => msg::friends_share_failed(),
        };
        let toggle = HeaderItem {
            label: msg::friends_share_label().to_string(),
            value: Some(value.to_string()),
            glyph: Some(if share == ShareFace::Off { Icon::Lock } else { Icon::Crowd }),
            focusable: true,
            dimmed: share == ShareFace::Pending,
            chip: Some(Chip::Filled),
            ..Default::default()
        };
        let chips = left.len();
        let page = PageHeader::new(PageHeaderSpec { left, right: vec![toggle], ..Default::default() }, &[], measure);
        let layout = page.layout_at(plx_ui::consts::SCR_W, measure, ROW_Y);
        let mut stops: Vec<(u32, usize)> = (0..chips).map(|i| (chip_elem(i), i)).collect();
        stops.push((share_elem, layout.items.len().saturating_sub(1)));
        Self { row: HeaderRow::new(page, layout, stops), title: cstring(msg::friends_page_title()), lead: cstring(msg::friends_page_lead()) }
    }

    /// The row as one focus group (`id`).
    pub fn focus(&self, entry: EntryId, id: GroupId) -> HeaderFocus<'_> {
        self.row.focus(entry, id, (0.0, 0.0), plx_ui::Rect::FULL)
    }

    /// The plate of stop `elem`, in document space.
    pub fn plate(&self, elem: u32) -> Option<plx_ui::Rect> {
        self.row.plate(elem)
    }

    /// One frame of the row's motion ([`HeaderRow::step`]).
    pub fn step(&self, entry: EntryId, current: Option<FocusKey<u32>>, capsules: &mut TabStrip, pop: &mut CtlPop<1>, dt: f32) {
        self.row.step(entry, current, capsules, pop, dt);
    }

    /// Carry the selection plate over from the row this one replaces (a filter picked).
    pub fn begin_swap(&self, from: &Row, swap: &mut HeaderSwap, capsules: &mut TabStrip) {
        self.row.reflow_selection(&from.row, capsules);
        swap.begin(&from.row, &self.row);
    }

    /// Paint the head `dy` px from the document origin: the title block with its lead, then the row.
    pub fn draw(&self, p: Painter, dy: f32, capsules: &TabStrip, pop: f32, swap: &HeaderSwap) {
        page_header::draw_title_noted(p, HEADER_TOP + dy, &self.title, None, Some(&self.lead));
        self.row.draw_with(p, -dy, capsules, pop, None, swap);
    }
}
