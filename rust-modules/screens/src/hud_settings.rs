//! **Settings > Playback > Player HUD** (`docs/hud-plugins.md`, "Storage per install mode", "Sideload"):
//! the built-in HUD, the installed packages and the files in the drop folder, on the shared Settings
//! table. A FormTable keyed by package id, not a `PickerKind` (the vocabulary is whatever is installed).
//!
//! **The page owns no store and no IO.** Everything it shows is a [`HudList`] the Bridge sends down the
//! channel its `HudCmd::Open` carried (the page's own data, like the receipts the Playback pickers
//! poll). The finish-time switch uses the shared profile preference write; package operations leave
//! as `AppFx::Huds`. OK on a package picks it
//! (`HudCmd::Select`), OK on a drop file asks, through the shared two-answer prompt
//! ([`DecisionPrompt`]), whether to install an unreviewed file, and the *Remove* row of an installed package
//! deletes it. The list is read again each time the page opens.
//!
//! **Install from address.** One more row, *Install from address*: OK raises the television keyboard (the Search page's
//! mechanism: a `Delivery::Keyboard`, then the OS's `InputKind::Text` edits into a [`TextBuffer`]) and the row's detail
//! line shows what was typed. OK on the keyboard parses it (`hudpacks::address::parse`); a refusal is the line under the
//! title, a good address leaves as `HudCmd::Fetch`. The store fetches, validates and *offers* the package: the next
//! listing carries it as `HudList::offer` and the page asks the same two-answer prompt a drop file gets, with the
//! package's name and version, where it came from and *Unreviewed*, Cancel focused. Confirming sends `InstallOffer`,
//! anything else `DeclineOffer`, which leaves nothing on disk. While the fetch runs the row is dimmed and OK on it does
//! nothing (the Preferences pages' busy idiom).
//!
//! **Fetch a development source again** (PR15, `tools/hud-dev`). A package installed from a development source (an
//! `http://` address on the home network: an author's machine) is listed with that address ([`Entry::dev_source`]).
//! Play on its row sends the same `HudCmd::Fetch` the address row sends, so the author sees the next build through
//! the same offer, prompt and confirmation, and nothing is installed without the OK. It is the remote's own ▶ read by
//! the shared classifier ([`consts::classify_input`]), taken on a fresh press only and only while the page is free
//! (no fetch or package work in flight, no keyboard or prompt up). On any other row Play is left unhandled; no row
//! and no string is added.
//!
//! **What a row says** (Settings v2). A row is its label and, when it cannot be used, the reason as its value
//! ([`Row::disabled`]: the ghost focus pill, `rust-modules/ui/src/CLAUDE.md` rule 5, the disabled paragraph): the package's `name` (the author's text: a
//! server-style label, never measured by the fit report; the app's own HUD is named *Built-in*, which is also its
//! tier, so its row says no tier under it), *Unreviewed* under it for a sideload (a status, so it keeps its sub-line),
//! and, for a package that cannot run, why as the value: a format the app does not read, an app range it is outside
//! of, or "Doesn't fit in {language}" (a compile the television refused in the current language, where
//! classic draws and the selection is kept; the row is DISABLED, so OK on it does nothing). "Installed until restart", when only the volatile directory holds a
//! usable package, joins the sub-line. Everything else a row used to say under its name is the **sentence that follows
//! focus** in the left column ([`FollowCopy`]; [`HudPage::sentence`] is a pure function of the focused row and the
//! listing): the page's own while a row has none, a refusal or fetch notice outranking every row's.
use std::borrow::Cow;
use std::sync::mpsc::{self, Receiver};

use plx_data::hudpacks::address::{self, AddressError};
use plx_data::hudpacks::{is_builtin, DropFile, Entry, FetchError, Notice, OfferView, Refusal, Sha, Trust, Unloadable, BUILTIN_MULTIPLEX};
use plx_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputEvent, InputKind, InputOwner, InstanceId, Key, LogicalState,
    Machine, MachineId, TextEdit,
};
use plx_ui::decision_prompt::{DecisionPrompt, PromptStep};
use plx_ui::follow_copy::FollowCopy;
use plx_ui::text_buffer::TextBuffer;
use plx_ui::form::{Form, FormSection, FormTable, RowKey, RowKind};
use plx_ui::frame::Budget;
use plx_ui::route_screen::RouteLayout;
use plx_ui::screen::{At, Dir, DrawFrame, Enter, Focusable, FocusSource, FocusTarget, GroupSpec, HitSource, Placed, RenderStrategy, Screen, ScreenEvent, Step};
use plx_ui::table::Row;
use plx_ui::table_screen::{Header, TableScreen};
use plx_ui::{consts, theme, Rect};

use super::family::{form_activate, form_focus, form_right_target, InnerHost, SettingsPage, ALERT_GROUP};
use super::registry::{word, AppFx, HudCmd, ALERT, BAND};

