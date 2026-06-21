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
                        let path_int1 = c_char_to_string(info.path);
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
                                
                                // Wake the screen, set LCD brightness, clear all icons, and refresh to register connection
                                (lib.transport_wakeup_screen)(handle);
                                (lib.transport_set_key_brightness)(handle, 100);
                                (lib.transport_clear_all_keys)(handle);
                                (lib.transport_refresh)(handle);

                                let mut fw_buf = [0 as libc::c_char; 256];
                                (lib.transport_get_firmware_version)(handle, fw_buf.as_mut_ptr(), fw_buf.len());
                                let firmware_version = c_char_to_string(fw_buf.as_ptr());

                                log::info!("Registered K1 Pro device. Firmware version: {}", firmware_version);

                                // Initialize default backlight settings (Cyan, brightness 4, static effect)
                                (lib.transport_set_keyboard_lighting_speed)(handle, 3);
                                (lib.transport_set_keyboard_lighting_effects)(handle, 0);
                                
                                // Write raw HID backlight report for default settings (Cyan at 4/6 brightness)
                                // Scaled default color (0, 150, 255) by default brightness 4 (4/6):
                                // R: 0
                                // G: 150 * (4/6) * 0.47 = 47 (calibrated)
                                // B: 255 * (4/6) = 170
                                let write_res = (|| -> Result<(), std::io::Error> {
                                    use std::io::Write;
                                    let mut file = std::fs::OpenOptions::new()
                                        .write(true)
                                        .open(&path_int1)?;

                                    let mut color_report = [0u8; 513];
                                    color_report[0] = 0x04;
                                    color_report[1..4].copy_from_slice(b"CRT");
                                    color_report[6..11].copy_from_slice(b"COLOR");
                                    color_report[11] = 100; // 4/6 of 150 brightness
                                    color_report[12] = 0;   // R
                                    color_report[13] = 80;  // G (gamma corrected 150)
                                    color_report[14] = 255; // B (gamma corrected 255)
                                    file.write_all(&color_report)?;

                                    let mut cpos_report = [0u8; 513];
                                    cpos_report[0] = 0x04;
                                    cpos_report[1..4].copy_from_slice(b"CRT");
                                    cpos_report[6..10].copy_from_slice(b"CPOS");
                                    cpos_report[12] = 0x57; // 'W' for Windows
                                    file.write_all(&cpos_report)?;

                                    Ok(())
                                })();

                                if let Err(e) = write_res {
                                    log::error!("Failed to write default raw HID backlight report to {}: {:?}", path_int1, e);
                                }
                                
                                // Ensure report size and ID are 0x04 and switch OS mode to Windows
                                (lib.transport_set_reportSize)(handle, 513, 1025, 0);
                                (lib.transport_set_reportID)(handle, 0x04);
                                let res_os = (lib.transport_keyboard_os_mode_switch)(handle, 0);
                                log::info!("OS mode switch result for {}: {}", id, res_os);

                                // Find and open Interface 0 for standard keyboard reports (used during standalone mode)
                                let mut handle_int0 = None;
                                let mut path_int0 = None;
                                let mut scan = devs;
                                while !scan.is_null() {
                                    let scan_info = &*scan;
                                    if scan_info.interface_number == 0 {
                                        let scan_serial = wchar_to_string(scan_info.serial_number);
                                        if scan_serial == serial {
                                            let scan_path = c_char_to_string(scan_info.path);
                                            path_int0 = Some(scan_path);

                                            let mut h0 = TransportHandle(std::ptr::null_mut());
                                            let res_int0 = (lib.transport_create)(scan, &mut h0);
                                            if res_int0 == 0 {
                                                log::info!("Successfully opened Interface 0 for K1 Pro: {}", id);
                                                handle_int0 = Some(h0);
                                            } else {
                                                log::error!("Failed to create transport handle for K1 Pro Interface 0: {:#08x}", res_int0);
                                            }
                                            break;
                                        }
                                    }
                                    scan = scan_info.next;
                                }

                                let device = Arc::new(Device {
                                    id: id.clone(),
                                    serial_number: serial,
                                    firmware_version,
                                    handle,
                                    handle_int0,
                                    lib: lib.clone(),
                                    write_lock: tokio::sync::Mutex::new(()),
                                    standalone_mode: tokio::sync::Mutex::new(false),
                                    standalone_current_scr: tokio::sync::Mutex::new(None),
                                    path_int0,
                                    path_int1,
                                    last_host_transition: tokio::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(10)),
                                    last_standalone_transition: tokio::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(10)),
                                    last_non_zero_scr: tokio::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(60)),
                                    backlight_brightness: tokio::sync::Mutex::new(4),
                                    backlight_last_non_zero_brightness: tokio::sync::Mutex::new(4),
                                    backlight_speed: tokio::sync::Mutex::new(3),
                                    backlight_effect: tokio::sync::Mutex::new(0),
                                    backlight_color: tokio::sync::Mutex::new((0, 150, 255)),
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
