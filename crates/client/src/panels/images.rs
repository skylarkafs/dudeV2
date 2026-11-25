use eframe::egui;
use image::{ImageFormat, RgbaImage};
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_common::proto::v1;
use std::collections::HashMap;
use std::io::Cursor;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct BulkUploadCandidate {
    pub name: String,
    pub vendor: String,
    pub model: String,
    pub raw_data: Vec<u8>,
}

#[derive(Default)]
pub struct ImagesPanelState {
    pub import_url: String,
    pub import_name: String,
    pub import_vendor: String,
    pub import_model: String,
    pub bulk_candidates: Vec<BulkUploadCandidate>,
    pub status_msg: String,
}

pub fn guess_vendor_model_from_filename(filename: &str) -> (String, String, String) {
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    let lower = stem.to_ascii_lowercase();
    let known_vendors = [
        ("mikrotik", "MikroTik"),
        ("cisco", "Cisco"),
        ("juniper", "Juniper"),
        ("huawei", "Huawei"),
        ("zte", "ZTE"),
        ("ruijie", "Ruijie"),
        ("alcatel", "Alcatel-Lucent"),
        ("ubiquiti", "Ubiquiti"),
        ("ubnt", "Ubiquiti"),
    ];
    let vendor = known_vendors
        .iter()
        .find(|(k, _)| lower.contains(k))
        .map(|(_, v)| (*v).to_string())
        .unwrap_or_default();
    let model = stem
        .split(['_', '-'])
        .skip(if vendor.is_empty() { 0 } else { 1 })
        .collect::<Vec<_>>()
        .join(" ");
    (stem.to_string(), vendor, model)
}

