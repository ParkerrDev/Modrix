// SPDX-License-Identifier: GPL-2.0-only
//! Tiny widget-drawn icons (canvas triangles, grip bars, status dots).
//!
//! Drawn, not typed: decorative Unicode glyphs render as tofu on systems
//! whose default sans lacks them, so every icon here is real geometry.

use iced::widget::{canvas, column, container};
use iced::{Color, Rectangle, Renderer, Theme, mouse};

use crate::theme;

/// A small filled triangle pointing up or down.
struct Triangle {
    up: bool,
    color: Color,
}

impl<Message> canvas::Program<Message> for Triangle {
    type State = ();

    fn draw(
        &self,
        (): &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (frame.width(), frame.height());
        let path = canvas::Path::new(|b| {
            if self.up {
                b.move_to(iced::Point::new(0.0, h));
                b.line_to(iced::Point::new(w, h));
                b.line_to(iced::Point::new(w / 2.0, 0.0));
            } else {
                b.move_to(iced::Point::new(0.0, 0.0));
                b.line_to(iced::Point::new(w, 0.0));
                b.line_to(iced::Point::new(w / 2.0, h));
            }
            b.close();
        });
        frame.fill(&path, self.color);
        vec![frame.into_geometry()]
    }
}

/// An up/down arrow glyph as an element (for the reorder buttons).
pub fn arrow<'a, M: 'a>(up: bool) -> iced::Element<'a, M> {
    canvas(Triangle {
        up,
        color: theme::MUTED,
    })
    .width(10)
    .height(8)
    .into()
}

/// Three horizontal grip bars (the drag handle).
pub fn grip<'a, M: 'a>() -> iced::Element<'a, M> {
    let bar = || {
        container(iced::widget::Space::new(14, 2)).style(|_: &Theme| {
            iced::widget::container::Style {
                background: Some(iced::Background::Color(theme::FAINT)),
                border: iced::Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 2.0.into(),
                },
                ..iced::widget::container::Style::default()
            }
        })
    };
    container(column![bar(), bar(), bar()].spacing(2))
        .padding([4, 2])
        .into()
}

/// A bell glyph with a status dot on its shoulder, both drawn as geometry.
struct Bell {
    body: Color,
    dot: Color,
}

impl<Message> canvas::Program<Message> for Bell {
    type State = ();

    fn draw(
        &self,
        (): &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (frame.width(), frame.height());
        // Bell body: a rounded dome over a flared rim.
        let dome = canvas::Path::new(|b| {
            b.move_to(iced::Point::new(w * 0.2, h * 0.72));
            b.line_to(iced::Point::new(w * 0.8, h * 0.72));
            b.line_to(iced::Point::new(w * 0.68, h * 0.6));
            b.line_to(iced::Point::new(w * 0.68, h * 0.38));
            b.quadratic_curve_to(
                iced::Point::new(w * 0.5, h * 0.12),
                iced::Point::new(w * 0.32, h * 0.38),
            );
            b.line_to(iced::Point::new(w * 0.32, h * 0.6));
            b.close();
        });
        frame.fill(&dome, self.body);
        // Clapper.
        let clapper = canvas::Path::circle(iced::Point::new(w * 0.5, h * 0.82), h * 0.07);
        frame.fill(&clapper, self.body);
        // Status dot.
        let dot = canvas::Path::circle(iced::Point::new(w * 0.78, h * 0.28), h * 0.2);
        frame.fill(&dot, self.dot);
        vec![frame.into_geometry()]
    }
}

/// The notification bell with a coloured status dot.
pub fn bell<'a, M: 'a>(body: Color, dot: Color) -> iced::Element<'a, M> {
    canvas(Bell { body, dot }).width(18).height(18).into()
}

/// A small colored circle (status/notification dot).
pub fn dot<'a, M: 'a>(size: f32, color: Color) -> iced::Element<'a, M> {
    container(iced::widget::Space::new(size, size))
        .style(move |_: &Theme| iced::widget::container::Style {
            background: Some(iced::Background::Color(color)),
            border: iced::Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 99.0.into(),
            },
            ..iced::widget::container::Style::default()
        })
        .into()
}
