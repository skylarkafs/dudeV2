use crate::ui_kit::{
    accent_rgb, border_color, centered_modal, hairline_separator, link_status_info,
    modal_footer_buttons, muted_color, notice_frame, pick_lang, primary_button, probe_state_label,
    segmented_track, segmented_value, status_pill, surface_fill, text_color, toolbar_frame,
    warning_rgb,
};
use eframe::egui::{self, Color32, FontId, Margin, Painter, Pos2, Rect, Rounding, Sense, Stroke, Vec2};
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::geometry::{
    avoid_label_overlaps, compute_elastic_link_path, distance_to_polyline, Vec2d,
};
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::{ClientStateCache, DeviceImageSource};
use nextdude_client_core::theme::{
    state_badge_glyph, state_color, utilization_color, RgbColor, ThemeMode,
};
use nextdude_common::proto::v1;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapTool {
    Select,
    AddDevice,
    AddLink,
    AddNetwork,
    AddSubmap,
    AddStatic,
    AddDependency,
}

pub struct MapCanvasState {
    pub active_map_id: Option<i64>,
    pub open_map_tabs: Vec<i64>,
    pub pan: Vec2,
    pub zoom: f32,
    pub active_tool: MapTool,
    pub layer_mode: v1::MapLayerMode,
    pub snap_to_grid: bool,
    pub selected_devices: HashSet<i64>,
    pub selected_links: HashSet<i64>,
    pub selected_submap: Option<i64>,
    pub selected_network: Option<i64>,
    pub selected_static: Option<i64>,
    pub link_source_device: Option<i64>,
    pub dependency_parent_device: Option<i64>,
    pub dragging_nodes: bool,
    pub dragging_bend: Option<(i64, usize)>,
    pub marquee_start: Option<Pos2>,
    pub find_query: String,
    pub find_match_index: usize,
    pub show_map_settings: bool,
    pub clipboard_devices: Vec<i64>,
    pub clipboard_source_map: Option<i64>,
    pub open_device_dialog: Option<i64>,
    pub open_link_dialog: Option<i64>,
    pub open_tool_for_device: Option<(i64, v1::ToolKind)>,
    pub pending_new_device_pos: Option<(f64, f64)>,
    pub pending_new_link: Option<(i64, i64)>,
    pub pending_open_submap_tab: Option<i64>,
    pub show_new_map_dialog: bool,
    pub new_map_name: String,
    pub export_png_requested: bool,
    pub fit_view_requested: bool,
    pub show_grid: bool,
    pub show_link_labels: bool,
    pub clear_links_requested: bool,
}

impl Default for MapCanvasState {
    fn default() -> Self {
        Self {
            active_map_id: None,
            open_map_tabs: Vec::new(),
            pan: Vec2::new(60.0, 60.0),
            zoom: 1.0,
            active_tool: MapTool::Select,
            layer_mode: v1::MapLayerMode::MapLayerBoth,
            snap_to_grid: true,
            selected_devices: HashSet::new(),
            selected_links: HashSet::new(),
            selected_submap: None,
            selected_network: None,
            selected_static: None,
            link_source_device: None,
            dependency_parent_device: None,
            dragging_nodes: false,
            dragging_bend: None,
            marquee_start: None,
            find_query: String::new(),
            find_match_index: 0,
            show_map_settings: false,
            clipboard_devices: Vec::new(),
            clipboard_source_map: None,
            open_device_dialog: None,
            open_link_dialog: None,
            open_tool_for_device: None,
            pending_new_device_pos: None,
            pending_new_link: None,
            pending_open_submap_tab: None,
            show_new_map_dialog: false,
            new_map_name: String::new(),
            export_png_requested: false,
            fit_view_requested: false,
            show_grid: false,
            show_link_labels: true,
            clear_links_requested: false,
        }
    }
}

impl MapCanvasState {
    pub fn world_to_screen(&self, canvas_origin: Pos2, wx: f64, wy: f64) -> Pos2 {
        Pos2::new(
            canvas_origin.x + (wx as f32) * self.zoom + self.pan.x,
            canvas_origin.y + (wy as f32) * self.zoom + self.pan.y,
        )
    }

    pub fn screen_to_world(&self, canvas_origin: Pos2, s: Pos2) -> Vec2d {
        Vec2d::new(
            f64::from((s.x - canvas_origin.x - self.pan.x) / self.zoom),
            f64::from((s.y - canvas_origin.y - self.pan.y) / self.zoom),
        )
    }

    pub fn focus_world_point(&mut self, viewport: Rect, wx: f64, wy: f64) {
        self.pan = Vec2::new(
            viewport.width() * 0.5 - (wx as f32) * self.zoom,
            viewport.height() * 0.5 - (wy as f32) * self.zoom,
        );
    }

    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom * 1.2).clamp(0.15, 4.0);
    }

    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom / 1.2).clamp(0.15, 4.0);
    }

    pub fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::new(60.0, 60.0);
    }

    pub fn fit_to_devices(&mut self, viewport: Rect, devs: &[&v1::Device]) {
        if devs.is_empty() {
            self.reset_view();
            return;
        }
        let mut x0 = f64::INFINITY;
        let mut y0 = f64::INFINITY;
        let mut x1 = f64::NEG_INFINITY;
        let mut y1 = f64::NEG_INFINITY;
        for d in devs {
            x0 = x0.min(d.pos_x - 70.0);
            y0 = y0.min(d.pos_y - 70.0);
            x1 = x1.max(d.pos_x + 70.0);
            y1 = y1.max(d.pos_y + 70.0);
        }
        let bw = ((x1 - x0) as f32).max(200.0);
        let bh = ((y1 - y0) as f32).max(160.0);
        let zx = (viewport.width() * 0.85) / bw;
        let zy = (viewport.height() * 0.85) / bh;
        self.zoom = zx.min(zy).clamp(0.15, 2.0);
        let cx = (x0 + x1) * 0.5;
        let cy = (y0 + y1) * 0.5;
        self.focus_world_point(viewport, cx, cy);
    }

    fn clear_selection(&mut self) {
        self.selected_devices.clear();
        self.selected_links.clear();
        self.selected_submap = None;
        self.selected_network = None;
        self.selected_static = None;
    }

    fn cancel_active_tool(&mut self) {
        self.active_tool = MapTool::Select;
        self.link_source_device = None;
        self.dependency_parent_device = None;
    }
}

fn to_color32(c: RgbColor, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, alpha)
}

fn node_half_extents(dev: &v1::Device) -> (f64, f64) {
    let base = match v1::NodeSize::try_from(dev.size).unwrap_or(v1::NodeSize::Medium) {
        v1::NodeSize::Small => 18.0,
        v1::NodeSize::Large => 34.0,
        v1::NodeSize::Custom => (dev.custom_size_px * 0.5).clamp(12.0, 96.0),
        _ => 24.0,
    };
    (base, base)
}

