use eframe::egui::{self, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Sense, Stroke, Vec2};
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::geometry::{
    avoid_label_overlaps, compute_elastic_link_path, distance_to_polyline, Vec2d,
};
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::{ClientStateCache, DeviceImageSource};
use nextdude_client_core::theme::{state_badge_glyph, state_color, utilization_color, RgbColor, ThemeMode};
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
    pub link_source_device: Option<i64>,
    pub dependency_parent_device: Option<i64>,
    pub dragging_nodes: bool,
    pub dragging_bend: Option<(i64, usize)>,
    pub marquee_start: Option<Pos2>,
    pub find_query: String,
    pub show_map_settings: bool,
    pub clipboard_devices: Vec<i64>,
    pub clipboard_source_map: Option<i64>,
    pub open_device_dialog: Option<i64>,
    pub open_link_dialog: Option<i64>,
    pub pending_new_device_pos: Option<(f64, f64)>,
    pub pending_open_submap_tab: Option<i64>,
    pub show_new_map_dialog: bool,
    pub new_map_name: String,
    pub export_png_requested: bool,
}

impl Default for MapCanvasState {
    fn default() -> Self {
        Self {
            active_map_id: None,
            open_map_tabs: Vec::new(),
            pan: Vec2::new(60.0, 60.0),
            zoom: 1.0,
            active_tool: MapTool::Select,
            layer_mode: v1::MapLayerMode::Both,
            snap_to_grid: true,
            selected_devices: HashSet::new(),
            selected_links: HashSet::new(),
            selected_submap: None,
            link_source_device: None,
            dependency_parent_device: None,
            dragging_nodes: false,
            dragging_bend: None,
            marquee_start: None,
            find_query: String::new(),
            show_map_settings: false,
            clipboard_devices: Vec::new(),
            clipboard_source_map: None,
            open_device_dialog: None,
            open_link_dialog: None,
            pending_new_device_pos: None,
            pending_open_submap_tab: None,
            show_new_map_dialog: false,
            new_map_name: String::new(),
            export_png_requested: false,
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

    fn clear_selection(&mut self) {
        self.selected_devices.clear();
        self.selected_links.clear();
        self.selected_submap = None;
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
        canvas
            .open_map_tabs
            .retain(|id| state.maps.contains_key(id));
        if canvas.open_map_tabs.is_empty() {
            if let Some(&first_id) = state.maps.keys().next() {
                canvas.open_map_tabs.push(first_id);
                canvas.active_map_id = Some(first_id);
            }
        }
        if canvas.active_map_id.map_or(true, |id| !state.maps.contains_key(&id)) {
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

    // Map Tabs Bar
    ui.horizontal(|ui| {
        let map_ids = state.maps.keys().copied().collect::<Vec<_>>();
        for mid in &map_ids {
            if let Some(m) = state.maps.get(mid) {
                let selected = canvas.active_map_id == Some(*mid);
                if ui.selectable_label(selected, &m.name).clicked() {
                    if !canvas.open_map_tabs.contains(mid) {
                        canvas.open_map_tabs.push(*mid);
                    }
                    canvas.active_map_id = Some(*mid);
                    let _ = cmd_tx.try_send(ClientCommand::UpdateSubscription(v1::SubscribeRequest {
                        active_map_id: Some(*mid),
                        active_device_id: None,
                        active_chart_ids: Vec::new(),
                        include_events: true,
                        include_syslog: true,
                        include_traps: true,
                        include_outages: true,
                    }));
                }
            }
        }
        if ui.button("+ Map").clicked() {
            canvas.show_new_map_dialog = true;
            canvas.new_map_name.clear();
        }
    });

    if canvas.show_new_map_dialog {
        egui::Window::new(tr(lang, TextKey::TabMap))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::Name));
                    ui.text_edit_singleline(&mut canvas.new_map_name);
                });
                ui.horizontal(|ui| {
                    if ui.button(tr(lang, TextKey::Save)).clicked() && !canvas.new_map_name.trim().is_empty() {
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
                            default_layer: v1::MapLayerMode::Both as i32,
                        };
                        let _ = cmd_tx.try_send(ClientCommand::SaveMap(new_map));
                        canvas.show_new_map_dialog = false;
                    }
                    if ui.button(tr(lang, TextKey::Cancel)).clicked() {
                        canvas.show_new_map_dialog = false;
                    }
                });
            });
    }

    let Some(map_id) = canvas.active_map_id else {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.heading(tr(lang, TextKey::OnboardingTitle));
                ui.label(tr(lang, TextKey::OnboardingBody));
            });
        });
        return;
    };

    let Some(active_map) = state.maps.get(&map_id).cloned() else {
        return;
    };

    // Map Toolbar
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut canvas.active_tool, MapTool::Select, tr(lang, TextKey::MapToolSelect));
        ui.selectable_value(&mut canvas.active_tool, MapTool::AddDevice, tr(lang, TextKey::MapToolAddDevice));
        ui.selectable_value(&mut canvas.active_tool, MapTool::AddLink, tr(lang, TextKey::MapToolAddLink));
        ui.selectable_value(&mut canvas.active_tool, MapTool::AddNetwork, tr(lang, TextKey::MapToolAddNetwork));
        ui.selectable_value(&mut canvas.active_tool, MapTool::AddSubmap, tr(lang, TextKey::MapToolAddSubmap));
        ui.selectable_value(&mut canvas.active_tool, MapTool::AddStatic, tr(lang, TextKey::MapToolAddStatic));
        ui.selectable_value(&mut canvas.active_tool, MapTool::AddDependency, tr(lang, TextKey::MapToolAddDependency));
        ui.separator();

        ui.selectable_value(&mut canvas.layer_mode, v1::MapLayerMode::Links, tr(lang, TextKey::MapLayerLinks));
        ui.selectable_value(&mut canvas.layer_mode, v1::MapLayerMode::Dependencies, tr(lang, TextKey::MapLayerDependencies));
        ui.selectable_value(&mut canvas.layer_mode, v1::MapLayerMode::Both, tr(lang, TextKey::MapLayerBoth));
        ui.separator();

        let mut locked = active_map.locked;
        if ui.checkbox(&mut locked, tr(lang, TextKey::MapLock)).changed() {
            let mut updated = active_map.clone();
            updated.locked = locked;
            let _ = cmd_tx.try_send(ClientCommand::SaveMap(updated));
        }
        ui.checkbox(&mut canvas.snap_to_grid, tr(lang, TextKey::MapSnapGrid));

        if ui.button(tr(lang, TextKey::MapAutoLayout)).clicked() && !active_map.locked {
            let _ = cmd_tx.try_send(ClientCommand::AutoLayoutMap(v1::AutoLayoutRequest {
                map_id,
                iterations: 60,
            }));
        }
        if ui.button(tr(lang, TextKey::MapAlignRow)).clicked() && !active_map.locked {
            align_selected_devices(state, canvas, AlignMode::Row, cmd_tx);
        }
        if ui.button(tr(lang, TextKey::MapAlignCircle)).clicked() && !active_map.locked {
            align_selected_devices(state, canvas, AlignMode::Circle, cmd_tx);
        }
        if ui.button(tr(lang, TextKey::MapAlignLine)).clicked() && !active_map.locked {
            align_selected_devices(state, canvas, AlignMode::Line, cmd_tx);
        }
        if ui.button(tr(lang, TextKey::MapUndo)).clicked() {
            let _ = cmd_tx.try_send(ClientCommand::Undo);
        }
        if ui.button(tr(lang, TextKey::MapRedo)).clicked() {
            let _ = cmd_tx.try_send(ClientCommand::Redo);
        }
        if ui.button(tr(lang, TextKey::MapCopy)).clicked() && !canvas.selected_devices.is_empty() {
            canvas.clipboard_devices = canvas.selected_devices.iter().copied().collect();
            canvas.clipboard_source_map = Some(map_id);
        }
        if ui.button(tr(lang, TextKey::MapPaste)).clicked() && !canvas.clipboard_devices.is_empty() {
            if let Some(src_map) = canvas.clipboard_source_map {
                let _ = cmd_tx.try_send(ClientCommand::CopyPasteItems(v1::CopyPasteRequest {
                    source_map_id: src_map,
                    target_map_id: map_id,
                    device_ids: canvas.clipboard_devices.clone(),
                    network_ids: Vec::new(),
                    static_ids: Vec::new(),
                    offset_x: 40.0,
                    offset_y: 40.0,
                }));
            }
        }
        if ui.button(tr(lang, TextKey::MapExportPng)).clicked() {
            canvas.export_png_requested = true;
        }
        if ui.button(tr(lang, TextKey::MapSettings)).clicked() {
            canvas.show_map_settings = !canvas.show_map_settings;
        }
    });

    // Find bar on map
    ui.horizontal(|ui| {
        ui.label(tr(lang, TextKey::MapFind));
        let resp = ui.text_edit_singleline(&mut canvas.find_query);
        if resp.changed() && !canvas.find_query.trim().is_empty() {
            let q = canvas.find_query.trim().to_ascii_lowercase();
            if let Some(dev) = state
                .devices
                .values()
                .find(|d| d.map_id == map_id && (d.name.to_ascii_lowercase().contains(&q) || d.address.contains(&q)))
            {
                canvas.selected_devices.clear();
                canvas.selected_devices.insert(dev.id);
                let avail = ui.available_rect_before_wrap();
                canvas.focus_world_point(avail, dev.pos_x, dev.pos_y);
            }
        }
    });

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
    let (response, painter) = ui.allocate_painter(avail.size(), Sense::click_and_drag());
    let canvas_rect = response.rect;
    let origin = canvas_rect.min;

    let bg_col = match theme {
        ThemeMode::Dark => Color32::from_rgb(15, 23, 42),
        ThemeMode::Light => Color32::from_rgb(241, 245, 249),
    };
    painter.rect_filled(canvas_rect, 0.0, bg_col);

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

    // Subtle grid when zoomed in enough
    if canvas.zoom >= 0.45 {
        let step = 32.0 * canvas.zoom;
        let grid_col = match theme {
            ThemeMode::Dark => Color32::from_rgba_unmultiplied(51, 65, 85, 70),
            ThemeMode::Light => Color32::from_rgba_unmultiplied(148, 163, 184, 70),
        };
        let mut gx = origin.x + (canvas.pan.x % step);
        while gx < canvas_rect.max.x {
            painter.line_segment(
                [Pos2::new(gx, canvas_rect.min.y), Pos2::new(gx, canvas_rect.max.y)],
                Stroke::new(1.0, grid_col),
            );
            gx += step;
        }
        let mut gy = origin.y + (canvas.pan.y % step);
        while gy < canvas_rect.max.y {
            painter.line_segment(
                [Pos2::new(canvas_rect.min.x, gy), Pos2::new(canvas_rect.max.x, gy)],
                Stroke::new(1.0, grid_col),
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

    // Pan with middle mouse button
    if response.dragged_by(egui::PointerButton::Middle) {
        canvas.pan += response.drag_delta();
    }

    // Collect map entities
    let map_devices: Vec<&v1::Device> = state.devices.values().filter(|d| d.map_id == map_id).collect();
    let dev_lookup: HashMap<i64, &v1::Device> = map_devices.iter().map(|d| (d.id, *d)).collect();
    let map_links: Vec<&v1::NetworkLink> = state.links.values().filter(|l| l.map_id == map_id).collect();

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
        let col = to_color32(RgbColor::from_hex(&st_item.color_hex, RgbColor::new(51, 65, 85)), 90);
        painter.rect_filled(rect, CornerRadius::same(6), col);
        painter.rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.5, Color32::from_gray(150)),
            egui::StrokeKind::Outside,
        );
        if canvas.zoom >= 0.35 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &st_item.label,
                FontId::proportional((st_item.font_size as f32 * canvas.zoom).clamp(9.0, 32.0)),
                Color32::WHITE,
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
        let n_state = v1::ProbeState::try_from(net.state).unwrap_or(v1::ProbeState::Unknown);
        let col = to_color32(state_color(theme, n_state), 45);
        let border = to_color32(state_color(theme, n_state), 220);
        painter.rect_filled(rect, CornerRadius::same(8), col);
        painter.rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(2.0, border),
            egui::StrokeKind::Outside,
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
        v1::MapLayerMode::Dependencies | v1::MapLayerMode::Both
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
                let dep_col = Color32::from_rgba_unmultiplied(249, 115, 22, 180);
                draw_dashed_segment(&painter, p1, p2, 6.0, 4.0, Stroke::new(1.6, dep_col));
                draw_arrowhead(&painter, p1, p2, 8.0, dep_col);
            }
        }
    }

    // Draw Elastic Links Layer + collect hit-testable paths
    let mut link_paths: Vec<(i64, Vec<Vec2d>, Vec2d, Vec2d)> = Vec::with_capacity(map_links.len());
    let mut label_anchors: Vec<(i64, Vec2d, String)> = Vec::new();
    let time_sec = ctx.input(|i| i.time);
    let mut any_flapping = false;

    if matches!(
        canvas.layer_mode,
        v1::MapLayerMode::Links | v1::MapLayerMode::Both
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
            let bends: Vec<Vec2d> = link.bend_points.iter().map(|p| Vec2d::new(p.x, p.y)).collect();

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
                v1::LinkStatus::Down => to_color32(RgbColor::new(239, 68, 68), alpha),
                v1::LinkStatus::NoReply => to_color32(RgbColor::new(245, 158, 11), alpha),
                v1::LinkStatus::Flapping => to_color32(RgbColor::new(168, 85, 247), alpha),
                v1::LinkStatus::Unknown => to_color32(RgbColor::new(148, 163, 184), alpha),
                _ => to_color32(utilization_color(theme, link.in_utilization_pct), alpha),
            };
            let out_col = match lstatus {
                v1::LinkStatus::Down => to_color32(RgbColor::new(239, 68, 68), alpha),
                v1::LinkStatus::NoReply => to_color32(RgbColor::new(245, 158, 11), alpha),
                v1::LinkStatus::Flapping => to_color32(RgbColor::new(168, 85, 247), alpha),
                v1::LinkStatus::Unknown => to_color32(RgbColor::new(148, 163, 184), alpha),
                _ => to_color32(utilization_color(theme, link.out_utilization_pct), alpha),
            };

            let is_selected = canvas.selected_links.contains(&link.id);
            let stroke_w = if is_selected { thickness + 2.0 } else { thickness };
            let to_screen_pts = |pts: &[Vec2d]| -> Vec<Pos2> {
                pts.iter()
                    .map(|p| canvas.world_to_screen(origin, p.x, p.y))
                    .collect()
            };

            let src_screen = to_screen_pts(&computed.src_half_points);
            let dst_screen = to_screen_pts(&computed.dst_half_points);

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
                let h_pos = canvas.world_to_screen(origin, computed.active_handle.x, computed.active_handle.y);
                if is_selected {
                    painter.circle_filled(h_pos, 5.5, Color32::WHITE);
                    painter.circle_stroke(h_pos, 5.5, Stroke::new(1.5, Color32::from_rgb(14, 165, 233)));
                }
                if lstatus == v1::LinkStatus::NoReply {
                    let m_pos = canvas.world_to_screen(origin, computed.midpoint.x, computed.midpoint.y);
                    painter.circle_filled(m_pos, 7.0, Color32::from_rgb(245, 158, 11));
                    painter.text(
                        m_pos,
                        egui::Align2::CENTER_CENTER,
                        "!",
                        FontId::proportional(11.0),
                        Color32::BLACK,
                    );
                }
            }

            if canvas.zoom >= 0.55 {
                let lbl = format!(
                    "{}/{} ({:.0}%/{:.0}%)",
                    format_bps(link.in_bps),
                    format_bps(link.out_bps),
                    link.in_utilization_pct,
                    link.out_utilization_pct
                );
                label_anchors.push((link.id, computed.midpoint, lbl));
            }

            link_paths.push((
                link.id,
                computed.full_points,
                computed.active_handle,
                computed.default_handle,
            ));
        }

        // Draw mid-link traffic labels with simple overlap avoidance
        let mut anchor_pts: Vec<Vec2d> = label_anchors.iter().map(|(_, p, _)| *p).collect();
        avoid_label_overlaps(&mut anchor_pts, 110.0 / f64::from(canvas.zoom), 18.0 / f64::from(canvas.zoom));
        for ((_, _, lbl), pos) in label_anchors.into_iter().zip(anchor_pts.into_iter()) {
            let sp = canvas.world_to_screen(origin, pos.x, pos.y);
            let bg = match theme {
                ThemeMode::Dark => Color32::from_rgba_unmultiplied(15, 23, 42, 210),
                ThemeMode::Light => Color32::from_rgba_unmultiplied(255, 255, 255, 215),
            };
            let fg = match theme {
                ThemeMode::Dark => Color32::from_rgb(226, 232, 240),
                ThemeMode::Light => Color32::from_rgb(15, 23, 42),
            };
            let galley = painter.layout_no_wrap(lbl, FontId::proportional(10.0), fg);
            let pill = Rect::from_center_size(sp, galley.size() + Vec2::new(8.0, 4.0));
            painter.rect_filled(pill, CornerRadius::same(3), bg);
            painter.galley(pill.min + Vec2::new(4.0, 2.0), galley, fg);
        }
    }

    if any_flapping {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    // Draw Submap items
    for sub in state.submaps.values().filter(|s| s.map_id == map_id) {
        if !is_world_visible(sub.pos_x, sub.pos_y) {
            continue;
        }
        let sp = canvas.world_to_screen(origin, sub.pos_x, sub.pos_y);
        let r = 24.0 * canvas.zoom;
        let rect = Rect::from_center_size(sp, Vec2::splat(r * 2.0));
        let st = v1::ProbeState::try_from(sub.aggregate_state).unwrap_or(v1::ProbeState::Unknown);
        let col = to_color32(state_color(theme, st), 230);
        painter.rect_filled(rect, CornerRadius::same(6), Color32::from_rgb(30, 41, 59));
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(2.5, col), egui::StrokeKind::Outside);
        if canvas.zoom >= 0.35 {
            painter.text(
                Pos2::new(sp.x, rect.max.y + 10.0),
                egui::Align2::CENTER_TOP,
                format!("[Submap] {}", sub.label),
                FontId::proportional((11.5 * canvas.zoom).clamp(9.0, 16.0)),
                col,
            );
        }
    }

    // Draw Devices (LOD-aware & frustum-culled for 2,000+ node smoothness)
    for dev in &map_devices {
        if !is_world_visible(dev.pos_x, dev.pos_y) {
            continue;
        }
        let sp = canvas.world_to_screen(origin, dev.pos_x, dev.pos_y);
        let (hw, hh) = node_half_extents(dev);
        let rw = (hw as f32) * canvas.zoom;
        let rh = (hh as f32) * canvas.zoom;
        let d_state = v1::ProbeState::try_from(dev.aggregate_state).unwrap_or(v1::ProbeState::Unknown);
        let st_col = to_color32(state_color(theme, d_state), 255);
        let selected = canvas.selected_devices.contains(&dev.id);

        if canvas.zoom < 0.35 {
            // Fast LOD circle
            painter.circle_filled(sp, (rw * 0.7).max(3.5), st_col);
            if selected {
                painter.circle_stroke(sp, (rw * 0.7).max(3.5) + 2.0, Stroke::new(1.5, Color32::WHITE));
            }
            continue;
        }

        let node_rect = Rect::from_center_size(sp, Vec2::new(rw * 2.0, rh * 2.0));
        let shape = v1::NodeShape::try_from(dev.shape).unwrap_or(v1::NodeShape::RoundedRect);
        let fill = match theme {
            ThemeMode::Dark => Color32::from_rgb(30, 41, 59),
            ThemeMode::Light => Color32::from_rgb(255, 255, 255),
        };
        let border_stroke = Stroke::new(if selected { 3.5 } else { 2.2 }, st_col);

        match shape {
            v1::NodeShape::Circle => {
                painter.circle_filled(sp, rw, fill);
                painter.circle_stroke(sp, rw, border_stroke);
            }
            v1::NodeShape::Frameless => {
                if selected {
                    painter.rect_stroke(node_rect, CornerRadius::same(4), border_stroke, egui::StrokeKind::Outside);
                }
            }
            _ => {
                painter.rect_filled(node_rect, CornerRadius::same(8), fill);
                painter.rect_stroke(node_rect, CornerRadius::same(8), border_stroke, egui::StrokeKind::Outside);
            }
        }

        // Render icon texture resolved via 4-tier priority
        let img_rect = node_rect.shrink(rw * 0.18);
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

        // Accessibility status badge (non-color indicator)
        let badge_pos = Pos2::new(node_rect.max.x - 4.0, node_rect.min.y + 4.0);
        painter.circle_filled(badge_pos, 9.0, st_col);
        painter.text(
            badge_pos,
            egui::Align2::CENTER_CENTER,
            state_badge_glyph(d_state),
            FontId::proportional(8.0),
            Color32::WHITE,
        );

        // Node label below
        let label = dev
            .label_template
            .replace("{name}", &dev.name)
            .replace("{ip}", &dev.address)
            .replace("{status}", state_badge_glyph(d_state))
            .replace("{rtt}", &format!("{:.1}ms", dev.last_rtt_ms));
        let lbl_col = match theme {
            ThemeMode::Dark => Color32::from_rgb(226, 232, 240),
            ThemeMode::Light => Color32::from_rgb(15, 23, 42),
        };
        painter.text(
            Pos2::new(sp.x, node_rect.max.y + 4.0),
            egui::Align2::CENTER_TOP,
            label,
            FontId::proportional((11.0 * canvas.zoom).clamp(9.0, 16.0)),
            lbl_col,
        );
    }

    // Handle pointer interactions (hit-test nodes, submaps, link handles, curved links)
    let pointer_world = response
        .hover_pos()
        .map(|p| canvas.screen_to_world(origin, p));

    if let Some(pw) = pointer_world {
        let hit_dev = map_devices
            .iter()
            .rev()
            .find(|d| {
                let (hw, hh) = node_half_extents(d);
                (pw.x - d.pos_x).abs() <= hw && (pw.y - d.pos_y).abs() <= hh
            })
            .map(|d| d.id);

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
                .find(|(_, pts, _, _)| distance_to_polyline(pw, pts) <= 8.0 / f64::from(canvas.zoom))
                .map(|(lid, _, _, _)| *lid)
        } else {
            None
        };

        // Hover tooltip
        if let Some(did) = hit_dev {
            if let Some(d) = dev_lookup.get(&did) {
                response.clone().on_hover_text(format!(
                    "{} ({})\n{} {}\nRTT: {:.2} ms",
                    d.name, d.address, d.vendor, d.model, d.last_rtt_ms
                ));
            }
        } else if let Some(lid) = hit_link {
            if let Some(l) = state.links.get(&lid) {
                response.clone().on_hover_text(format!(
                    "Link #{} [{}]\nIn: {} ({:.1}%) | Out: {} ({:.1}%)",
                    l.id,
                    l.interface_key,
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
                                let new_link = v1::NetworkLink {
                                    id: 0,
                                    version: 1,
                                    map_id,
                                    src_device_id: src_id,
                                    dst_device_id: did,
                                    master_device_id: src_id,
                                    mastering_type: v1::MasteringType::SnmpIfTraffic as i32,
                                    link_type_id: state.link_types.keys().next().copied().unwrap_or(1),
                                    history_enabled: true,
                                    ..Default::default()
                                };
                                let _ = cmd_tx.try_send(ClientCommand::SaveLink(new_link));
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
                                let _ = cmd_tx.try_send(ClientCommand::SaveDependency(v1::Dependency {
                                    id: 0,
                                    version: 1,
                                    map_id,
                                    parent_device_id: parent_id,
                                    child_device_id: did,
                                }));
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
                        let lbl = state.maps.get(&other_map_id).map_or("Submap".to_string(), |m| m.name.clone());
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
                    }
                }
            }
        }

        // Drag start / active / stop (respecting Map Lock!)
        if response.drag_started_by(egui::PointerButton::Primary) && !active_map.locked {
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

        if response.dragged_by(egui::PointerButton::Primary) {
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
                painter.rect_stroke(
                    m_rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, Color32::from_rgb(56, 189, 248)),
                    egui::StrokeKind::Outside,
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
                for d in &map_devices {
                    if d.pos_x >= min_x && d.pos_x <= max_x && d.pos_y >= min_y && d.pos_y <= max_y {
                        canvas.selected_devices.insert(d.id);
                    }
                }
            }
        }
    }

    // Context Menu on right-click
    response.context_menu(|ui| {
        if let Some(&did) = canvas.selected_devices.iter().next() {
            if ui.button(tr(lang, TextKey::PanelDevices)).clicked() {
                canvas.open_device_dialog = Some(did);
                ui.close_menu();
            }
            if ui.button("Ping").clicked() {
                if let Some(d) = state.devices.get(&did) {
                    let _ = cmd_tx.try_send(ClientCommand::RunTool(v1::RunToolRequest {
                        kind: v1::ToolKind::Ping as i32,
                        target: d.address.clone(),
                        port: 0,
                        oid: String::new(),
                        device_id: Some(d.id),
                        count: 5,
                    }));
                }
                ui.close_menu();
            }
            if ui.button(tr(lang, TextKey::Delete)).clicked() {
                if let Some(d) = state.devices.get(&did) {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteDevice(v1::IdRequest {
                        id: d.id,
                        expected_version: d.version,
                    }));
                }
                ui.close_menu();
            }
        } else if let Some(&lid) = canvas.selected_links.iter().next() {
            if ui.button(tr(lang, TextKey::PanelLinks)).clicked() {
                canvas.open_link_dialog = Some(lid);
                ui.close_menu();
            }
            if ui.button(tr(lang, TextKey::Delete)).clicked() {
                if let Some(l) = state.links.get(&lid) {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteLink(v1::IdRequest {
                        id: l.id,
                        expected_version: l.version,
                    }));
                }
                ui.close_menu();
            }
        }
    });

    // Map Settings modal
    if canvas.show_map_settings {
        let mut edit_map = active_map.clone();
        let mut save_clicked = false;
        let mut close_clicked = false;
        egui::Window::new(tr(lang, TextKey::MapSettings))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::Name));
                    ui.text_edit_singleline(&mut edit_map.name);
                });
                let mut poll_ov = edit_map.poll_interval_sec_override.unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.label("Poll Override (0=global)");
                    ui.add(egui::DragValue::new(&mut poll_ov).range(0..=3600));
                });
                edit_map.poll_interval_sec_override = if poll_ov == 0 { None } else { Some(poll_ov) };

                let mut lbl_ref = edit_map.label_refresh_sec;
                ui.horizontal(|ui| {
                    ui.label("Label Refresh (s)");
                    ui.add(egui::DragValue::new(&mut lbl_ref).range(1..=300));
                });
                edit_map.label_refresh_sec = lbl_ref;

                ui.horizontal(|ui| {
                    ui.label("Default Link Thickness");
                    ui.add(egui::DragValue::new(&mut edit_map.default_link_thickness).range(0.5..=10.0));
                });

                egui::ComboBox::from_label("Default Link Style")
                    .selected_text(format!("{:?}", v1::LinkStyle::try_from(edit_map.default_link_style).unwrap_or(v1::LinkStyle::Curved)))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut edit_map.default_link_style, v1::LinkStyle::Straight as i32, tr(lang, TextKey::LinkStyleStraight));
                        ui.selectable_value(&mut edit_map.default_link_style, v1::LinkStyle::Curved as i32, tr(lang, TextKey::LinkStyleCurved));
                        ui.selectable_value(&mut edit_map.default_link_style, v1::LinkStyle::OrthogonalRounded as i32, tr(lang, TextKey::LinkStyleOrthogonal));
                    });

                ui.checkbox(&mut edit_map.background_tile, "Tile Background Image");
                ui.horizontal(|ui| {
                    ui.label("Background Scale");
                    ui.add(egui::DragValue::new(&mut edit_map.background_scale).speed(0.05).range(0.1..=5.0));
                });

                ui.horizontal(|ui| {
                    if ui.button(tr(lang, TextKey::Save)).clicked() {
                        save_clicked = true;
                    }
                    if ui.button(tr(lang, TextKey::Close)).clicked() {
                        close_clicked = true;
                    }
                });
            });
        if save_clicked {
            let _ = cmd_tx.try_send(ClientCommand::SaveMap(edit_map));
            canvas.show_map_settings = false;
        } else if close_clicked {
            canvas.show_map_settings = false;
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
    painter.add(egui::Shape::convex_polygon(vec![tip, p1, p2], color, Stroke::NONE));
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
    let devs: Vec<&v1::Device> = state.devices.values().filter(|d| d.map_id == map_id).collect();
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
        ThemeMode::Dark => image::Rgba([15, 23, 42, 255]),
        ThemeMode::Light => image::Rgba([241, 245, 249, 255]),
    };
    let mut img = image::RgbaImage::from_pixel(w, h, bg);
    let dev_map: HashMap<i64, &v1::Device> = devs.iter().map(|d| (d.id, *d)).collect();

    for l in state.links.values().filter(|l| l.map_id == map_id) {
        if let (Some(s), Some(d)) = (dev_map.get(&l.src_device_id), dev_map.get(&l.dst_device_id)) {
            let c = utilization_color(theme, l.in_utilization_pct.max(l.out_utilization_pct));
            draw_line_rgba(
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
        let cx = (d.pos_x - min_x) as i32;
        let cy = (d.pos_y - min_y) as i32;
        for dy in -18..=18 {
            for dx in -18..=18 {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    let edge = dx.abs() >= 15 || dy.abs() >= 15;
                    let col = if edge {
                        image::Rgba([c.r, c.g, c.b, 255])
                    } else {
                        image::Rgba([30, 41, 59, 255])
                    };
                    img.put_pixel(px as u32, py as u32, col);
                }
            }
        }
    }

    img.save(path).map_err(|e| format!("PNG save failed: {e}"))
}

fn draw_line_rgba(
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
        if x0 >= 0 && y0 >= 0 && (x0 as u32) < w && (y0 as u32) < h {
            img.put_pixel(x0 as u32, y0 as u32, color);
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
