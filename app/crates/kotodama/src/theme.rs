//! Kotodama's look (the user's pick, 2026-10-06: "Ember", with the voice-wave icon; near-black, the user's ask): warm near-black, a red-to-amber
//! accent, rounded cards, the system's own UI font (fonts.rs). Dark only: it sits next to a game.
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Mesh, Pos2, Rect, RichText, Sense,
    Stroke, TextStyle, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(0x0b, 0x09, 0x09);
pub const CARD: Color32 = Color32::from_rgb(0x16, 0x11, 0x11);
/// inputs, the log, the "hearing" box: sunk below the cards
pub const SUNK: Color32 = Color32::from_rgb(0x05, 0x04, 0x04);
pub const LINE: Color32 = Color32::from_rgb(0x2a, 0x20, 0x1f);
pub const WIDGET: Color32 = Color32::from_rgb(0x20, 0x18, 0x17);
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x2c, 0x21, 0x1f);
pub const FG: Color32 = Color32::from_rgb(0xee, 0xe7, 0xe6);
pub const MUTED: Color32 = Color32::from_rgb(0xa8, 0x98, 0x96);
pub const RED: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);
pub const AMBER: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);
/// the accent where one colour is needed (between the two)
pub const ACCENT: Color32 = Color32::from_rgb(0xf2, 0x6d, 0x2a);
/// light accent text (chips, what is being heard)
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0xfd, 0xba, 0x74);
pub const GOOD: Color32 = Color32::from_rgb(0x34, 0xd3, 0x99);
pub const WARN: Color32 = AMBER;
pub const BAD: Color32 = RED;

/// The family of the semibold face (fonts.rs registers it; the regular one where there is none).
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn apply(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark)); // (a dark title bar)
    ctx.set_theme(egui::ThemePreference::Dark); // (dark whatever the system's setting)
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.extreme_bg_color = SUNK;
    v.faint_bg_color = CARD;
    v.code_bg_color = SUNK;
    v.hyperlink_color = ACCENT_TEXT;
    v.warn_fg_color = WARN;
    v.error_fg_color = BAD;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0, ACCENT_TEXT);
    v.slider_trailing_fill = true;
    let r = CornerRadius::same(8);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = r;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.weak_bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, FG);
    w.noninteractive.corner_radius = r;
    for (s, fill, stroke) in [
        (&mut w.inactive, WIDGET, LINE),
        (&mut w.hovered, WIDGET_HOVER, ACCENT.gamma_multiply(0.7)),
        (&mut w.active, WIDGET_HOVER, ACCENT),
        (&mut w.open, WIDGET_HOVER, ACCENT.gamma_multiply(0.7)),
    ] {
        s.bg_fill = fill;
        s.weak_bg_fill = fill;
        s.bg_stroke = Stroke::new(1.0, stroke);
        s.fg_stroke = Stroke::new(1.5, FG);
        s.corner_radius = r;
        s.expansion = 0.0;
    }
    style.visuals = v;
    let sp = &mut style.spacing;
    sp.item_spacing = Vec2::new(10.0, 8.0);
    sp.button_padding = Vec2::new(12.0, 5.0);
    sp.interact_size = Vec2::new(40.0, 26.0);
    sp.slider_width = 300.0;
    sp.combo_width = 300.0;
    sp.window_margin = Margin::same(12);
    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (TextStyle::Heading, FontId::new(19.0, semibold())),
        (
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        ),
    ]
    .into();
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// A section: a rounded card with a small upper-case title.
pub fn card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(title.to_uppercase())
                    .size(11.5)
                    .color(MUTED)
                    .family(semibold()),
            );
            ui.add_space(2.0);
            add(ui)
        })
        .inner
}

/// A sunk box (the log, what is being heard).
pub fn sunk<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(SUNK)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A rounded tag in the accent (a language in use).
pub fn chip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(ACCENT.gamma_multiply(0.16))
        .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(9, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .size(13.0)
                    .color(ACCENT_TEXT)
                    .family(semibold()),
            );
        });
}

/// A small state label in a colour (loaded, loading...).
pub fn pill(ui: &mut egui::Ui, text: &str, colour: Color32) {
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(8, 1))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .size(12.0)
                    .color(colour)
                    .family(semibold()),
            );
        });
}

/// A dot in a colour (the connection, each model).
pub fn dot(ui: &mut egui::Ui, colour: Color32, glow: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(12.0, 16.0), Sense::hover());
    if glow {
        ui.painter()
            .circle_filled(rect.center(), 6.5, colour.gamma_multiply(0.25));
    }
    ui.painter().circle_filled(rect.center(), 4.0, colour);
}

