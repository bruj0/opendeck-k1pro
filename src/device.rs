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
    pub lib: Arc<TransportLib>,
    pub write_lock: Mutex<()>,
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
    tracker.spawn(device_heartbeat_task(device.clone(), token.clone()));
    
    tokio::select! {
        _ = token.cancelled() => {}
    };

    FLUSH_NOTIFY.write().await.remove(&id);
    
    log::info!("Shutting down device task for {}", id);
    unsafe {
        (lib.transport_disconnected)(handle);
        (lib.transport_destroy)(handle);
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
    loop {
        if token.is_cancelled() {
            break;
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

/// Sends periodic keepalive reports to prevent connection resets
async fn device_heartbeat_task(device: Arc<Device>, token: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                let _guard = device.write_lock.lock().await;
                unsafe {
                    let res = (device.lib.transport_heartbeat)(device.handle);
                    log::debug!("Heartbeat result: {:#08x}", res);
                    (device.lib.transport_set_reportSize)(device.handle, 513, 1025, 0);
                    (device.lib.transport_set_reportID)(device.handle, 0x04);
                    (device.lib.transport_keyboard_os_mode_switch)(device.handle, 0);
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
