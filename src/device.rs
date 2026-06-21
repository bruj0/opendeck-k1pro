use data_url::DataUrl;
use image::{load_from_memory_with_format, DynamicImage};
use openaction::{OUTBOUND_EVENT_MANAGER, SetImageEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{
    DEVICES, FLUSH_NOTIFY, TOKENS, TRACKER,
    transport::{TransportHandle, TransportLib},
    mappings::opendeck_to_device,
};

pub struct Device {
    pub id: String,
    pub serial_number: String,
    pub firmware_version: String,
    pub handle: TransportHandle,
    pub handle_int0: Option<TransportHandle>,
    pub lib: Arc<TransportLib>,
    pub write_lock: Mutex<()>,
    pub standalone_mode: Mutex<bool>,
    pub standalone_initial_scr: Mutex<Option<i64>>,
    pub path_int0: Option<String>,
    pub path_int1: String,
    pub last_host_transition: Mutex<std::time::Instant>,
    pub last_standalone_transition: Mutex<std::time::Instant>,
}

unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Device {
    pub async fn set_brightness(&self, brightness: u8) -> Result<(), u32> {
        let _guard = self.write_lock.lock().await;
        unsafe {
            let res = (self.lib.transport_set_key_brightness)(self.handle, brightness);
            if res != 0 {
                return Err(res);
            }
            (self.lib.transport_refresh)(self.handle);
            (self.lib.transport_set_reportSize)(self.handle, 513, 1025, 0);
            (self.lib.transport_set_reportID)(self.handle, 0x04);
            (self.lib.transport_keyboard_os_mode_switch)(self.handle, 0);
        }
        Ok(())
    }

    pub async fn switch_to_standalone(&self) -> Result<(), u32> {
        let _guard = self.write_lock.lock().await;
        let mut mode = self.standalone_mode.lock().await;
        if !*mode {
            unsafe {
                let res = (self.lib.transport_disconnected)(self.handle);
                if res != 0 {
                    return Err(res);
                }
            }
            *mode = true;
            let mut initial_scr = self.standalone_initial_scr.lock().await;
            *initial_scr = None;
            
            let mut last_trans = self.last_standalone_transition.lock().await;
            *last_trans = std::time::Instant::now();
            
            log::info!("Switched K1 Pro device to standalone mode.");

            // Deregister from OpenDeck host since we are in standalone mode
            let id = self.id.clone();
            tokio::spawn(async move {
                if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
                    log::info!("Deregistering device {} from OpenDeck host for standalone mode...", id);
                    let _ = outbound.deregister_device(id).await;
                }
            });
        }
        Ok(())
    }

    /// Transitions the device from standalone mode back to host-controlled mode.
    ///
    /// Re-runs the connection handshake (report sizes, screen wakeup, brightness, OS mode switch),
    /// updates the `last_host_transition` timestamp to prevent click loops, and spawns a task
    /// to deregister and re-register the device with the OpenDeck host to trigger key image redraws.
    ///
    /// # Assumptions
    /// - The dynamic FFI library is loaded and the handle is valid.
    /// - Thread-safe lock concurrency is managed via `write_lock`.
    ///
    /// # Errors
    /// - Silent failure logs if deregister/reregister commands to OpenDeck host fail.
    pub async fn switch_to_host_controlled(&self) -> Result<(), u32> {
        let _guard = self.write_lock.lock().await;
        let mut mode = self.standalone_mode.lock().await;
        if *mode {
            unsafe {
                (self.lib.transport_set_keyboard_backlight_brightness)(self.handle, 4);
                (self.lib.transport_set_keyboard_lighting_speed)(self.handle, 3);
                (self.lib.transport_set_keyboard_lighting_effects)(self.handle, 0);
                (self.lib.transport_set_keyboard_rgb_backlight)(self.handle, 0, 150, 255);

                (self.lib.transport_set_reportSize)(self.handle, 513, 1025, 0);
                (self.lib.transport_set_reportID)(self.handle, 0x04);
                
                (self.lib.transport_wakeup_screen)(self.handle);
                (self.lib.transport_set_key_brightness)(self.handle, 100);
                (self.lib.transport_clear_all_keys)(self.handle);
                (self.lib.transport_refresh)(self.handle);
                
                (self.lib.transport_keyboard_os_mode_switch)(self.handle, 0);
            }
            *mode = false;
            
            // Record transition timestamp to prevent immediate toggle loops
            let mut last_trans = self.last_host_transition.lock().await;
            *last_trans = std::time::Instant::now();
            
            log::info!("Switched K1 Pro device back to host-controlled mode.");

            // Request host to refresh layout by deregistering and reregistering the device
            let id = self.id.clone();
            tokio::spawn(async move {
                if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
                    log::info!("Reregistering device {} with OpenDeck host to refresh images...", id);
                    let _ = outbound.deregister_device(id.clone()).await;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let _ = outbound.register_device(
                        id.clone(),
                        "StreamDock K1 Pro".to_string(),
                        crate::mappings::ROW_COUNT as u8,
                        crate::mappings::COL_COUNT as u8,
                        crate::mappings::ENCODER_COUNT as u8,
                        0,
                    ).await;
                }
            });
        }
        Ok(())
    }
}

