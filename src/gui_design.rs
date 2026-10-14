use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};
use open_room::listening::{CurvePreset, Placement};

pub const BLUE: Color32 = Color32::from_rgb(10, 132, 255);
pub const GREEN: Color32 = Color32::from_rgb(40, 160, 105);

pub fn secondary(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(168, 174, 185)
    } else {
        Color32::from_rgb(102, 109, 121)
    }
}

pub fn configure(ctx: &egui::Context) {
    // Read the font from the user's OS; never redistribute Apple's font files.
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        fonts
            .font_data
            .insert("macOS system".into(), egui::FontData::from_owned(bytes));
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "macOS system".into());
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::ThemePreference::System);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut style = (*ctx.style_of(theme)).clone();
        let dark = theme == egui::Theme::Dark;
        style.spacing.item_spacing = Vec2::new(10., 10.);
        style.spacing.button_padding = Vec2::new(12., 7.);
        style.spacing.interact_size.y = 32.;
        style.spacing.combo_width = 220.;
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(22.));
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(12.));
        style.visuals.panel_fill = if dark {
            Color32::from_rgb(28, 29, 32)
        } else {
            Color32::from_rgb(246, 247, 249)
        };
        style.visuals.window_fill = if dark {
            Color32::from_rgb(37, 38, 42)
        } else {
            Color32::WHITE
        };
        style.visuals.extreme_bg_color = style.visuals.window_fill;
        style.visuals.faint_bg_color = if dark {
            Color32::from_rgb(43, 44, 49)
        } else {
            Color32::from_rgb(239, 241, 245)
        };
        style.visuals.widgets.noninteractive.fg_stroke.color = if dark {
            Color32::from_rgb(220, 223, 230)
        } else {
            Color32::from_rgb(35, 39, 47)
        };
        style.visuals.widgets.inactive.fg_stroke.color =
            style.visuals.widgets.noninteractive.fg_stroke.color;
        style.visuals.selection.bg_fill = BLUE;
        style.visuals.selection.stroke = Stroke::new(1_f32, Color32::WHITE);
        style.visuals.hyperlink_color = if dark {
            Color32::from_rgb(90, 175, 255)
        } else {
            Color32::from_rgb(0, 100, 210)
        };
        for widget in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
        ] {
            widget.rounding = egui::Rounding::same(7.);
        }
        style.visuals.widgets.inactive.bg_fill = style.visuals.faint_bg_color;
        style.visuals.widgets.inactive.weak_bg_fill = style.visuals.faint_bg_color;
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(
            1_f32,
            if dark {
                Color32::from_rgb(65, 67, 73)
            } else {
                Color32::from_rgb(221, 224, 230)
            },
        );
        style.visuals.window_rounding = egui::Rounding::same(12.);
        ctx.set_style_of(theme, style);
    }
}

pub fn card(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::none()
        .fill(ui.visuals().window_fill)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .rounding(12.)
        .inner_margin(16.)
}
pub fn intro(ui: &mut egui::Ui, eyebrow: &str, title: &str, subtitle: &str) {
    ui.label(
        RichText::new(eyebrow)
            .small()
            .strong()
            .color(ui.visuals().hyperlink_color),
    );
    ui.label(RichText::new(title).size(30.).strong());
    ui.label(RichText::new(subtitle).color(secondary(ui)));
    ui.add_space(12.);
}
pub fn primary(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(BLUE),
    )
}
pub fn note(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::none()
        .fill(ui.visuals().faint_bg_color)
        .rounding(9.)
        .inner_margin(14.)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong());
            ui.label(body);
        });
}

pub fn preset_tile(ui: &mut egui::Ui, preset: CurvePreset, selected: bool, width: f32) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 88.), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), preset.name())
    });
    let painter = ui.painter();
    painter.rect(
        rect,
        9.,
        if selected {
            BLUE.gamma_multiply(0.12)
        } else {
            ui.visuals().window_fill
        },
        Stroke::new(
            if selected { 2_f32 } else { 1_f32 },
            if selected {
                BLUE
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke.color
            },
        ),
    );
    painter.text(
        rect.min + Vec2::new(12., 17.),
        egui::Align2::LEFT_CENTER,
        preset.name(),
        FontId::proportional(14.),
        ui.visuals().text_color(),
    );
    let graph = Rect::from_min_max(
        rect.min + Vec2::new(12., 39.),
        rect.max - Vec2::new(12., 24.),
    );
    painter.line_segment(
        [
            Pos2::new(graph.left(), graph.center().y),
            Pos2::new(graph.right(), graph.center().y),
        ],
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    let points = preset
        .points()
        .iter()
        .map(|p| {
            Pos2::new(
                graph.left() + ((p.hz / 20.).log10() / 3.) as f32 * graph.width(),
                graph.center().y - p.db as f32 * graph.height() / 14.,
            )
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(2_f32, BLUE)));
    let bass = preset.levels()[0];
    painter.text(
        rect.min + Vec2::new(12., 74.),
        egui::Align2::LEFT_CENTER,
        format!("Bass {bass:+.0} dB"),
        FontId::proportional(11.),
        secondary(ui),
    );
    let keyboard = response.has_focus()
        && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space));
    response.on_hover_text(preset.purpose()).clicked() || keyboard
}

