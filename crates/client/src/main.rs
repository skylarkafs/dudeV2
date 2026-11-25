#![deny(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod map_canvas;
mod panels;
#[allow(unsafe_code)]
mod platform;

use eframe::egui;
use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use eframe::wgpu;
use nextdude_client_core::config::ClientLocalConfig;
use nextdude_common::APP_NAME;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    platform::register_app_user_model_id();

    let _instance_guard = match platform::acquire_single_instance_mutex() {
        Ok(g) => g,
        Err(msg) => {
            platform::show_windows_toast(APP_NAME, &msg);
            return Ok(());
        }
    };

    let logs_dir = ClientLocalConfig::logs_dir();
    std::fs::create_dir_all(&logs_dir)?;
    let appender = tracing_appender::rolling::daily(&logs_dir, "nextdude-client.log");
    let (non_blocking, _log_guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().with_writer(non_blocking).with_ansi(false))
        .init();

    info!("Starting {} Windows Client", APP_NAME);

    let cfg_path = ClientLocalConfig::config_file_path();
    let local_cfg = ClientLocalConfig::load_from(&cfg_path).unwrap_or_default();
    platform::set_prevent_sleep(local_cfg.prevent_sleep);

    let icon_rgba = build_app_icon_rgba();
    let icon_data = Arc::new(egui::IconData {
        rgba: icon_rgba.clone(),
        width: 32,
        height: 32,
    });

    // Attempt DX12/GL with graceful software fallback adapter (supports RDP and GPU-less PCs).
    let primary_opts = build_native_options(Arc::clone(&icon_data), false);
    match eframe::run_native(
        APP_NAME,
        primary_opts,
        Box::new(|cc| Ok(Box::new(app::NextDudeApp::new(cc, local_cfg.clone(), icon_rgba)))),
    ) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            tracing::warn!("Primary GPU adapter failed ({first_err}); retrying with software fallback adapter");
            let fallback_cfg = ClientLocalConfig::load_from(&cfg_path).unwrap_or_default();
            let fallback_rgba = build_app_icon_rgba();
            let fallback_opts = build_native_options(icon_data, true);
            eframe::run_native(
                APP_NAME,
                fallback_opts,
                Box::new(|cc| {
                    Ok(Box::new(app::NextDudeApp::new(
                        cc,
                        fallback_cfg,
                        fallback_rgba,
                    )))
                }),
            )
            .map_err(|e| anyhow::anyhow!("eframe failed on fallback adapter: {e}"))
        }
    }
}

fn build_native_options(
    icon: Arc<egui::IconData>,
    force_software_fallback: bool,
) -> eframe::NativeOptions {
    let wgpu_setup = WgpuSetup::CreateNew(WgpuSetupCreateNew {
        instance_descriptor: wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12 | wgpu::Backends::GL,
            ..Default::default()
        },
        power_preference: if force_software_fallback {
            wgpu::PowerPreference::LowPower
        } else {
            wgpu::PowerPreference::HighPerformance
        },
        force_fallback_adapter: force_software_fallback,
        native_adapter_selector: if force_software_fallback {
            None
        } else {
            Some(Arc::new(|adapters, surface| {
                let compatible = |a: &wgpu::Adapter| {
                    surface.map_or(true, |s| a.is_surface_supported(s))
                };
                adapters
                    .iter()
                    .find(|a| {
                        compatible(a) && a.get_info().device_type != wgpu::DeviceType::Cpu
                    })
                    .or_else(|| adapters.iter().find(|a| compatible(a)))
                    .cloned()
            }))
        },
        device_descriptor: Arc::new(|_adapter| wgpu::DeviceDescriptor {
            label: Some("nextdude-wgpu-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
        }),
        ..Default::default()
    });

    eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: WgpuConfiguration {
            wgpu_setup,
            ..Default::default()
        },
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([960.0, 620.0])
            .with_icon(icon),
        ..Default::default()
    }
}

fn build_app_icon_rgba() -> Vec<u8> {
    let mut rgba = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32usize {
        for x in 0..32usize {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let r = (dx * dx + dy * dy).sqrt();
            let px = if r <= 4.5 {
                [34u8, 197u8, 94u8, 255u8]
            } else if r <= 11.0 {
                [30u8, 41u8, 59u8, 255u8]
            } else if r <= 14.5 {
                [56u8, 189u8, 248u8, 255u8]
            } else {
                [0u8, 0u8, 0u8, 0u8]
            };
            rgba.extend_from_slice(&px);
        }
    }
    rgba
}
