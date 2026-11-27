use eframe::egui::{
    self, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Margin, Pos2,
    Rounding, Stroke, TextStyle, Vec2,
};
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::theme::{state_color, RgbColor, ThemeMode};
use nextdude_common::proto::v1;
use std::sync::Arc;

/// Returns a bilingual string matching the selected UI language.
pub fn pick_lang<'a>(lang: Language, id: &'a str, en: &'a str) -> &'a str {
    match lang {
        Language::Id => id,
        Language::En => en,
    }
}

/// Loads crisp native Windows UI (`Segoe UI`) and monospace (`Consolas`) fonts when available,
/// falling back cleanly to `egui`'s bundled fonts if missing.
pub fn install_system_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let proportional_candidates = [
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\arial.ttf",
    ];
    for path in proportional_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("system_ui".to_string(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "system_ui".to_string());
            break;
        }
    }

    let mono_candidates = [
        r"C:\Windows\Fonts\consola.ttf",
        r"C:\Windows\Fonts\lucon.ttf",
    ];
    for path in mono_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("system_mono".to_string(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, "system_mono".to_string());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

/// Apple System Blue accent RGB token.
pub fn accent_rgb(theme: ThemeMode) -> RgbColor {
    match theme {
        ThemeMode::Dark => RgbColor::new(10, 132, 255),
        ThemeMode::Light => RgbColor::new(0, 122, 255),
    }
}

/// Apple System Amber/Orange warning token.
pub fn warning_rgb(theme: ThemeMode) -> RgbColor {
    match theme {
        ThemeMode::Dark => RgbColor::new(255, 159, 10),
        ThemeMode::Light => RgbColor::new(201, 52, 0),
    }
}

/// Primary Apple accent as `Color32`.
pub fn accent_color(theme: ThemeMode) -> Color32 {
    let c = accent_rgb(theme);
    Color32::from_rgb(c.r, c.g, c.b)
}

/// Apple primary label color.
pub fn text_color(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(245, 245, 247),
        ThemeMode::Light => Color32::from_rgb(29, 29, 31),
    }
}

/// Apple secondary label color for calm subtitles and metadata.
pub fn muted_color(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(152, 152, 157),
        ThemeMode::Light => Color32::from_rgb(110, 110, 115),
    }
}

/// Main grouped window backdrop color.
pub fn window_bg(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(20, 20, 22),
        ThemeMode::Light => Color32::from_rgb(245, 245, 247),
    }
}

/// Translucent-feel macOS sidebar / top chrome surface color.
pub fn sidebar_bg(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(28, 28, 30),
        ThemeMode::Light => Color32::from_rgb(236, 236, 239),
    }
}

/// Elevated grouped card surface color.
pub fn surface_fill(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(34, 34, 37),
        ThemeMode::Light => Color32::from_rgb(255, 255, 255),
    }
}

/// Nested control / input background color.
pub fn control_fill(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(44, 44, 46),
        ThemeMode::Light => Color32::from_rgb(229, 229, 234),
    }
}

/// Ultra-subtle 1px Apple hairline divider / container border.
pub fn border_color(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgba_unmultiplied(255, 255, 255, 22),
        ThemeMode::Light => Color32::from_rgba_unmultiplied(0, 0, 0, 20),
    }
}

