//! The notes side panel: the note editors for the line (and its pattern) the user picked, then
//! every note on the file.

use iced::widget::{button, column, container, row, scrollable, space, text, text_editor, tooltip};
use iced::{Alignment, Element, Length};

use crate::format::thousands;
use crate::tag_ui::with_tip;
use crate::theme;

pub const PANEL_W: f32 = 340.0;

#[derive(Debug, Clone)]
pub enum NotesMessage {
    Close,
    EditLine(text_editor::Action),
    EditPattern(text_editor::Action),
    /// Show and focus a real (0-based) line.
    JumpToLine(i64),
    /// Focus a pattern note, by index.
    FocusPattern(usize),
    DeleteLine(usize),
    DeletePattern(usize),
}

/// What the editors at the top of the panel are for.
pub struct Focus<'a> {
    /// 0-based line and its text, when a line is focused.
    pub line: Option<(i64, String)>,
    pub line_editor: &'a text_editor::Content,
    /// The focused pattern's text, when there is one.
    pub pattern: Option<String>,
    pub pattern_editor: &'a text_editor::Content,
}

pub struct LineRow {
    pub line: usize,
    pub note: String,
}

pub struct PatternRow {
    pub pattern: String,
    pub note: String,
    /// Why the note isn't showing on any lines, if it isn't.
    pub status: Option<&'static str>,
}

pub struct Panel<'a> {
    pub focus: Option<Focus<'a>>,
    pub lines: Vec<LineRow>,
    pub patterns: Vec<PatternRow>,
    /// Line notes are still being matched to the file.
    pub placing: bool,
    /// Why notes can't be saved, if they can't.
    pub read_only: Option<String>,
}

pub fn notes_panel(panel: Panel<'_>) -> Element<'_, NotesMessage> {
    let header = row![
        heading("NOTES"),
        space().width(Length::Fill),
        close_button(NotesMessage::Close),
    ]
    .align_y(Alignment::Center);

    let mut body = column![].spacing(14);
    if panel.placing {
        body = body.push(dim("Placing notes on the file…"));
    }
    if let Some(reason) = panel.read_only {
        body = body.push(
            text(reason)
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL)
                .color(theme::palette().warn),
        );
    }
    if let Some(focus) = panel.focus {
        body = body.push(focus_section(focus, panel.placing));
        body = body.push(divider());
    }

    let total = panel.lines.len() + panel.patterns.len();
    body = body.push(heading_owned(format!("ALL NOTES ({total})")));
    if total == 0 {
        body = body.push(dim(
            "Click the gutter left of a line number, or press Ctrl+Shift+N, to add a note.",
        ));
    }
    let mut list = column![].spacing(4);
    for (i, line) in panel.lines.into_iter().enumerate() {
        let entry = column![
            text(format!("Line {}", thousands(line.line as u64 + 1)))
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL)
                .color(theme::palette().accent),
            text(first_line(&line.note))
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL),
        ];
        list = list.push(list_row(
            entry,
            NotesMessage::JumpToLine(line.line as i64),
            NotesMessage::DeleteLine(i),
        ));
    }
    for (i, pattern) in panel.patterns.into_iter().enumerate() {
        let mut entry = column![row![
            text("Pattern")
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL)
                .color(theme::palette().warn),
            text(pattern.status.unwrap_or(""))
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL)
                .color(theme::palette().text_faint),
        ]
        .spacing(8)];
        entry = entry.push(
            text(pattern.pattern)
                .font(theme::MONO)
                .size(10.5)
                .color(theme::palette().text_dim)
                .wrapping(text::Wrapping::None),
        );
        entry = entry.push(
            text(first_line(&pattern.note))
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL),
        );
        list = list.push(list_row(
            entry,
            NotesMessage::FocusPattern(i),
            NotesMessage::DeletePattern(i),
        ));
    }
    body = body.push(list);

    container(column![header, scrollable(body).height(Length::Fill).spacing(6)].spacing(12))
        .width(PANEL_W)
        .height(Length::Fill)
        .padding([10, 14])
        .clip(true)
        .style(theme::surface)
        .into()
}

fn focus_section<'a>(focus: Focus<'a>, placing: bool) -> Element<'a, NotesMessage> {
    let mut section = column![].spacing(8);
    let has_line = focus.line.is_some();
    if let Some((line, line_text)) = focus.line {
        section = section.push(heading_owned(format!(
            "LINE {}",
            thousands(line as u64 + 1)
        )));
        section = section.push(
            text(line_text)
                .font(theme::MONO)
                .size(10.5)
                .color(theme::palette().text_dim)
                .wrapping(text::Wrapping::None),
        );
        let mut editor = text_editor(focus.line_editor)
            .placeholder("Add a note for this line…")
            .height(90)
            .size(theme::UI_SIZE);
        if !placing {
            editor = editor.on_action(NotesMessage::EditLine);
        }
        section = section.push(editor);
    }
    if let Some(pattern) = focus.pattern {
        section = section.push(heading("PATTERN"));
        section = section.push(
            text(pattern)
                .font(theme::MONO)
                .size(10.5)
                .color(theme::palette().text_dim)
                .wrapping(text::Wrapping::None),
        );
        section = section.push(
            text_editor(focus.pattern_editor)
                .placeholder("Add a note for every line in this pattern…")
                .height(90)
                .size(theme::UI_SIZE)
                .on_action(NotesMessage::EditPattern),
        );
    } else if has_line {
        section = section.push(dim(
            "Run Patterns (Ctrl+M) to note every line like this one.",
        ));
    }
    section.into()
}

fn list_row<'a>(
    entry: impl Into<Element<'a, NotesMessage>>,
    open: NotesMessage,
    delete: NotesMessage,
) -> Element<'a, NotesMessage> {
    row![
        button(entry)
            .width(Length::Fill)
            .padding([4, 6])
            .style(theme::ghost)
            .on_press(open),
        with_tip(close_button(delete), "Delete note", tooltip::Position::Left),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

fn close_button<'a>(message: NotesMessage) -> Element<'a, NotesMessage> {
    button(text("×").size(14).center())
        .width(22)
        .height(22)
        .padding(0)
        .style(theme::chip_remove)
        .on_press(message)
        .into()
}

fn heading<'a>(label: &'static str) -> Element<'a, NotesMessage> {
    text(label)
        .size(10.5)
        .color(theme::palette().text_faint)
        .into()
}

fn heading_owned<'a>(label: String) -> Element<'a, NotesMessage> {
    text(label)
        .size(10.5)
        .color(theme::palette().text_faint)
        .into()
}

fn dim<'a>(label: &'static str) -> Element<'a, NotesMessage> {
    text(label)
        .size(theme::SMALL_SIZE)
        .font(theme::UI_SMALL)
        .color(theme::palette().text_dim)
        .into()
}

fn divider<'a>() -> Element<'a, NotesMessage> {
    container(space())
        .width(Length::Fill)
        .height(1)
        .style(theme::divider)
        .into()
}

/// The first line of a note, for the list.
fn first_line(note: &str) -> String {
    let first = note.lines().next().unwrap_or("");
    if note.lines().nth(1).is_some() {
        format!("{first} …")
    } else {
        first.to_string()
    }
}