pub(crate) fn wchar_to_string(wstr: *const libc::wchar_t) -> String {
    if wstr.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0;
        while *wstr.offset(len) != 0 {
            len += 1;
        }
        #[cfg(target_os = "windows")]
        {
            let slice = std::slice::from_raw_parts(wstr as *const u16, len as usize);
            String::from_utf16_lossy(slice)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let slice = std::slice::from_raw_parts(wstr, len as usize);
            slice.iter().map(|&c| std::char::from_u32(c as u32).unwrap_or('?')).collect()
        }
    }
}

pub(crate) fn c_char_to_string(cstr: *const libc::c_char) -> String {
    if cstr.is_null() {
        return String::new();
    }
    unsafe {
        std::ffi::CStr::from_ptr(cstr).to_string_lossy().into_owned()
    }
}

/// Sets up device, configures backlight, and spawns read/heartbeat loops
pub async fn device_task(
    device: Arc<Device>,
    token: CancellationToken,
) {
    let id = device.id.clone();
    let handle = device.handle;
    let lib = device.lib.clone();

    let tracker = TRACKER.lock().await.clone();
    
    // Spawn tasks
    tracker.spawn(device_events_task(device.clone(), id.clone(), token.clone()));
    if device.handle_int0.is_some() {
        tracker.spawn(device_events_int0_task(device.clone(), id.clone(), token.clone()));
    }
    tracker.spawn(device_heartbeat_task(device.clone(), token.clone()));
    
    tokio::select! {
        _ = token.cancelled() => {}
    };

    FLUSH_NOTIFY.write().await.remove(&id);
    
    log::info!("Shutting down device task for {}", id);
    unsafe {
        (lib.transport_disconnected)(handle);
        (lib.transport_destroy)(handle);
        if let Some(h0) = device.handle_int0 {
            (lib.transport_destroy)(h0);
        }
    }
    
    DEVICES.write().await.remove(&id);
    log::info!("Shutdown complete for device {}", id);
}

/// Handles errors by clean deregistration and token cancellation
pub async fn handle_error(id: &str) {
    log::info!("Handling device error for {}", id);
    
    if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
        outbound.deregister_device(id.to_string()).await.ok();
    }

    if let Some(token) = TOKENS.read().await.get(id) {
        token.cancel();
    }
}