/// Applies the iOS / macOS Sonoma-inspired visual system onto `egui::Context`.
pub fn apply_app_theme(ctx: &egui::Context, theme: ThemeMode) {
    let accent = accent_color(theme);
    let mut visuals = match theme {
        ThemeMode::Dark => {
            let mut v = egui::Visuals::dark();
            v.panel_fill = window_bg(theme);
            v.window_fill = Color32::from_rgb(30, 30, 33);
            v.faint_bg_color = Color32::from_rgb(38, 38, 42);
            v.extreme_bg_color = Color32::from_rgb(24, 24, 26);
            v.code_bg_color = Color32::from_rgb(28, 28, 30);
            v.override_text_color = Some(text_color(theme));
            v.selection.bg_fill = Color32::from_rgba_unmultiplied(10, 132, 255, 70);
            v.selection.stroke = Stroke::new(1.5_f32, accent);
            v.hyperlink_color = accent;

            v.widgets.noninteractive.bg_fill = surface_fill(theme);
            v.widgets.noninteractive.bg_stroke = Stroke::NONE;
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text_color(theme));

            v.widgets.inactive.bg_fill = control_fill(theme);
            v.widgets.inactive.weak_bg_fill = control_fill(theme);
            v.widgets.inactive.bg_stroke = Stroke::NONE;
            v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text_color(theme));

            v.widgets.hovered.bg_fill = Color32::from_rgb(58, 58, 62);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 58, 62);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(38));
            v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);

            v.widgets.active.bg_fill = Color32::from_rgb(72, 72, 76);
            v.widgets.active.weak_bg_fill = Color32::from_rgb(72, 72, 76);
            v.widgets.active.bg_stroke = Stroke::new(1.5_f32, accent);
            v.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);

            v.widgets.open.bg_fill = Color32::from_rgb(48, 48, 52);
            v.widgets.open.bg_stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(36));
            v
        }
        ThemeMode::Light => {
            let mut v = egui::Visuals::light();
            v.panel_fill = window_bg(theme);
            v.window_fill = Color32::from_rgb(255, 255, 255);
            v.faint_bg_color = Color32::from_rgb(248, 248, 250);
            v.extreme_bg_color = Color32::from_rgb(242, 242, 247);
            v.code_bg_color = Color32::from_rgb(242, 242, 247);
            v.override_text_color = Some(text_color(theme));
            v.selection.bg_fill = Color32::from_rgba_unmultiplied(0, 122, 255, 42);
            v.selection.stroke = Stroke::new(1.5_f32, accent);
            v.hyperlink_color = accent;

            v.widgets.noninteractive.bg_fill = surface_fill(theme);
            v.widgets.noninteractive.bg_stroke = Stroke::NONE;
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text_color(theme));

            v.widgets.inactive.bg_fill = control_fill(theme);
            v.widgets.inactive.weak_bg_fill = control_fill(theme);
            v.widgets.inactive.bg_stroke = Stroke::NONE;
            v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text_color(theme));

            v.widgets.hovered.bg_fill = Color32::from_rgb(216, 216, 222);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(216, 216, 222);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_black_alpha(28));
            v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text_color(theme));

            v.widgets.active.bg_fill = Color32::from_rgb(204, 204, 212);
            v.widgets.active.weak_bg_fill = Color32::from_rgb(204, 204, 212);
            v.widgets.active.bg_stroke = Stroke::new(1.5_f32, accent);
            v.widgets.active.fg_stroke = Stroke::new(1.0_f32, text_color(theme));

            v.widgets.open.bg_fill = Color32::from_rgb(220, 220, 226);
            v.widgets.open.bg_stroke = Stroke::new(1.0_f32, Color32::from_black_alpha(28));
            v
        }
    };

    let r_widget = Rounding::same(4.0);
    visuals.window_rounding = Rounding::same(6.0);
    visuals.menu_rounding = Rounding::same(6.0);
    visuals.window_stroke = Stroke::new(1.0_f32, border_color(theme));
    visuals.widgets.noninteractive.rounding = r_widget;
    visuals.widgets.inactive.rounding = r_widget;
    visuals.widgets.hovered.rounding = r_widget;
    visuals.widgets.active.rounding = r_widget;
    visuals.widgets.open.rounding = r_widget;
    visuals.striped = false;

    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(18.5, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Body,
            FontId::new(13.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(12.5, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(11.5, FontFamily::Proportional),
        );
        style.spacing.item_spacing = Vec2::new(7.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.5);
        style.spacing.window_margin = Margin::same(14.0);
        style.spacing.interact_size.y = 24.0;
    });
}

/// Frame for top & bottom window bars.
pub fn bar_frame(theme: ThemeMode, margin: Margin) -> egui::Frame {
    egui::Frame::default()
        .fill(sidebar_bg(theme))
        .stroke(Stroke::NONE)
        .inner_margin(margin)
}