#[allow(clippy::too_many_arguments)]
pub fn render_map_workspace(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    theme: ThemeMode,
    state: &mut ClientStateCache,
    canvas: &mut MapCanvasState,
    textures: &HashMap<String, egui::TextureHandle>,
    cmd_tx: &mpsc::Sender<ClientCommand>,
    request_image_cb: &mut dyn FnMut(i64, &str),
) {
    // Sync tabs with available maps
    if state.maps.is_empty() {
        canvas.active_map_id = None;
        canvas.open_map_tabs.clear();
    } else {
        canvas.open_map_tabs.retain(|id| state.maps.contains_key(id));
        if canvas.open_map_tabs.is_empty() {
            if let Some(&first_id) = state.maps.keys().next() {
                canvas.open_map_tabs.push(first_id);
                canvas.active_map_id = Some(first_id);
            }
        }
        if canvas
            .active_map_id
            .map_or(true, |id| !state.maps.contains_key(&id))
        {
            canvas.active_map_id = canvas.open_map_tabs.first().copied();
        }
    }

    // Handle submap double-click open
    if let Some(target_id) = canvas.pending_open_submap_tab.take() {
        if state.maps.contains_key(&target_id) {
            if !canvas.open_map_tabs.contains(&target_id) {
                canvas.open_map_tabs.push(target_id);
            }
            canvas.active_map_id = Some(target_id);
        }
    }

    // Row 1: Map Tabs + Layer / Lock / Snap / Settings strip
    ui.horizontal_wrapped(|ui| {
        let map_ids = state.maps.keys().copied().collect::<Vec<_>>();
        for mid in &map_ids {
            if let Some(m) = state.maps.get(mid) {
                let selected = canvas.active_map_id == Some(*mid);
                let total_devs = state.devices.values().filter(|d| d.map_id == *mid).count();
                let down_devs = state
                    .devices
                    .values()
                    .filter(|d| {
                        d.map_id == *mid && d.aggregate_state == v1::ProbeState::Down as i32
                    })
                    .count();
                let label = if down_devs > 0 {
                    format!("{} ({total_devs} • {down_devs} DN)", m.name)
                } else {
                    format!("{} ({total_devs})", m.name)
                };
                if ui.selectable_label(selected, label).clicked() {
                    if !canvas.open_map_tabs.contains(mid) {
                        canvas.open_map_tabs.push(*mid);
                    }
                    canvas.active_map_id = Some(*mid);
                    let _ = cmd_tx.try_send(ClientCommand::UpdateSubscription(
                        v1::SubscribeRequest {
                            active_map_id: Some(*mid),
                            active_device_id: None,
                            active_chart_ids: Vec::new(),
                            include_events: true,
                            include_syslog: true,
                            include_traps: true,
                            include_outages: true,
                        },
                    ));
                }
            }
        }
        if ui
            .button(pick_lang(lang, "+ Peta Baru", "+ New Map"))
            .clicked()
        {
            canvas.show_new_map_dialog = true;
            canvas.new_map_name.clear();
        }

        if let Some(map_id) = canvas.active_map_id {
            if let Some(active_map) = state.maps.get(&map_id).cloned() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr(lang, TextKey::MapSettings)).clicked() {
                        canvas.show_map_settings = !canvas.show_map_settings;
                    }
                    if ui.button(tr(lang, TextKey::MapExportPng)).clicked() {
                        canvas.export_png_requested = true;
                    }
                    ui.checkbox(&mut canvas.show_link_labels, "RX/TX");
                    ui.checkbox(&mut canvas.show_grid, "Grid");
                    ui.checkbox(&mut canvas.snap_to_grid, tr(lang, TextKey::MapSnapGrid));
                    let mut locked = active_map.locked;
                    if ui.checkbox(&mut locked, tr(lang, TextKey::MapLock)).changed() {
                        let mut updated = active_map.clone();
                        updated.locked = locked;
                        let _ = cmd_tx.try_send(ClientCommand::SaveMap(updated));
                    }
                    if active_map.locked {
                        status_pill(
                            ui,
                            pick_lang(lang, "TERKUNCI", "LOCKED"),
                            warning_rgb(theme),
                        );
                    }
                    segmented_track(ui, theme, |ui| {
                        segmented_value(
                            ui,
                            theme,
                            &mut canvas.layer_mode,
                            v1::MapLayerMode::MapLayerBoth,
                            tr(lang, TextKey::MapLayerBoth),
                        );
                        segmented_value(
                            ui,
                            theme,
                            &mut canvas.layer_mode,
                            v1::MapLayerMode::MapLayerDependencies,
                            tr(lang, TextKey::MapLayerDependencies),
                        );
                        segmented_value(
                            ui,
                            theme,
                            &mut canvas.layer_mode,
                            v1::MapLayerMode::MapLayerLinks,
                            tr(lang, TextKey::MapLayerLinks),
                        );
                    });
                });
            }
        }
    });

    if canvas.show_new_map_dialog {
        let mut submit = false;
        let mut cancel = false;
        centered_modal(pick_lang(lang, "Peta Baru", "New Map"), 340.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr(lang, TextKey::Name));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut canvas.new_map_name).desired_width(220.0),
                );
                if resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
            });
            ui.add_space(10.0);
            let valid = !canvas.new_map_name.trim().is_empty();
            let (s_click, c_click) = modal_footer_buttons(
                ui,
                theme,
                valid,
                tr(lang, TextKey::Save),
                tr(lang, TextKey::Cancel),
            );
            submit = (submit && valid) || s_click;
            cancel = c_click;
        });
        if submit {
            let (d, n, s, st, l) = nextdude_common::proto::default_map_palettes();
            let new_map = v1::NetworkMap {
                id: 0,
                version: 1,
                name: canvas.new_map_name.trim().to_string(),
                description: String::new(),
                poll_interval_sec_override: None,
                label_refresh_sec: 5,
                device_colors: Some(d),
                network_colors: Some(n),
                submap_colors: Some(s),
                static_colors: Some(st),
                link_colors: Some(l),
                default_node_shape: v1::NodeShape::RoundedRect as i32,
                default_link_thickness: 2.0,
                default_link_style: v1::LinkStyle::Curved as i32,
                background_image_id: None,
                background_scale: 1.0,
                background_tile: false,
                locked: false,
                default_layer: v1::MapLayerMode::MapLayerBoth as i32,
            };
            let _ = cmd_tx.try_send(ClientCommand::SaveMap(new_map));
            canvas.show_new_map_dialog = false;
        } else if cancel {
            canvas.show_new_map_dialog = false;
        }
    }

    let Some(map_id) = canvas.active_map_id else {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.heading(tr(lang, TextKey::OnboardingTitle));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tr(lang, TextKey::OnboardingBody))
                        .color(muted_color(theme)),
                );
                ui.add_space(12.0);
                if primary_button(
                    ui,
                    theme,
                    true,
                    pick_lang(lang, "+ Buat Peta Pertama", "+ Create First Map"),
                )
                .clicked()
                {
                    canvas.show_new_map_dialog = true;
                    canvas.new_map_name = "Main Backbone".to_string();
                }
            });
        });
        return;
    };

    let Some(active_map) = state.maps.get(&map_id).cloned() else {
        return;
    };

    // Collect map devices once for Find cycling & rendering
    let map_devices: Vec<&v1::Device> = state
        .devices
        .values()
        .filter(|d| d.map_id == map_id)
        .collect();
    let map_links: Vec<&v1::NetworkLink> = state
        .links
        .values()
        .filter(|l| l.map_id == map_id)
        .collect();
    let map_device_count = map_devices.len();
    let map_link_count = map_links.len();

    ui.add_space(4.0);

    // Row 2: Calm macOS Map Toolbar (Tools segmented track + Arrange/Edit menus + Find + Zoom)
    let editable = !active_map.locked;
    let mut focus_target: Option<(f64, f64)> = None;
    toolbar_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let tools: &[(MapTool, TextKey)] = &[
                (MapTool::Select, TextKey::MapToolSelect),
                (MapTool::AddDevice, TextKey::MapToolAddDevice),
                (MapTool::AddLink, TextKey::MapToolAddLink),
                (MapTool::AddNetwork, TextKey::MapToolAddNetwork),
                (MapTool::AddSubmap, TextKey::MapToolAddSubmap),
                (MapTool::AddStatic, TextKey::MapToolAddStatic),
                (MapTool::AddDependency, TextKey::MapToolAddDependency),
            ];
            segmented_track(ui, theme, |ui| {
                for &(tool, key) in tools {
                    if segmented_value(ui, theme, &mut canvas.active_tool, tool, tr(lang, key)) {
                        canvas.link_source_device = None;
                        canvas.dependency_parent_device = None;
                    }
                }
            });

            ui.separator();

            ui.menu_button(pick_lang(lang, "Tata Letak", "Arrange"), |ui| {
                if ui
                    .add_enabled(editable, egui::Button::new(tr(lang, TextKey::MapAutoLayout)))
                    .clicked()
                {
                    let _ = cmd_tx.try_send(ClientCommand::AutoLayoutMap(v1::AutoLayoutRequest {
                        map_id,
                        iterations: 60,
                    }));
                    ui.close_menu();
                }
                hairline_separator(ui, theme);
                let align_enabled = editable && canvas.selected_devices.len() >= 2;
                if ui
                    .add_enabled(align_enabled, egui::Button::new(tr(lang, TextKey::MapAlignRow)))
                    .clicked()
                {
                    align_selected_devices(state, canvas, AlignMode::Row, cmd_tx);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        align_enabled,
                        egui::Button::new(tr(lang, TextKey::MapAlignCircle)),
                    )
                    .clicked()
                {
                    align_selected_devices(state, canvas, AlignMode::Circle, cmd_tx);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(align_enabled, egui::Button::new(tr(lang, TextKey::MapAlignLine)))
                    .clicked()
                {
                    align_selected_devices(state, canvas, AlignMode::Line, cmd_tx);
                    ui.close_menu();
                }
            });

            ui.menu_button(pick_lang(lang, "Sunting", "Edit"), |ui| {
                if ui.button(tr(lang, TextKey::MapUndo)).clicked() {
                    let _ = cmd_tx.try_send(ClientCommand::Undo);
                    ui.close_menu();
                }
                if ui.button(tr(lang, TextKey::MapRedo)).clicked() {
                    let _ = cmd_tx.try_send(ClientCommand::Redo);
                    ui.close_menu();
                }
                hairline_separator(ui, theme);
                if ui
                    .add_enabled(
                        !canvas.selected_devices.is_empty(),
                        egui::Button::new(tr(lang, TextKey::MapCopy)),
                    )
                    .clicked()
                {
                    canvas.clipboard_devices = canvas.selected_devices.iter().copied().collect();
                    canvas.clipboard_source_map = Some(map_id);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        editable && !canvas.clipboard_devices.is_empty(),
                        egui::Button::new(tr(lang, TextKey::MapPaste)),
                    )
                    .clicked()
                {
                    if let Some(src_map) = canvas.clipboard_source_map {
                        let _ = cmd_tx.try_send(ClientCommand::CopyPasteItems(
                            v1::CopyPasteRequest {
                                source_map_id: src_map,
                                target_map_id: map_id,
                                device_ids: canvas.clipboard_devices.clone(),
                                network_ids: Vec::new(),
                                static_ids: Vec::new(),
                                offset_x: 40.0,
                                offset_y: 40.0,
                            },
                        ));
                    }
                    ui.close_menu();
                }
                if editable {
                    hairline_separator(ui, theme);
                    if ui
                        .button(pick_lang(
                            lang,
                            "Hapus Semua Link Peta...",
                            "Clear Map Links...",
                        ))
                        .clicked()
                    {
                        canvas.clear_links_requested = true;
                        ui.close_menu();
                    }
                }
            });

            ui.separator();

            // Inline Find & Zoom
            let find_resp = ui.add(
                egui::TextEdit::singleline(&mut canvas.find_query)
                    .hint_text(tr(lang, TextKey::MapFind))
                    .desired_width(120.0),
            );
            let q = canvas.find_query.trim().to_ascii_lowercase();
            let mut matches: Vec<&v1::Device> = if q.is_empty() {
                Vec::new()
            } else {
                map_devices
                    .iter()
                    .copied()
                    .filter(|d| d.name.to_ascii_lowercase().contains(&q) || d.address.contains(&q))
                    .collect()
            };
            matches.sort_by_key(|d| d.id);

            if find_resp.changed() {
                canvas.find_match_index = 0;
            }
            let enter_next =
                find_resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter));
            let next_clicked = if !q.is_empty() {
                ui.label(
                    egui::RichText::new(format!("{}", matches.len()))
                        .small()
                        .color(muted_color(theme)),
                );
                ui.add_enabled(
                    !matches.is_empty(),
                    egui::Button::new(pick_lang(lang, ">", "Next")),
                )
                .clicked()
            } else {
                false
            };

            if (find_resp.changed() || enter_next || next_clicked) && !matches.is_empty() {
                if next_clicked || enter_next {
                    canvas.find_match_index = (canvas.find_match_index + 1) % matches.len();
                }
                if let Some(dev) = matches.get(canvas.find_match_index % matches.len()) {
                    canvas.clear_selection();
                    canvas.selected_devices.insert(dev.id);
                    focus_target = Some((dev.pos_x, dev.pos_y));
                }
            }

            ui.separator();
            if ui.button("-").clicked() {
                canvas.zoom_out();
            }
            ui.monospace(format!("{:.0}%", canvas.zoom * 100.0));
            if ui.button("+").clicked() {
                canvas.zoom_in();
            }
            if ui.button("100%").clicked() {
                canvas.reset_view();
            }
            if ui
                .button(pick_lang(lang, "Sesuaikan", "Fit"))
                .clicked()
            {
                canvas.fit_view_requested = true;
            }
        });
    });

    ui.add_space(6.0);

    if canvas.export_png_requested {
        canvas.export_png_requested = false;
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}.png", active_map.name))
            .add_filter("PNG Image", &["png"])
            .save_file()
        {
            let _ = export_map_to_png(&path, map_id, state, theme);
        }
    }

    // Allocate interactive canvas
    let avail = ui.available_rect_before_wrap();
    if let Some((wx, wy)) = focus_target {
        canvas.focus_world_point(avail, wx, wy);
    }
    if canvas.fit_view_requested {
        canvas.fit_view_requested = false;
        canvas.fit_to_devices(avail, &map_devices);
    }

    let (response, painter) = ui.allocate_painter(avail.size(), Sense::click_and_drag());
    let canvas_rect = response.rect;
    let origin = canvas_rect.min;

    // Keyboard Escape & Delete handling on canvas (only when text input isn't consuming focus)
    if !ctx.wants_keyboard_input() {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if canvas.active_tool != MapTool::Select
                || canvas.link_source_device.is_some()
                || canvas.dependency_parent_device.is_some()
            {
                canvas.cancel_active_tool();
            } else {
                canvas.clear_selection();
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete)) && !active_map.locked {
            delete_selected_items(state, canvas, cmd_tx);
        }
    }

    let bg_col = match theme {
        ThemeMode::Dark => Color32::from_rgb(22, 22, 25),
        ThemeMode::Light => Color32::from_rgb(247, 247, 249),
    };
    painter.rect_filled(canvas_rect, Rounding::same(5.0), bg_col);
    painter.rect_stroke(
        canvas_rect,
        Rounding::same(5.0),
        Stroke::new(1.0_f32, border_color(theme)),
    );

    // Optional background image (scaled & tiled)
    if let Some(bg_img_id) = active_map.background_image_id {
        let key = format!("img:{bg_img_id}:original");
        if let Some(tex) = textures.get(&key) {
            let sz = tex.size_vec2() * (active_map.background_scale as f32) * canvas.zoom;
            if sz.x > 4.0 && sz.y > 4.0 {
                if active_map.background_tile {
                    let mut x = origin.x + (canvas.pan.x % sz.x);
                    while x < canvas_rect.max.x {
                        let mut y = origin.y + (canvas.pan.y % sz.y);
                        while y < canvas_rect.max.y {
                            let r = Rect::from_min_size(Pos2::new(x, y), sz);
                            painter.image(
                                tex.id(),
                                r,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                            y += sz.y;
                        }
                        x += sz.x;
                    }
                } else {
                    let r = Rect::from_min_size(canvas.world_to_screen(origin, 0.0, 0.0), sz);
                    painter.image(
                        tex.id(),
                        r,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
        } else {
            request_image_cb(bg_img_id, "original");
        }
    }

    // Whisper-quiet grid lines only when enabled
    if canvas.show_grid && canvas.zoom >= 0.45 {
        let step = 48.0 * canvas.zoom;
        let grid_col = match theme {
            ThemeMode::Dark => Color32::from_rgba_unmultiplied(255, 255, 255, 6),
            ThemeMode::Light => Color32::from_rgba_unmultiplied(0, 0, 0, 8),
        };
        let mut gx = origin.x + (canvas.pan.x % step);
        while gx < canvas_rect.max.x {
            painter.line_segment(
                [Pos2::new(gx, canvas_rect.min.y), Pos2::new(gx, canvas_rect.max.y)],
                Stroke::new(1.0_f32, grid_col),
            );
            gx += step;
        }
        let mut gy = origin.y + (canvas.pan.y % step);
        while gy < canvas_rect.max.y {
            painter.line_segment(
                [Pos2::new(canvas_rect.min.x, gy), Pos2::new(canvas_rect.max.x, gy)],
                Stroke::new(1.0_f32, grid_col),
            );
            gy += step;
        }
    }

    // Zoom with wheel around pointer
    if response.hovered() {
        let scroll_y = ctx.input(|i| i.raw_scroll_delta.y);
        if scroll_y.abs() > 0.0 {
            if let Some(pointer) = response.hover_pos() {
                let before = canvas.screen_to_world(origin, pointer);
                let factor = (1.0 + scroll_y * 0.0015).clamp(0.75, 1.35);
                canvas.zoom = (canvas.zoom * factor).clamp(0.15, 4.0);
                let after_screen = canvas.world_to_screen(origin, before.x, before.y);
                canvas.pan += pointer - after_screen;
            }
        }
    }

    // Pan with middle mouse button (or Alt + Primary drag)
    if response.dragged_by(egui::PointerButton::Middle)
        || (response.dragged_by(egui::PointerButton::Primary) && ctx.input(|i| i.modifiers.alt))
    {
        canvas.pan += response.drag_delta();
    }

    let dev_lookup: HashMap<i64, &v1::Device> = map_devices.iter().map(|d| (d.id, *d)).collect();

    // Group parallel links between identical node pairs for fan-out calculation
    let mut pair_totals: HashMap<(i64, i64), Vec<i64>> = HashMap::new();
    for l in &map_links {
        let key = if l.src_device_id < l.dst_device_id {
            (l.src_device_id, l.dst_device_id)
        } else {
            (l.dst_device_id, l.src_device_id)
        };
        pair_totals.entry(key).or_default().push(l.id);
    }

    // Compute world-space viewport bounds for culling
    let world_min = canvas.screen_to_world(origin, canvas_rect.min);
    let world_max = canvas.screen_to_world(origin, canvas_rect.max);
    let cull_margin = 160.0;
    let is_world_visible = |x: f64, y: f64| -> bool {
        x >= world_min.x - cull_margin
            && x <= world_max.x + cull_margin
            && y >= world_min.y - cull_margin
            && y <= world_max.y + cull_margin
    };

    let text_primary = text_color(theme);
    let node_fill = surface_fill(theme);

    // Draw Static Items & Map Networks (background layers)
    for st_item in state.static_items.values().filter(|s| s.map_id == map_id) {
        let tl = canvas.world_to_screen(origin, st_item.pos_x, st_item.pos_y);
        let br = canvas.world_to_screen(
            origin,
            st_item.pos_x + st_item.width,
            st_item.pos_y + st_item.height,
        );
        let rect = Rect::from_min_max(tl, br);
        if !canvas_rect.intersects(rect) {
            continue;
        }
        let is_sel = canvas.selected_static == Some(st_item.id);
        let base_rgb = RgbColor::from_hex(&st_item.color_hex, RgbColor::new(51, 65, 85));
        let col = to_color32(base_rgb, if theme == ThemeMode::Dark { 80 } else { 42 });
        let border_col = if is_sel {
            to_color32(accent_rgb(theme), 255)
        } else {
            to_color32(base_rgb, 180)
        };
        painter.rect_filled(rect, Rounding::same(6.0), col);
        painter.rect_stroke(
            rect,
            Rounding::same(6.0),
            Stroke::new(if is_sel { 2.2_f32 } else { 1.2_f32 }, border_col),
        );
        if canvas.zoom >= 0.35 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &st_item.label,
                FontId::proportional((st_item.font_size as f32 * canvas.zoom).clamp(9.0, 32.0)),
                text_primary,
            );
        }
    }

    for net in state.networks.values().filter(|n| n.map_id == map_id) {
        let tl = canvas.world_to_screen(origin, net.pos_x, net.pos_y);
        let br = canvas.world_to_screen(origin, net.pos_x + net.width, net.pos_y + net.height);
        let rect = Rect::from_min_max(tl, br);
        if !canvas_rect.intersects(rect) {
            continue;
        }
        let is_sel = canvas.selected_network == Some(net.id);
        let n_state = v1::ProbeState::try_from(net.state).unwrap_or(v1::ProbeState::Unknown);
        let st_rgb = state_color(theme, n_state);
        let col = to_color32(st_rgb, if theme == ThemeMode::Dark { 42 } else { 28 });
        let border = if is_sel {
            to_color32(accent_rgb(theme), 255)
        } else {
            to_color32(st_rgb, 210)
        };
        painter.rect_filled(rect, Rounding::same(8.0), col);
        painter.rect_stroke(
            rect,
            Rounding::same(8.0),
            Stroke::new(if is_sel { 2.6_f32 } else { 1.8_f32 }, border),
        );
        if canvas.zoom >= 0.35 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}\n{}", net.name, net.cidr),
                FontId::proportional((12.0 * canvas.zoom).clamp(9.0, 20.0)),
                border,
            );
        }
    }

    // Draw Dependency Layer
    if matches!(
        canvas.layer_mode,
        v1::MapLayerMode::MapLayerDependencies | v1::MapLayerMode::MapLayerBoth
    ) {
        for dep in state.dependencies.values().filter(|d| d.map_id == map_id) {
            if let (Some(parent), Some(child)) = (
                dev_lookup.get(&dep.parent_device_id),
                dev_lookup.get(&dep.child_device_id),
            ) {
                if !is_world_visible(parent.pos_x, parent.pos_y)
                    && !is_world_visible(child.pos_x, child.pos_y)
                {
                    continue;
                }
                let p1 = canvas.world_to_screen(origin, parent.pos_x, parent.pos_y);
                let p2 = canvas.world_to_screen(origin, child.pos_x, child.pos_y);
                let dep_col = Color32::from_rgba_unmultiplied(249, 115, 22, 185);
                draw_dashed_segment(&painter, p1, p2, 6.0, 4.0, Stroke::new(1.6_f32, dep_col));
                draw_arrowhead(&painter, p1, p2, 8.0, dep_col);
            }
        }
    }

    // Pointer hover computation BEFORE painting links/nodes so hover rings render cleanly
    let pointer_world = response
        .hover_pos()
        .map(|p| canvas.screen_to_world(origin, p));
    let hovered_dev = pointer_world.and_then(|pw| {
        map_devices
            .iter()
            .rev()
            .find(|d| {
                let (hw, hh) = node_half_extents(d);
                (pw.x - d.pos_x).abs() <= hw && (pw.y - d.pos_y).abs() <= hh
            })
            .map(|d| d.id)
    });

    // Draw Elastic Links Layer + collect hit-testable paths
    let mut link_paths: Vec<(i64, Vec<Vec2d>, Vec2d, Vec2d)> = Vec::with_capacity(map_links.len());
    let mut label_anchors: Vec<(i64, Vec2d, String)> = Vec::new();
    let time_sec = ctx.input(|i| i.time);
    let mut any_flapping = false;

    if matches!(
        canvas.layer_mode,
        v1::MapLayerMode::MapLayerLinks | v1::MapLayerMode::MapLayerBoth
    ) {
        for link in &map_links {
            let (Some(src), Some(dst)) = (
                dev_lookup.get(&link.src_device_id),
                dev_lookup.get(&link.dst_device_id),
            ) else {
                continue;
            };
            if !is_world_visible(src.pos_x, src.pos_y) && !is_world_visible(dst.pos_x, dst.pos_y) {
                continue;
            }

            let pair_key = if link.src_device_id < link.dst_device_id {
                (link.src_device_id, link.dst_device_id)
            } else {
                (link.dst_device_id, link.src_device_id)
            };
            let group = &pair_totals[&pair_key];
            let p_idx = group.iter().position(|&id| id == link.id).unwrap_or(0);
            let p_tot = group.len();

            let ltype = state.link_types.get(&link.link_type_id);
            let style = v1::LinkStyle::try_from(
                link.style_override
                    .unwrap_or_else(|| ltype.map_or(active_map.default_link_style, |lt| lt.style)),
            )
            .unwrap_or(v1::LinkStyle::Curved);
            let thickness = ltype.map_or(active_map.default_link_thickness, |lt| lt.thickness) as f32
                * canvas.zoom.clamp(0.4, 2.0);

            let (shw, shh) = node_half_extents(src);
            let (dhw, dhh) = node_half_extents(dst);
            let s_shape = v1::NodeShape::try_from(src.shape).unwrap_or(v1::NodeShape::RoundedRect);
            let d_shape = v1::NodeShape::try_from(dst.shape).unwrap_or(v1::NodeShape::RoundedRect);
            let bends: Vec<Vec2d> = link
                .bend_points
                .iter()
                .map(|p| Vec2d::new(p.x, p.y))
                .collect();

            let computed = compute_elastic_link_path(
                Vec2d::new(src.pos_x, src.pos_y),
                shw,
                shh,
                s_shape,
                Vec2d::new(dst.pos_x, dst.pos_y),
                dhw,
                dhh,
                d_shape,
                style,
                &bends,
                p_idx,
                p_tot,
            );

            let lstatus = v1::LinkStatus::try_from(link.status).unwrap_or(v1::LinkStatus::Unknown);
            let mut alpha = 235u8;
            if lstatus == v1::LinkStatus::Flapping {
                any_flapping = true;
                let pulse = (time_sec * 5.0).sin() * 0.4 + 0.6;
                alpha = (pulse * 255.0) as u8;
            }

            let in_col = match lstatus {
                v1::LinkStatus::Down => to_color32(state_color(theme, v1::ProbeState::Down), alpha),
                v1::LinkStatus::NoReply => to_color32(warning_rgb(theme), alpha),
                v1::LinkStatus::Flapping => to_color32(RgbColor::new(191, 90, 242), alpha),
                v1::LinkStatus::Unknown => Color32::from_rgba_unmultiplied(125, 130, 140, 110),
                _ => to_color32(utilization_color(theme, link.in_utilization_pct), alpha),
            };
            let out_col = match lstatus {
                v1::LinkStatus::Down => to_color32(state_color(theme, v1::ProbeState::Down), alpha),
                v1::LinkStatus::NoReply => to_color32(warning_rgb(theme), alpha),
                v1::LinkStatus::Flapping => to_color32(RgbColor::new(191, 90, 242), alpha),
                v1::LinkStatus::Unknown => Color32::from_rgba_unmultiplied(125, 130, 140, 110),
                _ => to_color32(utilization_color(theme, link.out_utilization_pct), alpha),
            };

            let is_selected = canvas.selected_links.contains(&link.id);
            let stroke_w = if is_selected {
                thickness + 1.5
            } else {
                (thickness * 0.85).max(1.2)
            };
            let to_screen_pts = |pts: &[Vec2d]| -> Vec<Pos2> {
                pts.iter()
                    .map(|p| canvas.world_to_screen(origin, p.x, p.y))
                    .collect()
            };

            let src_screen = to_screen_pts(&computed.src_half_points);
            let dst_screen = to_screen_pts(&computed.dst_half_points);

            if is_selected {
                let full_screen = to_screen_pts(&computed.full_points);
                painter.add(egui::Shape::line(
                    full_screen,
                    Stroke::new(stroke_w + 4.0, to_color32(accent_rgb(theme), 75)),
                ));
            }

            if lstatus == v1::LinkStatus::Down {
                for w in src_screen.windows(2) {
                    draw_dashed_segment(&painter, w[0], w[1], 7.0, 5.0, Stroke::new(stroke_w, in_col));
                }
                for w in dst_screen.windows(2) {
                    draw_dashed_segment(&painter, w[0], w[1], 7.0, 5.0, Stroke::new(stroke_w, out_col));
                }
            } else {
                painter.add(egui::Shape::line(src_screen, Stroke::new(stroke_w, in_col)));
                painter.add(egui::Shape::line(dst_screen, Stroke::new(stroke_w, out_col)));
            }

            if is_selected || canvas.zoom >= 0.6 {
                let h_pos = canvas.world_to_screen(
                    origin,
                    computed.active_handle.x,
                    computed.active_handle.y,
                );
                if is_selected {
                    painter.circle_filled(h_pos, 5.0, Color32::WHITE);
                    painter.circle_stroke(
                        h_pos,
                        5.0,
                        Stroke::new(1.5_f32, to_color32(accent_rgb(theme), 255)),
                    );
                }
                if lstatus == v1::LinkStatus::NoReply {
                    let m_pos =
                        canvas.world_to_screen(origin, computed.midpoint.x, computed.midpoint.y);
                    painter.circle_filled(m_pos, 6.5, to_color32(warning_rgb(theme), 255));
                    painter.text(
                        m_pos,
                        egui::Align2::CENTER_CENTER,
                        "!",
                        FontId::proportional(10.5),
                        Color32::BLACK,
                    );
                }
            }

            let has_link_data = !link.interface_key.is_empty()
                || link.in_bps > 0.0
                || link.out_bps > 0.0
                || !link.label.is_empty();
            if canvas.show_link_labels && canvas.zoom >= 0.55 && has_link_data {
                let lbl = if !link.interface_key.is_empty() {
                    if canvas.zoom >= 0.85 {
                        format!(
                            "[{}] RX {} | TX {}",
                            link.interface_key,
                            format_bps(link.in_bps),
                            format_bps(link.out_bps)
                        )
                    } else {
                        format!(
                            "RX {} | TX {}",
                            format_bps(link.in_bps),
                            format_bps(link.out_bps)
                        )
                    }
                } else if !link.label.is_empty() {
                    link.label.clone()
                } else {
                    format!(
                        "RX {} | TX {}",
                        format_bps(link.in_bps),
                        format_bps(link.out_bps)
                    )
                };
                label_anchors.push((link.id, computed.midpoint, lbl));
            }

            link_paths.push((
                link.id,
                computed.full_points,
                computed.active_handle,
                computed.default_handle,
            ));
        }

        // Draw mid-link RX/TX labels with crisp solid backing
        let mut anchor_pts: Vec<Vec2d> = label_anchors.iter().map(|(_, p, _)| *p).collect();
        avoid_label_overlaps(
            &mut anchor_pts,
            115.0 / f64::from(canvas.zoom),
            18.0 / f64::from(canvas.zoom),
        );
        let lbl_bg = surface_fill(theme).gamma_multiply(0.92);
        for ((_, _, lbl), pos) in label_anchors.into_iter().zip(anchor_pts) {
            let sp = canvas.world_to_screen(origin, pos.x, pos.y);
            let galley = painter.layout_no_wrap(lbl, FontId::proportional(10.0), text_primary);
            let pill = Rect::from_center_size(sp, galley.size() + Vec2::new(8.0, 4.0));
            painter.rect_filled(pill, Rounding::same(3.0), lbl_bg);
            painter.rect_stroke(
                pill,
                Rounding::same(3.0),
                Stroke::new(1.0_f32, border_color(theme)),
            );
            painter.galley(pill.min + Vec2::new(4.0, 2.0), galley, text_primary);
        }
    }

    if any_flapping {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    // Rubber-band dashed preview line when placing a Link or Dependency
    if let (Some(src_id), Some(pointer_screen)) = (
        canvas
            .link_source_device
            .or(canvas.dependency_parent_device),
        response.hover_pos(),
    ) {
        if let Some(src_dev) = dev_lookup.get(&src_id) {
            let sp = canvas.world_to_screen(origin, src_dev.pos_x, src_dev.pos_y);
            let col = if canvas.link_source_device.is_some() {
                to_color32(accent_rgb(theme), 230)
            } else {
                Color32::from_rgb(249, 115, 22)
            };
            draw_dashed_segment(&painter, sp, pointer_screen, 8.0, 5.0, Stroke::new(2.0_f32, col));
            ctx.request_repaint();
        }
    }

    // Draw Submap items (theme-aware)
    for sub in state.submaps.values().filter(|s| s.map_id == map_id) {
        if !is_world_visible(sub.pos_x, sub.pos_y) {
            continue;
        }
        let sp = canvas.world_to_screen(origin, sub.pos_x, sub.pos_y);
        let r = 24.0 * canvas.zoom;
        let rect = Rect::from_center_size(sp, Vec2::splat(r * 2.0));
        let st = v1::ProbeState::try_from(sub.aggregate_state).unwrap_or(v1::ProbeState::Unknown);
        let col = to_color32(state_color(theme, st), 230);
        let is_sel = canvas.selected_submap == Some(sub.id);
        painter.rect_filled(rect, Rounding::same(6.0), node_fill);
        painter.rect_stroke(
            rect,
            Rounding::same(6.0),
            Stroke::new(if is_sel { 3.2_f32 } else { 2.2_f32 }, col),
        );
        if canvas.zoom >= 0.35 {
            painter.text(
                sp,
                egui::Align2::CENTER_CENTER,
                "MAP",
                FontId::proportional((10.0 * canvas.zoom).clamp(8.0, 14.0)),
                col,
            );
            painter.text(
                Pos2::new(sp.x, rect.max.y + 8.0),
                egui::Align2::CENTER_TOP,
                &sub.label,
                FontId::proportional((11.5 * canvas.zoom).clamp(9.0, 16.0)),
                col,
            );
        }
    }

    // Draw Devices (LOD-aware & frustum-culled for 2,000+ node smoothness)
    let find_q_lower = canvas.find_query.trim().to_ascii_lowercase();
    for dev in &map_devices {
        if !is_world_visible(dev.pos_x, dev.pos_y) {
            continue;
        }
        let sp = canvas.world_to_screen(origin, dev.pos_x, dev.pos_y);
        let (hw, hh) = node_half_extents(dev);
        let rw = (hw as f32) * canvas.zoom;
        let rh = (hh as f32) * canvas.zoom;
        let d_state =
            v1::ProbeState::try_from(dev.aggregate_state).unwrap_or(v1::ProbeState::Unknown);
        let st_col = to_color32(state_color(theme, d_state), 255);
        let selected = canvas.selected_devices.contains(&dev.id);
        let is_hovered = hovered_dev == Some(dev.id);
        let is_find_match = !find_q_lower.is_empty()
            && (dev.name.to_ascii_lowercase().contains(&find_q_lower)
                || dev.address.contains(&find_q_lower));

        if canvas.zoom < 0.35 {
            let rad = (rw * 0.7).max(3.8);
            painter.circle_filled(sp, rad, st_col);
            if selected || is_find_match {
                painter.circle_stroke(
                    sp,
                    rad + 2.5,
                    Stroke::new(1.8_f32, to_color32(accent_rgb(theme), 255)),
                );
            }
            continue;
        }

        let node_rect = Rect::from_center_size(sp, Vec2::new(rw * 2.0, rh * 2.0));
        let shape = v1::NodeShape::try_from(dev.shape).unwrap_or(v1::NodeShape::RoundedRect);
        let border_stroke = Stroke::new(if selected { 2.4_f32 } else { 1.4_f32 }, st_col);
        let shadow_col = Color32::from_rgba_unmultiplied(
            0,
            0,
            0,
            if theme == ThemeMode::Dark { 36 } else { 14 },
        );

        // Selection or hover halo
        if selected || is_hovered || is_find_match {
            let halo_col = if is_find_match && !selected {
                Color32::from_rgba_unmultiplied(255, 214, 10, 90)
            } else if selected {
                to_color32(accent_rgb(theme), 85)
            } else {
                to_color32(accent_rgb(theme), 40)
            };
            if shape == v1::NodeShape::Circle {
                painter.circle_stroke(sp, rw + 4.0, Stroke::new(2.5_f32, halo_col));
            } else {
                painter.rect_stroke(
                    node_rect.expand(3.5),
                    Rounding::same(6.0),
                    Stroke::new(2.5_f32, halo_col),
                );
            }
        }

        match shape {
            v1::NodeShape::Circle => {
                painter.circle_filled(sp + Vec2::new(0.0, 1.5), rw, shadow_col);
                painter.circle_filled(sp, rw, node_fill);
                painter.circle_stroke(sp, rw, border_stroke);
            }
            v1::NodeShape::Frameless => {
                if selected {
                    painter.rect_stroke(node_rect, Rounding::same(4.0), border_stroke);
                }
            }
            _ => {
                painter.rect_filled(
                    node_rect.translate(Vec2::new(0.0, 1.5)),
                    Rounding::same(4.0),
                    shadow_col,
                );
                painter.rect_filled(node_rect, Rounding::same(4.0), node_fill);
                painter.rect_stroke(node_rect, Rounding::same(4.0), border_stroke);
            }
        }

        // Render icon or vendor image cleanly inside node bounds
        let img_rect = node_rect.shrink(rw * 0.14);
        match state.resolve_device_image(dev) {
            DeviceImageSource::UploadedImage(iid) => {
                let tkey = format!("img:{iid}:64");
                if let Some(tex) = textures.get(&tkey) {
                    painter.image(
                        tex.id(),
                        img_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    request_image_cb(iid, "64");
                }
            }
            DeviceImageSource::BuiltinIconKey(bkey) => {
                let tkey = format!("builtin:{bkey}");
                if let Some(tex) = textures.get(&tkey) {
                    painter.image(
                        tex.id(),
                        img_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
        }

        // Minimal corner status dot; show glyph only when not healthy
        let badge_pos = Pos2::new(node_rect.max.x - 4.0, node_rect.min.y + 4.0);
        if d_state == v1::ProbeState::Up {
            painter.circle_filled(badge_pos, 5.0, node_fill);
            painter.circle_filled(badge_pos, 3.5, st_col);
        } else {
            painter.circle_filled(badge_pos, 7.5, node_fill);
            painter.circle_filled(badge_pos, 6.0, st_col);
            painter.text(
                badge_pos,
                egui::Align2::CENTER_CENTER,
                state_badge_glyph(d_state),
                FontId::proportional(7.0),
                Color32::WHITE,
            );
        }

        // Node label below with solid contrast backing so link lines never obscure text
        let label = dev
            .label_template
            .replace("{name}", &dev.name)
            .replace("{ip}", &dev.address)
            .replace("{status}", state_badge_glyph(d_state))
            .replace("{rtt}", &format!("{:.1}ms", dev.last_rtt_ms));
        let lbl_font = FontId::proportional((11.0 * canvas.zoom).clamp(9.0, 15.0));
        let galley = painter.layout(label, lbl_font, text_primary, f32::INFINITY);
        let lbl_pos = Pos2::new(sp.x - galley.size().x * 0.5, node_rect.max.y + 4.0);
        let lbl_rect = Rect::from_min_size(lbl_pos, galley.size()).expand2(Vec2::new(4.0, 1.5));
        painter.rect_filled(lbl_rect, Rounding::same(3.0), bg_col.gamma_multiply(0.88));
        painter.galley(lbl_pos, galley, text_primary);
    }

    // Handle pointer interactions (hit-test nodes, submaps, networks, static, link handles, links)
    if let Some(pw) = pointer_world {
        let hit_dev = hovered_dev;

        let hit_sub = state
            .submaps
            .values()
            .filter(|s| s.map_id == map_id)
            .find(|s| (pw.x - s.pos_x).abs() <= 24.0 && (pw.y - s.pos_y).abs() <= 24.0)
            .map(|s| s.id);

        let hit_handle = link_paths
            .iter()
            .find(|(_, _, h, _)| pw.dist(*h) <= 10.0 / f64::from(canvas.zoom))
            .map(|(lid, _, _, _)| *lid);

        let hit_link = if hit_dev.is_none() {
            link_paths
                .iter()
                .find(|(_, pts, _, _)| {
                    distance_to_polyline(pw, pts) <= 8.0 / f64::from(canvas.zoom)
                })
                .map(|(lid, _, _, _)| *lid)
        } else {
            None
        };

        let hit_net = if hit_dev.is_none() && hit_sub.is_none() && hit_link.is_none() {
            state
                .networks
                .values()
                .filter(|n| n.map_id == map_id)
                .find(|n| {
                    pw.x >= n.pos_x
                        && pw.x <= n.pos_x + n.width
                        && pw.y >= n.pos_y
                        && pw.y <= n.pos_y + n.height
                })
                .map(|n| n.id)
        } else {
            None
        };

        let hit_static = if hit_dev.is_none()
            && hit_sub.is_none()
            && hit_link.is_none()
            && hit_net.is_none()
        {
            state
                .static_items
                .values()
                .filter(|s| s.map_id == map_id)
                .find(|s| {
                    pw.x >= s.pos_x
                        && pw.x <= s.pos_x + s.width
                        && pw.y >= s.pos_y
                        && pw.y <= s.pos_y + s.height
                })
                .map(|s| s.id)
        } else {
            None
        };

        // Hover tooltip
        if let Some(did) = hit_dev {
            if let Some(d) = dev_lookup.get(&did) {
                let st = v1::ProbeState::try_from(d.aggregate_state)
                    .unwrap_or(v1::ProbeState::Unknown);
                response.clone().on_hover_text(format!(
                    "{} ({})\nStatus: {} | {} {}\nRTT: {:.2} ms",
                    d.name,
                    d.address,
                    probe_state_label(lang, st),
                    d.vendor,
                    d.model,
                    d.last_rtt_ms
                ));
            }
        } else if let Some(lid) = hit_link {
            if let Some(l) = state.links.get(&lid) {
                let ls = v1::LinkStatus::try_from(l.status).unwrap_or(v1::LinkStatus::Unknown);
                let (st_lbl, _) = link_status_info(lang, theme, ls);
                let if_disp = if l.interface_key.is_empty() {
                    "manual / none"
                } else {
                    l.interface_key.as_str()
                };
                response.clone().on_hover_text(format!(
                    "Link #{} [{}] - {}\nRX: {} ({:.1}%) | TX: {} ({:.1}%)",
                    l.id,
                    if_disp,
                    st_lbl,
                    format_bps(l.in_bps),
                    l.in_utilization_pct,
                    format_bps(l.out_bps),
                    l.out_utilization_pct
                ));
            }
        }

        // Double click actions
        if response.double_clicked() {
            if let Some(sid) = hit_sub {
                if let Some(sub) = state.submaps.get(&sid) {
                    canvas.pending_open_submap_tab = Some(sub.target_map_id);
                }
            } else if let Some(lid) = hit_handle {
                if let Some(mut link) = state.links.get(&lid).cloned() {
                    link.bend_points.clear();
                    let _ = cmd_tx.try_send(ClientCommand::SaveLink(link));
                }
            } else if let Some(did) = hit_dev {
                canvas.open_device_dialog = Some(did);
            } else if let Some(lid) = hit_link {
                canvas.open_link_dialog = Some(lid);
            }
        }

        // Primary click
        if response.clicked() {
            let ctrl = ctx.input(|i| i.modifiers.ctrl);
            match canvas.active_tool {
                MapTool::AddDevice => {
                    canvas.pending_new_device_pos = Some((pw.x, pw.y));
                    canvas.active_tool = MapTool::Select;
                }
                MapTool::AddLink => {
                    if let Some(did) = hit_dev {
                        if let Some(src_id) = canvas.link_source_device.take() {
                            if src_id != did {
                                canvas.pending_new_link = Some((src_id, did));
                            }
                            canvas.active_tool = MapTool::Select;
                        } else {
                            canvas.link_source_device = Some(did);
                        }
                    }
                }
                MapTool::AddDependency => {
                    if let Some(did) = hit_dev {
                        if let Some(parent_id) = canvas.dependency_parent_device.take() {
                            if parent_id != did {
                                let _ = cmd_tx.try_send(ClientCommand::SaveDependency(
                                    v1::Dependency {
                                        id: 0,
                                        version: 1,
                                        map_id,
                                        parent_device_id: parent_id,
                                        child_device_id: did,
                                    },
                                ));
                            }
                            canvas.active_tool = MapTool::Select;
                        } else {
                            canvas.dependency_parent_device = Some(did);
                        }
                    }
                }
                MapTool::AddNetwork => {
                    let _ = cmd_tx.try_send(ClientCommand::SaveNetwork(v1::MapNetwork {
                        id: 0,
                        version: 1,
                        map_id,
                        name: "LAN Segment".to_string(),
                        cidr: "10.0.0.0/24".to_string(),
                        pos_x: pw.x,
                        pos_y: pw.y,
                        width: 180.0,
                        height: 90.0,
                        image_id: None,
                        state: v1::ProbeState::Unknown as i32,
                    }));
                    canvas.active_tool = MapTool::Select;
                }
                MapTool::AddStatic => {
                    let _ = cmd_tx.try_send(ClientCommand::SaveStaticItem(v1::StaticItem {
                        id: 0,
                        version: 1,
                        map_id,
                        label: "Group".to_string(),
                        kind: "box".to_string(),
                        pos_x: pw.x,
                        pos_y: pw.y,
                        width: 180.0,
                        height: 90.0,
                        color_hex: "#334155".to_string(),
                        image_id: None,
                        font_size: 14,
                    }));
                    canvas.active_tool = MapTool::Select;
                }
                MapTool::AddSubmap => {
                    if let Some(other_map_id) = state.maps.keys().copied().find(|&id| id != map_id) {
                        let lbl = state
                            .maps
                            .get(&other_map_id)
                            .map_or("Submap".to_string(), |m| m.name.clone());
                        let _ = cmd_tx.try_send(ClientCommand::SaveSubmap(v1::SubmapItem {
                            id: 0,
                            version: 1,
                            map_id,
                            target_map_id: other_map_id,
                            label: lbl,
                            pos_x: pw.x,
                            pos_y: pw.y,
                            image_id: None,
                            aggregate_state: v1::ProbeState::Unknown as i32,
                        }));
                    }
                    canvas.active_tool = MapTool::Select;
                }
                MapTool::Select => {
                    if !ctrl {
                        canvas.clear_selection();
                    }
                    if let Some(did) = hit_dev {
                        canvas.selected_devices.insert(did);
                    } else if let Some(sid) = hit_sub {
                        canvas.selected_submap = Some(sid);
                    } else if let Some(lid) = hit_link {
                        canvas.selected_links.insert(lid);
                    } else if let Some(nid) = hit_net {
                        canvas.selected_network = Some(nid);
                    } else if let Some(stid) = hit_static {
                        canvas.selected_static = Some(stid);
                    }
                }
            }
        }

        // Drag start / active / stop (respecting Map Lock and Alt-pan!)
        let alt_down = ctx.input(|i| i.modifiers.alt);
        if response.drag_started_by(egui::PointerButton::Primary)
            && !active_map.locked
            && !alt_down
        {
            if let Some(lid) = hit_handle {
                canvas.dragging_bend = Some((lid, 0));
                canvas.selected_links.clear();
                canvas.selected_links.insert(lid);
            } else if let Some(did) = hit_dev {
                if !canvas.selected_devices.contains(&did) {
                    canvas.clear_selection();
                    canvas.selected_devices.insert(did);
                }
                canvas.dragging_nodes = true;
            } else if let Some(hp) = response.hover_pos() {
                canvas.marquee_start = Some(hp);
            }
        }

        if response.dragged_by(egui::PointerButton::Primary) && !alt_down {
            let delta = response.drag_delta() / canvas.zoom;
            if let Some((lid, _)) = canvas.dragging_bend {
                if let Some(link) = state.links.get_mut(&lid) {
                    if link.bend_points.is_empty() {
                        link.bend_points.push(v1::Point2d { x: pw.x, y: pw.y });
                    } else {
                        link.bend_points[0].x = pw.x;
                        link.bend_points[0].y = pw.y;
                    }
                }
            } else if canvas.dragging_nodes && !active_map.locked {
                for did in &canvas.selected_devices {
                    if let Some(d) = state.devices.get_mut(did) {
                        d.pos_x += f64::from(delta.x);
                        d.pos_y += f64::from(delta.y);
                    }
                }
            } else if let (Some(start), Some(curr)) = (canvas.marquee_start, response.hover_pos()) {
                let m_rect = Rect::from_two_pos(start, curr);
                painter.rect_filled(
                    m_rect,
                    Rounding::ZERO,
                    to_color32(accent_rgb(theme), 35),
                );
                painter.rect_stroke(
                    m_rect,
                    Rounding::ZERO,
                    Stroke::new(1.2_f32, to_color32(accent_rgb(theme), 230)),
                );
            }
        }

        if response.drag_stopped_by(egui::PointerButton::Primary) {
            if let Some((lid, _)) = canvas.dragging_bend.take() {
                if let Some(link) = state.links.get(&lid).cloned() {
                    let _ = cmd_tx.try_send(ClientCommand::SaveLink(link));
                }
            }
            if canvas.dragging_nodes {
                canvas.dragging_nodes = false;
                for did in &canvas.selected_devices {
                    if let Some(mut d) = state.devices.get(did).cloned() {
                        if canvas.snap_to_grid {
                            d.pos_x = (d.pos_x / 16.0).round() * 16.0;
                            d.pos_y = (d.pos_y / 16.0).round() * 16.0;
                        }
                        let _ = cmd_tx.try_send(ClientCommand::SaveDevice(d));
                    }
                }
            }
            if let (Some(start), Some(curr)) = (canvas.marquee_start.take(), response.hover_pos()) {
                let w1 = canvas.screen_to_world(origin, start);
                let w2 = canvas.screen_to_world(origin, curr);
                let min_x = w1.x.min(w2.x);
                let max_x = w1.x.max(w2.x);
                let min_y = w1.y.min(w2.y);
                let max_y = w1.y.max(w2.y);
                canvas.clear_selection();
                for d in state.devices.values().filter(|d| d.map_id == map_id) {
                    if d.pos_x >= min_x
                        && d.pos_x <= max_x
                        && d.pos_y >= min_y
                        && d.pos_y <= max_y
                    {
                        canvas.selected_devices.insert(d.id);
                    }
                }
            }
        }
    }

    // Floating top-left tool hint overlay
    let tool_hint = match canvas.active_tool {
        MapTool::AddDevice => Some(pick_lang(
            lang,
            "Klik pada kanvas untuk menambah perangkat • Esc batal",
            "Click canvas to place new device • Esc to cancel",
        )),
        MapTool::AddLink => Some(if canvas.link_source_device.is_some() {
            pick_lang(
                lang,
                "Klik perangkat tujuan untuk membuat link • Esc batal",
                "Click destination device to finish link • Esc to cancel",
            )
        } else {
            pick_lang(
                lang,
                "Klik perangkat sumber link • Esc batal",
                "Click source device to start link • Esc to cancel",
            )
        }),
        MapTool::AddDependency => Some(if canvas.dependency_parent_device.is_some() {
            pick_lang(
                lang,
                "Klik perangkat child (terdampak) • Esc batal",
                "Click dependent child device • Esc to cancel",
            )
        } else {
            pick_lang(
                lang,
                "Klik perangkat parent (hulu) • Esc batal",
                "Click upstream parent device • Esc to cancel",
            )
        }),
        MapTool::AddNetwork => Some(pick_lang(
            lang,
            "Klik pada kanvas untuk menaruh segmen jaringan • Esc batal",
            "Click canvas to place network segment • Esc to cancel",
        )),
        MapTool::AddSubmap => Some(pick_lang(
            lang,
            "Klik pada kanvas untuk menautkan sub-peta • Esc batal",
            "Click canvas to place submap link • Esc to cancel",
        )),
        MapTool::AddStatic => Some(pick_lang(
            lang,
            "Klik pada kanvas untuk menaruh label grup • Esc batal",
            "Click canvas to place static group box • Esc to cancel",
        )),
        MapTool::Select => None,
    };
    if let Some(hint_str) = tool_hint {
        let hint_pos = Pos2::new(
            (origin.x + 12.0).min((canvas_rect.max.x - 160.0).max(origin.x + 12.0)),
            origin.y + 12.0,
        );
        egui::Area::new(egui::Id::new("map_tool_hint"))
            .order(egui::Order::Middle)
            .interactable(false)
            .fixed_pos(hint_pos)
            .show(ctx, |ui| {
                notice_frame(theme, accent_rgb(theme)).show(ui, |ui| {
                    ui.label(egui::RichText::new(hint_str).small().strong());
                });
            });
    }

    // Floating bottom-right HUD pill (non-interactive, hairline Apple capsule)
    let hud_pos = Pos2::new(
        (canvas_rect.max.x - 210.0).max(origin.x + 12.0),
        (canvas_rect.max.y - 36.0).max(origin.y + 12.0),
    );
    egui::Area::new(egui::Id::new("map_hud_pill"))
        .order(egui::Order::Middle)
        .interactable(false)
        .fixed_pos(hud_pos)
        .show(ctx, |ui| {
            let hud_bg = match theme {
                ThemeMode::Dark => Color32::from_rgba_unmultiplied(28, 28, 30, 220),
                ThemeMode::Light => Color32::from_rgba_unmultiplied(255, 255, 255, 232),
            };
            egui::Frame::default()
                .fill(hud_bg)
                .stroke(Stroke::new(1.0_f32, border_color(theme)))
                .rounding(Rounding::same(4.0))
                .inner_margin(Margin::symmetric(9.0, 3.0))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} dev | {} links | {:.0}%",
                            map_device_count,
                            map_link_count,
                            canvas.zoom * 100.0
                        ))
                        .small()
                        .color(muted_color(theme)),
                    );
                });
        });

    // Context Menu on right-click
    response.context_menu(|ui| {
        if let Some(&did) = canvas.selected_devices.iter().next() {
            if ui.button(pick_lang(lang, "Edit Perangkat", "Edit Device")).clicked() {
                canvas.open_device_dialog = Some(did);
                ui.close_menu();
            }
            if ui.button("Ping (Tools)").clicked() {
                canvas.open_tool_for_device = Some((did, v1::ToolKind::Ping));
                ui.close_menu();
            }
            if ui.button("Traceroute (Tools)").clicked() {
                canvas.open_tool_for_device = Some((did, v1::ToolKind::Traceroute));
                ui.close_menu();
            }
            ui.separator();
            if ui
                .add_enabled(!active_map.locked, egui::Button::new(tr(lang, TextKey::Delete)))
                .clicked()
            {
                if let Some(d) = state.devices.get(&did) {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteDevice(v1::IdRequest {
                        id: d.id,
                        expected_version: d.version,
                    }));
                }
                ui.close_menu();
            }
        } else if let Some(&lid) = canvas.selected_links.iter().next() {
            if ui.button(pick_lang(lang, "Edit Tautan", "Edit Link")).clicked() {
                canvas.open_link_dialog = Some(lid);
                ui.close_menu();
            }
            if ui
                .add_enabled(!active_map.locked, egui::Button::new(tr(lang, TextKey::Delete)))
                .clicked()
            {
                if let Some(l) = state.links.get(&lid) {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteLink(v1::IdRequest {
                        id: l.id,
                        expected_version: l.version,
                    }));
                }
                ui.close_menu();
            }
        } else if let Some(nid) = canvas.selected_network {
            if ui
                .add_enabled(!active_map.locked, egui::Button::new(tr(lang, TextKey::Delete)))
                .clicked()
            {
                if let Some(n) = state.networks.get(&nid) {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteNetwork(v1::IdRequest {
                        id: n.id,
                        expected_version: n.version,
                    }));
                }
                canvas.selected_network = None;
                ui.close_menu();
            }
        } else if let Some(stid) = canvas.selected_static {
            if ui
                .add_enabled(!active_map.locked, egui::Button::new(tr(lang, TextKey::Delete)))
                .clicked()
            {
                if let Some(s) = state.static_items.get(&stid) {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteStaticItem(v1::IdRequest {
                        id: s.id,
                        expected_version: s.version,
                    }));
                }
                canvas.selected_static = None;
                ui.close_menu();
            }
        } else {
            if ui.button(tr(lang, TextKey::MapAutoLayout)).clicked() && !active_map.locked {
                let _ = cmd_tx.try_send(ClientCommand::AutoLayoutMap(v1::AutoLayoutRequest {
                    map_id,
                    iterations: 60,
                }));
                ui.close_menu();
            }
            if ui.button(pick_lang(lang, "Sesuaikan Layar", "Fit View")).clicked() {
                canvas.fit_view_requested = true;
                ui.close_menu();
            }
        }
    });

    // Map Settings modal
    if canvas.show_map_settings {
        let mut edit_map = active_map.clone();
        let mut save_clicked = false;
        let mut close_clicked = false;
        centered_modal(tr(lang, TextKey::MapSettings), 400.0).show(ctx, |ui| {
            egui::Grid::new("map_settings_grid")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .show(ui, |ui| {
                    ui.label(tr(lang, TextKey::Name));
                    ui.text_edit_singleline(&mut edit_map.name);
                    ui.end_row();

                    let mut poll_ov = edit_map.poll_interval_sec_override.unwrap_or(0);
                    ui.label("Poll Override (s, 0=global)");
                    ui.add(egui::DragValue::new(&mut poll_ov).range(0..=3600));
                    edit_map.poll_interval_sec_override =
                        if poll_ov == 0 { None } else { Some(poll_ov) };
                    ui.end_row();

                    ui.label("Label Refresh (s)");
                    ui.add(egui::DragValue::new(&mut edit_map.label_refresh_sec).range(1..=300));
                    ui.end_row();

                    ui.label("Default Link Thickness");
                    ui.add(
                        egui::DragValue::new(&mut edit_map.default_link_thickness)
                            .range(0.5..=10.0),
                    );
                    ui.end_row();

                    ui.label("Default Link Style");
                    let curr_style = v1::LinkStyle::try_from(edit_map.default_link_style)
                        .unwrap_or(v1::LinkStyle::Curved);
                    let style_text = match curr_style {
                        v1::LinkStyle::Straight => tr(lang, TextKey::LinkStyleStraight),
                        v1::LinkStyle::OrthogonalRounded => {
                            tr(lang, TextKey::LinkStyleOrthogonal)
                        }
                        _ => tr(lang, TextKey::LinkStyleCurved),
                    };
                    egui::ComboBox::from_id_salt("map_default_link_style")
                        .selected_text(style_text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut edit_map.default_link_style,
                                v1::LinkStyle::Straight as i32,
                                tr(lang, TextKey::LinkStyleStraight),
                            );
                            ui.selectable_value(
                                &mut edit_map.default_link_style,
                                v1::LinkStyle::Curved as i32,
                                tr(lang, TextKey::LinkStyleCurved),
                            );
                            ui.selectable_value(
                                &mut edit_map.default_link_style,
                                v1::LinkStyle::OrthogonalRounded as i32,
                                tr(lang, TextKey::LinkStyleOrthogonal),
                            );
                        });
                    ui.end_row();

                    ui.label("Background Scale");
                    ui.add(
                        egui::DragValue::new(&mut edit_map.background_scale)
                            .speed(0.05)
                            .range(0.1..=5.0),
                    );
                    ui.end_row();
                });

            ui.add_space(6.0);
            ui.checkbox(&mut edit_map.background_tile, "Tile Background Image");
            ui.add_space(10.0);

            let (s_click, c_click) = modal_footer_buttons(
                ui,
                theme,
                !edit_map.name.trim().is_empty(),
                tr(lang, TextKey::Save),
                tr(lang, TextKey::Close),
            );
            save_clicked = s_click;
            close_clicked = c_click;
        });
        if save_clicked {
            let _ = cmd_tx.try_send(ClientCommand::SaveMap(edit_map));
            canvas.show_map_settings = false;
        } else if close_clicked {
            canvas.show_map_settings = false;
        }
    }
}