/// What the Bridge publishes to the page: the store's listing at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HudList {
    /// The seated profile's preference, or no switch when no profile is seated or the HUD in use declares no
    /// finish-time lane (`Compiled::declares_end_time`): the switch would change nothing it draws.
    pub end_time: Option<bool>,
    /// The built-in first, then one row per installed id.
    pub entries: Vec<Entry>,
    /// The files in the drop folder.
    pub drops: Vec<DropFile>,
    /// The packages whose compile the television refused in the current language.
    pub unfit: Vec<Sha>,
    /// The current language's own name, for "Doesn't fit in {language}".
    pub language: String,
    /// What the last install or removal said, until the page is opened again.
    pub notice: Option<Notice>,
    /// A package fetched from an address, validated and waiting for the viewer's answer: the prompt shows it.
    pub offer: Option<OfferView>,
    /// A fetch from an address is in flight (the row is busy and takes no second request).
    pub fetching: bool,
    /// Any package operation, including a held offer and a confirmed write, excludes removal.
    pub package_busy: bool,
}

/// A row's id: what it IS, so the cursor keeps its place across a re-listing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum HudId {
    EndTime,
    Pack(String),
    Remove(String),
    Drop(Sha),
    /// The *Install from address* row.
    Address,
    /// A file the store could not hash (over the size bound): its position in the folder listing.
    DropFile(usize),
}

#[derive(Clone, Debug)]
enum HudAction {
    EndTime,
    Select(String),
    Remove(String),
    Install(Sha),
    /// Raise the keyboard for an address.
    Address,
    /// A row with nothing to do: a file that cannot be installed and has no hash to name.
    Inert,
}

/// Row keys: a package's position, then the removals and the drop files in their own ranges.
const REMOVE_KEY: u32 = 0x1000;
const DROP_KEY: u32 = 0x2000;
const ADDRESS_KEY: u32 = 0x3000;
const END_TIME_KEY: u32 = 0x5000;

/// What the open prompt asks about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asking {
    /// A file in the drop folder.
    Drop(Sha),
    /// A package fetched from an address and held by the store.
    Offer(Sha),
}

struct State {
    end_time: Option<bool>,
    saving_end_time: bool,
    selected: u32,
    rows: u32,
    notice: u8,
    confirming: bool,
    affirmative: bool,
    alert_scroll: u32,
    /// The address field has the keyboard.
    editing: bool,
    /// A fetch is in flight (the list says so).
    fetching: bool,
    visible: bool,
    pending_select_focus: bool,
    provisional: bool,
}
impl LogicalState for State {
    fn write(&self, c: &mut Canon) {
        c.u32(self.selected).u32(self.rows).u8(self.notice).bool(self.confirming).bool(self.affirmative).u32(self.alert_scroll)
            .bool(self.editing).bool(self.fetching).bool(self.visible).bool(self.pending_select_focus).bool(self.provisional);
        if let Some(on) = self.end_time {
            c.bool(on).bool(self.saving_end_time);
        }
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!(
            "player-hud sel={} rows={} confirm={} editing={} fetching={}",
            self.selected, self.rows, self.confirming, self.editing, self.fetching
        ));
    }
}

pub struct HudPage {
    entry: EntryId,
    title: String,
    parent: String,
    form: FormTable<HudId, HudAction, SettingsPage>,
    state: State,
    list: HudList,
    copy: String,
    /// The sentence each row with one of its own says beside the table while it has focus ([`HudPage::sentence`]).
    says: Says,
    /// The sentence beside the table, which follows the focused row. Decoration: not part of the canonical state.
    follow: FollowCopy,
    /// The list's channel: the sender went to the Bridge with `HudCmd::Open`.
    rx: Receiver<HudList>,
    tx: Option<mpsc::Sender<HudList>>,
    end_time_receipt: Option<Receiver<bool>>,
    /// What the open prompt asks about.
    asking: Option<Asking>,
    /// The offer the prompt was last opened for, so a listing that still carries it after the answer does not reopen it.
    offered: Option<Sha>,
    /// What the viewer has typed into the address row; kept across a refusal so it can be corrected.
    draft: TextBuffer,
    /// Why the last submitted address was refused, until the next edit.
    address_error: Option<AddressError>,
    prompt: DecisionPrompt,
}

/// The crumb over the title: the page this one is pushed from, Video & playback (the Playback page's Player HUD row
/// opens it), as `PickerKind::parent_title` names it for a picker on that page.
fn parent_title() -> &'static str {
    plx_platform::i18n::msg::settings_playback_title()
}

/// The package row's name: a built-in's is the app's own word for it ("MultiPlex", the default, or "Classic"; the one
/// string its row says), any other the author's text.
fn name_of(entry: &Entry) -> String {
    match &entry.meta {
        Some(meta) => meta.name.clone(),
        None if entry.id == BUILTIN_MULTIPLEX => plx_platform::i18n::msg::settings_hud_builtin_multiplex().to_owned(),
        None => plx_platform::i18n::msg::settings_hud_builtin_classic().to_owned(),
    }
}

