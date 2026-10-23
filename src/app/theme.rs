//! Colours, text styles and the handful of custom widgets the pages are built from.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Frame, Layout, Margin, Response,
    RichText, Role, Sense, Stroke, TextStyle, Theme, Ui, Vec2, WidgetInfo, pos2, vec2,
};

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) bg: Color32,
    pub(crate) sidebar: Color32,
    pub(crate) card: Color32,
    pub(crate) card_hover: Color32,
    pub(crate) border: Color32,
    pub(crate) text: Color32,
    pub(crate) muted: Color32,
    pub(crate) accent: Color32,
    pub(crate) accent_soft: Color32,
    pub(crate) on_accent: Color32,
    pub(crate) success: Color32,
    pub(crate) warning: Color32,
    pub(crate) danger: Color32,
}

const DARK: Palette = Palette {
    bg: Color32::from_rgb(15, 18, 24),
    sidebar: Color32::from_rgb(11, 13, 18),
    card: Color32::from_rgb(23, 27, 35),
    card_hover: Color32::from_rgb(30, 35, 45),
    border: Color32::from_rgb(38, 44, 56),
    text: Color32::from_rgb(230, 233, 239),
    muted: Color32::from_rgb(139, 148, 165),
    accent: Color32::from_rgb(79, 140, 255),
    accent_soft: Color32::from_rgb(30, 45, 74),
    on_accent: Color32::WHITE,
    success: Color32::from_rgb(34, 197, 94),
    warning: Color32::from_rgb(245, 158, 11),
    danger: Color32::from_rgb(239, 68, 68),
};

const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(244, 246, 250),
    sidebar: Color32::from_rgb(234, 237, 243),
    card: Color32::WHITE,
    card_hover: Color32::from_rgb(246, 248, 252),
    border: Color32::from_rgb(222, 227, 235),
    text: Color32::from_rgb(24, 32, 44),
    muted: Color32::from_rgb(95, 107, 125),
    accent: Color32::from_rgb(37, 99, 235),
    accent_soft: Color32::from_rgb(222, 233, 253),
    on_accent: Color32::WHITE,
    success: Color32::from_rgb(22, 163, 74),
    warning: Color32::from_rgb(202, 120, 6),
    danger: Color32::from_rgb(220, 38, 38),
};

pub(crate) fn palette(ui: &Ui) -> Palette {
    palette_dark(ui.visuals().dark_mode)
}

pub(crate) fn palette_dark(dark: bool) -> Palette {
    if dark { DARK } else { LIGHT }
}

/// Installs the look for both themes; eframe then follows the Windows light/dark setting live.
pub(crate) fn install(ctx: &egui::Context) {
    ctx.style_mut_of(Theme::Dark, |style| configure(style, &DARK, true));
    ctx.style_mut_of(Theme::Light, |style| configure(style, &LIGHT, false));
}

fn configure(style: &mut egui::Style, palette: &Palette, dark: bool) {
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
    ]
    .into();

    style.spacing.item_spacing = vec2(8.0, 8.0);
    style.spacing.button_padding = vec2(12.0, 6.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.combo_height = 320.0;
    style.spacing.scroll.floating = true;
    style.animation_time = 0.14;

    let visuals = &mut style.visuals;
    *visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.override_text_color = Some(palette.text);
    visuals.weak_text_color = Some(palette.muted);
    visuals.hyperlink_color = palette.accent;
    visuals.panel_fill = palette.bg;
    visuals.window_fill = palette.card;
    visuals.window_stroke = Stroke::new(1.0, palette.border);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.extreme_bg_color = if dark {
        Color32::from_rgb(12, 14, 19)
    } else {
        Color32::WHITE
    };
    visuals.faint_bg_color = palette.card_hover;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);

    let radius = CornerRadius::same(8);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = palette.card;
    widgets.noninteractive.weak_bg_fill = palette.card;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.noninteractive.corner_radius = radius;

    widgets.inactive.bg_fill = palette.card_hover;
    widgets.inactive.weak_bg_fill = palette.card_hover;
    widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border);
    widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.inactive.corner_radius = radius;

    widgets.hovered.bg_fill = palette.accent_soft;
    widgets.hovered.weak_bg_fill = palette.accent_soft;
    widgets.hovered.bg_stroke = Stroke::new(1.0, palette.accent);
    widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.hovered.corner_radius = radius;
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = palette.accent;
    widgets.active.weak_bg_fill = palette.accent;
    widgets.active.bg_stroke = Stroke::new(1.0, palette.accent);
    widgets.active.fg_stroke = Stroke::new(1.0, palette.on_accent);
    widgets.active.corner_radius = radius;
    widgets.active.expansion = 0.0;

    widgets.open.bg_fill = palette.card_hover;
    widgets.open.weak_bg_fill = palette.card_hover;
    widgets.open.bg_stroke = Stroke::new(1.0, palette.accent);
    widgets.open.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.open.corner_radius = radius;
}