fn delete_selected_items(
    state: &ClientStateCache,
    canvas: &mut MapCanvasState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    for did in canvas.selected_devices.drain() {
        if let Some(d) = state.devices.get(&did) {
            let _ = cmd_tx.try_send(ClientCommand::DeleteDevice(v1::IdRequest {
                id: d.id,
                expected_version: d.version,
            }));
        }
    }
    for lid in canvas.selected_links.drain() {
        if let Some(l) = state.links.get(&lid) {
            let _ = cmd_tx.try_send(ClientCommand::DeleteLink(v1::IdRequest {
                id: l.id,
                expected_version: l.version,
            }));
        }
    }
    if let Some(nid) = canvas.selected_network.take() {
        if let Some(n) = state.networks.get(&nid) {
            let _ = cmd_tx.try_send(ClientCommand::DeleteNetwork(v1::IdRequest {
                id: n.id,
                expected_version: n.version,
            }));
        }
    }
    if let Some(stid) = canvas.selected_static.take() {
        if let Some(s) = state.static_items.get(&stid) {
            let _ = cmd_tx.try_send(ClientCommand::DeleteStaticItem(v1::IdRequest {
                id: s.id,
                expected_version: s.version,
            }));
        }
    }
}

enum AlignMode {
    Row,
    Circle,
    Line,
}

