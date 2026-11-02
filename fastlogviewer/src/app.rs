//! The window: panes split side by side or stacked, each with its own strip of tabs, and the
//! window chrome. Everything about one log lives in `tab.rs`; this file only routes messages to
//! the right tab and keeps the panes and strips in order.
//!
//! Keyboard input goes to the focused pane's shown tab. Clicking anywhere in a pane focuses it.
//! Tags are saved for every log, so an edit in one tab is handed to all the others.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::keyboard::{self, key::Named, Key};
use iced::widget::pane_grid::{self, Axis, PaneGrid};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{button, column, container, row, scrollable, space, text, tooltip};
use iced::{event, window, Alignment, Element, Widget, Event, Length, Subscription, Task};

use crate::tab::{self, icon, Tab};
use crate::tag_ui::with_tip;
use crate::theme::{self, icons};
use crate::win;

/// How often each open file's size and modified time are compared with what was loaded.
const DISK_CHECK: Duration = Duration::from_secs(2);

type TabId = u64;

/// One pane's tabs, in strip order, and which one it shows.
struct Pane {
    tabs: Vec<TabId>,
    active: TabId,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tab(TabId, tab::Message),
    SelectTab(pane_grid::Pane, TabId),
    CloseTab(pane_grid::Pane, TabId),
    /// Opens a start-screen tab in the pane (the home button, Ctrl+T).
    NewTab(pane_grid::Pane),
    Split(pane_grid::Pane, Axis),
    ClosePane(pane_grid::Pane),
    PaneClicked(pane_grid::Pane),
    PaneResized(pane_grid::ResizeEvent),
    KeyPressed {
        key: Key,
        modifiers: keyboard::Modifiers,
        /// A focused text input already handled it.
        captured: bool,
    },
    ModifiersChanged(keyboard::Modifiers),
    FileDropped(PathBuf),
    Tick,
    CheckDisk,
    /// The HWND is only known once the window exists; the title bar is tinted from there. Opening
    /// can race the subscription that would see it, so the first focus counts too.
    WindowShown(window::Id),
    WindowReady(u64),
}

pub struct App {
    panes: pane_grid::State<Pane>,
    focus: pane_grid::Pane,
    tabs: HashMap<TabId, Tab>,
    next_tab: TabId,
    modifiers: keyboard::Modifiers,

    chrome_applied: bool,
    /// The HWND, kept so switching themes can retint the title bar.
    window_handle: Option<u64>,
}

impl App {
    pub fn new(initial_file: Option<String>) -> (Self, Task<Message>) {
        let mut first = Tab::new();
        if let Some(path) = initial_file {
            if Path::new(&path).is_file() {
                first.load_file(&path);
            }
        }
        let mut tabs = HashMap::new();
        tabs.insert(0, first);
        let (panes, focus) = pane_grid::State::new(Pane {
            tabs: vec![0],
            active: 0,
        });
        let app = App {
            panes,
            focus,
            tabs,
            next_tab: 1,
            modifiers: keyboard::Modifiers::default(),
            chrome_applied: false,
            window_handle: None,
        };
        (app, Task::none())
    }

    pub fn title(&self) -> String {
        self.active_tab(self.focus)
            .map_or_else(|| "FastLogViewer".to_string(), Tab::title)
    }

    fn active_id(&self, pane: pane_grid::Pane) -> Option<TabId> {
        self.panes.get(pane).map(|p| p.active)
    }

    fn active_tab(&self, pane: pane_grid::Pane) -> Option<&Tab> {
        self.active_id(pane).and_then(|id| self.tabs.get(&id))
    }

    /// Adds a tab after the pane's shown one and shows it.
    fn add_tab(&mut self, pane: pane_grid::Pane, mut tab: Tab) -> TabId {
        let id = self.next_tab;
        self.next_tab += 1;
        tab.set_modifiers(self.modifiers);
        self.tabs.insert(id, tab);
        if let Some(state) = self.panes.get_mut(pane) {
            let at = state
                .tabs
                .iter()
                .position(|t| *t == state.active)
                .map_or(state.tabs.len(), |i| i + 1);
            state.tabs.insert(at, id);
            state.active = id;
        }
        id
    }