/// Why `unloadable` says this package cannot run, in the viewer's words.
fn unloadable_text(why: Unloadable) -> &'static str {
    match why {
        Unloadable::NewerFormat => plx_platform::i18n::msg::settings_hud_newer_format(),
        Unloadable::OlderFormat => plx_platform::i18n::msg::settings_hud_older_format(),
        Unloadable::AppRange => plx_platform::i18n::msg::settings_hud_app_range(),
    }
}

/// Why a file in the drop folder cannot be installed now.
fn refusal_text(why: Refusal) -> &'static str {
    match why {
        Refusal::Unloadable(why) => unloadable_text(why),
        Refusal::AlreadyInstalled | Refusal::NotNewer => plx_platform::i18n::msg::settings_hud_already(),
        Refusal::Invalid => plx_platform::i18n::msg::settings_hud_not_a_hud(),
        Refusal::TooLarge => plx_platform::i18n::msg::settings_hud_too_large(),
        Refusal::BuiltinId | Refusal::Changed => plx_platform::i18n::msg::settings_hud_cant_install(),
    }
}

/// Why a fetch from an address brought nothing, in the viewer's words.
fn fetch_text(why: FetchError) -> &'static str {
    match why {
        FetchError::NotFound => plx_platform::i18n::msg::settings_hud_fetch_not_found(),
        FetchError::TooLarge => plx_platform::i18n::msg::settings_hud_too_large(),
        FetchError::Unreachable => plx_platform::i18n::msg::settings_hud_fetch_unreachable(),
        FetchError::Certificate => plx_platform::i18n::msg::settings_hud_fetch_certificate(),
    }
}

/// Why the typed address was refused: the accepted forms, or the one rule a plain `http://` address broke.
fn address_text(why: AddressError) -> &'static str {
    match why {
        AddressError::HttpNotPrivate => plx_platform::i18n::msg::settings_hud_address_lan(),
        _ => plx_platform::i18n::msg::settings_hud_address_form(),
    }
}

fn join(parts: &[String]) -> String {
    parts.join(" \u{b7} ")
}

/// Why a package cannot be used now. The reason is its row's value ([`why_value`]); the sentence under the title says
/// what to do ([`why_sentence`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Why {
    /// The app will not run it: a format or app range it does not accept.
    Format(Unloadable),
    /// The television refused its compile in the current language, so classic draws and the selection is kept.
    Language,
}

/// Why `entry` cannot be used, if it cannot.
fn why_of(entry: &Entry, list: &HudList) -> Option<Why> {
    match entry.loadable {
        Err(why) => Some(Why::Format(why)),
        Ok(()) if entry.sha.is_some_and(|sha| list.unfit.contains(&sha)) => Some(Why::Language),
        Ok(()) => None,
    }
}

/// The reason, as the row's value: short, in the viewer's words.
fn why_value(why: Why, list: &HudList) -> String {
    match why {
        Why::Format(why) => unloadable_text(why).to_owned(),
        Why::Language => plx_platform::i18n::msg::settings_hud_unfit_language(&list.language),
    }
}

/// What the line under the title says while the row of a HUD that cannot be used has focus: why, and what to do.
fn why_sentence(why: Why, list: &HudList) -> String {
    use plx_platform::i18n::msg;
    match why {
        Why::Format(Unloadable::NewerFormat) => msg::settings_hud_newer_detail().to_owned(),
        Why::Format(Unloadable::OlderFormat) => msg::settings_hud_older_detail().to_owned(),
        Why::Format(Unloadable::AppRange) => msg::settings_hud_app_range_detail().to_owned(),
        Why::Language => msg::settings_hud_unfit_detail(&list.language),
    }
}

/// `row` as the row of a HUD that has `why` (or none): the reason as its value. The ghost state, and OK doing nothing,
/// are the section's `.disabled(why.is_some())`: one mechanism ([`plx_ui::form::FormSection::disabled`]).
fn with_why(row: Row, why: Option<Why>, list: &HudList) -> Row {
    match why {
        Some(why) => row.value(why_value(why, list)),
        None => row,
    }
}

/// What a package row says under its name: its trust tier (*Unreviewed* for a sideload; a package an earlier build
/// installed from the former registry gallery says none, and the built-in's tier is its name, [`name_of`], so it is
/// not said twice) and, when the package is usable, "Installed until restart" when only the volatile directory holds it. Why a package cannot be used is its row's
/// value ([`why_of`]), not a fact here.
fn facts_of(entry: &Entry, list: &HudList) -> Vec<String> {
    let mut facts = Vec::new();
    match entry.trust {
        Trust::BuiltIn | Trust::Registry => {}
        Trust::Unreviewed => facts.push(plx_platform::i18n::msg::settings_hud_unreviewed().to_owned()),
    }
    // The reason a package cannot be used outranks "installed until restart": the value column and a second fact do
    // not both fit a row in every language.
    if entry.until_restart && why_of(entry, list).is_none() {
        facts.push(plx_platform::i18n::msg::settings_hud_until_restart().to_owned());
    }
    facts
}

/// One row's sentence: the row, and what the line under the title says while it has focus.
type Says = Vec<(HudId, String)>;