/// Frame for the left navigation sidebar.
pub fn sidebar_frame(theme: ThemeMode) -> egui::Frame {
    egui::Frame::default()
        .fill(sidebar_bg(theme))
        .stroke(Stroke::NONE)
        .inner_margin(Margin::symmetric(8.0, 10.0))
}

/// Crisp elevated card container.
pub fn card_frame(theme: ThemeMode) -> egui::Frame {
    egui::Frame::default()
        .fill(surface_fill(theme))
        .stroke(Stroke::new(1.0_f32, border_color(theme)))
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::same(14.0))
}

/// Compact solid toolbar strip container.
pub fn toolbar_frame(theme: ThemeMode) -> egui::Frame {
    egui::Frame::default()
        .fill(surface_fill(theme))
        .stroke(Stroke::new(1.0_f32, border_color(theme)))
        .rounding(Rounding::same(5.0))
        .inner_margin(Margin::symmetric(10.0, 6.0))
}

/// Soft tinted callout banner.
pub fn notice_frame(theme: ThemeMode, rgb: RgbColor) -> egui::Frame {
    let bg_alpha = match theme {
        ThemeMode::Dark => 28,
        ThemeMode::Light => 20,
    };
    egui::Frame::default()
        .fill(Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, bg_alpha))
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, 80),
        ))
        .rounding(Rounding::same(5.0))
        .inner_margin(Margin::symmetric(10.0, 7.0))
}

/// Draws a crisp 1px horizontal hairline divider.
pub fn hairline_separator(ui: &mut egui::Ui, theme: ThemeMode) {
    let w = ui.available_width().max(1.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), egui::Sense::hover());
    ui.painter()
        .line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.0_f32, border_color(theme)));
}

/// Crisp flat status badge with high-contrast dot and label.
pub fn status_pill(ui: &mut egui::Ui, label: &str, rgb: RgbColor) {
    let bg = Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, 26);
    let border = Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, 75);
    let fg = Color32::from_rgb(rgb.r, rgb.g, rgb.b);
    egui::Frame::default()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .rounding(Rounding::same(4.0))
        .inner_margin(Margin::symmetric(8.0, 2.5))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).color(fg).small().strong());
        });
}

/// Calm indicator dot + plain label (used in status bar and compact rows).
pub fn status_dot_label(ui: &mut egui::Ui, theme: ThemeMode, label: &str, rgb: RgbColor) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 3.0, Color32::from_rgb(rgb.r, rgb.g, rgb.b));
        ui.label(egui::RichText::new(label).small().color(muted_color(theme)));
    });
}

/// Primary CTA button.
pub fn primary_button(
    ui: &mut egui::Ui,
    theme: ThemeMode,
    enabled: bool,
    label: &str,
) -> egui::Response {
    let fill = if enabled {
        accent_color(theme)
    } else {
        control_fill(theme)
    };
    let fg = if enabled {
        Color32::WHITE
    } else {
        muted_color(theme)
    };
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).color(fg).strong())
            .fill(fill)
            .stroke(Stroke::NONE)
            .rounding(Rounding::same(4.0)),
    )
}

/// Destructive action button.
pub fn danger_button(ui: &mut egui::Ui, theme: ThemeMode, label: &str) -> egui::Response {
    let red = state_color(theme, v1::ProbeState::Down);
    let bg = Color32::from_rgba_unmultiplied(red.r, red.g, red.b, 26);
    let fg = Color32::from_rgb(red.r, red.g, red.b);
    ui.add(
        egui::Button::new(egui::RichText::new(label).color(fg))
            .fill(bg)
            .stroke(Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(red.r, red.g, red.b, 70),
            ))
            .rounding(Rounding::same(4.0)),
    )
}

/// Single item inside a segmented control strip.
pub fn segmented_value<T: PartialEq>(
    ui: &mut egui::Ui,
    theme: ThemeMode,
    current: &mut T,
    target: T,
    label: &str,
) -> bool {
    let selected = *current == target;
    let (bg, fg) = if selected {
        (surface_fill(theme), text_color(theme))
    } else {
        (Color32::TRANSPARENT, muted_color(theme))
    };
    let stroke = if selected {
        Stroke::new(1.0_f32, border_color(theme))
    } else {
        Stroke::NONE
    };
    let text = if selected {
        egui::RichText::new(label).color(fg).strong()
    } else {
        egui::RichText::new(label).color(fg)
    };
    let resp = ui.add(
        egui::Button::new(text)
            .fill(bg)
            .stroke(stroke)
            .rounding(Rounding::same(4.0)),
    );
    if resp.clicked() {
        *current = target;
        true
    } else {
        false
    }
}

