use eframe::egui::{self, Color32, Margin, Rounding, Stroke, Vec2};
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::theme::{state_color, RgbColor, ThemeMode};
use nextdude_common::proto::v1;

/// Returns a bilingual string matching the selected UI language.
pub fn pick_lang<'a>(lang: Language, id: &'a str, en: &'a str) -> &'a str {
    match lang {
        Language::Id => id,
        Language::En => en,
    }
}

/// Returns the primary accent RGB color for interactive highlights.
pub fn accent_rgb(theme: ThemeMode) -> RgbColor {
    match theme {
        ThemeMode::Dark => RgbColor::new(56, 189, 248),
        ThemeMode::Light => RgbColor::new(2, 132, 199),
    }
}

/// Returns neutral muted label color for secondary metadata.
pub fn muted_color(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(148, 163, 184),
        ThemeMode::Light => Color32::from_rgb(71, 85, 105),
    }
}

/// Returns crisp elevated surface background color for cards and toolbars.
pub fn surface_fill(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(19, 28, 46),
        ThemeMode::Light => Color32::from_rgb(255, 255, 255),
    }
}

/// Returns crisp subtle 1px hairline border color for containers.
pub fn border_color(theme: ThemeMode) -> Color32 {
    match theme {
        ThemeMode::Dark => Color32::from_rgb(51, 65, 85),
        ThemeMode::Light => Color32::from_rgb(203, 213, 225),
    }
}

/// Applies the cohesive Dark Slate / Light Paper NOC visual design tokens onto `egui::Context`.
pub fn apply_app_theme(ctx: &egui::Context, theme: ThemeMode) {
    let mut visuals = match theme {
        ThemeMode::Dark => {
            let mut v = egui::Visuals::dark();
            v.panel_fill = Color32::from_rgb(15, 23, 42);
            v.window_fill = Color32::from_rgb(19, 28, 46);
            v.faint_bg_color = Color32::from_rgb(23, 33, 54);
            v.extreme_bg_color = Color32::from_rgb(11, 17, 32);
            v.code_bg_color = Color32::from_rgb(15, 23, 42);
            v.override_text_color = Some(Color32::from_rgb(226, 232, 240));
            v.selection.bg_fill = Color32::from_rgba_unmultiplied(14, 165, 233, 85);
            v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(56, 189, 248));
            v.hyperlink_color = Color32::from_rgb(56, 189, 248);

            v.widgets.noninteractive.bg_fill = Color32::from_rgb(19, 28, 46);
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(41, 53, 72));
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(203, 213, 225));

            v.widgets.inactive.bg_fill = Color32::from_rgb(30, 41, 59);
            v.widgets.inactive.weak_bg_fill = Color32::from_rgb(30, 41, 59);
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(51, 65, 85));
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(226, 232, 240));

            v.widgets.hovered.bg_fill = Color32::from_rgb(39, 53, 73);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(39, 53, 73);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(56, 189, 248));
            v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);

            v.widgets.active.bg_fill = Color32::from_rgb(12, 74, 110);
            v.widgets.active.weak_bg_fill = Color32::from_rgb(12, 74, 110);
            v.widgets.active.bg_stroke = Stroke::new(1.5, Color32::from_rgb(56, 189, 248));
            v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);

            v.widgets.open.bg_fill = Color32::from_rgb(30, 41, 59);
            v.widgets.open.bg_stroke = Stroke::new(1.0, Color32::from_rgb(56, 189, 248));
            v
        }
        ThemeMode::Light => {
            let mut v = egui::Visuals::light();
            v.panel_fill = Color32::from_rgb(244, 247, 251);
            v.window_fill = Color32::from_rgb(255, 255, 255);
            v.faint_bg_color = Color32::from_rgb(238, 242, 247);
            v.extreme_bg_color = Color32::from_rgb(255, 255, 255);
            v.override_text_color = Some(Color32::from_rgb(15, 23, 42));
            v.selection.bg_fill = Color32::from_rgba_unmultiplied(2, 132, 199, 50);
            v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(2, 132, 199));
            v.hyperlink_color = Color32::from_rgb(2, 132, 199);

            v.widgets.noninteractive.bg_fill = Color32::from_rgb(255, 255, 255);
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(226, 232, 240));
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(51, 65, 85));

            v.widgets.inactive.bg_fill = Color32::from_rgb(232, 238, 246);
            v.widgets.inactive.weak_bg_fill = Color32::from_rgb(232, 238, 246);
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(203, 213, 225));
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(15, 23, 42));

            v.widgets.hovered.bg_fill = Color32::from_rgb(219, 234, 254);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(219, 234, 254);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(2, 132, 199));

            v.widgets.active.bg_fill = Color32::from_rgb(186, 230, 253);
            v.widgets.active.bg_stroke = Stroke::new(1.5, Color32::from_rgb(2, 132, 199));
            v
        }
    };

    let r_widget = Rounding::same(5.0);
    visuals.window_rounding = Rounding::same(8.0);
    visuals.menu_rounding = Rounding::same(6.0);
    visuals.window_stroke = Stroke::new(1.0, border_color(theme));
    visuals.widgets.noninteractive.rounding = r_widget;
    visuals.widgets.inactive.rounding = r_widget;
    visuals.widgets.hovered.rounding = r_widget;
    visuals.widgets.active.rounding = r_widget;
    visuals.widgets.open.rounding = r_widget;
    visuals.striped = true;

    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.spacing.window_margin = Margin::same(12.0);
    });
}