fn align_selected_devices(
    state: &ClientStateCache,
    canvas: &MapCanvasState,
    mode: AlignMode,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    let mut devs: Vec<v1::Device> = canvas
        .selected_devices
        .iter()
        .filter_map(|id| state.devices.get(id).cloned())
        .collect();
    if devs.len() < 2 {
        return;
    }
    devs.sort_by_key(|d| d.id);
    let cx = devs.iter().map(|d| d.pos_x).sum::<f64>() / (devs.len() as f64);
    let cy = devs.iter().map(|d| d.pos_y).sum::<f64>() / (devs.len() as f64);

    match mode {
        AlignMode::Row => {
            let start_x = cx - ((devs.len() as f64 - 1.0) * 120.0) * 0.5;
            for (i, mut d) in devs.into_iter().enumerate() {
                d.pos_x = start_x + (i as f64) * 120.0;
                d.pos_y = cy;
                let _ = cmd_tx.try_send(ClientCommand::SaveDevice(d));
            }
        }
        AlignMode::Line => {
            let start_y = cy - ((devs.len() as f64 - 1.0) * 100.0) * 0.5;
            for (i, mut d) in devs.into_iter().enumerate() {
                d.pos_x = cx;
                d.pos_y = start_y + (i as f64) * 100.0;
                let _ = cmd_tx.try_send(ClientCommand::SaveDevice(d));
            }
        }
        AlignMode::Circle => {
            let radius = ((devs.len() as f64) * 35.0).max(120.0);
            let n = devs.len() as f64;
            for (i, mut d) in devs.into_iter().enumerate() {
                let angle = (i as f64 / n) * std::f64::consts::TAU;
                d.pos_x = cx + radius * angle.cos();
                d.pos_y = cy + radius * angle.sin();
                let _ = cmd_tx.try_send(ClientCommand::SaveDevice(d));
            }
        }
    }
}

