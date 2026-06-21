use openaction::*;
use std::{collections::HashMap, process::exit, sync::{Arc, LazyLock}};
use std::path::PathBuf;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[cfg(not(target_os = "windows"))]
use tokio::signal::unix::{SignalKind, signal};

mod device;
mod mappings;
mod transport;
mod watcher;

use device::{Device, handle_error, handle_set_image};
use transport::TransportLib;
use watcher::watcher_task;

pub static DEVICES: LazyLock<RwLock<HashMap<String, Arc<Device>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
pub static TOKENS: LazyLock<RwLock<HashMap<String, CancellationToken>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
pub static FLUSH_NOTIFY: LazyLock<RwLock<HashMap<String, Arc<Notify>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
pub static TRACKER: LazyLock<Mutex<TaskTracker>> = LazyLock::new(|| Mutex::new(TaskTracker::new()));

struct GlobalEventHandler {
    lib: Arc<TransportLib>,
}

impl openaction::GlobalEventHandler for GlobalEventHandler {
    async fn plugin_ready(
        &self,
        _outbound: &mut openaction::OutboundEventManager,
    ) -> EventHandlerResult {
        let tracker = TRACKER.lock().await.clone();

        let token = CancellationToken::new();
        tracker.spawn(watcher_task(self.lib.clone(), token.clone()));

        TOKENS
            .write()
            .await
            .insert("_watcher_task".to_string(), token);

        log::info!("Plugin initialized and watcher task spawned");

        Ok(())
    }

    async fn set_image(
        &self,
        event: SetImageEvent,
        _outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        if log::log_enabled!(log::Level::Debug) {
            let img_desc = match &event.image {
                Some(img) => {
                    if img.len() > 50 {
                        format!("Some(\"{}... (len: {})\")", &img[..50], img.len())
                    } else {
                        format!("Some({:?})", img)
                    }
                }
                None => "None".to_string(),
            };
            log::debug!(
                "SetImageEvent {{ device: {:?}, controller: {:?}, position: {:?}, image: {} }}",
                event.device,
                event.controller,
                event.position,
                img_desc
            );
        }

        if event.controller == Some("Encoder".to_string()) {
            log::debug!("Skipping image set for encoder");
            return Ok(());
        }

        let id = event.device.clone();

        if let Some(device) = DEVICES.read().await.get(&event.device) {
            if let Err(err) = handle_set_image(device, &id, event).await {
                log::error!("Error setting image for {}: {:?}", id, err);
                handle_error(&id).await;
            }
        } else {
            log::error!("Received event for unknown device: {}", event.device);
        }

        Ok(())
    }

