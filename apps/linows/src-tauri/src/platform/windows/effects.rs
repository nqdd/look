//! Windows-specific window effects (Mica, Acrylic, Win11 rounded corners).

use tauri::utils::config::WindowEffectsConfig;
use tauri::window::Effect;
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};

pub(crate) fn apply(window: tauri::Window, effect: &str) -> Result<(), String> {
    let config: Option<WindowEffectsConfig> = match effect {
        "mica" => Some(WindowEffectsConfig {
            effects: vec![Effect::Mica],
            ..Default::default()
        }),
        "acrylic" => Some(WindowEffectsConfig {
            effects: vec![Effect::Acrylic],
            ..Default::default()
        }),
        "none" | "" => None,
        _ => return Err(format!("Unknown effect: {effect}")),
    };

    window
        .set_effects(config)
        .map_err(|e| format!("Failed to set effect: {e}"))
}

/// Ask DWM to round the window corners (Win11 only — Win10 silently ignores
/// the attribute). DWM does anti-aliased corner rendering at the compositor
/// level, which is the only path to smooth corners on Windows; GDI region
/// clipping gives aliased staircase edges.
///
/// The real fix for the "sharp rectangle behind rounded content" bug is
/// upstream of this call: the WebView2 default background must be set to
/// fully transparent (see `main.rs` Windows block). Without that, WebView2
/// paints opaque pixels in the corner triangles that DWM's rounded clip
/// can't hide.
pub(crate) fn apply_round_corners(window: &tauri::WebviewWindow) -> Result<(), String> {
    let hwnd = window
        .hwnd()
        .map_err(|e| format!("Failed to get HWND: {e}"))?;
    let pref = DWMWCP_ROUND;
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        )
    };
    result.map_err(|e| format!("DwmSetWindowAttribute: {e}"))
}