/// The main button: the red-to-amber gradient.
pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        FontId::new(14.0, semibold()),
        Color32::WHITE,
    );
    let size = galley.size() + Vec2::new(26.0, 12.0);
    let (rect, resp) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let t = if !enabled {
        0.45
    } else if resp.hovered() {
        1.0
    } else {
        0.9
    };
    gradient(
        ui.painter(),
        rect,
        RED.gamma_multiply(t),
        AMBER.gamma_multiply(t),
        8.0,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A level meter: frac of its width in the gradient, the rest sunk.
pub fn meter(ui: &mut egui::Ui, frac: f32, size: Vec2, bright: bool) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), SUNK);
    if frac > 0.0 {
        let mut r = rect;
        r.set_width(rect.width() * frac.clamp(0.0, 1.0));
        let t = if bright { 1.0 } else { 0.7 };
        gradient(
            ui.painter(),
            r,
            RED.gamma_multiply(t),
            AMBER.gamma_multiply(t),
            4.0,
        );
    }
}

/// A left-to-right gradient in a rectangle with rounded ends: the ends' caps in the end colours, the strip between
/// them drawn over (a mesh, one colour per vertex column).
fn gradient(p: &egui::Painter, rect: Rect, a: Color32, b: Color32, radius: f32) {
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    if r > 0.0 {
        let cr = CornerRadius::same(r as u8);
        p.rect_filled(
            Rect::from_min_max(rect.min, Pos2::new(rect.left() + 2.0 * r, rect.bottom())),
            cr,
            a,
        );
        p.rect_filled(
            Rect::from_min_max(Pos2::new(rect.right() - 2.0 * r, rect.top()), rect.max),
            cr,
            b,
        );
    }
    let (x0, x1) = (rect.left() + r, rect.right() - r);
    if x1 <= x0 {
        return;
    }
    let mut mesh = Mesh::default();
    let n = 24;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = x0 + (x1 - x0) * t;
        let c = lerp(a, b, t);
        mesh.colored_vertex(Pos2::new(x, rect.top()), c);
        mesh.colored_vertex(Pos2::new(x, rect.bottom()), c);
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    p.add(mesh);
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        m(a.r(), b.r()),
        m(a.g(), b.g()),
        m(a.b(), b.b()),
        m(a.a(), b.a()),
    )
}

/// A game mod as the picker shows it: a tile with the game's initial in the gradient, the game, the mod under it,
/// and a badge (a community profile).
fn game_face(ui: &egui::Ui, rect: Rect, game: &str, mod_name: &str, hovered: bool, selected: bool, badge: Option<&str>) {
    let p = ui.painter();
    let fill = if hovered { WIDGET_HOVER } else if selected { CARD } else { WIDGET };
    p.rect_filled(rect, CornerRadius::same(10), fill);
    let stroke = if hovered { ACCENT.gamma_multiply(0.7) } else { LINE };
    p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    let tile = Rect::from_min_size(rect.min + Vec2::new(8.0, (rect.height() - 30.0) / 2.0), Vec2::splat(30.0));
    gradient(p, tile, RED, AMBER, 8.0);
    // (the mod is the title - it is what links to Kotodama; the game it runs in under it)
    let letter = mod_name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    p.text(tile.center(), egui::Align2::CENTER_CENTER, letter, FontId::new(16.0, semibold()), Color32::WHITE);
    let x = tile.right() + 10.0;
    p.text(Pos2::new(x, rect.center().y - 9.0), egui::Align2::LEFT_CENTER, mod_name, FontId::new(14.5, semibold()), FG);
    let sub = p.text(
        Pos2::new(x, rect.center().y + 9.0),
        egui::Align2::LEFT_CENTER,
        game,
        FontId::new(12.5, FontFamily::Proportional),
        MUTED,
    );
    if let Some(b) = badge {
        let g = p.layout_no_wrap(b.to_string(), FontId::new(10.5, semibold()), ACCENT_TEXT);
        let r = Rect::from_min_size(Pos2::new(sub.right() + 8.0, sub.center().y - 8.0), g.size() + Vec2::new(12.0, 4.0));
        p.rect_filled(r, CornerRadius::same(255), ACCENT.gamma_multiply(0.18));
        p.galley(r.min + Vec2::new(6.0, 2.0), g, ACCENT_TEXT);
    }
}

/// The game mod picker's button: the chosen one, and a chevron.
pub fn game_button(ui: &mut egui::Ui, game: &str, mod_name: &str, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::click());
    game_face(ui, rect, game, mod_name, resp.hovered(), false, None);
    let c = Pos2::new(rect.right() - 18.0, rect.center().y);
    ui.painter().line(vec![c + Vec2::new(-5.0, -2.5), c + Vec2::new(0.0, 2.5), c + Vec2::new(5.0, -2.5)], Stroke::new(1.8, MUTED));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A game mod in the picker's list (community: a profile file, not built in).
pub fn game_row(ui: &mut egui::Ui, game: &str, mod_name: &str, selected: bool, community: bool, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::click());
    game_face(ui, rect, game, mod_name, resp.hovered(), selected, community.then_some("community"));
    if selected {
        let c = Pos2::new(rect.right() - 18.0, rect.center().y);
        ui.painter().line(vec![c + Vec2::new(-5.0, 0.0), c + Vec2::new(-1.5, 3.5), c + Vec2::new(5.0, -4.0)], Stroke::new(2.0, ACCENT_TEXT));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}
