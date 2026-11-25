use crate::error::ServerError;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat, RgbaImage};
use nextdude_common::proto::v1;
use nextdude_common::types::ImageSizeVariant;
use nextdude_common::validation::validate_https_url;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedImageFormat {
    Png,
    Jpeg,
    Webp,
    Svg,
}

impl DetectedImageFormat {
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Svg => "image/svg+xml",
        }
    }
}

/// Built-in original geometric SVG device icons (zero third-party trademarks).
pub fn builtin_svg_by_key(key: &str) -> Option<&'static str> {
    match key {
        "router" => Some(include_str!("../../../assets/icons/router.svg")),
        "switch" => Some(include_str!("../../../assets/icons/switch.svg")),
        "crs" => Some(include_str!("../../../assets/icons/crs.svg")),
        "olt" => Some(include_str!("../../../assets/icons/olt.svg")),
        "onu" => Some(include_str!("../../../assets/icons/onu.svg")),
        "ap" => Some(include_str!("../../../assets/icons/ap.svg")),
        "radio" => Some(include_str!("../../../assets/icons/radio.svg")),
        "server" => Some(include_str!("../../../assets/icons/server.svg")),
        "firewall" => Some(include_str!("../../../assets/icons/firewall.svg")),
        "generic" => Some(include_str!("../../../assets/icons/generic.svg")),
        _ => None,
    }
}

pub fn detect_magic_format(data: &[u8]) -> Result<DetectedImageFormat, ServerError> {
    if data.len() >= 8 && data[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return Ok(DetectedImageFormat::Png);
    }
    if data.len() >= 3 && data[..3] == [0xFF, 0xD8, 0xFF] {
        return Ok(DetectedImageFormat::Jpeg);
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Ok(DetectedImageFormat::Webp);
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let trimmed = text.trim_start_matches('\u{feff}').trim_start();
        if trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && trimmed.contains("<svg")) {
            return Ok(DetectedImageFormat::Svg);
        }
    }
    Err(ServerError::Image(
        "unsupported or spoofed image format: expected PNG, JPEG, WebP, or SVG magic bytes"
            .to_string(),
    ))
}

/// Validates and rejects dangerous SVG constructs (scripts, event handlers, external references).
pub fn validate_safe_svg(svg_bytes: &[u8]) -> Result<(), ServerError> {
    let text = std::str::from_utf8(svg_bytes)
        .map_err(|_| ServerError::Image("SVG must be valid UTF-8".to_string()))?;
    let lower = text.to_ascii_lowercase();
    if lower.contains("<!entity") || lower.contains("<!doctype") {
        return Err(ServerError::Image("SVG DOCTYPE/ENTITY declarations are forbidden".to_string()));
    }
    let doc = roxmltree::Document::parse(text)
        .map_err(|e| ServerError::Image(format!("invalid SVG XML: {e}")))?;
    let root = doc.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case("svg") {
        return Err(ServerError::Image("root XML element must be <svg>".to_string()));
    }

    for node in doc.descendants() {
        if !node.is_element() {
            continue;
        }
        let tag = node.tag_name().name().to_ascii_lowercase();
        if matches!(
            tag.as_str(),
            "script" | "foreignobject" | "iframe" | "object" | "embed" | "audio" | "video"
        ) {
            return Err(ServerError::Image(format!("forbidden SVG element <{tag}>")));
        }
        for attr in node.attributes() {
            let name = attr.name().to_ascii_lowercase();
            let val = attr.value().trim().to_ascii_lowercase();
            if name.starts_with("on") {
                return Err(ServerError::Image(format!(
                    "forbidden SVG event handler attribute '{name}'"
                )));
            }
            if val.contains("javascript:") || val.contains("vbscript:") {
                return Err(ServerError::Image("forbidden script URI in SVG".to_string()));
            }
            if (name == "href" || name == "xlink:href" || name == "src")
                && !val.is_empty()
                && !val.starts_with('#')
            {
                return Err(ServerError::Image(
                    "external references in SVG href/src are forbidden".to_string(),
                ));
            }
            if name == "style" && (val.contains("url(") || val.contains("expression(")) {
                return Err(ServerError::Image(
                    "external url() or expression() in SVG style is forbidden".to_string(),
                ));
            }
        }
    }
    Ok(())
}

pub struct ProcessedImage {
    pub format: DetectedImageFormat,
    pub width: u32,
    pub height: u32,
    pub original_bytes: Vec<u8>,
    pub variants_png: Vec<(ImageSizeVariant, Vec<u8>)>,
}