fn draw_dashed_segment(painter: &Painter, a: Pos2, b: Pos2, dash: f32, gap: f32, stroke: Stroke) {
    let diff = b - a;
    let len = diff.length();
    if len < 1.0 {
        return;
    }
    let dir = diff / len;
    let mut dist = 0.0;
    while dist < len {
        let end = (dist + dash).min(len);
        painter.line_segment([a + dir * dist, a + dir * end], stroke);
        dist += dash + gap;
    }
}

fn draw_arrowhead(painter: &Painter, a: Pos2, b: Pos2, size: f32, color: Color32) {
    let diff = b - a;
    let len = diff.length();
    if len < 2.0 {
        return;
    }
    let dir = diff / len;
    let perp = Vec2::new(-dir.y, dir.x);
    let tip = a + dir * (len * 0.65);
    let p1 = tip - dir * size + perp * (size * 0.5);
    let p2 = tip - dir * size - perp * (size * 0.5);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, p1, p2],
        color,
        Stroke::NONE,
    ));
}

pub fn format_bps(bps: f64) -> String {
    if bps >= 1_000_000_000.0 {
        format!("{:.2} Gbps", bps / 1_000_000_000.0)
    } else if bps >= 1_000_000.0 {
        format!("{:.2} Mbps", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.1} Kbps", bps / 1_000.0)
    } else {
        format!("{bps:.0} bps")
    }
}