pub(crate) fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = palette(ui);
    Frame::new()
        .fill(palette.card)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            // Columns hand out justified layouts, which would stretch wrapped text.
            ui.with_layout(Layout::top_down(Align::Min), add_contents)
                .inner
        })
        .inner
}

pub(crate) fn card_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(16.0).strong());
    if !subtitle.is_empty() {
        ui.add_space(-4.0);
        ui.label(RichText::new(subtitle).color(palette(ui).muted));
    }
    ui.add_space(6.0);
}

pub(crate) fn page_title(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).heading().strong());
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(palette(ui).muted));
    }
    ui.add_space(10.0);
}

/// Link that opens through `platform::open_url` (see there for why not `ui.hyperlink_to`).
pub(crate) fn link(ui: &mut Ui, text: &str, url: &str) {
    let response = ui
        .add(egui::Link::new(text))
        .on_hover_text(url)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() || response.middle_clicked() {
        crate::platform::open_url(url);
    }
}

pub(crate) fn hint(ui: &mut Ui, text: impl Into<String>) {
    let muted = palette(ui).muted;
    ui.add(egui::Label::new(RichText::new(text.into()).small().color(muted)).wrap());
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tone {
    Info,
    Warning,
    Danger,
}

pub(crate) fn tone_color(ui: &Ui, tone: Tone) -> Color32 {
    let palette = palette(ui);
    match tone {
        Tone::Info => palette.accent,
        Tone::Warning => palette.warning,
        Tone::Danger => palette.danger,
    }
}

/// Tinted box with a coloured left edge, used for warnings and inline explanations.
pub(crate) fn callout<R>(ui: &mut Ui, tone: Tone, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let color = tone_color(ui, tone);
    let response = Frame::new()
        .fill(color.gamma_multiply(0.12))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin {
            left: 14,
            right: 12,
            top: 10,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        });
    let rect = response.response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
        CornerRadius {
            nw: 8,
            sw: 8,
            ne: 0,
            se: 0,
        },
        color,
    );
    response.inner
}

pub(crate) fn callout_text(ui: &mut Ui, tone: Tone, text: impl Into<String>) {
    let text = text.into();
    callout(ui, tone, |ui| {
        ui.add(egui::Label::new(text).wrap());
    });
}

pub(crate) fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().strong().color(color));
        })
        .response
}

pub(crate) fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let palette = palette(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).strong().color(palette.on_accent))
            .fill(palette.accent)
            .stroke(Stroke::NONE)
            .min_size(vec2(0.0, 34.0)),
    )
}

pub(crate) fn secondary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    ui.add_enabled(enabled, egui::Button::new(text).min_size(vec2(0.0, 34.0)))
}

pub(crate) fn danger_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let danger = palette(ui).danger;
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(danger))
            .stroke(Stroke::new(1.0, danger.gamma_multiply(0.6)))
            .min_size(vec2(0.0, 34.0)),
    )
}

/// iOS-style switch.
pub(crate) fn toggle(ui: &mut Ui, on: &mut bool, enabled: bool) -> Response {
    let palette = palette(ui);
    let size = vec2(40.0, 22.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(size, sense);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(Role::Switch, enabled, *on, ""));

    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let off_fill = if ui.visuals().dark_mode {
            Color32::from_rgb(55, 62, 76)
        } else {
            Color32::from_rgb(203, 210, 222)
        };
        let mut fill = lerp_color(off_fill, palette.accent, t);
        if !enabled {
            fill = fill.gamma_multiply(0.45);
        }
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, radius, fill);
        let knob_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob_radius = radius - 3.0 + if response.hovered() { 0.5 } else { 0.0 };
        ui.painter()
            .circle_filled(pos2(knob_x, rect.center().y), knob_radius, Color32::WHITE);
    }
    response
}

/// Title + explanation on the left, switch on the right. Returns true when toggled.
pub(crate) fn toggle_row(
    ui: &mut Ui,
    title: &str,
    description: &str,
    value: &mut bool,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = toggle(ui, value, enabled).changed();
            ui.add_space(12.0);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                let text = palette(ui).text;
                let title_color = if enabled {
                    text
                } else {
                    text.gamma_multiply(0.55)
                };
                ui.label(RichText::new(title).color(title_color));
                if !description.is_empty() {
                    ui.add_space(-6.0);
                    hint(ui, description);
                }
            });
        });
    });
    changed
}

/// A row of mutually exclusive buttons. Returns true when the selection changed.
pub(crate) fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
    enabled: bool,
) -> bool {
    let palette = palette(ui);
    let mut changed = false;
    Frame::new()
        .fill(palette.card_hover)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (option, label) in options {
                    let selected = *value == *option;
                    let text = if selected {
                        RichText::new(*label).strong().color(palette.on_accent)
                    } else {
                        RichText::new(*label)
                    };
                    let button = egui::Button::new(text)
                        .fill(if selected {
                            palette.accent
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(7))
                        .min_size(vec2(0.0, 26.0));
                    if ui.add_enabled(enabled, button).clicked() && !selected {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

pub(crate) fn kv_row(ui: &mut Ui, key: &str, value: impl Into<RichText>) {
    let muted = palette(ui).muted;
    ui.horizontal(|ui| {
        ui.label(RichText::new(key).color(muted));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(value.into()).truncate());
        });
    });
}

pub(crate) fn status_dot(ui: &mut Ui, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, color);
}