    /// Opens a file in the pane: in its shown tab when that is still on the start screen,
    /// otherwise in a new tab.
    fn open_in(&mut self, pane: pane_grid::Pane, path: &Path) {
        let path = path.to_string_lossy();
        if let Some(id) = self.active_id(pane) {
            if let Some(tab) = self.tabs.get_mut(&id).filter(|t| !t.has_file()) {
                tab.load_file(&path);
                return;
            }
        }
        let mut tab = Tab::new();
        tab.load_file(&path);
        if tab.has_file() {
            self.add_tab(pane, tab);
        }
    }

    fn close_tab(&mut self, pane: pane_grid::Pane, id: TabId) {
        let Some(state) = self.panes.get_mut(pane) else {
            return;
        };
        let Some(index) = state.tabs.iter().position(|t| *t == id) else {
            return;
        };
        state.tabs.remove(index);
        // Dropping the tab cancels its workers through their tokens' owners going away.
        if let Some(mut tab) = self.tabs.remove(&id) {
            tab.close();
        }

        if state.tabs.is_empty() {
            if self.panes.len() > 1 {
                self.close_pane(pane);
                return;
            }
            // The last tab in the window: leave a start screen rather than an empty pane.
            self.add_tab(pane, Tab::new());
            return;
        }
        if state.active == id {
            state.active = state.tabs[index.min(state.tabs.len() - 1)];
        }
    }

    fn close_pane(&mut self, pane: pane_grid::Pane) {
        if self.panes.len() <= 1 {
            return;
        }
        if let Some((state, sibling)) = self.panes.close(pane) {
            for id in state.tabs {
                if let Some(mut tab) = self.tabs.remove(&id) {
                    tab.close();
                }
            }
            if self.focus == pane {
                self.focus = sibling;
            }
        }
    }

    /// Splits the pane, opening the file it shows in the new half (or a start screen).
    fn split(&mut self, pane: pane_grid::Pane, axis: Axis) {
        let path = self
            .active_tab(pane)
            .and_then(Tab::file_path)
            .map(Path::to_path_buf);
        let mut tab = Tab::new();
        if let Some(path) = path {
            tab.load_file(&path.to_string_lossy());
        }
        let id = self.next_tab;
        self.next_tab += 1;
        tab.set_modifiers(self.modifiers);
        self.tabs.insert(id, tab);
        if let Some((new_pane, _)) = self.panes.split(
            axis,
            pane,
            Pane {
                tabs: vec![id],
                active: id,
            },
        ) {
            self.focus = new_pane;
        }
    }

    /// Steps the focused pane's shown tab left or right, wrapping.
    fn cycle_tab(&mut self, delta: isize) {
        let Some(state) = self.panes.get_mut(self.focus) else {
            return;
        };
        let count = state.tabs.len() as isize;
        let Some(index) = state.tabs.iter().position(|t| *t == state.active) else {
            return;
        };
        state.active = state.tabs[(index as isize + delta).rem_euclid(count) as usize];
    }

    fn pane_of(&self, id: TabId) -> Option<pane_grid::Pane> {
        self.panes
            .iter()
            .find(|(_, state)| state.tabs.contains(&id))
            .map(|(pane, _)| *pane)
    }

    fn apply_chrome(&self) {
        if let Some(handle) = self.window_handle {
            let p = theme::palette();
            win::apply_chrome(handle, !theme::is_light(), p.surface, p.text_dim);
        }
    }