pub fn validate_and_process_image(
    raw: &[u8],
    max_bytes: usize,
    max_dim: u32,
) -> Result<ProcessedImage, ServerError> {
    if raw.is_empty() {
        return Err(ServerError::Image("image payload is empty".to_string()));
    }
    if raw.len() > max_bytes {
        return Err(ServerError::Image(format!(
            "image size {} bytes exceeds limit {} bytes",
            raw.len(),
            max_bytes
        )));
    }

    let format = detect_magic_format(raw)?;
    let (width, height, base_rgba) = match format {
        DetectedImageFormat::Svg => {
            validate_safe_svg(raw)?;
            let tree = Tree::from_data(raw, &Options::default())
                .map_err(|e| ServerError::Image(format!("SVG parse failed: {e}")))?;
            let sz = tree.size().to_int_size();
            let w = sz.width().clamp(16, max_dim);
            let h = sz.height().clamp(16, max_dim);
            let mut pixmap = Pixmap::new(256, 256)
                .ok_or_else(|| ServerError::Image("failed allocating SVG raster buffer".to_string()))?;
            let sx = 256.0 / (sz.width().max(1) as f32);
            let sy = 256.0 / (sz.height().max(1) as f32);
            let scale = sx.min(sy);
            resvg::render(&tree, Transform::from_scale(scale, scale), &mut pixmap.as_mut());
            let rgba = RgbaImage::from_raw(256, 256, pixmap.take())
                .ok_or_else(|| ServerError::Image("invalid raster RGBA buffer".to_string()))?;
            (w, h, DynamicImage::ImageRgba8(rgba))
        }
        DetectedImageFormat::Png | DetectedImageFormat::Jpeg | DetectedImageFormat::Webp => {
            let img_fmt = match format {
                DetectedImageFormat::Png => ImageFormat::Png,
                DetectedImageFormat::Jpeg => ImageFormat::Jpeg,
                _ => ImageFormat::WebP,
            };
            let decoded = image::load_from_memory_with_format(raw, img_fmt)
                .map_err(|e| ServerError::Image(format!("corrupt or invalid image data: {e}")))?;
            let (w, h) = decoded.dimensions();
            if w == 0 || h == 0 || w > max_dim || h > max_dim {
                return Err(ServerError::Image(format!(
                    "image dimensions {}x{} outside allowed bounds 1..={}",
                    w, h, max_dim
                )));
            }
            (w, h, decoded)
        }
    };

    let mut variants_png = Vec::with_capacity(4);
    for variant in [
        ImageSizeVariant::Thumb,
        ImageSizeVariant::Px64,
        ImageSizeVariant::Px128,
        ImageSizeVariant::Px256,
    ] {
        if let Some(px) = variant.target_pixels() {
            let resized = base_rgba.resize(px, px, FilterType::Lanczos3);
            let mut buf = Vec::new();
            resized
                .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
                .map_err(|e| ServerError::Image(format!("PNG encode failed: {e}")))?;
            variants_png.push((variant, buf));
        }
    }

    Ok(ProcessedImage {
        format,
        width,
        height,
        original_bytes: raw.to_vec(),
        variants_png,
    })
}

pub fn store_processed_image(
    images_dir: &Path,
    storage_key: &str,
    processed: &ProcessedImage,
) -> Result<(), ServerError> {
    let dir = images_dir.join(storage_key);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("original"), &processed.original_bytes)?;
    for (variant, data) in &processed.variants_png {
        fs::write(dir.join(format!("{}.png", variant.as_str())), data)?;
    }
    Ok(())
}

pub fn load_stored_image_blob(
    images_dir: &Path,
    storage_key: &str,
    mime_type: &str,
    variant: ImageSizeVariant,
) -> Result<(String, Vec<u8>), ServerError> {
    let dir = images_dir.join(storage_key);
    match variant {
        ImageSizeVariant::Original => {
            let data = fs::read(dir.join("original"))
                .map_err(|e| ServerError::NotFound(format!("image blob missing: {e}")))?;
            Ok((mime_type.to_string(), data))
        }
        v => {
            let data = fs::read(dir.join(format!("{}.png", v.as_str())))
                .map_err(|e| ServerError::NotFound(format!("image variant missing: {e}")))?;
            Ok(("image/png".to_string(), data))
        }
    }
}

pub async fn download_image_https(
    url: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, ServerError> {
    let clean = validate_https_url(url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| ServerError::Image(format!("HTTP client error: {e}")))?;

    let resp = client
        .get(&clean)
        .send()
        .await
        .map_err(|e| ServerError::Image(format!("image download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(ServerError::Image(format!(
            "remote server returned HTTP {}",
            resp.status()
        )));
    }
    if let Some(len) = resp.content_length() {
        if len as usize > max_bytes {
            return Err(ServerError::Image(format!(
                "remote Content-Length {len} exceeds max {max_bytes} bytes"
            )));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ServerError::Image(format!("image read failed: {e}")))?;
    if bytes.len() > max_bytes {
        return Err(ServerError::Image(format!(
            "downloaded image {} bytes exceeds max {max_bytes}",
            bytes.len()
        )));
    }
    Ok(bytes.to_vec())
}

pub fn delete_stored_image_files(images_dir: &Path, storage_key: &str) {
    let _ = fs::remove_dir_all(images_dir.join(storage_key));
}

pub fn build_image_meta(
    req_name: &str,
    vendor: &str,
    model: &str,
    tags: Vec<String>,
    processed: &ProcessedImage,
) -> v1::ImageAssetMeta {
    v1::ImageAssetMeta {
        id: 0,
        version: 1,
        name: req_name.to_string(),
        mime_type: processed.format.mime_type().to_string(),
        vendor: vendor.to_string(),
        model: model.to_string(),
        tags,
        width: processed.width,
        height: processed.height,
        byte_size: processed.original_bytes.len() as u64,
        builtin: false,
    }
}