/// The big round on/off button on the home page.
pub(crate) fn power_button(ui: &mut Ui, on: bool, busy: bool, enabled: bool) -> Response {
    let palette = palette(ui);
    let diameter = 132.0;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), sense);
    response.widget_info(|| {
        WidgetInfo::labeled(
            Role::Button,
            enabled,
            if on {
                "Выключить обход"
            } else {
                "Включить обход"
            },
        )
    });

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let center = rect.center();
        let radius = diameter / 2.0;
        let t = ui.ctx().animate_bool_responsive(response.id, on);
        let hover = ui
            .ctx()
            .animate_bool_responsive(response.id.with("hover"), response.hovered() && enabled);

        let ring = lerp_color(palette.border, palette.success, t);
        // Soft halo when on.
        if t > 0.0 {
            painter.circle_filled(
                center,
                radius + 6.0,
                palette.success.gamma_multiply(0.12 * t),
            );
        }
        let fill = lerp_color(palette.card_hover, palette.success.gamma_multiply(0.22), t);
        painter.circle_filled(center, radius - 2.0, fill);
        painter.circle_stroke(center, radius - 2.0, Stroke::new(2.0 + 1.5 * hover, ring));

        let mut glyph_color = lerp_color(palette.muted, palette.success, t);
        if hover > 0.0 && !on {
            glyph_color = lerp_color(glyph_color, palette.accent, hover);
        }
        if !enabled {
            glyph_color = glyph_color.gamma_multiply(0.5);
        }

        if busy {
            let spinner_rect = egui::Rect::from_center_size(center, Vec2::splat(44.0));
            egui::Spinner::new()
                .size(44.0)
                .color(glyph_color)
                .paint_at(ui, spinner_rect);
        } else {
            paint_power_glyph(painter, center, 24.0, Stroke::new(4.0, glyph_color));
        }
    }

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_power_glyph(painter: &egui::Painter, center: egui::Pos2, radius: f32, stroke: Stroke) {
    // Arc with a gap at the top, plus a vertical bar through the gap.
    let gap = 0.9_f32;
    let segments = 40;
    let start = -std::f32::consts::FRAC_PI_2 + gap / 2.0;
    let end = start + std::f32::consts::TAU - gap;
    let points: Vec<_> = (0..=segments)
        .map(|i| {
            let angle = egui::lerp(start..=end, i as f32 / segments as f32);
            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(points, stroke));
    painter.line_segment(
        [
            center - vec2(0.0, radius + 4.0),
            center - vec2(0.0, radius * 0.25),
        ],
        stroke,
    );
}

fn lerp_color(from: Color32, to: Color32, t: f32) -> Color32 {
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        lerp(from.r(), to.r()),
        lerp(from.g(), to.g()),
        lerp(from.b(), to.b()),
        lerp(from.a(), to.a()),
    )
}

pub(crate) const NAV_ITEM_HEIGHT: f32 = 38.0;

/// Sidebar navigation entry.
pub(crate) fn nav_item(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    selected: bool,
    badge: bool,
) -> Response {
    let palette = palette(ui);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, NAV_ITEM_HEIGHT), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Tab, true, selected, label));

    if ui.is_rect_visible(rect) {
        let hover = ui
            .ctx()
            .animate_bool_responsive(response.id, response.hovered() || selected);
        // The selected entry is highlighted by the sliding pill drawn in `sidebar`.
        if !selected {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(8),
                palette.card_hover.gamma_multiply(hover),
            );
        }
        let color = if selected {
            palette.accent
        } else {
            palette.text
        };
        let painter = ui.painter();
        painter.text(
            pos2(rect.left() + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            icon,
            FontId::proportional(16.0),
            color,
        );
        painter.text(
            pos2(rect.left() + 42.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.5),
            color,
        );
        if badge {
            painter.circle_filled(
                pos2(rect.right() - 14.0, rect.center().y),
                4.0,
                palette.warning,
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(crate) fn progress(ui: &mut Ui, fraction: Option<f32>, text: impl Into<String>) {
    let palette = palette(ui);
    let bar = match fraction {
        Some(fraction) => egui::ProgressBar::new(fraction.clamp(0.0, 1.0)),
        None => egui::ProgressBar::new(0.0).animate(true),
    };
    ui.add(
        bar.desired_height(8.0)
            .fill(palette.accent)
            .corner_radius(CornerRadius::same(4)),
    );
    hint(ui, text);
}
