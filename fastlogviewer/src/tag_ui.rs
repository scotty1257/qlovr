//! Tag chips, and the two modal dialogs: the tag editor and go-to-line.

use fastlogviewer_core::{Matcher, TagDef, TAG_PALETTE};
use iced::widget::{
    button, center, checkbox, column, container, opaque, row, space, text, text_input, tooltip, Id,
};
use iced::{Alignment, Color, Element, Length};
use uuid::Uuid;

use crate::theme;

pub const TAG_LABEL_INPUT: Id = Id::new("tag_label");
pub const GOTO_INPUT: Id = Id::new("goto_line");
pub const COPY_RANGE_INPUT: Id = Id::new("copy_range");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipAction {
    ToggleHighlight,
    ToggleFilter,
    Remove,
    Edit,
}

fn tag_color(tag: &TagDef) -> Color {
    let [r, g, b] = tag.color();
    Color::from_rgb8(r, g, b)
}

/// Small text with a styled tooltip; every icon-ish control in the window uses one.
pub fn with_tip<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    tip: &'a str,
    position: tooltip::Position,
) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(tip).size(theme::SMALL_SIZE))
            .padding([4, 8])
            .style(theme::tooltip),
        position,
    )
    .gap(6)
    .into()
}

/// `● label  H F ×` — clicking the label edits the tag, H and F toggle highlight and filter.
/// `in_filter` says whether the filter includes the tag, which for a level tag can be true with
/// its own F off.
pub fn tag_chip<'a, Message: Clone + 'a>(
    tag: &'a TagDef,
    in_filter: bool,
    on_action: impl Fn(Uuid, ChipAction) -> Message + 'a,
) -> Element<'a, Message> {
    let color = tag_color(tag);
    let switch = |label: &'static str, on: bool, implied: bool, action: ChipAction| {
        button(text(label).size(11).font(theme::MONO).center())
            .width(20)
            .height(20)
            .padding(0)
            .style(theme::chip_switch(on, implied, color))
            .on_press(on_action(tag.id, action))
    };

    let label = button(
        row![
            container(space())
                .width(8)
                .height(8)
                .style(theme::dot(color)),
            text(&tag.label).size(theme::SMALL_SIZE),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .padding([3, 4])
    .style(theme::chip_label)
    .on_press(on_action(tag.id, ChipAction::Edit));

    container(
        row![
            with_tip(label, "Edit tag", tooltip::Position::Bottom),
            with_tip(
                switch("H", tag.highlight_on, false, ChipAction::ToggleHighlight),
                "Highlight matches",
                tooltip::Position::Bottom,
            ),
            with_tip(
                switch("F", tag.filter_on, in_filter, ChipAction::ToggleFilter),
                if tag.level {
                    "Show this level and every level to its right"
                } else {
                    "Show only matching lines"
                },
                tooltip::Position::Bottom,
            ),
            button(text("×").size(14).center())
                .width(20)
                .height(20)
                .padding(0)
                .style(theme::chip_remove)
                .on_press(on_action(tag.id, ChipAction::Remove)),
        ]
        .spacing(2)
        .align_y(Alignment::Center),
    )
    .padding([2, 4])
    .style(theme::chip(color))
    .into()
}

/// Centres `content` over a dimmed backdrop that swallows clicks to the window behind it.
pub fn modal<'a, Message: 'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    opaque(center(content).style(theme::backdrop))
}

fn field_label<'a, Message: 'a>(label: &'a str) -> Element<'a, Message> {
    text(label)
        .size(theme::SMALL_SIZE)
        .color(theme::palette().text_dim)
        .into()
}

fn dialog_buttons<'a, Message: Clone + 'a>(
    accept_label: &'a str,
    accept: Option<Message>,
    cancel: Message,
) -> Element<'a, Message> {
    row![
        space().width(Length::Fill),
        button(text("Cancel").size(theme::UI_SIZE))
            .padding([7, 14])
            .style(theme::secondary)
            .on_press(cancel),
        button(text(accept_label).size(theme::UI_SIZE))
            .padding([7, 16])
            .style(theme::primary)
            .on_press_maybe(accept),
    ]
    .spacing(8)
    .into()
}

fn dialog_card<'a, Message: 'a>(
    width: f32,
    body: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(body)
        .width(width)
        .padding(22)
        .style(theme::card)
        .into()
}

// --------------------------------------------------------------- TagEditPopup

#[derive(Debug, Clone)]
pub enum PopupMessage {
    Label(String),
    Pattern(String),
    ToggleIgnoreCase,
    Highlight(bool),
    Filter(bool),
    Level(bool),
    /// Move the tag one place left (-1) or right (+1) in the tag bar.
    Move(i64),
    Color(usize),
    Accept,
    Cancel,
}

pub enum PopupOutcome {
    Open,
    Accepted,
    Cancelled,
    /// Move the tag being edited; applied right away, the dialog stays open.
    Move(i64),
}

