# Profile Switching Implementation for StreamDock K1 Pro

This document explains how profile (scene) and page switching is implemented and configured for the **StreamDock K1 Pro** device.

---

## 1. Overview of OpenDeck Profile Structure

OpenDeck manages layouts using **Profiles**. It supports nested layout structures by organizing profiles hierarchically using a forward slash (`/`) separator (e.g., `SceneName/PageName`).
*   **Scene**: The top-level category or folder (e.g., `SceneA`).
*   **Page**: The individual sub-profile inside that folder (e.g., `Page1`).
*   **Root Profile**: A profile created at the root level without a folder structure (e.g., `Default`), which behaves as a scene with a single implicit page.

Users configure these profiles, scenes, and pages directly in the OpenDeck Host UI.

---

## 2. Device Driver Role (The Plugin)

The K1 Pro plugin (`opendeck-k1pro`) behaves as a standard, stateless input driver. It does not contain hardcoded logic for switching scenes or pages. Instead, it reads raw USB HID events from the K1 Pro hardware and translates them into standard OpenDeck events.

When a physical knob is rotated, the plugin decodes the rotation and forwards it immediately to the OpenDeck Host as an `encoder_change` event:

```rust
// In opendeck-k1pro/src/device.rs
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
    outbound.encoder_change(id.clone(), knob_idx, ticks).await.ok();
}
```

*   **Knob 1 (Index 0)**: Sends `encoder_change` with `knob_idx = 0`.
*   **Knob 2 (Index 1)**: Sends `encoder_change` with `knob_idx = 1`.
*   **Knob 3 (Index 2)**: Sends `encoder_change` with `knob_idx = 2`.

---

## 3. Host-Side Actions & Configuration

All layout behavior, including page and profile transitions, is configured by the user via the OpenDeck UI:

1.  **UI Configuration**: Users map actions to the encoder events. For example:
    *   Rotate **Knob 1** -> Trigger Action: **Switch Scene / Profile**.
    *   Rotate **Knob 2** -> Trigger Action: **Switch Page**.
2.  **Tauri Host Execution**: The OpenDeck host receives the encoder events from the plugin, maps them to the user-defined actions, and switches the active profile.
3.  **Visual Update**: When the profile switches, the host automatically pushes the new layout's key images and labels to the plugin, which streams them back to the K1 Pro device screens.