/// Returns a clean rounded card container frame.
pub fn card_frame(theme: ThemeMode) -> egui::Frame {
    egui::Frame::default()
        .fill(surface_fill(theme))
        .stroke(Stroke::new(1.0, border_color(theme)))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::same(12.0))
}

/// Returns a compact horizontal toolbar strip frame.
pub fn toolbar_frame(theme: ThemeMode) -> egui::Frame {
    let fill = match theme {
        ThemeMode::Dark => Color32::from_rgb(19, 28, 46),
        ThemeMode::Light => Color32::from_rgb(255, 255, 255),
    };
    egui::Frame::default()
        .fill(fill)
        .stroke(Stroke::new(1.0, border_color(theme)))
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::symmetric(10.0, 6.0))
}

/// Returns a highlighted alert/notice callout frame with tinted background and border.
pub fn notice_frame(theme: ThemeMode, rgb: RgbColor) -> egui::Frame {
    let bg_alpha = match theme {
        ThemeMode::Dark => 28,
        ThemeMode::Light => 20,
    };
    egui::Frame::default()
        .fill(Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, bg_alpha))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, 160),
        ))
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::symmetric(10.0, 8.0))
}

/// Renders a crisp colored pill badge.
pub fn status_pill(ui: &mut egui::Ui, label: &str, rgb: RgbColor) {
    let bg = Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, 32);
    let border = Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, 150);
    let fg = Color32::from_rgb(rgb.r, rgb.g, rgb.b);
    egui::Frame::default()
        .fill(bg)
        .stroke(Stroke::new(1.0, border))
        .rounding(Rounding::same(99.0))
        .inner_margin(Margin::symmetric(8.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).color(fg).small().strong());
        });
}

/// Renders a consistent page title and optional muted subtitle.
pub fn section_heading(ui: &mut egui::Ui, theme: ThemeMode, title: &str, subtitle: Option<&str>) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(title).size(18.0).strong());
        if let Some(sub) = subtitle {
            ui.label(egui::RichText::new(sub).small().color(muted_color(theme)));
        }
    });
}

/// Renders a clean centered empty-state card when tables have no items.
pub fn empty_state_card(ui: &mut egui::Ui, theme: ThemeMode, title: &str, body: &str) {
    ui.add_space(16.0);
    card_frame(theme).show(ui, |ui| {
        ui.set_min_width(360.0);
        ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            ui.strong(title);
            ui.add_space(4.0);
            ui.label(egui::RichText::new(body).color(muted_color(theme)));
            ui.add_space(6.0);
        });
    });
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
        v1::LinkStatus::NoReply => (
            tr(lang, TextKey::LinkNoReply),
            RgbColor::new(245, 158, 11),
        ),
        v1::LinkStatus::Flapping => (
            tr(lang, TextKey::LinkFlapping),
            RgbColor::new(168, 85, 247),
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
            pick_lang(lang, "KRITIS", "CRITICAL"),
            state_color(theme, v1::ProbeState::Down),
        ),
        v1::Severity::Warning => (
            pick_lang(lang, "PERINGATAN", "WARNING"),
            RgbColor::new(245, 158, 11),
        ),
        _ => (pick_lang(lang, "INFO", "INFO"), accent_rgb(theme)),
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