/// Wraps a segmented control group inside a subtle solid track.
pub fn segmented_track(ui: &mut egui::Ui, theme: ThemeMode, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(control_fill(theme))
        .stroke(Stroke::NONE)
        .rounding(Rounding::same(5.0))
        .inner_margin(Margin::same(2.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                add_contents(ui);
            });
        });
}

/// Clean navigation sidebar row with optional right count indicator.
pub fn nav_row(
    ui: &mut egui::Ui,
    theme: ThemeMode,
    selected: bool,
    label: &str,
    count: Option<usize>,
) -> egui::Response {
    let height = 26.0;
    let width = ui.available_width().max(40.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click());

    let fill = if selected {
        match theme {
            ThemeMode::Dark => Color32::from_rgba_unmultiplied(10, 132, 255, 42),
            ThemeMode::Light => Color32::from_rgb(0, 122, 255),
        }
    } else if resp.hovered() {
        match theme {
            ThemeMode::Dark => Color32::from_rgba_unmultiplied(255, 255, 255, 14),
            ThemeMode::Light => Color32::from_rgba_unmultiplied(0, 0, 0, 10),
        }
    } else {
        Color32::TRANSPARENT
    };

    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, Rounding::same(4.0), fill);
    }

    let fg = if selected {
        match theme {
            ThemeMode::Dark => Color32::from_rgb(100, 175, 255),
            ThemeMode::Light => Color32::WHITE,
        }
    } else {
        text_color(theme)
    };

    let text_pos = Pos2::new(rect.min.x + 10.0, rect.center().y);
    let font = FontId::new(13.0, FontFamily::Proportional);
    ui.painter()
        .text(text_pos, egui::Align2::LEFT_CENTER, label, font, fg);

    if let Some(n) = count {
        let count_fg = if selected && theme == ThemeMode::Light {
            Color32::from_white_alpha(220)
        } else {
            muted_color(theme)
        };
        ui.painter().text(
            Pos2::new(rect.max.x - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            FontId::new(11.5, FontFamily::Monospace),
            count_fg,
        );
    }

    resp
}

/// Consistent section heading with crisp title and optional calm subtitle.
pub fn section_heading(ui: &mut egui::Ui, theme: ThemeMode, title: &str, subtitle: Option<&str>) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(egui::RichText::new(title).size(20.0).strong());
        if let Some(sub) = subtitle {
            ui.label(egui::RichText::new(sub).small().color(muted_color(theme)));
        }
    });
}

/// Full-width centered empty state card.
pub fn empty_state_card(ui: &mut egui::Ui, theme: ThemeMode, title: &str, body: &str) {
    ui.add_space(12.0);
    card_frame(theme).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(18.0);
            ui.label(egui::RichText::new(title).size(15.0).strong());
            ui.add_space(4.0);
            ui.label(egui::RichText::new(body).color(muted_color(theme)));
            ui.add_space(18.0);
        });
    });
}

/// Full-width card containing a horizontally & vertically scrollable table.
pub fn table_card(
    ui: &mut egui::Ui,
    theme: ThemeMode,
    id_salt: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    card_frame(theme).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        egui::ScrollArea::both()
            .id_salt(id_salt)
            .auto_shrink([false, false])
            .show(ui, add_contents);
    });
}

/// Creates a centered macOS-style modal dialog window.
pub fn centered_modal<'a>(title: &'a str, min_width: f32) -> egui::Window<'a> {
    egui::Window::new(title)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .min_width(min_width)
}