    // ------------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tab(id, message) => return self.update_tab(id, message),
            Message::SelectTab(pane, id) => {
                if let Some(state) = self.panes.get_mut(pane) {
                    state.active = id;
                }
                self.focus = pane;
            }
            Message::CloseTab(pane, id) => self.close_tab(pane, id),
            Message::NewTab(pane) => {
                self.add_tab(pane, Tab::new());
                self.focus = pane;
            }
            Message::Split(pane, axis) => self.split(pane, axis),
            Message::ClosePane(pane) => self.close_pane(pane),
            Message::PaneClicked(pane) => self.focus = pane,
            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                self.panes.resize(split, ratio);
            }
            Message::KeyPressed {
                key,
                modifiers,
                captured,
            } => return self.handle_key(key, modifiers, captured),
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                for tab in self.tabs.values_mut() {
                    tab.set_modifiers(modifiers);
                }
            }
            Message::FileDropped(path) => self.open_in(self.focus, &path),
            Message::Tick => self.broadcast(tab::Message::Tick),
            Message::CheckDisk => self.broadcast(tab::Message::CheckDisk),
            Message::WindowShown(id) if !self.chrome_applied => {
                self.chrome_applied = true;
                return window::raw_id::<Message>(id).map(Message::WindowReady);
            }
            Message::WindowShown(_) => {}
            Message::WindowReady(handle) => {
                self.window_handle = Some(handle);
                self.apply_chrome();
            }
        }
        Task::none()
    }

    fn broadcast(&mut self, message: tab::Message) {
        let ids: Vec<TabId> = self.tabs.keys().copied().collect();
        for id in ids {
            // Ticks and disk checks never return work.
            let _ = self.update_tab(id, message.clone());
        }
    }

    /// Hands a message to one tab, then picks up anything it asked of the window.
    fn update_tab(&mut self, id: TabId, message: tab::Message) -> Task<Message> {
        if let tab::Message::ToggleTheme = message {
            theme::set_light(!theme::is_light());
            self.apply_chrome();
            return Task::none();
        }
        let Some(tab) = self.tabs.get_mut(&id) else {
            return Task::none();
        };
        let task = tab.update(message).map(move |m| Message::Tab(id, m));

        let open = tab.take_open_request();
        if tab.take_tags_changed() {
            let tags = tab.tags().to_vec();
            for (other, tab) in self.tabs.iter_mut() {
                if *other != id {
                    tab.adopt_tags(&tags);
                }
            }
        }
        if let Some(path) = open {
            let pane = self.pane_of(id).unwrap_or(self.focus);
            self.open_in(pane, &path);
        }
        task
    }

    /// Tab and pane shortcuts; everything else goes to the focused pane's tab.
    fn handle_key(
        &mut self,
        key: Key,
        modifiers: keyboard::Modifiers,
        captured: bool,
    ) -> Task<Message> {
        if modifiers.command() {
            match &key {
                Key::Character(c) if c.eq_ignore_ascii_case("t") => {
                    self.add_tab(self.focus, Tab::new());
                    return Task::none();
                }
                Key::Character(c) if c.eq_ignore_ascii_case("w") => {
                    if let Some(id) = self.active_id(self.focus) {
                        self.close_tab(self.focus, id);
                    }
                    return Task::none();
                }
                Key::Named(Named::Tab) => {
                    self.cycle_tab(if modifiers.shift() { -1 } else { 1 });
                    return Task::none();
                }
                _ => {}
            }
        }
        let Some(id) = self.active_id(self.focus) else {
            return Task::none();
        };
        self.update_tab(
            id,
            tab::Message::KeyPressed {
                key,
                modifiers,
                captured,
            },
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let events = event::listen_with(|event, status, window| match event {
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                Some(Message::KeyPressed {
                    key,
                    modifiers,
                    captured: status == event::Status::Captured,
                })
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::ModifiersChanged(modifiers))
            }
            Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
            Event::Window(window::Event::Opened { .. } | window::Event::Focused) => {
                Some(Message::WindowShown(window))
            }
            _ => None,
        });

        let mut subscriptions = vec![events];
        if self.tabs.values().any(Tab::needs_tick) {
            subscriptions.push(iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick));
        }
        if self.tabs.values().any(Tab::has_file) {
            subscriptions.push(iced::time::every(DISK_CHECK).map(|_| Message::CheckDisk));
        }
        Subscription::batch(subscriptions)
    }

    // -------------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let grid = PaneGrid::new(&self.panes, |pane, state, _| {
            let body: Element<'_, Message> = match self.tabs.get(&state.active) {
                Some(tab) => {
                    let id = state.active;
                    tab.view().map(move |m| Message::Tab(id, m)).boxed()
                }
                None => space().boxed(),
            };
            pane_grid::Content::new(column![self.tab_strip(pane, state), body])
        })
        .spacing(1)
        .on_click(Message::PaneClicked)
        .on_resize(6, Message::PaneResized)
        .style(theme::pane_grid);

        // The 1px gap between panes shows this background, which makes it the divider.
        container(grid)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::divider)
            .boxed()
    }

    fn tab_strip<'a>(&'a self, pane: pane_grid::Pane, state: &'a Pane) -> Element<'a, Message> {
        let focused = pane == self.focus;
        let mut tabs = row![].align_y(Alignment::End);
        for &id in &state.tabs {
            let Some(tab) = self.tabs.get(&id) else {
                continue;
            };
            let active = id == state.active;
            let label = button(
                text(tab.label())
                    .size(theme::SMALL_SIZE)
                    .font(theme::UI_SMALL)
                    .wrapping(text::Wrapping::None),
            )
            .padding([6, 4])
            .style(theme::tab_label)
            .on_press(Message::SelectTab(pane, id));
            let close = button(icon(icons::CLOSE, 12.0, theme::palette().text_faint))
                .padding(3)
                .style(theme::chip_remove)
                .on_press(Message::CloseTab(pane, id));
            let body = row![label, close]
                .spacing(2)
                .padding([0, 6])
                .align_y(Alignment::Center);
            let element = column![
                container(space())
                    .width(Length::Fill)
                    .height(2)
                    .style(theme::tab_marker(active, focused)),
                body,
            ]
            .width(Length::Shrink);
            let element = container(element).style(theme::tab(active));
            tabs = tabs.push(match tab.file_path().and_then(Path::to_str) {
                Some(path) => with_tip(element, path, tooltip::Position::Bottom),
                None => element.boxed(),
            });
        }

        let tool = |bytes: &'static [u8], message: Message, tip: &'static str| {
            with_tip(
                button(icon(bytes, 14.0, theme::palette().text_dim))
                    .padding(6)
                    .style(theme::ghost)
                    .on_press(message),
                tip,
                tooltip::Position::Bottom,
            )
        };
        tabs = tabs.push(
            container(tool(
                icons::HOME,
                Message::NewTab(pane),
                "New tab on the start screen  (Ctrl+T)",
            ))
            .padding([0, 4])
            .boxed(),
        );

        let mut tools = row![
            tool(
                icons::SPLIT_RIGHT,
                Message::Split(pane, Axis::Vertical),
                "Split right"
            ),
            tool(
                icons::SPLIT_DOWN,
                Message::Split(pane, Axis::Horizontal),
                "Split down"
            ),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        if self.panes.len() > 1 {
            tools = tools.push(tool(
                icons::CLOSE,
                Message::ClosePane(pane),
                "Close this pane",
            ));
        }

        container(
            row![
                scrollable(tabs)
                    .direction(Direction::Horizontal(
                        Scrollbar::new().width(0).scroller_width(0),
                    ))
                    .width(Length::Fill),
                tools,
            ]
            .align_y(Alignment::Center)
            .padding([0, 6]),
        )
        .height(32)
        .align_y(Alignment::End)
        .width(Length::Fill)
        .style(theme::tab_strip)
        .boxed()
    }
}
