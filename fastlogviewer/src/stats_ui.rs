//! The tag stats side panel. Per tag: how many lines match and what share that is (of the file,
//! and of the lines shown when a filter is on), a strip showing where in the file the matches
//! fall, and the first and last match.

use iced::mouse;
use iced::widget::canvas::{self, Action, Event, Frame, Geometry};
use iced::widget::{
    button, canvas as canvas_widget, column, container, row, scrollable, space, text,
};
use iced::{Alignment, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use uuid::Uuid;

use crate::format::thousands;
use crate::theme;

pub const PANEL_W: f32 = 320.0;
/// Buckets in each tag's distribution strip.
pub const STRIP_BUCKETS: usize = 120;
const STRIP_H: f32 = 18.0;

#[derive(Debug, Clone)]
pub enum StatsMessage {
    Close,
    /// Go to a real (0-based) file line.
    JumpToLine(i64),
    /// Go to the tag's first match inside a strip bucket.
    JumpToBucket(Uuid, usize),
}

/// One tag's row, worked out by the app.
pub struct TagStat {
    pub id: Uuid,
    pub label: String,
    pub color: Color,
    pub count: usize,
    pub complete: bool,
    /// Percent of all lines.
    pub share: f64,
    /// Percent of the displayed lines, when a filter is on and the count is known.
    pub shown_share: Option<f64>,
    pub buckets: Vec<usize>,
    /// 0-based line and its timestamp (empty when none was recognised).
    pub first: Option<(i64, String)>,
    pub last: Option<(i64, String)>,
}

pub fn stats_panel(stats: Vec<TagStat>, filter_on: bool) -> Element<'static, StatsMessage> {
    let header = row![
        text("TAG STATS").size(10.5).color(theme::TEXT_FAINT),
        space().width(Length::Fill),
        button(text("×").size(14).center())
            .width(22)
            .height(22)
            .padding(0)
            .style(theme::chip_remove)
            .on_press(StatsMessage::Close),
    ]
    .align_y(Alignment::Center);

    let mut body = column![].spacing(18);
    if stats.is_empty() {
        body = body.push(
            text("Add a tag to see how often it matches.")
                .size(theme::SMALL_SIZE)
                .color(theme::TEXT_DIM),
        );
    }
    for stat in stats {
        body = body.push(tag_block(stat, filter_on));
    }

    container(column![header, scrollable(body).height(Length::Fill).spacing(6)].spacing(12))
        .width(PANEL_W)
        .height(Length::Fill)
        .padding([10, 14])
        .style(theme::surface)
        .into()
}

fn tag_block(stat: TagStat, filter_on: bool) -> Element<'static, StatsMessage> {
    let count = if stat.complete {
        thousands(stat.count as u64)
    } else {
        format!("{}…", thousands(stat.count as u64))
    };
    let title = row![
        container(space())
            .width(8)
            .height(8)
            .style(theme::dot(stat.color)),
        text(stat.label).size(theme::UI_SIZE),
        space().width(Length::Fill),
        text(count).font(theme::MONO).size(theme::SMALL_SIZE),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut share = format!("{:.1}% of lines", stat.share);
    if filter_on {
        match stat.shown_share {
            Some(shown) => share.push_str(&format!("  ·  {shown:.1}% of shown")),
            None => share.push_str("  ·  counting shown…"),
        }
    }

    let strip = canvas_widget(DensityStrip {
        id: stat.id,
        buckets: stat.buckets,
        color: stat.color,
    })
    .width(Length::Fill)
    .height(STRIP_H);

    let end = |label: &'static str, at: Option<(i64, String)>| -> Element<'static, StatsMessage> {
        let Some((line, stamp)) = at else {
            return text(format!("{label}  —"))
                .size(theme::SMALL_SIZE)
                .color(theme::TEXT_FAINT)
                .into();
        };
        let mut content = column![
            text(format!("{label}  line {}", thousands(line as u64 + 1))).size(theme::SMALL_SIZE)
        ];
        if !stamp.is_empty() {
            content = content.push(
                text(stamp)
                    .font(theme::MONO)
                    .size(10.5)
                    .color(theme::TEXT_DIM),
            );
        }
        button(content)
            .padding([2, 4])
            .style(theme::ghost)
            .on_press(StatsMessage::JumpToLine(line))
            .into()
    };

    column![
        title,
        text(share).size(theme::SMALL_SIZE).color(theme::TEXT_DIM),
        strip,
        row![
            end("first", stat.first),
            space().width(Length::Fill),
            end("last", stat.last)
        ]
        .align_y(Alignment::Start),
    ]
    .spacing(6)
    .into()
}

/// The file as a horizontal bar, one cell per bucket, shaded by how many matches it holds.
/// Clicking a cell jumps to the first match in it.
struct DensityStrip {
    id: Uuid,
    buckets: Vec<usize>,
    color: Color,
}

impl canvas::Program<StatsMessage> for DensityStrip {
    type State = ();

    fn update(
        &self,
        _state: &mut (),
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<StatsMessage>> {
        let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else {
            return None;
        };
        let p = cursor.position_in(bounds)?;
        if self.buckets.is_empty() {
            return None;
        }
        let bucket = ((p.x / bounds.width) * self.buckets.len() as f32) as usize;
        let bucket = bucket.min(self.buckets.len() - 1);
        Some(Action::publish(StatsMessage::JumpToBucket(self.id, bucket)).and_capture())
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::BG);

        let busiest = self.buckets.iter().copied().max().unwrap_or(0);
        if busiest > 0 {
            let cell = bounds.width / self.buckets.len() as f32;
            for (i, &n) in self.buckets.iter().enumerate() {
                if n == 0 {
                    continue;
                }
                // Square-root scaling keeps a lone match visible next to a dense burst.
                let strength = (n as f32 / busiest as f32).sqrt();
                frame.fill_rectangle(
                    Point::new(i as f32 * cell, 0.0),
                    Size::new(cell.max(1.0), bounds.height),
                    theme::alpha(self.color, 0.2 + 0.8 * strength),
                );
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &(),
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}