/// The new/edit tag dialog.
pub struct TagEditPopup {
    /// `None` for a new tag, otherwise the id of the tag being edited.
    pub editing: Option<Uuid>,
    pub label: String,
    pub pattern: String,
    /// The "Aa" toggle. It carries the C#'s meaning: on means *ignore* case.
    pub ignore_case: bool,
    pub highlight: bool,
    pub filter: bool,
    pub level: bool,
    pub color_index: usize,
}

impl TagEditPopup {
    pub fn new_tag(default_color_index: usize) -> Self {
        TagEditPopup {
            editing: None,
            label: String::new(),
            pattern: String::new(),
            ignore_case: false,
            highlight: true,
            filter: false,
            level: false,
            color_index: default_color_index,
        }
    }

    pub fn edit(tag: &TagDef) -> Self {
        TagEditPopup {
            editing: Some(tag.id),
            label: tag.label.clone(),
            pattern: tag.pattern.clone(),
            ignore_case: tag.ignore_case,
            highlight: tag.highlight_on,
            filter: tag.filter_on,
            level: tag.level,
            color_index: tag.color_index,
        }
    }

    /// Blank label falls back to the pattern, matching the C# `Commit`.
    pub fn result_label(&self) -> String {
        if self.label.trim().is_empty() {
            self.pattern.clone()
        } else {
            self.label.trim().to_string()
        }
    }

    fn matcher(&self) -> Matcher {
        Matcher::new(&self.pattern, true, self.ignore_case)
    }

    fn can_accept(&self) -> bool {
        let matcher = self.matcher();
        !matcher.is_empty() && matcher.is_valid()
    }

    pub fn update(&mut self, message: PopupMessage) -> PopupOutcome {
        match message {
            PopupMessage::Label(label) => self.label = label,
            PopupMessage::Pattern(pattern) => self.pattern = pattern,
            PopupMessage::ToggleIgnoreCase => self.ignore_case = !self.ignore_case,
            PopupMessage::Highlight(on) => self.highlight = on,
            PopupMessage::Filter(on) => self.filter = on,
            PopupMessage::Level(on) => self.level = on,
            PopupMessage::Move(delta) => return PopupOutcome::Move(delta),
            PopupMessage::Color(index) => self.color_index = index,
            PopupMessage::Accept if self.can_accept() => return PopupOutcome::Accepted,
            PopupMessage::Accept => {}
            PopupMessage::Cancel => return PopupOutcome::Cancelled,
        }
        PopupOutcome::Open
    }

    pub fn view(&self) -> Element<'_, PopupMessage> {
        let title = if self.editing.is_none() {
            "New tag"
        } else {
            "Edit tag"
        };
        let error = self.matcher().error.clone().unwrap_or_default();
        let accept = self.can_accept().then_some(PopupMessage::Accept);

        let label_input = text_input("Defaults to the pattern", &self.label)
            .id(TAG_LABEL_INPUT)
            .on_input(PopupMessage::Label)
            .on_submit(PopupMessage::Accept)
            .padding([8, 10])
            .size(theme::UI_SIZE)
            .style(theme::input);

        let pattern_input = text_input("e.g. timeout|refused", &self.pattern)
            .on_input(PopupMessage::Pattern)
            .on_submit(PopupMessage::Accept)
            .font(theme::MONO)
            .padding([8, 10])
            .size(theme::MONO_SIZE)
            .style(theme::input);

        let case_toggle = button(text("Aa").font(theme::MONO).size(theme::MONO_SIZE).center())
            .width(36)
            .height(34)
            .style(theme::toggle(self.ignore_case))
            .on_press(PopupMessage::ToggleIgnoreCase);

        let swatches = row(TAG_PALETTE.iter().enumerate().map(|(i, [r, g, b])| {
            button(space())
                .width(22)
                .height(22)
                .style(theme::swatch(
                    Color::from_rgb8(*r, *g, *b),
                    i == self.color_index,
                ))
                .on_press(PopupMessage::Color(i))
                .into()
        }))
        .spacing(8);

        let checkbox_row = row![
            checkbox(self.highlight)
                .label("Highlight")
                .on_toggle(PopupMessage::Highlight)
                .size(16)
                .text_size(theme::UI_SIZE)
                .style(theme::check),
            checkbox(self.filter)
                .label("Filter")
                .on_toggle(PopupMessage::Filter)
                .size(16)
                .text_size(theme::UI_SIZE)
                .style(theme::check),
            with_tip(
                checkbox(self.level)
                    .label("Severity level")
                    .on_toggle(PopupMessage::Level)
                    .size(16)
                    .text_size(theme::UI_SIZE)
                    .style(theme::check),
                "Levels rank least to most severe, left to right in the tag bar.
                 Filtering a level also shows every level to its right.",
                tooltip::Position::Top,
            ),
        ]
        .spacing(24);