pub fn sofa_map(
    ui: &mut egui::Ui,
    selected: &mut Placement,
    high: bool,
    states: &[(Placement, bool)],
) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 260.), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let text = secondary(ui);
    painter.text(
        rect.center_top() + Vec2::new(0., 12.),
        egui::Align2::CENTER_CENTER,
        "TOP VIEW · facing your speakers",
        FontId::proportional(11.),
        text,
    );
    let sofa = Rect::from_center_size(
        rect.center() + Vec2::new(0., 18.),
        Vec2::new(rect.width() * 0.72, 102.),
    );
    painter.rect(
        sofa,
        14.,
        ui.visuals().faint_bg_color,
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    painter.rect(
        Rect::from_min_max(sofa.min + Vec2::new(8., 76.), sofa.max - Vec2::new(8., 8.)),
        6.,
        ui.visuals().widgets.inactive.bg_fill,
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    for x in [1., 2.] {
        let x = sofa.left() + sofa.width() * x / 3.;
        painter.line_segment(
            [
                Pos2::new(x, sofa.top() + 8.),
                Pos2::new(x, sofa.bottom() - 30.),
            ],
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
    }
    painter.text(
        Pos2::new(rect.left() + 16., rect.center().y - 76.),
        egui::Align2::LEFT_CENTER,
        "FRONT",
        FontId::proportional(10.),
        text,
    );
    painter.text(
        Pos2::new(rect.left() + 16., rect.center().y + 100.),
        egui::Align2::LEFT_CENTER,
        "BACK",
        FontId::proportional(10.),
        text,
    );
    for p in Placement::ALL {
        let [x, y, z] = p.axes();
        if p != Placement::Main && (z > 0.) != high {
            continue;
        }
        let centre = rect.center() + Vec2::new(x * rect.width() * 0.34, y * 78.);
        let done = states
            .iter()
            .any(|(placed, included)| *placed == p && *included);
        let pending = !done && states.iter().any(|(placed, _)| *placed == p);
        let hit = Rect::from_center_size(centre, Vec2::splat(38.));
        let response = ui.interact(
            hit,
            ui.id().with(("point", p.number())),
            egui::Sense::click(),
        );
        if response.clicked() {
            *selected = p;
        }
        let fill = if *selected == p {
            BLUE
        } else if done {
            GREEN
        } else {
            ui.visuals().window_fill
        };
        painter.circle(
            centre,
            16.,
            fill,
            Stroke::new(
                if *selected == p { 2_f32 } else { 1.5_f32 },
                if pending {
                    ui.visuals().warn_fg_color
                } else {
                    BLUE
                },
            ),
        );
        painter.text(
            centre,
            egui::Align2::CENTER_CENTER,
            p.number().to_string(),
            FontId::proportional(13.),
            if done || *selected == p {
                Color32::WHITE
            } else {
                ui.visuals().text_color()
            },
        );
        response.on_hover_text(format!("{} · {}", p.number(), p.label()));
    }
    painter.text(
        rect.center_bottom() - Vec2::new(0., 8.),
        egui::Align2::CENTER_CENTER,
        "Left and right are from your seated perspective",
        FontId::proportional(11.),
        text,
    );
}

pub fn elevation(ui: &mut egui::Ui, selected: Placement, height: f32, depth: f32) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 260.), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let text = secondary(ui);
    p.text(
        rect.center_top() + Vec2::new(0., 12.),
        egui::Align2::CENTER_CENTER,
        "SIDE VIEW · microphone height",
        FontId::proportional(11.),
        text,
    );
    let centre = rect.center();
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    let sofa = Rect::from_min_max(centre + Vec2::new(-20., 36.), centre + Vec2::new(90., 102.));
    p.rect(sofa, 10., ui.visuals().faint_bg_color, stroke);
    p.rect(
        Rect::from_min_max(centre + Vec2::new(68., -8.), centre + Vec2::new(92., 80.)),
        8.,
        ui.visuals().faint_bg_color,
        stroke,
    );
    p.line_segment(
        [
            Pos2::new(rect.left() + 12., centre.y),
            Pos2::new(rect.right() - 12., centre.y),
        ],
        Stroke::new(1_f32, BLUE.gamma_multiply(0.4)),
    );
    p.text(
        centre + Vec2::new(0., -14.),
        egui::Align2::CENTER_CENTER,
        "Seated ear level · point 1",
        FontId::proportional(11.),
        text,
    );
    for y in [-58., 58.] {
        for x in [-72., 72.] {
            p.circle(
                centre + Vec2::new(x, y),
                7.,
                ui.visuals().window_fill,
                Stroke::new(1.5_f32, BLUE),
            );
        }
    }
    p.text(
        centre + Vec2::new(0., -80.),
        egui::Align2::CENTER_CENTER,
        format!("HIGH · {height:.0} cm above ears"),
        FontId::proportional(11.),
        text,
    );
    p.text(
        centre + Vec2::new(0., 82.),
        egui::Align2::CENTER_CENTER,
        format!("LOW · {height:.0} cm below ears"),
        FontId::proportional(11.),
        text,
    );
    let [_, y, z] = selected.axes();
    let mic = centre + Vec2::new(y * 72., -z * 58.);
    p.line_segment(
        [mic + Vec2::new(0., 12.), mic + Vec2::new(0., 34.)],
        Stroke::new(2_f32, BLUE),
    );
    p.circle(mic, 11., BLUE, Stroke::NONE);
    p.text(
        mic,
        egui::Align2::CENTER_CENTER,
        selected.number().to_string(),
        FontId::proportional(11.),
        Color32::WHITE,
    );
    p.text(
        rect.center_bottom() - Vec2::new(0., 8.),
        egui::Align2::CENTER_CENTER,
        format!("Front / back · {depth:.0} cm from point 1"),
        FontId::proportional(11.),
        text,
    );
}