/// The page's rows from the list alone: the packages (the built-in first), a removal row for each installed
/// one, and the drop files under their own header when there are any. Beside them, the sentence each row that has
/// one of its own says while it has focus.
fn hud_form(list: &HudList, draft: Option<&TextBuffer>, saving_end_time: bool) -> (Form<HudId, HudAction, SettingsPage>, Says) {
    use plx_platform::i18n::msg;
    let mut says = Says::new();
    let mut packs = FormSection::new("");
    let mut removals = FormSection::new("");
    for (i, entry) in list.entries.iter().enumerate() {
        let name = name_of(entry);
        let why = why_of(entry, list);
        let mut row = with_why(Row::new(&name).checked(entry.selected).detail(join(&facts_of(entry, list))), why, list);
        if entry.meta.is_some() {
            row = row.server_label();
        }
        packs = packs.item_keyed(HudId::Pack(entry.id.clone()), RowKey(i as u32), RowKind::Choice, HudAction::Select(entry.id.clone()), row)
            .disabled(why.is_some());
        if let Some(why) = why {
            says.push((HudId::Pack(entry.id.clone()), why_sentence(why, list)));
        }
        if !is_builtin(&entry.id) {
            let row = Row::new(msg::settings_hud_remove_named(&name)).server_label();
            removals = removals.item_keyed(HudId::Remove(entry.id.clone()), RowKey(REMOVE_KEY + i as u32), RowKind::Button, HudAction::Remove(entry.id.clone()), row).disabled(list.package_busy);
            says.push((HudId::Remove(entry.id.clone()), msg::settings_hud_remove_detail().to_owned()));
        }
    }
    // The address row: the keyboard's text while it is up, "Downloading…" while a fetch runs, else what to type. The
    // line is a field hint (and the typed text), so it stays the row's sub-line.
    let detail = match draft {
        Some(text) => text.with_insertion_marker("|"),
        None if list.fetching => msg::settings_hud_fetching().to_owned(),
        None => msg::settings_hud_address_hint().to_owned(),
    };
    let row = Row::new(msg::settings_hud_address()).detail(detail).dim(list.fetching);
    let address = FormSection::new("").item_keyed(HudId::Address, RowKey(ADDRESS_KEY), RowKind::Button, HudAction::Address, row);
    let mut folder = FormSection::new(msg::settings_hud_folder()).visible(!list.drops.is_empty());
    for (i, file) in list.drops.iter().enumerate() {
        let name = file.meta.as_ref().map_or_else(|| file.name.clone(), |m| m.name.clone());
        // An installable file keeps its sub-line (what OK does, and that the file is unreviewed); one that cannot be
        // installed says why as its value, and the sentence says what to do.
        let mut row = Row::new(&name).server_label();
        let (id, action) = match file.sha {
            Some(sha) => (HudId::Drop(sha), HudAction::Install(sha)),
            None => (HudId::DropFile(i), HudAction::Inert),
        };
        match file.status {
            Ok(()) => row = row.detail(join(&[msg::settings_hud_install().to_owned(), msg::settings_hud_unreviewed().to_owned()])),
            Err(why) => {
                row = row.value(refusal_text(why));
                says.push((id.clone(), match why {
                    Refusal::Unloadable(why) => why_sentence(Why::Format(why), list),
                    _ => msg::settings_hud_file_detail().to_owned(),
                }));
            }
        }
        folder = folder.item_keyed(id, RowKey(DROP_KEY + i as u32), RowKind::Button, action, row).disabled(file.status.is_err());
    }
    let mut display = FormSection::new("").visible(list.end_time.is_some());
    if let Some(on) = list.end_time {
        display = display.item_keyed(HudId::EndTime, RowKey(END_TIME_KEY), RowKind::Toggle, HudAction::EndTime,
            Row::new(msg::settings_hud_end_time_title()).toggle(on)).disabled(saving_end_time);
        says.push((HudId::EndTime, msg::settings_hud_end_time_detail().to_owned()));
    }
    (Form::new().section(packs).section(display).section(removals).section(address).section(folder), says)
}

/// What a refusal, a failed fetch or an install no storage took puts under the title, while one is in force: the typed
/// address's refusal, else the last notice's. `None` when nothing is, and the page says its own sentence.
fn notice_text(list: &HudList, refused: Option<AddressError>) -> Option<String> {
    if let Some(why) = refused {
        return Some(address_text(why).to_owned());
    }
    match &list.notice {
        Some(Notice::Refused { reason, .. }) => Some(refusal_text(*reason).to_owned()),
        Some(Notice::FetchFailed { reason }) => Some(fetch_text(*reason).to_owned()),
        Some(Notice::NotWritten { .. }) => Some(plx_platform::i18n::msg::settings_hud_not_saved().to_owned()),
        _ => None,
    }
}

/// What the line under the title says: the last notice, else what the page is.
fn copy_of(list: &HudList, refused: Option<AddressError>) -> String {
    notice_text(list, refused).unwrap_or_else(|| plx_platform::i18n::msg::settings_hud_copy().to_owned())
}