/// Background thread monitoring K1 Pro input events via FFI read
async fn device_events_task(device: Arc<Device>, id: String, token: CancellationToken) {
    let mut buffer = [0u8; 1024];
    let mut raw_file: Option<std::fs::File> = None;
    loop {
        if token.is_cancelled() {
            break;
        }

        let is_standalone = {
            let state = device.standalone_mode.lock().await;
            *state
        };

        if is_standalone {
            if raw_file.is_none() {
                use std::os::unix::fs::OpenOptionsExt;
                match std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&device.path_int1)
                {
                    Ok(f) => {
                        log::info!("Opened raw hidraw file {} for standalone monitoring (non-blocking)", device.path_int1);
                        raw_file = Some(f);
                    }
                    Err(e) => {
                        log::error!("Failed to open raw hidraw file {}: {:?}", device.path_int1, e);
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue;
                    }
                }
            }

            let file = raw_file.as_mut().unwrap();
            let mut read_buf = [0u8; 512];
            use std::io::Read;
            match file.read(&mut read_buf) {
                Ok(n) => {
                    if n > 0 {
                        let data = &read_buf[..n];
                        // Parse DEVCFG report (ID = 4) and detect scr page index changes
                        if n >= 12 && data[0] == 4 && &data[1..7] == b"DEVCFG" {
                            let json_bytes = &data[12..];
                            if let Some(null_pos) = json_bytes.iter().position(|&b| b == 0) {
                                let json_str = String::from_utf8_lossy(&json_bytes[..null_pos]);
                                if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&json_str) {
                                    if let Some(scr) = json_val.get("scr").and_then(|v| v.as_i64()) {
                                        let mut initial_scr = device.standalone_initial_scr.lock().await;
                                        if let Some(init_scr) = *initial_scr {
                                            if scr != init_scr {
                                                log::info!("Detected standalone page change (scr: {} -> {}). Reconnecting to Host-Controlled mode...", init_scr, scr);
                                                drop(initial_scr); // Release lock before await
                                                
                                                // Close raw file first to release the hidraw device
                                                raw_file = None;
                                                
                                                if let Err(e) = device.switch_to_host_controlled().await {
                                                    log::error!("Failed to switch to host-controlled mode: {:?}", e);
                                                }
                                                tokio::time::sleep(Duration::from_millis(5)).await;
                                                continue;
                                            }
                                        } else {
                                            *initial_scr = Some(scr);
                                            log::info!("Stored initial standalone page index: {}", scr);
                                        }
                                    }
                                }
                            }
                        }
                        log::debug!("Raw Standalone read: len={}, bytes={:?}", n, &data[..std::cmp::min(16, n)]);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No data available, sleep and continue
                }
                Err(e) => {
                    log::error!("Error reading raw hidraw file: {:?}", e);
                    raw_file = None; // Force reopen
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        } else {
            // Close raw file if it was open
            if raw_file.is_some() {
                raw_file = None;
            }
        }

        let mut length = 1024;
        let res = unsafe {
            (device.lib.transport_read)(device.handle, buffer.as_mut_ptr(), &mut length, 100)
        };

        if res == 0 {
            let data = &buffer[..length];
            if length > 0 {
                log::debug!("Raw HID read from K1 Pro: len={}, bytes={:?}", length, &data[..std::cmp::min(16, length)]);
            }
            // Verify report ID = 4 and ACK prefix [A, C, K]
            if length >= 12 && data[0] == 0x04 && data[1] == 65 && data[2] == 67 && data[3] == 75 {
                let hw_code = data[10];
                let state = data[11];
                
                log::info!("Received K1 Pro raw event: code={:#04x}, state={}", hw_code, state);
                
                let normalized_state = if state == 0x01 { 1 } else { 0 };

                if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
                    // 1. Regular LCD buttons
                    if let Some(logical_pos) = crate::mappings::device_to_opendeck(hw_code) {
                        log::info!("Mapping code {:#04x} to LCD key {}. State: {}", hw_code, logical_pos, normalized_state);
                        if normalized_state == 1 {
                            outbound.key_down(id.clone(), logical_pos).await.ok();
                        } else {
                            outbound.key_up(id.clone(), logical_pos).await.ok();
                        }
                    }
                    // 2. Encoder press events
                    else if hw_code == 0x25 || hw_code == 0x30 || hw_code == 0x31 {
                        let knob_idx = match hw_code {
                            0x25 => 0,
                            0x30 => 1,
                            0x31 => 2,
                            _ => unreachable!(),
                        };

                        // Hardwired toggle for Knob 1 Click (knob_idx == 0)
                        if knob_idx == 0 {
                            if normalized_state == 1 {
                                let elapsed = {
                                    let last_trans = device.last_host_transition.lock().await;
                                    last_trans.elapsed()
                                };
                                if elapsed < Duration::from_millis(1000) {
                                    log::info!("Ignoring Knob 1 click event right after host transition to prevent infinite loop. Elapsed: {:?}", elapsed);
                                    continue;
                                }
                                log::info!("Knob 1 pressed. Hardwired toggle: switching to STANDALONE mode...");
                                if let Err(e) = device.switch_to_standalone().await {
                                    log::error!("Failed to switch to standalone mode: {:?}", e);
                                }
                            }
                            continue; // Skip forwarding Knob 1 press/release to host
                        }

                        log::info!("Mapping code {:#04x} to Encoder {} Click. State: {}", hw_code, knob_idx, normalized_state);
                        if normalized_state == 1 {
                            outbound.encoder_down(id.clone(), knob_idx).await.ok();
                        } else {
                            outbound.encoder_up(id.clone(), knob_idx).await.ok();
                        }
                    }
                    // 3. Encoder rotation events
                    else if hw_code == 0x50 || hw_code == 0x51 || hw_code == 0x60 || hw_code == 0x61 || hw_code == 0x90 || hw_code == 0x91 {
                        let (knob_idx, ticks) = match hw_code {
                            0x50 => (0u8, -1i16),
                            0x51 => (0u8, 1i16),
                            0x60 => (1u8, -1i16),
                            0x61 => (1u8, 1i16),
                            0x90 => (2u8, -1i16),
                            0x91 => (2u8, 1i16),
                            _ => unreachable!(),
                        };
                        log::info!("Mapping code {:#04x} to Encoder {} Rotation. Ticks: {}", hw_code, knob_idx, ticks);
                        if let Err(e) = outbound.encoder_change(id.clone(), knob_idx, ticks).await {
                            log::error!("Failed to forward Encoder {} change to OpenDeck host: {:?}", knob_idx, e);
                        } else {
                            log::info!("Successfully forwarded Encoder {} change (ticks={}) to host", knob_idx, ticks);
                        }
                    } else {
                        log::warn!("Unhandled K1 Pro hardware event code: {:#04x}", hw_code);
                    }
                }
            }
        } else if res != 0 && res != 0x05000302 { // Filter out read timeouts (0x05000302)
            log::error!("Read hardware error on {}: {:#08x}", id, res);
            handle_error(&id).await;
            break;
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Background thread monitoring K1 Pro keyboard interface events
async fn device_events_int0_task(device: Arc<Device>, _id: String, token: CancellationToken) {
    let mut raw_file: Option<std::fs::File> = None;
    loop {
        if token.is_cancelled() {
            break;
        }

        // Only read from Interface 0 if we are in standalone mode
        let is_standalone = {
            let state = device.standalone_mode.lock().await;
            *state
        };

        if !is_standalone {
            if raw_file.is_some() {
                raw_file = None;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }

        if raw_file.is_none() {
            if let Some(path) = &device.path_int0 {
                use std::os::unix::fs::OpenOptionsExt;
                match std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(path)
                {
                    Ok(f) => {
                        log::info!("Opened raw hidraw file {} for Interface 0 standalone monitoring (non-blocking)", path);
                        raw_file = Some(f);
                    }
                    Err(e) => {
                        log::error!("Failed to open raw hidraw file {}: {:?}", path, e);
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue;
                    }
                }
            } else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        }

        let file = raw_file.as_mut().unwrap();
        let mut read_buf = [0u8; 1024];
        use std::io::Read;
        match file.read(&mut read_buf) {
            Ok(n) => {
                if n > 0 {
                    log::debug!("Raw HID read from K1 Pro Interface 0: len={}, bytes={:?}", n, &read_buf[..std::cmp::min(16, n)]);
                    let elapsed = {
                        let last_trans = device.last_standalone_transition.lock().await;
                        last_trans.elapsed()
                    };
                    if elapsed < Duration::from_millis(1000) {
                        log::info!("Ignoring Interface 0 activity right after standalone transition. Elapsed: {:?}", elapsed);
                    } else {
                        log::info!("Activity detected on K1 Pro Interface 0 (Standalone). Reconnecting to Host-Controlled mode...");
                        raw_file = None; // Close file before transition
                        if let Err(e) = device.switch_to_host_controlled().await {
                            log::error!("Failed to switch to host-controlled mode: {:?}", e);
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data, continue
            }
            Err(e) => {
                log::error!("Error reading raw hidraw file for Interface 0: {:?}", e);
                raw_file = None; // Force reopen
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Sends periodic keepalive reports to prevent connection resets
async fn device_heartbeat_task(device: Arc<Device>, token: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                let is_standalone = {
                    let state = device.standalone_mode.lock().await;
                    *state
                };
                if !is_standalone {
                    let _guard = device.write_lock.lock().await;
                    unsafe {
                        let res = (device.lib.transport_heartbeat)(device.handle);
                        log::debug!("Heartbeat result: {:#08x}", res);
                        (device.lib.transport_set_reportSize)(device.handle, 513, 1025, 0);
                        (device.lib.transport_set_reportID)(device.handle, 0x04);
                        (device.lib.transport_keyboard_os_mode_switch)(device.handle, 0);
                    }
                }
            }
            _ = token.cancelled() => break,
        }
    }
}

/// Formats and updates key images
pub async fn handle_set_image(
    device: &Device,
    _id: &str,
    evt: SetImageEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _guard = device.write_lock.lock().await;

    match (evt.position, evt.image) {
        (Some(position), Some(image)) => {
            log::info!("Setting image for button {}", position);

            let url = DataUrl::process(image.as_str()).map_err(|e| format!("{:?}", e))?;
            let (body, _fragment) = url.decode_to_vec().map_err(|e| format!("{:?}", e))?;

            if url.mime_type().subtype != "jpeg" {
                log::error!("Incorrect mime type: {}", url.mime_type());
                return Ok(());
            }

            let image = load_from_memory_with_format(body.as_slice(), image::ImageFormat::Jpeg)?;
            
            // 1. Convert to RGB8 to handle transparency correctly
            let rgb_img = image.to_rgb8();

            // 2. Rotate -90 degrees (which is 90 degrees clockwise)
            let rotated = DynamicImage::ImageRgb8(rgb_img).rotate90();

            // 3. Convert to RGB8 again since rotate90 returns DynamicImage
            let rgb_rotated = rotated.to_rgb8();

            // 4. Resize to 64x64
            let resized = image::imageops::resize(
                &rgb_rotated,
                64,
                64,
                image::imageops::FilterType::Lanczos3,
            );

            // 5. Encode back to JPEG stream with quality 90 to match official SDK and avoid hardware decoder failures
            let mut jpeg_bytes = Vec::new();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 90);
            encoder.encode(
                resized.as_raw(),
                64,
                64,
                image::ColorType::Rgb8.into(),
            )?;

            if let Some(hw_key) = opendeck_to_device(position) {
                unsafe {
                    let res = (device.lib.transport_set_key_image_stream)(
                        device.handle,
                        jpeg_bytes.as_ptr() as *const libc::c_char,
                        jpeg_bytes.len(),
                        hw_key,
                    );
                    if res != 0 {
                        log::error!("Failed to stream key image: {:#08x}", res);
                    }
                    (device.lib.transport_refresh)(device.handle);
                }
            }
        }
        (Some(position), None) => {
            if let Some(hw_key) = opendeck_to_device(position) {
                unsafe {
                    (device.lib.fn_transport_clear_key)(device.handle, hw_key);
                    (device.lib.transport_refresh)(device.handle);
                }
            }
        }
        (None, None) => {
            unsafe {
                (device.lib.transport_clear_all_keys)(device.handle);
                (device.lib.transport_refresh)(device.handle);
            }
        }
        _ => {}
    }

    unsafe {
        (device.lib.transport_set_reportSize)(device.handle, 513, 1025, 0);
        (device.lib.transport_set_reportID)(device.handle, 0x04);
        (device.lib.transport_keyboard_os_mode_switch)(device.handle, 0);
    }

    Ok(())
}