fn export_map_to_png(
    path: &std::path::Path,
    map_id: i64,
    state: &ClientStateCache,
    theme: ThemeMode,
) -> Result<(), String> {
    let devs: Vec<&v1::Device> = state
        .devices
        .values()
        .filter(|d| d.map_id == map_id)
        .collect();
    let (min_x, min_y, max_x, max_y) = if devs.is_empty() {
        (0.0, 0.0, 1200.0, 800.0)
    } else {
        let mut x0 = f64::INFINITY;
        let mut y0 = f64::INFINITY;
        let mut x1 = f64::NEG_INFINITY;
        let mut y1 = f64::NEG_INFINITY;
        for d in &devs {
            x0 = x0.min(d.pos_x - 100.0);
            y0 = y0.min(d.pos_y - 100.0);
            x1 = x1.max(d.pos_x + 100.0);
            y1 = y1.max(d.pos_y + 100.0);
        }
        (x0, y0, x1, y1)
    };
    let w = ((max_x - min_x).ceil() as u32).clamp(640, 4096);
    let h = ((max_y - min_y).ceil() as u32).clamp(480, 4096);
    let bg = match theme {
        ThemeMode::Dark => image::Rgba([13, 20, 36, 255]),
        ThemeMode::Light => image::Rgba([241, 245, 249, 255]),
    };
    let fill = match theme {
        ThemeMode::Dark => image::Rgba([24, 34, 56, 255]),
        ThemeMode::Light => image::Rgba([255, 255, 255, 255]),
    };
    let mut img = image::RgbaImage::from_pixel(w, h, bg);
    let dev_map: HashMap<i64, &v1::Device> = devs.iter().map(|d| (d.id, *d)).collect();

    for l in state.links.values().filter(|l| l.map_id == map_id) {
        if let (Some(s), Some(d)) = (
            dev_map.get(&l.src_device_id),
            dev_map.get(&l.dst_device_id),
        ) {
            let c = utilization_color(theme, l.in_utilization_pct.max(l.out_utilization_pct));
            draw_thick_line_rgba(
                &mut img,
                (s.pos_x - min_x) as i32,
                (s.pos_y - min_y) as i32,
                (d.pos_x - min_x) as i32,
                (d.pos_y - min_y) as i32,
                image::Rgba([c.r, c.g, c.b, 255]),
            );
        }
    }

    for d in &devs {
        let st = v1::ProbeState::try_from(d.aggregate_state).unwrap_or(v1::ProbeState::Unknown);
        let c = state_color(theme, st);
        let ring = image::Rgba([c.r, c.g, c.b, 255]);
        let cx = (d.pos_x - min_x) as i32;
        let cy = (d.pos_y - min_y) as i32;
        let radius = 20i32;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let dist_sq = dx * dx + dy * dy;
                if dist_sq <= radius * radius {
                    let px = cx + dx;
                    let py = cy + dy;
                    if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                        let col = if dist_sq >= (radius - 3) * (radius - 3) {
                            ring
                        } else {
                            fill
                        };
                        img.put_pixel(px as u32, py as u32, col);
                    }
                }
            }
        }
    }

    img.save(path).map_err(|e| format!("PNG save failed: {e}"))
}

fn draw_thick_line_rgba(
    img: &mut image::RgbaImage,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: image::Rgba<u8>,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (w, h) = img.dimensions();
    loop {
        for oy in -1..=1 {
            for ox in -1..=1 {
                let px = x0 + ox;
                let py = y0 + oy;
                if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    img.put_pixel(px as u32, py as u32, color);
                }
            }
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}
