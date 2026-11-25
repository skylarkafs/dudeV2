use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    if env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap_or_else(|_| ".".to_string()));
        let ico_path = out_dir.join("nextdude.ico");
        if write_minimal_ico(&ico_path).is_ok() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon(&ico_path.to_string_lossy());
            res.set_manifest_file("app.manifest");
            res.set("ProductName", "NextDude");
            res.set("FileDescription", "NextDude Network Monitoring Desktop Client");
            res.set("CompanyName", "NextDude Contributors");
            if let Err(e) = res.compile() {
                println!("cargo:warning=winresource compile skipped or failed: {e}");
            }
        }
    }
}

/// Generates a deterministic 32x32 32-bit BGRA Windows ICO file so no binary asset is required in git.
fn write_minimal_ico(path: &PathBuf) -> std::io::Result<()> {
    let w = 32usize;
    let h = 32usize;
    let xor_size = w * h * 4;
    let and_size = (w / 8) * h;
    let dib_header_size = 40usize;
    let image_size = dib_header_size + xor_size + and_size;

    let mut buf = Vec::with_capacity(6 + 16 + image_size);
    // ICONDIR
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // type = icon
    buf.extend_from_slice(&1u16.to_le_bytes()); // count = 1
    // ICONDIRENTRY
    buf.push(w as u8);
    buf.push(h as u8);
    buf.push(0); // color count
    buf.push(0); // reserved
    buf.extend_from_slice(&1u16.to_le_bytes()); // planes
    buf.extend_from_slice(&32u16.to_le_bytes()); // bpp
    buf.extend_from_slice(&(image_size as u32).to_le_bytes());
    buf.extend_from_slice(&22u32.to_le_bytes()); // image offset = 6 + 16

    // BITMAPINFOHEADER
    buf.extend_from_slice(&(dib_header_size as u32).to_le_bytes());
    buf.extend_from_slice(&(w as i32).to_le_bytes());
    buf.extend_from_slice(&((h * 2) as i32).to_le_bytes()); // height * 2 for XOR+AND
    buf.extend_from_slice(&1u16.to_le_bytes()); // planes
    buf.extend_from_slice(&32u16.to_le_bytes()); // bpp
    buf.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    buf.extend_from_slice(&((xor_size + and_size) as u32).to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());

    // Bottom-up 32x32 BGRA pixels: dark slate circle with cyan/green node ring
    for row_inv in 0..h {
        let y = h - 1 - row_inv;
        for x in 0..w {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let r = (dx * dx + dy * dy).sqrt();
            let (b, g, red, a) = if r <= 4.5 {
                (94u8, 197u8, 34u8, 255u8) // #22C55E green core
            } else if r <= 11.0 {
                (59u8, 41u8, 30u8, 255u8) // #1E293B dark slate
            } else if r <= 14.5 {
                (248u8, 189u8, 56u8, 255u8) // #38BDF8 cyan ring
            } else {
                (0u8, 0u8, 0u8, 0u8)
            };
            buf.extend_from_slice(&[b, g, red, a]);
        }
    }
    // AND mask (all zero = alpha channel governs transparency)
    buf.resize(buf.len() + and_size, 0);
    fs::write(path, buf)
}