        // Only a saved tag has a place in the tag bar to move.
        let move_button = |label: &'static str, delta: i64| {
            button(text(label).size(theme::SMALL_SIZE).center())
                .width(30)
                .height(26)
                .padding(0)
                .style(theme::secondary)
                .on_press_maybe(self.editing.map(|_| PopupMessage::Move(delta)))
        };
        let position_row = row![
            field_label("Position in tag bar"),
            space().width(Length::Fill),
            with_tip(move_button("◀", -1), "Less severe", tooltip::Position::Top),
            with_tip(move_button("▶", 1), "More severe", tooltip::Position::Top),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let body = column![
            text(title).size(17),
            column![field_label("Label"), label_input].spacing(6),
            column![
                field_label("Pattern (regex)"),
                row![
                    pattern_input,
                    with_tip(case_toggle, "Ignore case", tooltip::Position::Top)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                text(error)
                    .size(theme::SMALL_SIZE)
                    .color(theme::palette().error),
            ]
            .spacing(6),
            checkbox_row,
            position_row,
            column![field_label("Colour"), swatches].spacing(8),
            space().height(2),
            dialog_buttons(
                if self.editing.is_none() {
                    "Add tag"
                } else {
                    "Save"
                },
                accept,
                PopupMessage::Cancel,
            ),
        ]
        .spacing(16);

        dialog_card(400.0, body)
    }
}

// ------------------------------------------------------------------ go to line

#[derive(Debug, Clone)]
pub enum GotoMessage {
    Input(String),
    Go,
    Cancel,
}

pub fn goto_view(input: &str, line_count: usize) -> Element<'_, GotoMessage> {
    let body = column![
        text("Go to line").size(17),
        text_input(
            &format!("1 – {}", crate::format::thousands(line_count as u64)),
            input
        )
        .id(GOTO_INPUT)
        .on_input(GotoMessage::Input)
        .on_submit(GotoMessage::Go)
        .font(theme::MONO)
        .padding([8, 10])
        .size(theme::MONO_SIZE)
        .style(theme::input),
        dialog_buttons("Go", Some(GotoMessage::Go), GotoMessage::Cancel),
    ]
    .spacing(16);

    dialog_card(300.0, body)
}

// ------------------------------------------------------------------------ copy

#[derive(Debug, Clone)]
pub enum CopyMessage {
    Selection,
    Line,
    Lines,
    RangeInput(String),
    Range,
    Cancel,
}

/// What there is to copy right now, so the dialog can grey out what isn't available.
pub struct CopyChoices {
    pub has_text: bool,
    /// 1-based number of the current line.
    pub line: Option<i64>,
    /// How many lines are selected.
    pub lines: usize,
}

pub fn copy_view<'a>(
    choices: CopyChoices,
    range: &'a str,
    error: Option<&'a str>,
    line_count: usize,
) -> Element<'a, CopyMessage> {
    let choice = |label: String, hint: &'static str, message: Option<CopyMessage>| {
        button(
            row![
                text(label).size(theme::UI_SIZE),
                space().width(Length::Fill),
                text(hint)
                    .size(theme::SMALL_SIZE)
                    .color(theme::palette().text_faint),
            ]
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([7, 12])
        .style(theme::secondary)
        .on_press_maybe(message)
    };

    let line_label = match choices.line {
        Some(n) => format!("Current line ({})", crate::format::thousands(n as u64)),
        None => "Current line".to_string(),
    };
    let lines_label = if choices.lines > 1 {
        format!(
            "Selected lines ({})",
            crate::format::thousands(choices.lines as u64)
        )
    } else {
        "Selected lines".to_string()
    };

    let body = column![
        text("Copy").size(17),
        column![
            choice(
                "Selected text".to_string(),
                "Ctrl+C",
                choices.has_text.then_some(CopyMessage::Selection),
            ),
            choice(line_label, "", choices.line.map(|_| CopyMessage::Line)),
            choice(
                lines_label,
                "Shift+click",
                (choices.lines > 0).then_some(CopyMessage::Lines),
            ),
        ]
        .spacing(6),
        column![
            field_label("Line range"),
            row![
                text_input(&format!("from-to, e.g. 1-{}", line_count.min(100)), range)
                    .id(COPY_RANGE_INPUT)
                    .on_input(CopyMessage::RangeInput)
                    .on_submit(CopyMessage::Range)
                    .font(theme::MONO)
                    .padding([8, 10])
                    .size(theme::MONO_SIZE)
                    .style(theme::input),
                button(text("Copy range").size(theme::UI_SIZE))
                    .padding([8, 14])
                    .style(theme::primary)
                    .on_press_maybe((!range.trim().is_empty()).then_some(CopyMessage::Range)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text(error.unwrap_or(""))
                .size(theme::SMALL_SIZE)
                .color(theme::palette().error),
        ]
        .spacing(6),
        row![
            space().width(Length::Fill),
            button(text("Cancel").size(theme::UI_SIZE))
                .padding([7, 14])
                .style(theme::secondary)
                .on_press(CopyMessage::Cancel),
        ],
    ]
    .spacing(16);

    dialog_card(380.0, body)
}