#[allow(clippy::too_many_arguments)]
pub fn render_images_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    state: &ClientStateCache,
    img_state: &mut ImagesPanelState,
    textures: &HashMap<String, egui::TextureHandle>,
    cmd_tx: &mpsc::Sender<ClientCommand>,
    request_image_cb: &mut dyn FnMut(i64, &str),
) {
    ui.heading(tr(lang, TextKey::PanelFiles));
    if !img_state.status_msg.is_empty() {
        ui.label(&img_state.status_msg);
    }

    if state.images.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(56, 189, 248),
            tr(lang, TextKey::EmptyVendorLibraryHint),
        );
    }

    // Handle drag-and-drop image files onto the window
    let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .filter_map(|f| f.path.clone())
            .collect()
    });
    for p in dropped {
        if let Ok(bytes) = std::fs::read(&p) {
            let fname = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("dropped.png");
            let (name, vendor, model) = guess_vendor_model_from_filename(fname);
            img_state.bulk_candidates.push(BulkUploadCandidate {
                name,
                vendor,
                model,
                raw_data: bytes,
            });
        }
    }

    ui.horizontal_wrapped(|ui| {
        if ui.button(tr(lang, TextKey::UploadFile)).clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "svg"])
                .pick_file()
            {
                if let Ok(bytes) = std::fs::read(&path) {
                    let fname = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("upload.png");
                    let (name, vendor, model) = guess_vendor_model_from_filename(fname);
                    let _ = cmd_tx.try_send(ClientCommand::UploadImage(v1::UploadImageRequest {
                        name,
                        vendor,
                        model,
                        tags: Vec::new(),
                        raw_data: bytes,
                    }));
                }
            }
        }
        if ui.button(tr(lang, TextKey::BulkUpload)).clicked() {
            if let Some(paths) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "svg"])
                .pick_files()
            {
                for p in paths {
                    if let Ok(bytes) = std::fs::read(&p) {
                        let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("file");
                        let (name, vendor, model) = guess_vendor_model_from_filename(fname);
                        img_state.bulk_candidates.push(BulkUploadCandidate {
                            name,
                            vendor,
                            model,
                            raw_data: bytes,
                        });
                    }
                }
            }
        }
        if ui.button(tr(lang, TextKey::PasteClipboard)).clicked() {
            match arboard::Clipboard::new().and_then(|mut cb| cb.get_image()) {
                Ok(img) => {
                    if let Some(rgba) = RgbaImage::from_raw(
                        img.width as u32,
                        img.height as u32,
                        img.bytes.into_owned(),
                    ) {
                        let mut png_bytes = Vec::new();
                        if rgba
                            .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
                            .is_ok()
                        {
                            let _ = cmd_tx.try_send(ClientCommand::UploadImage(v1::UploadImageRequest {
                                name: format!("Clipboard-{}", chrono::Utc::now().timestamp()),
                                vendor: String::new(),
                                model: String::new(),
                                tags: vec!["clipboard".to_string()],
                                raw_data: png_bytes,
                            }));
                        }
                    }
                }
                Err(e) => {
                    img_state.status_msg = format!("Clipboard has no image: {e}");
                }
            }
        }
    });

    // HTTPS URL import (gated by server setting)
    let url_enabled = state
        .server_settings
        .as_ref()
        .map_or(false, |s| s.allow_image_url_import);
    ui.add_enabled_ui(url_enabled, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr(lang, TextKey::ImportFromUrl));
            ui.text_edit_singleline(&mut img_state.import_url);
            ui.label(tr(lang, TextKey::Name));
            ui.text_edit_singleline(&mut img_state.import_name);
            ui.label(tr(lang, TextKey::Vendor));
            ui.text_edit_singleline(&mut img_state.import_vendor);
            ui.label(tr(lang, TextKey::Model));
            ui.text_edit_singleline(&mut img_state.import_model);
            if ui.button(tr(lang, TextKey::ImportFromUrl)).clicked() {
                let _ = cmd_tx.try_send(ClientCommand::ImportImageFromUrl(v1::ImportImageUrlRequest {
                    url: img_state.import_url.clone(),
                    name: if img_state.import_name.is_empty() {
                        "Imported".to_string()
                    } else {
                        img_state.import_name.clone()
                    },
                    vendor: img_state.import_vendor.clone(),
                    model: img_state.import_model.clone(),
                    tags: Vec::new(),
                }));
            }
        });
    });

    // Bulk upload confirmation screen
    if !img_state.bulk_candidates.is_empty() {
        ui.separator();
        ui.strong("Confirm Bulk Upload Metadata");
        let mut do_upload_all = false;
        let mut clear_all = false;
        egui::Grid::new("bulk_grid").striped(true).num_columns(4).show(ui, |ui| {
            ui.strong(tr(lang, TextKey::Name));
            ui.strong(tr(lang, TextKey::Vendor));
            ui.strong(tr(lang, TextKey::Model));
            ui.strong("Bytes");
            ui.end_row();
            for cand in &mut img_state.bulk_candidates {
                ui.text_edit_singleline(&mut cand.name);
                ui.text_edit_singleline(&mut cand.vendor);
                ui.text_edit_singleline(&mut cand.model);
                ui.label(cand.raw_data.len().to_string());
                ui.end_row();
            }
        });
        ui.horizontal(|ui| {
            if ui.button(tr(lang, TextKey::Apply)).clicked() {
                do_upload_all = true;
            }
            if ui.button(tr(lang, TextKey::Cancel)).clicked() {
                clear_all = true;
            }
        });
        if do_upload_all {
            for cand in img_state.bulk_candidates.drain(..) {
                let _ = cmd_tx.try_send(ClientCommand::UploadImage(v1::UploadImageRequest {
                    name: cand.name,
                    vendor: cand.vendor,
                    model: cand.model,
                    tags: Vec::new(),
                    raw_data: cand.raw_data,
                }));
            }
        } else if clear_all {
            img_state.bulk_candidates.clear();
        }
    }

    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("images_library_grid").striped(true).num_columns(6).show(ui, |ui| {
            ui.strong("Preview");
            ui.strong(tr(lang, TextKey::Name));
            ui.strong(tr(lang, TextKey::Vendor));
            ui.strong(tr(lang, TextKey::Model));
            ui.strong("Size");
            ui.strong(tr(lang, TextKey::Action));
            ui.end_row();
            for im in state.images.values() {
                let key = format!("img:{}:thumb", im.id);
                if let Some(tex) = textures.get(&key) {
                    ui.image((tex.id(), egui::Vec2::splat(32.0)));
                } else {
                    request_image_cb(im.id, "thumb");
                    ui.label("...");
                }
                ui.label(&im.name);
                ui.label(&im.vendor);
                ui.label(&im.model);
                ui.label(format!("{}x{} ({} B)", im.width, im.height, im.byte_size));
                if ui.button(tr(lang, TextKey::Delete)).clicked() {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteImage(v1::IdRequest {
                        id: im.id,
                        expected_version: im.version,
                    }));
                }
                ui.end_row();
            }
        });
    });
}