    async fn set_brightness(
        &self,
        event: SetBrightnessEvent,
        _outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        log::debug!("SetBrightnessEvent: {:?}", event);

        let id = event.device.clone();

        if let Some(device) = DEVICES.read().await.get(&event.device) {
            if let Err(err) = device.set_brightness(event.brightness).await {
                log::error!("Error setting brightness for {}: {:#08x}", id, err);
                handle_error(&id).await;
            }
        } else {
            log::error!("Received event for unknown device: {}", event.device);
        }

        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct BacklightSettings {
    #[serde(default = "default_mode")]
    mode: String,
    
    #[serde(default = "default_brightness")]
    brightness_value: u8,
    
    #[serde(default = "default_color")]
    color_value: String, // hex string, e.g. "#0096ff"
    
    #[serde(default = "default_effect")]
    effect_value: u8,
    
    #[serde(default = "default_speed")]
    speed_value: u8,

    #[serde(default = "default_press_action")]
    press_action: String, // "toggle", "set", "cycle"
}

fn default_mode() -> String { "brightness".to_string() }
fn default_brightness() -> u8 { 4 }
fn default_color() -> String { "#0096ff".to_string() }
fn default_effect() -> u8 { 0 }
fn default_speed() -> u8 { 3 }
fn default_press_action() -> String { "toggle".to_string() }

fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

const COLOR_PRESETS: &[(u8, u8, u8)] = &[
    (255, 0, 0),     // Red
    (255, 127, 0),   // Orange
    (255, 255, 0),   // Yellow
    (0, 255, 0),     // Green
    (0, 150, 255),   // Cyan
    (0, 0, 255),     // Blue
    (127, 0, 255),   // Purple
    (255, 0, 255),   // Magenta
    (255, 255, 255), // White
];

fn find_closest_color_preset(color: (u8, u8, u8)) -> usize {
    let mut closest_idx = 0;
    let mut min_diff = u32::MAX;
    for (idx, &(r, g, b)) in COLOR_PRESETS.iter().enumerate() {
        let diff = ((r as i32 - color.0 as i32).pow(2)
            + (g as i32 - color.1 as i32).pow(2)
            + (b as i32 - color.2 as i32).pow(2)) as u32;
        if diff < min_diff {
            min_diff = diff;
            closest_idx = idx;
        }
    }
    closest_idx
}

async fn sync_settings_to_host(
    device: &Device,
    settings: &BacklightSettings,
    context: String,
    outbound: &mut OutboundEventManager,
) -> Result<(), anyhow::Error> {
    let brightness = *device.backlight_brightness.lock().await;
    let speed = *device.backlight_speed.lock().await;
    let effect = *device.backlight_effect.lock().await;
    let (r, g, b) = *device.backlight_color.lock().await;
    let color_hex = format!("#{:02x}{:02x}{:02x}", r, g, b);

    let mut updated = settings.clone();
    updated.brightness_value = brightness;
    updated.speed_value = speed;
    updated.effect_value = effect;
    updated.color_value = color_hex;

    outbound.set_settings(context, serde_json::to_value(&updated)?)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to set settings: {:?}", e))?;
    Ok(())
}

async fn apply_backlight_settings(device: &Device, settings: &BacklightSettings) -> Result<(), anyhow::Error> {
    {
        let mut curr_bright = device.backlight_brightness.lock().await;
        *curr_bright = settings.brightness_value;
        if settings.brightness_value > 0 {
            let mut last_non_zero = device.backlight_last_non_zero_brightness.lock().await;
            *last_non_zero = settings.brightness_value;
        }
    }
    {
        let mut curr_speed = device.backlight_speed.lock().await;
        *curr_speed = settings.speed_value;
    }
    {
        let mut curr_eff = device.backlight_effect.lock().await;
        *curr_eff = settings.effect_value;
    }
    if let Some((r, g, b)) = hex_to_rgb(&settings.color_value) {
        let mut curr_color = device.backlight_color.lock().await;
        *curr_color = (r, g, b);
    }
    
    device.apply_all_backlight_settings().await
        .map_err(|e| anyhow::anyhow!("Failed to apply backlight settings: {:#08x}", e))?;
    Ok(())
}

async fn handle_press(device: &Device, settings: &BacklightSettings) -> Result<(), anyhow::Error> {
    match settings.press_action.as_str() {
        "toggle" => {
            device.toggle_keyboard_backlight().await
                .map_err(|e| anyhow::anyhow!("Failed to toggle backlight: {:#08x}", e))?;
        }
        "set" => {
            apply_backlight_settings(device, settings).await?;
        }
        "cycle" => {
            match settings.mode.as_str() {
                "brightness" => {
                    let current = *device.backlight_brightness.lock().await;
                    let next = (current + 1) % 7; // 0 to 6
                    device.set_keyboard_backlight_brightness(next).await
                        .map_err(|e| anyhow::anyhow!("Failed to cycle brightness: {:#08x}", e))?;
                }
                "color" => {
                    let current = *device.backlight_color.lock().await;
                    let current_idx = find_closest_color_preset(current);
                    let next_idx = (current_idx + 1) % COLOR_PRESETS.len();
                    let (r, g, b) = COLOR_PRESETS[next_idx];
                    device.set_keyboard_rgb_backlight(r, g, b).await
                        .map_err(|e| anyhow::anyhow!("Failed to cycle color: {:#08x}", e))?;
                }
                "effect" => {
                    let current = *device.backlight_effect.lock().await;
                    let next = (current + 1) % 10; // 0 to 9
                    device.set_keyboard_lighting_effects(next).await
                        .map_err(|e| anyhow::anyhow!("Failed to cycle effect: {:#08x}", e))?;
                }
                "speed" => {
                    let current = *device.backlight_speed.lock().await;
                    let next = (current + 1) % 8; // 0 to 7
                    device.set_keyboard_lighting_speed(next).await
                        .map_err(|e| anyhow::anyhow!("Failed to cycle speed: {:#08x}", e))?;
                }
                _ => {}
            }
        }
        _ => {}
    }
    Ok(())
}

async fn handle_dial_rotate(device: &Device, settings: &BacklightSettings, ticks: i16) -> Result<(), anyhow::Error> {
    match settings.mode.as_str() {
        "brightness" => {
            let current = *device.backlight_brightness.lock().await;
            let target = (current as i16 + ticks).clamp(0, 6) as u8;
            device.set_keyboard_backlight_brightness(target).await
                .map_err(|e| anyhow::anyhow!("Failed to set brightness: {:#08x}", e))?;
        }
        "color" => {
            let current = *device.backlight_color.lock().await;
            let current_idx = find_closest_color_preset(current);
            let target_idx = (current_idx as i16 + ticks).rem_euclid(COLOR_PRESETS.len() as i16) as usize;
            let (r, g, b) = COLOR_PRESETS[target_idx];
            device.set_keyboard_rgb_backlight(r, g, b).await
                .map_err(|e| anyhow::anyhow!("Failed to set color: {:#08x}", e))?;
        }
        "effect" => {
            let current = *device.backlight_effect.lock().await;
            let target = (current as i16 + ticks).rem_euclid(10) as u8;
            device.set_keyboard_lighting_effects(target).await
                .map_err(|e| anyhow::anyhow!("Failed to set effect: {:#08x}", e))?;
        }
        "speed" => {
            let current = *device.backlight_speed.lock().await;
            let target = (current as i16 + ticks).clamp(0, 7) as u8;
            device.set_keyboard_lighting_speed(target).await
                .map_err(|e| anyhow::anyhow!("Failed to set speed: {:#08x}", e))?;
        }
        _ => {}
    }
    Ok(())
}

struct ActionEventHandler {}

impl openaction::ActionEventHandler for ActionEventHandler {
    async fn did_receive_settings(
        &self,
        event: DidReceiveSettingsEvent,
        _outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        log::debug!("did_receive_settings: {:?}", event);
        if event.action == "st.lynx.plugins.opendeck-k1pro.backlight" {
            let settings: BacklightSettings = serde_json::from_value(event.payload.settings)?;
            if let Some(device) = DEVICES.read().await.get(&event.device) {
                apply_backlight_settings(&device, &settings).await?;
            }
        }
        Ok(())
    }

    async fn key_up(
        &self,
        event: KeyEvent,
        outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        log::debug!("key_up action: {:?}", event);
        if event.action == "st.lynx.plugins.opendeck-k1pro.backlight" {
            let settings: BacklightSettings = serde_json::from_value(event.payload.settings)?;
            if let Some(device) = DEVICES.read().await.get(&event.device) {
                handle_press(&device, &settings).await?;
                let _ = sync_settings_to_host(&device, &settings, event.context, outbound).await;
            }
        }
        Ok(())
    }

    async fn dial_up(
        &self,
        event: DialPressEvent,
        outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        log::debug!("dial_up action: {:?}", event);
        if event.action == "st.lynx.plugins.opendeck-k1pro.backlight" {
            let settings: BacklightSettings = serde_json::from_value(event.payload.settings)?;
            if let Some(device) = DEVICES.read().await.get(&event.device) {
                handle_press(&device, &settings).await?;
                let _ = sync_settings_to_host(&device, &settings, event.context, outbound).await;
            }
        }
        Ok(())
    }

    async fn dial_rotate(
        &self,
        event: DialRotateEvent,
        outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        log::debug!("dial_rotate action: {:?}", event);
        if event.action == "st.lynx.plugins.opendeck-k1pro.backlight" {
            let settings: BacklightSettings = serde_json::from_value(event.payload.settings)?;
            if let Some(device) = DEVICES.read().await.get(&event.device) {
                handle_dial_rotate(&device, &settings, event.payload.ticks).await?;
                let _ = sync_settings_to_host(&device, &settings, event.context, outbound).await;
            }
        }
        Ok(())
    }
}

async fn shutdown() {
    let tokens = TOKENS.write().await;
    for (_, token) in tokens.iter() {
        token.cancel();
    }
}

async fn connect_websocket(lib: Arc<TransportLib>) {
    if let Err(error) = init_plugin(GlobalEventHandler { lib }, ActionEventHandler {}).await {
        log::error!("Failed to initialize websocket plugin interface: {}", error);
        exit(1);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn sigterm() -> Result<(), Box<dyn std::error::Error>> {
    let mut sig = signal(SignalKind::terminate())?;
    sig.recv().await;
    Ok(())
}

#[cfg(target_os = "windows")]
async fn sigterm() -> Result<(), Box<dyn std::error::Error>> {
    std::future::pending::<()>().await;
    Ok(())
}

fn locate_library() -> Result<PathBuf, Box<dyn std::error::Error>> {
    // Try executable directory first
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let lib_in_exe_dir = exe_dir.join("libtransport.so");
            if lib_in_exe_dir.exists() {
                return Ok(lib_in_exe_dir);
            }
        }
    }

    // Try current working directory
    let lib_in_cwd = PathBuf::from("./libtransport.so");
    if lib_in_cwd.exists() {
        return Ok(lib_in_cwd);
    }

    // Check system paths
    let lib_system = PathBuf::from("/usr/local/lib/libtransport.so");
    if lib_system.exists() {
        return Ok(lib_system);
    }

    let lib_system_usr = PathBuf::from("/usr/lib/libtransport.so");
    if lib_system_usr.exists() {
        return Ok(lib_system_usr);
    }

    Err("Could not find libtransport.so in exe directory, CWD, or system library paths".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    simplelog::TermLogger::init(
        simplelog::LevelFilter::Debug,
        simplelog::Config::default(),
        simplelog::TerminalMode::Stdout,
        simplelog::ColorChoice::Never,
    )
    .unwrap();

    let lib_path = match locate_library() {
        Ok(path) => path,
        Err(e) => {
            log::error!("Fatal configuration error: {}", e);
            exit(1);
        }
    };

    log::info!("Loading dynamic transport library from: {:?}", lib_path);
    let lib = unsafe { TransportLib::load(lib_path).map_err(|e| e as Box<dyn std::error::Error>)? };

    tokio::select! {
        _ = connect_websocket(lib) => {},
        _ = sigterm() => {},
    }

    log::info!("Shutting down");
    shutdown().await;

    let tracker = TRACKER.lock().await.clone();
    log::info!("Waiting for tasks to finish");
    tracker.close();
    tracker.wait().await;
    log::info!("Tasks finished, exiting");

    Ok(())
}
