//! Custom window chrome: title bar with minimize / maximize / close, drag, and edge resizing.
//! The window is created without OS decorations (see main.rs).

use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Rect, ResizeDirection, Sense, Stroke,
    StrokeKind, Ui, ViewportCommand, pos2, vec2,
};

use super::ZapretHubApp;
use super::theme::{palette, status_dot};
use crate::core::build_info::APP_NAME;

const TITLE_BAR_HEIGHT: f32 = 38.0;
const BUTTON_WIDTH: f32 = 46.0;
const RESIZE_MARGIN: f32 = 5.0;

impl ZapretHubApp {
    pub(super) fn title_bar(&mut self, ui: &mut Ui) {
        let palette = palette(ui);
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));

        egui::Panel::top("title_bar")
            .exact_size(TITLE_BAR_HEIGHT)
            .frame(Frame::new().fill(palette.sidebar))
            .show(ui, |ui| {
                let bar = ui.max_rect();

                // Drag area first, so the buttons drawn after it win the hit test.
                let drag =
                    ui.interact(bar, egui::Id::unique("title_drag"), Sense::click_and_drag());
                if drag.double_clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                } else if drag.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }

                ui.painter().line_segment(
                    [bar.left_bottom(), bar.right_bottom()],
                    Stroke::new(1.0, palette.border),
                );

                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(bar.shrink2(vec2(14.0, 0.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        let (color, text) = self.state_summary(ui);
                        status_dot(ui, color, 9.0);
                        ui.label(egui::RichText::new(APP_NAME).strong());
                        ui.label(egui::RichText::new(text).small().color(palette.muted));
                    },
                );

                let mut x = bar.right();
                let close = window_button(ui, &mut x, bar, Glyph::Close, "Закрыть");
                let maximize = window_button(
                    ui,
                    &mut x,
                    bar,
                    if maximized {
                        Glyph::Restore
                    } else {
                        Glyph::Maximize
                    },
                    if maximized {
                        "Восстановить"
                    } else {
                        "Развернуть"
                    },
                );
                let minimize = window_button(ui, &mut x, bar, Glyph::Minimize, "Свернуть");

                if close.clicked() {
                    // Goes through the close handler: hides to tray or quits, as configured.
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                if maximize.clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }
                if minimize.clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                }
            });
    }

    /// Thin border around the borderless window plus resize handles on its edges.
    pub(super) fn window_frame_and_resize(&mut self, ctx: &egui::Context) {
        let (screen, maximized, fullscreen) = ctx.input(|input| {
            (
                input.viewport_rect(),
                input.viewport().maximized.unwrap_or(false),
                input.viewport().fullscreen.unwrap_or(false),
            )
        });
        if maximized || fullscreen {
            return;
        }

        let border = palette_for(ctx).border;
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("window_border"),
        ))
        .rect_stroke(
            screen.shrink(0.5),
            CornerRadius::ZERO,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );

        let Some(pointer) = ctx.input(|input| input.pointer.hover_pos()) else {
            return;
        };
        let Some(direction) = resize_direction(screen, pointer) else {
            return;
        };
        ctx.set_cursor_icon(cursor_for(direction));
        if ctx.input(|input| input.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

fn palette_for(ctx: &egui::Context) -> super::theme::Palette {
    super::theme::palette_dark(ctx.global_style().visuals.dark_mode)
}

fn resize_direction(screen: Rect, pointer: egui::Pos2) -> Option<ResizeDirection> {
    let left = pointer.x - screen.left() < RESIZE_MARGIN;
    let right = screen.right() - pointer.x < RESIZE_MARGIN;
    let top = pointer.y - screen.top() < RESIZE_MARGIN;
    let bottom = screen.bottom() - pointer.y < RESIZE_MARGIN;
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (_, true, true, _) => ResizeDirection::NorthEast,
        (true, _, _, true) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::West,
        (_, true, ..) => ResizeDirection::East,
        (_, _, true, _) => ResizeDirection::North,
        (_, _, _, true) => ResizeDirection::South,
        _ => return None,
    })
}

fn cursor_for(direction: ResizeDirection) -> egui::CursorIcon {
    match direction {
        ResizeDirection::North | ResizeDirection::South => egui::CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => egui::CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => egui::CursorIcon::ResizeNeSw,
    }
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// One caption button, laid out right to left. `x` is the right edge of the next button.
fn window_button(
    ui: &mut Ui,
    x: &mut f32,
    bar: Rect,
    glyph: Glyph,
    tooltip: &str,
) -> egui::Response {
    let palette = palette(ui);
    let rect = Rect::from_min_size(
        pos2(*x - BUTTON_WIDTH, bar.top()),
        vec2(BUTTON_WIDTH, bar.height()),
    );
    *x -= BUTTON_WIDTH;

    let response = ui
        .interact(rect, egui::Id::unique(("caption", tooltip)), Sense::click())
        .on_hover_text(tooltip);
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);

    let is_close = matches!(glyph, Glyph::Close);
    let hover_fill = if is_close {
        Color32::from_rgb(232, 17, 35)
    } else {
        palette.card_hover
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, hover_fill.gamma_multiply(hover));
    let ink = if is_close && hover > 0.5 {
        Color32::WHITE
    } else {
        palette.text
    };
    let stroke = Stroke::new(1.3, ink);
    let c = rect.center();
    let painter = ui.painter();
    match glyph {
        Glyph::Minimize => {
            painter.line_segment([c + vec2(-5.0, 0.5), c + vec2(5.0, 0.5)], stroke);
        }
        Glyph::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(10.0, 10.0)),
                CornerRadius::ZERO,
                stroke,
                StrokeKind::Inside,
            );
        }
        Glyph::Restore => {
            painter.rect_stroke(
                Rect::from_center_size(c + vec2(-1.0, 1.0), vec2(8.0, 8.0)),
                CornerRadius::ZERO,
                stroke,
                StrokeKind::Inside,
            );
            painter.line_segment([c + vec2(-1.0, -5.0), c + vec2(5.0, -5.0)], stroke);
            painter.line_segment([c + vec2(5.0, -5.0), c + vec2(5.0, 3.0)], stroke);
        }
        Glyph::Close => {
            painter.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], stroke);
            painter.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], stroke);
        }
    }
    response
}
