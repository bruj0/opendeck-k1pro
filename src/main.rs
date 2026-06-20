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
        log::debug!("SetImageEvent: {:?}", event);

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

struct ActionEventHandler {}
impl openaction::ActionEventHandler for ActionEventHandler {}

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
        simplelog::LevelFilter::Info,
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