/// What a page builds from the listing.
struct Built {
    form: Form<HudId, HudAction, SettingsPage>,
    says: Says,
    title: String,
    parent: String,
    /// The page's own sentence.
    copy: String,
}

impl HudPage {
    pub fn new(entry: EntryId) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut page = Self {
            entry,
            title: String::new(),
            parent: String::new(),
            form: { let mut form = FormTable::new(BAND); form.table.set_empty_text(Some("")); form },
            state: State { end_time: None, saving_end_time: false, selected: 0, rows: 0, notice: 0, confirming: false, affirmative: false, alert_scroll: 0, editing: false, fetching: false, visible: false, pending_select_focus: false, provisional: true },
            list: HudList::default(),
            copy: String::new(),
            says: Says::new(),
            follow: FollowCopy::new(""),
            rx,
            tx: Some(tx),
            end_time_receipt: None,
            asking: None,
            offered: None,
            draft: TextBuffer::new(String::new(), 0),
            address_error: None,
            prompt: DecisionPrompt::new(
                ALERT_GROUP,
                ALERT,
                ALERT + 1,
                plx_platform::i18n::msg::settings_cancel_c(),
                plx_platform::i18n::msg::settings_hud_install_c(),
            ),
        };
        page.rebuild(None);
        // a page opens on its first sentence at once, with nothing fading
        page.follow = FollowCopy::new(&page.sentence());
        page
    }

    /// The page's rows and words from the listing alone.
    fn built(&self) -> Built {
        let (form, says) = hud_form(&self.list, self.state.editing.then_some(&self.draft), self.state.saving_end_time);
        Built { form, says, title: plx_platform::i18n::msg::settings_hud_title().into(), parent: parent_title().into(), copy: copy_of(&self.list, self.address_error) }
    }

    /// **The sentence beside the table**, a pure function of the focused row and the listing: a refusal or fetch
    /// notice in force outranks everything; else the focused row's own ([`Says`]: why it cannot be used, or what it
    /// does); else the page's.
    fn sentence(&self) -> String {
        if notice_text(&self.list, self.address_error).is_some() {
            return self.copy.clone();
        }
        self.form.selected_id()
            .and_then(|id| self.says.iter().find(|(row, _)| row == id))
            .map_or_else(|| self.copy.clone(), |(_, says)| says.clone())
    }

    fn view(&self) -> TableScreen<'_> {
        let header = Header::new(RouteLayout::settings_page(), Some(&self.parent), &self.title, &self.copy);
        let header = header.with_follow(&self.follow);
        TableScreen::new(header, &self.form.table, GroupId(0), self.entry).keyed(&self.form)
    }

    fn focus(&self, fx: &mut Effects<'_, InnerHost>, group: GroupId) {
        fx.push(Fx::Deliver(
            MachineId::Instance(InstanceId(0)),
            Delivery::Screen(ScreenEvent::Enter(Enter::Fresh {
                focus: if group == GroupId(0) {
                    FocusTarget::Elem(FocusKey { entry: self.entry, elem: self.state.selected })
                } else {
                    FocusTarget::ContainerGroup(group)
                },
            })),
        ));
    }

    /// Build the rows from `self.list`, keeping the cursor on its row by identity.
    fn rebuild(&mut self, keep: Option<HudId>) {
        let Built { form, says, title, parent, copy } = self.built();
        self.copy = copy;
        self.says = says;
        self.title = title;
        self.parent = parent;
        self.form.table.compact = false;
        self.form.table.board_metrics(true);
        self.form.table.header_ink = theme::TEXT_READING;
        self.form.table.list_focused = true;
        self.form.set(form, keep.as_ref());
        self.state.fetching = self.list.fetching;
        self.state.end_time = self.list.end_time;
        self.state.rows = self.form.table.n_rows() as u32;
        self.state.notice = match &self.list.notice {
            None => 0,
            Some(Notice::Installed { .. }) => 1,
            Some(Notice::Refused { .. }) => 2,
            Some(Notice::NotWritten { .. }) => 3,
            Some(Notice::Removed { .. }) => 4,
            Some(Notice::RemoveIncomplete { .. }) => 5,
            Some(Notice::FetchFailed { .. }) => 7,
        };
        self.state.selected = self.form.key_at(self.form.table.sel.max(0) as usize).map_or(0, |k| k.0);
        self.follow.follow(&self.sentence());
    }

    /// Open the page's channel on the first event, so a page that never gets one asks for nothing.
    fn open(&mut self, fx: &mut Effects<'_, InnerHost>) {
        if let Some(tx) = self.tx.take() {
            fx.push(Fx::App(AppFx::Huds(HudCmd::Open(tx))));
        }
    }

    /// Open the two-answer prompt about `asking`: `paragraphs` under `question`, Cancel focused.
    fn ask(&mut self, asking: Asking, question: &'static std::ffi::CStr, paragraphs: Vec<Cow<'static, str>>, fx: &mut Effects<'_, InnerHost>) {
        self.asking = Some(asking);
        self.prompt.open_paragraphs(question, paragraphs);
        self.state.confirming = true;
        self.state.affirmative = false;
        self.state.alert_scroll = self.prompt.scroll_target_bits();
        self.focus(fx, ALERT_GROUP);
    }

    /// Ask about the package the store holds from an address when the listing brings a new one: its name and version,
    /// where it came from (and that a plain `http://` source is a development one), *Unreviewed*, and what installing does.
    fn sync_offer(&mut self, fx: &mut Effects<'_, InnerHost>) {
        if !self.state.visible { return }
        let Some(offer) = self.list.offer.clone() else {
            self.offered = None;
            return;
        };
        if self.offered == Some(offer.sha) {
            return;
        }
        self.offered = Some(offer.sha);
        let mut facts = Vec::new();
        if let Some(from) = &offer.from {
            facts.push(plx_platform::i18n::msg::settings_hud_offer_from(from.host()));
            if from.dev_source() {
                facts.push(plx_platform::i18n::msg::settings_hud_offer_dev().to_owned());
            }
        }
        facts.push(plx_platform::i18n::msg::settings_hud_unreviewed().to_owned());
        let paragraphs = vec![
            Cow::Owned(format!("{} {}", offer.meta.name, offer.meta.version)),
            Cow::Owned(join(&facts)),
            Cow::Borrowed(plx_platform::i18n::msg::settings_hud_offer_body()),
        ];
        self.ask(Asking::Offer(offer.sha), plx_platform::i18n::msg::settings_hud_install_question_c(), paragraphs, fx);
    }

    /// Put the keyboard down (its text stays in the draft) and show the row as it is without it.
    fn leave_field(&mut self, fx: &mut Effects<'_, InnerHost>) {
        if !self.state.editing {
            return;
        }
        self.state.editing = false;
        fx.push(Fx::Deliver(MachineId::Instance(InstanceId(0)), Delivery::Keyboard { up: false }));
        self.rebuild(Some(HudId::Address));
        fx.invalidate(plx_machine::present::Provenance::Input);
    }

    /// Edit the draft; any edit clears the refusal of the address before it.
    fn edit(&mut self, edit: &TextEdit, fx: &mut Effects<'_, InnerHost>) {
        self.draft.edit(edit);
        self.address_error = None;
        self.rebuild(Some(HudId::Address));
        fx.invalidate(plx_machine::present::Provenance::Input);
    }

    /// OK on the keyboard: a good address leaves as a fetch, a refused one is said under the title.
    fn submit(&mut self, fx: &mut Effects<'_, InnerHost>) {
        match address::parse(self.draft.text()) {
            Ok(address) => {
                self.address_error = None;
                self.leave_field(fx);
                fx.push(Fx::App(AppFx::Huds(HudCmd::Fetch(address))));
            }
            Err(why) => {
                self.address_error = Some(why);
                self.leave_field(fx);
            }
        }
    }

    /// Input while the address field has the keyboard (or the OS reports it): the OS's text edits, the remote's
    /// Left/Right/Backspace, OK to submit and Back to put the keyboard down. Every other key is the field's, not focus travel.
    fn typing(&mut self, kind: &InputKind<u32>, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        match kind {
            // The television took its keyboard down by itself: the field closes with what was typed kept.
            InputKind::SystemKeyboard(up) => {
                if !*up && self.state.editing {
                    self.state.editing = false;
                    self.rebuild(Some(HudId::Address));
                    fx.invalidate(plx_machine::present::Provenance::Input);
                }
                Handled::Yes
            }
            InputKind::Text(edit) if self.state.editing || matches!(cx.owner, InputOwner::System(_)) => {
                self.edit(edit, fx);
                Handled::Yes
            }
            InputKind::Key { key, sym, edge, .. } if self.state.editing => {
                if *edge == Edge::Up {
                    return Handled::Yes;
                }
                let edit = match (*key, *sym) {
                    (Key::Left, _) => Some(TextEdit::Left),
                    (Key::Right, _) => Some(TextEdit::Right),
                    (_, plx_ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
                    (_, plx_ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear),
                    _ => None,
                };
                match (edit, *key) {
                    (Some(edit), _) => self.edit(&edit, fx),
                    (None, Key::Back) => self.leave_field(fx),
                    (None, Key::Ok) => self.submit(fx),
                    _ => {}
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }

    /// Play on the row of a package from a development source: its recorded address leaves as the fetch the address row
    /// sends. Unhandled on any other row, and while a fetch or other package work runs (an open prompt has already
    /// taken the key in `step`, before this arm).
    fn refetch(&mut self, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        if self.list.fetching || self.list.package_busy {
            return Handled::No;
        }
        let row = cx.focus.current.and_then(|key| self.form.index_of_key(RowKey(key.elem)));
        let Some(HudId::Pack(id)) = row.and_then(|i| self.form.id_at(i)) else { return Handled::No };
        let Some(address) = self.list.entries.iter().find(|entry| entry.id == *id).and_then(|entry| entry.dev_source.clone()) else {
            return Handled::No;
        };
        fx.push(Fx::App(AppFx::Huds(HudCmd::Fetch(address))));
        Handled::Yes
    }

    fn activate(&mut self, key: u32, fx: &mut Effects<'_, InnerHost>) {
        self.state.provisional = false;
        match form_activate(&self.form, key, fx) {
            Some(HudAction::EndTime) => {
                if let Some(on) = self.list.end_time.filter(|_| self.end_time_receipt.is_none()) {
                    let (reply, receipt) = mpsc::channel();
                    fx.push(Fx::App(AppFx::Preferences(super::registry::PreferenceCmd::Choose(
                        plx_media::route::Choice::HudEndTime(!on), reply))));
                    self.end_time_receipt = Some(receipt);
                    self.state.saving_end_time = true;
                    self.rebuild(Some(HudId::EndTime));
                }
            }
            Some(HudAction::Select(id)) => fx.push(Fx::App(AppFx::Huds(HudCmd::Select(id)))),
            Some(HudAction::Remove(id)) => fx.push(Fx::App(AppFx::Huds(HudCmd::Remove(id)))),
            Some(HudAction::Install(sha)) => {
                self.ask(
                    Asking::Drop(sha),
                    plx_platform::i18n::msg::settings_hud_install_question_c(),
                    vec![Cow::Borrowed(plx_platform::i18n::msg::settings_hud_install_body())],
                    fx,
                );
            }
            // A fetch in flight takes no second request: the row is dimmed and OK on it does nothing.
            Some(HudAction::Address) if !self.list.fetching && !self.state.editing => {
                self.state.editing = true;
                self.address_error = None;
                let text = self.draft.text().to_owned();
                let end = text.len();
                self.draft = TextBuffer::new(text, end);
                fx.push(Fx::Deliver(MachineId::Instance(InstanceId(0)), Delivery::Keyboard { up: true }));
                self.rebuild(Some(HudId::Address));
            }
            Some(HudAction::Address) | Some(HudAction::Inert) | None => {}
        }
        fx.invalidate(plx_machine::present::Provenance::Input);
    }
}

impl Machine<InnerHost> for HudPage {
    type Ev = ScreenEvent<InnerHost>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        match self.prompt.step(ev, cx) {
            PromptStep::Pass => (),
            PromptStep::Done(handled) => {
                self.state.affirmative = self.prompt.choice();
                self.state.alert_scroll = self.prompt.scroll_target_bits();
                return handled;
            }
            PromptStep::Answer(yes) => {
                self.state.confirming = false;
                match self.asking.take() {
                    Some(Asking::Drop(sha)) if yes => fx.push(Fx::App(AppFx::Huds(HudCmd::InstallDrop(sha)))),
                    Some(Asking::Offer(sha)) => {
                        fx.push(Fx::App(AppFx::Huds(if yes { HudCmd::InstallOffer(sha) } else { HudCmd::DeclineOffer(sha) })));
                    }
                    _ => {}
                }
                self.focus(fx, GroupId(0));
                fx.invalidate(plx_machine::present::Provenance::Input);
                return Handled::Yes;
            }
        }
        match ev {
            ScreenEvent::Enter(enter) => {
                if matches!(enter, Enter::Restored) { self.state.provisional = false; }
                self.state.visible = true;
                self.open(fx);
                Handled::No
            }
            ScreenEvent::Tick(t) => {
                self.open(fx);
                self.prompt.update(t.dt());
                if self.end_time_receipt.as_ref().is_some_and(|reply| !matches!(reply.try_recv(), Err(mpsc::TryRecvError::Empty))) {
                    self.end_time_receipt = None;
                    self.state.saving_end_time = false;
                    self.rebuild(self.form.selected_id().cloned());
                    fx.invalidate(plx_machine::present::Provenance::Landing(MachineId::Session));
                }
                let had_rows = self.form.table.n_rows() > 0;
                let mut latest = None;
                while let Ok(list) = self.rx.try_recv() {
                    latest = Some(list);
                }
                if let Some(list) = latest.filter(|l| *l != self.list) {
                    let before = self.list.entries.iter().find(|e| e.selected).map(|e| e.id.as_str());
                    let after = list.entries.iter().find(|e| e.selected).map(|e| e.id.as_str());
                    let picked = (((before.is_some() && (before != after || matches!(list.notice, Some(Notice::Removed {..}))))
                        || (self.state.provisional && after.is_some())))
                        .then(|| after.map(str::to_owned)).flatten();
                    self.list = list;
                    let keep = if let Some(id) = picked {
                        self.state.provisional = false;
                        self.state.pending_select_focus = true;
                        Some(HudId::Pack(id))
                    } else { self.form.selected_id().cloned() };
                    self.rebuild(keep);
                    if !had_rows && self.form.table.n_rows() > 0 {
                        // the first rows are the page arriving, not a focus move: its sentence is there at once
                        self.follow = FollowCopy::new(&self.sentence());
                    }
                    if !had_rows && self.form.table.n_rows() > 0 && cx.focus.current.is_none() {
                        self.focus(fx, GroupId(0));
                    }
                    fx.invalidate(plx_machine::present::Provenance::Landing(MachineId::Session));
                }
                // Run after the surface has finished restoring its remembered cursor on a pop.
                if self.state.visible && self.state.pending_select_focus {
                    self.state.pending_select_focus = false;
                    self.focus(fx, GroupId(0));
                }
                self.sync_offer(fx);
                self.form.table.update(t.dt(), RouteLayout::settings_page().sectioned_table().h);
                self.follow.update(t.dt());
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind, .. }) if self.state.editing || matches!(kind, InputKind::SystemKeyboard(_)) => self.typing(kind, cx, fx),
            ScreenEvent::Suspend => {
                self.leave_field(fx);
                Handled::No
            }
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover => {
                self.state.visible = false;
                self.leave_field(fx);
                Handled::No
            }
            ScreenEvent::FocusMoved { to, by, .. } => {
                if matches!(by, plx_machine::machine::By::Dir | plx_machine::machine::By::Pointer) { self.state.provisional = false; }
                form_focus(&mut self.form, to.elem);
                self.state.selected = self.form.key_at(self.form.table.sel.max(0) as usize).map_or(0, |k| k.0);
                self.follow.follow(&self.sentence());
                Handled::Yes
            }
            ScreenEvent::Activate(key) => {
                self.activate(*key, fx);
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key, sym, wcode, edge: Edge::Down, .. }, .. })
                if matches!(consts::classify_input(*key, *sym, *wcode), consts::Key::Play) => self.refetch(cx, fx),
            ScreenEvent::Input(plx_machine::machine::InputEvent {
                kind: plx_machine::machine::InputKind::Key { key: plx_machine::machine::Key::Right, at_edge: true, .. },
                ..
            }) => {
                if let Some(key) = cx.focus.current.and_then(|k| form_right_target(&self.form, k.elem)) {
                    self.activate(key, fx);
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

impl Focusable<InnerHost> for HudPage {
    fn groups(&self, cx: &Cx<'_, InnerHost>, out: &mut Vec<GroupSpec>) {
        if !self.prompt.groups(out, cx.measure) {
            Focusable::<InnerHost>::groups(&self.view(), cx, out)
        }
    }
    fn group_of(&self, key: &u32, cx: &Cx<'_, InnerHost>) -> Option<GroupId> {
        match self.prompt.group_of(*key) {
            Some(answer) => answer,
            None => Focusable::<InnerHost>::group_of(&self.view(), key, cx),
        }
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, cx: &Cx<'_, InnerHost>) -> Step<u32> {
        match self.prompt.neighbour(key, dir) {
            Some(step) => step,
            None => Focusable::<InnerHost>::neighbour(&self.view(), key, dir, cx),
        }
    }
    fn place(&self, key: &u32, cx: &Cx<'_, InnerHost>, at: At) -> Option<Placed> {
        match self.prompt.place(*key, cx.measure) {
            Some(placed) => placed,
            None => Focusable::<InnerHost>::place(&self.view(), key, cx, at),
        }
    }
    fn reconcile(&self, want: FocusKey<u32>, cx: &Cx<'_, InnerHost>) -> FocusKey<u32> {
        if let Some(key) = self.prompt.reconcile(want) {
            return key;
        }
        if self.prompt.owns(want.elem) {
            return FocusKey { entry: self.entry, elem: self.state.selected };
        }
        Focusable::<InnerHost>::reconcile(&self.view(), want, cx)
    }
    fn seat(&self, g: GroupId, from: Placed, cx: &Cx<'_, InnerHost>) -> FocusKey<u32> {
        match self.prompt.seat(g, self.entry) {
            Some(key) => key,
            None => Focusable::<InnerHost>::seat(&self.view(), g, from, cx),
        }
    }
}

impl Screen<InnerHost> for HudPage {
    fn name(&self) -> &'static str {
        word::PLAYER_HUD
    }
    fn state(&self) -> &dyn LogicalState {
        &self.state
    }
    fn crumb(&self, _cx: &Cx<'_, InnerHost>) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(&self.parent))
    }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, InnerHost>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, InnerHost>) {
        let mut view = self.view();
        plx_ui::screen::Part::<InnerHost>::draw(&mut view, f, Rect::FULL);
        self.prompt.draw(f, self.entry);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
}

/// The `(focus key, id)` of every row `list` shows, for a test: the page's own builder.
#[cfg(test)]
fn rows_for_test(list: &HudList) -> Vec<(u32, HudId)> {
    let mut form: FormTable<HudId, HudAction, SettingsPage> = FormTable::new(BAND);
    form.set(hud_form(list, None, false).0, None);
    (0..form.table.n_rows() as usize).filter_map(|i| Some((form.key_at(i)?.0, form.binding_at(i)?.id.clone()))).collect()
}

#[cfg(test)]
#[path = "hud_settings_tests.rs"]
mod tests;
