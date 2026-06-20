use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use openaction::OUTBOUND_EVENT_MANAGER;

use crate::{
    DEVICES, TOKENS, TRACKER, FLUSH_NOTIFY,
    device::{device_task, handle_error, wchar_to_string, c_char_to_string, Device},
    transport::{TransportLib, TransportHandle},
};

pub async fn watcher_task(lib: Arc<TransportLib>, token: CancellationToken) {
    log::info!("Starting K1 Pro device watcher task");

    loop {
        if token.is_cancelled() {
            break;
        }

        // 1. Get the list of already connected device IDs safely
        let connected_ids: HashSet<String> = DEVICES.read().await.keys().cloned().collect();

        // 2. Perform FFI enumeration and initialization in a synchronous block (no .await)
        let mut active_ids = HashSet::new();
        let mut new_devices = Vec::new();

        unsafe {
            let devs = (lib.transport_hid_enumerate)(0x5548, 0x1025);
            if !devs.is_null() {
                let mut current = devs;
                while !current.is_null() {
                    let info = &*current;
                    // Filter matching K1 Pro usage page and usage
                    if info.usage_page > 1025 && info.usage == 1 {
                        let serial = wchar_to_string(info.serial_number);
                        let id = format!("{}-{}", crate::mappings::DEVICE_NAMESPACE, serial);
                        active_ids.insert(id.clone());

                        if !connected_ids.contains(&id) {
                            log::info!("Discovered new K1 Pro device: {}", id);
                            
                            let mut handle = TransportHandle(std::ptr::null_mut());
                            let res = (lib.transport_create)(current, &mut handle);
                            if res == 0 {
                                // Configure report size and ID for K1 Pro
                                (lib.transport_set_reportSize)(handle, 513, 1025, 0);
                                (lib.transport_set_reportID)(handle, 0x04);
                                
                                // Initialize default backlight settings (Cyan, brightness 4)
                                (lib.transport_set_keyboard_backlight_brightness)(handle, 4);
                                (lib.transport_set_keyboard_rgb_backlight)(handle, 0, 150, 255);
                                (lib.transport_clear_all_keys)(handle);
                                (lib.transport_refresh)(handle);

                                let mut fw_buf = [0 as libc::c_char; 256];
                                (lib.transport_get_firmware_version)(handle, fw_buf.as_mut_ptr(), fw_buf.len());
                                let firmware_version = c_char_to_string(fw_buf.as_ptr());

                                log::info!("Registered K1 Pro device. Firmware version: {}", firmware_version);

                                let device = Arc::new(Device {
                                    id: id.clone(),
                                    serial_number: serial,
                                    firmware_version,
                                    handle,
                                    lib: lib.clone(),
                                    write_lock: tokio::sync::Mutex::new(()),
                                });
                                new_devices.push(device);
                            } else {
                                log::error!("Failed to create transport handle for {}: {:#08x}", id, res);
                            }
                        }
                    }
                    current = info.next;
                }
                (lib.transport_hid_free_enumeration)(devs);
            }
        }

        // 3. Register and spawn tasks for new devices (with .await)
        for device in new_devices {
            let id = device.id.clone();
            let dev_token = CancellationToken::new();
            TOKENS.write().await.insert(id.clone(), dev_token.clone());

            // Register device with OpenDeck
            if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
                if let Err(e) = outbound
                    .register_device(
                        id.clone(),
                        "StreamDock K1 Pro".to_string(),
                        crate::mappings::ROW_COUNT as u8,
                        crate::mappings::COL_COUNT as u8,
                        crate::mappings::ENCODER_COUNT as u8,
                        0,
                    )
                    .await
                {
                    log::error!("Failed to register device {} with host: {:?}", id, e);
                    continue;
                }
            }

            DEVICES.write().await.insert(id.clone(), device.clone());

            let flush_notify = Arc::new(tokio::sync::Notify::new());
            FLUSH_NOTIFY
                .write()
                .await
                .insert(id.clone(), flush_notify.clone());

            let tracker = TRACKER.lock().await.clone();
            tracker.spawn(device_task(device, dev_token));
        }

        // 4. Clean up disconnected devices
        let connected_ids_list: Vec<String> = DEVICES.read().await.keys().cloned().collect();
        for id in connected_ids_list {
            if !active_ids.contains(&id) {
                log::warn!("K1 Pro device disconnected: {}", id);
                handle_error(&id).await;
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = token.cancelled() => break,
        }
    }

    log::info!("K1 Pro device watcher task finished");
}