/// Right-aligned Apple modal footer (`[Cancel]  [Primary Save]`), returning `(save_clicked, cancel_clicked)`.
pub fn modal_footer_buttons(
    ui: &mut egui::Ui,
    theme: ThemeMode,
    can_save: bool,
    save_label: &str,
    cancel_label: &str,
) -> (bool, bool) {
    let mut save_clicked = false;
    let mut cancel_clicked = false;
    ui.add_space(8.0);
    hairline_separator(ui, theme);
    ui.add_space(8.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if primary_button(ui, theme, can_save, save_label).clicked() {
            save_clicked = true;
        }
        if ui.button(cancel_label).clicked() {
            cancel_clicked = true;
        }
    });
    (save_clicked, cancel_clicked)
}

/// Localized label for a device/service probe state.
pub fn probe_state_label(lang: Language, state: v1::ProbeState) -> &'static str {
    match state {
        v1::ProbeState::Up => tr(lang, TextKey::StateUp),
        v1::ProbeState::Down => tr(lang, TextKey::StateDown),
        v1::ProbeState::Unreachable => tr(lang, TextKey::StateUnreachable),
        v1::ProbeState::Partial => tr(lang, TextKey::StatePartial),
        _ => tr(lang, TextKey::StateUnknown),
    }
}

/// Localized badge pill for a device/service probe state.
pub fn probe_state_pill(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: v1::ProbeState,
) {
    status_pill(ui, probe_state_label(lang, state), state_color(theme, state));
}

/// Maps `LinkStatus` to theme color and localized label.
pub fn link_status_info(
    lang: Language,
    theme: ThemeMode,
    status: v1::LinkStatus,
) -> (&'static str, RgbColor) {
    match status {
        v1::LinkStatus::Up => (tr(lang, TextKey::LinkUp), state_color(theme, v1::ProbeState::Up)),
        v1::LinkStatus::Down => (
            tr(lang, TextKey::LinkDown),
            state_color(theme, v1::ProbeState::Down),
        ),
        v1::LinkStatus::NoReply => (tr(lang, TextKey::LinkNoReply), warning_rgb(theme)),
        v1::LinkStatus::Flapping => (
            tr(lang, TextKey::LinkFlapping),
            RgbColor::new(191, 90, 242),
        ),
        _ => (
            tr(lang, TextKey::LinkUnknown),
            state_color(theme, v1::ProbeState::Unknown),
        ),
    }
}

/// Localized badge pill for a network link status.
pub fn link_status_pill(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    status: v1::LinkStatus,
) {
    let (label, rgb) = link_status_info(lang, theme, status);
    status_pill(ui, label, rgb);
}

/// Localized badge pill for event / alarm severity.
pub fn severity_pill(ui: &mut egui::Ui, lang: Language, theme: ThemeMode, sev: v1::Severity) {
    let (label, rgb) = match sev {
        v1::Severity::Critical => (
            pick_lang(lang, "Kritis", "Critical"),
            state_color(theme, v1::ProbeState::Down),
        ),
        v1::Severity::Warning => (
            pick_lang(lang, "Peringatan", "Warning"),
            warning_rgb(theme),
        ),
        _ => (pick_lang(lang, "Info", "Info"), accent_rgb(theme)),
    };
    status_pill(ui, label, rgb);
}

/// Maps `ProbeKind` to a readable protocol name rather than Rust Debug enum syntax.
pub fn probe_kind_label(kind: v1::ProbeKind) -> &'static str {
    match kind {
        v1::ProbeKind::Icmp => "ICMP Ping",
        v1::ProbeKind::Tcp => "TCP Connect",
        v1::ProbeKind::Http => "HTTP",
        v1::ProbeKind::Https => "HTTPS",
        v1::ProbeKind::Dns => "DNS",
        v1::ProbeKind::SnmpOid => "SNMP OID",
        v1::ProbeKind::SnmpInterface => "SNMP Interface",
        v1::ProbeKind::Ftp => "FTP",
        v1::ProbeKind::Smtp => "SMTP",
        v1::ProbeKind::Imap => "IMAP",
        v1::ProbeKind::Pop3 => "POP3",
        v1::ProbeKind::Ssh => "SSH",
        v1::ProbeKind::Telnet => "Telnet",
        v1::ProbeKind::Ntp => "NTP",
        v1::ProbeKind::SnmpCustom => "SNMP Custom",
        _ => "Unknown",
    }
}
