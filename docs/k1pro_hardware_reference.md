# StreamDock K1 Pro Hardware and Protocol Reference

This document compiles the confirmed technical details, USB HID protocol specifics, hardware event codes, and connection handshake sequences for the **StreamDock K1 Pro** device.

---

## 1. Device Identification & Interface Layout

The StreamDock K1 Pro registers two primary USB interfaces with the host. Under Linux, these map to two distinct `/dev/hidraw` devices.

*   **USB VID**: `0x5548`
*   **USB PID**: `0x1025`

### Interface Division
1.  **Interface 0 (e.g., `/dev/hidraw4`)**: 
    *   **Usage Page**: `1` (Generic Desktop), **Usage**: `6` (Keyboard)
    *   Acts as a standard USB Keyboard and Consumer Control device. 
    *   *Behavior*: In standalone mode (when `"Stream Dock": "Disconnected"`), dial rotations and keypresses are mapped directly in hardware and sent as keyboard strokes/media keys over this interface.
2.  **Interface 1 (e.g., `/dev/hidraw5`)**:
    *   **Usage Page**: `12` (Consumer), **Usage**: `1` (Consumer Control) / Vendor Pages.
    *   The raw vendor-defined interface used by host-side controller software (e.g., OpenDeck or StreamDock SDK).
    *   *Behavior*: All raw event streaming, image drawing, and lighting commands are transmitted over this interface.

---

## 2. Connection Handshake & State Transition

When powered, the K1 Pro operates in a **standalone firmware mode** by default. In this mode, the device executes internal page/scene switching and **suppresses all raw input event reports** on Interface 1.

The device reports its connection status via a periodic `DEVCFG` JSON packet. When disconnected, it reports `"Stream Dock": "Disconnected"`.

### The "Connected" Handshake Sequence
To disable standalone firmware mode and unlock raw event streaming, the host must execute the following exact FFI initialization sequence on Interface 1:

1.  Set HID Report Size: Input = `513` bytes, Output = `1025` bytes, Feature = `0` bytes.
2.  Set Report ID: `0x04`.
3.  Wake LCD Screen: `transport_wakeup_screen`.
4.  Set LCD Brightness: `transport_set_key_brightness(handle, brightness_level)`.
5.  Clear all LCD button frames: `transport_clear_all_keys`.
6.  Refresh Device Framebuffer: `transport_refresh`.
7.  Switch OS Keyboard Mode: `transport_keyboard_os_mode_switch(handle, 0)` (Windows mode).

Once this sequence is complete, the K1 Pro sends a success packet starting with:
`[4, 83, 85, 67, 67, 69, 83, 83, 70, 85, 76, 76, 89, 32, 67, 79...]` (which decodes to `SUCCESSFULLY CONNECTED TO STREAM DOCK`). The periodic status updates will transition to `"Stream Dock": "Connected"`.

---

## 3. USB Protocol & Packet Structure

*   **Report ID**: `0x04`
*   **Input Report Size**: `513` bytes
*   **Output Report Size**: `1025` bytes

### Inbound JSON Status (`DEVCFG`)
The device periodically sends 512-byte reports containing device settings formatted as a JSON string:
*   **Bytes 0**: Report ID (`0x04`)
*   **Bytes 1–6**: `"DEVCFG"` ASCII header
*   **Bytes 7–11**: Padding/Null bytes
*   **Bytes 12+**: Null-terminated UTF-8 JSON payload.
  
*Example JSON:*
```json
{
        "version":      " V3.010.03.013",
        "os":   "Windows",
        "scr":  1,
        "style":        3,
        "slpt": 600,
        "Stream Dock":  "Connected",
        "led_info":     {
                "mode": 0,
                "speed":        0,
                "brightness":   0,
                "flag": 1,
                "hsv":  [0, 0, 0],
                "base_hs":      [0, 0]
        }
}
```

### Inbound Input Events (`ACK`)
When a key is pressed or a knob is rotated, the device sends a report with the following structure:
*   **Byte 0**: Report ID (`0x04`)
*   **Bytes 1–3**: `"ACK"` ASCII header
*   **Bytes 6–7**: `"OK"` ASCII header
*   **Byte 10**: Hardware Event Code (indicating which key/dial was triggered).
*   **Byte 11**: State (indicating press/release direction).

---

## 4. Hardware Event Mappings

When the device is in the `"Connected"` state, interactions trigger the following hardware codes:

### LCD Buttons (1–6)
*   **State Value**: `0x01` (Press), `0x00` (Release)
*   **Physical to Hardware Code Map**:
    *   Button 1: `0x05`
    *   Button 2: `0x03`
    *   Button 3: `0x01`
    *   Button 4: `0x06`
    *   Button 5: `0x04`
    *   Button 6: `0x02`

### Knobs (1–3) Press / Click
*   **State Value**: `0x01` (Press), `0x00` (Release)
*   **Hardware Code Map**:
    *   Knob 1 Press: `0x25`
    *   Knob 2 Press: `0x30`
    *   Knob 3 Press: `0x31`

### Knobs (1–3) Rotations
*   **State Value**: Always `0x00`
*   **Hardware Code Map**:
    *   Knob 1: `0x50` (Rotate Left), `0x51` (Rotate Right)
    *   Knob 2: `0x60` (Rotate Left), `0x61` (Rotate Right)
    *   Knob 3: `0x90` (Rotate Left), `0x91` (Rotate Right)

---

## 5. LCD Image Specifications

*   **Resolution**: 64x64 pixels per key.
*   **Format**: `JPEG` (progressive JPEG streaming supported).
*   **Rotation**: Rotated **-90 degrees** (90 degrees clockwise) relative to standard canvas layouts before transmission.
*   **Stream Endpoint**: `transport_set_key_image_stream(handle, jpeg_bytes, length, hardware_key_code)`.

---

## 6. Implementation Code Examples

### Python (Empirical Hardware Setup)
The following snippet demonstrates how to initialize the connection and read events using `libtransport.so` via ctypes:

```python
import ctypes
import time
from ctypes import c_void_p, c_size_t, c_uint8, byref

# Load C Library
lib = ctypes.CDLL("./libtransport.so")

# Initialize device handle
dev_info = ... # HidDeviceInfo populated via enumeration
handle_ptr = c_void_p()
lib.transport_create(byref(dev_info), byref(handle_ptr))
handle = handle_ptr.value

# 1. Establish Report Size and ID
lib.transport_set_reportSize(handle, 513, 1025, 0)
lib.transport_set_reportID(handle, 0x04)

# 2. Wake and Initialize Screen
lib.transport_wakeup_screen(handle)
lib.transport_set_key_brightness(handle, 100)
lib.transport_clear_all_keys(handle)
lib.transport_refresh(handle)

# 3. Unlock OS Mode (Mac = 1, Windows/Raw = 0)
lib.transport_keyboard_os_mode_switch(handle, 0)

# 4. Heartbeat & Event Reading Loop
buffer = (c_uint8 * 1024)()
while True:
    lib.transport_heartbeat(handle)
    length = c_size_t(1024)
    result = lib.transport_read(handle, buffer, byref(length), 100)
    if result == 0 and length.value > 0:
        data = bytes(buffer[:length.value])
        # Check for ACK header
        if length.value >= 12 and data[0] == 0x04 and data[1:4] == b"ACK":
            hw_code = data[10]
            state = data[11]
            print(f"Event: Code={hex(hw_code)}, State={state}")
    time.sleep(2.0)
```

### Rust (OpenDeck Plugin Device Initialization)
The following snippet shows how this is implemented in Rust using loaded dynamic FFI symbols:

```rust
unsafe {
    // 1. Establish reports
    (lib.transport_set_reportSize)(handle, 513, 1025, 0);
    (lib.transport_set_reportID)(handle, 0x04);
    
    // 2. Wake the screen and set LCD brightness to establish connection
    (lib.transport_wakeup_screen)(handle);
    (lib.transport_set_key_brightness)(handle, 100);
    (lib.transport_clear_all_keys)(handle);
    (lib.transport_refresh)(handle);

    // 3. Switch Keyboard OS Mode to raw/Windows
    (lib.transport_keyboard_os_mode_switch)(handle, 0);
}
```

---

## 7. Keyboard Backlight and RGB LED Control

The StreamDock K1 Pro features a full RGB keyboard backlight. However, the precompiled dynamic transport library FFI functions for backlight control contain critical channel truncation bugs.

### FFI Library Limitations & Bugs
1. **Missing Blue Channel**: The FFI function `transport_set_keyboard_rgb_backlight(handle, r, g, b)` only writes 3 bytes starting at offset 10 of the `COLOR` report. In the K1 Pro keyboard firmware, the `COLOR` payload layout is:
   - Offset 10: Brightness (0–255)
   - Offset 11: Red (0–255)
   - Offset 12: Green (0–255)
   - Offset 13: Blue (0–255)
   Because the FFI function only writes 3 bytes, offset 13 (Blue) remains `0`, completely disabling the Blue channel (preventing Purple, Cyan, etc. from rendering).
2. **Brightness Offset Bug**: The first argument `r` is written to offset 10 (interpreted as brightness), while `g` is written to Red (offset 11) and `b` is written to Green (offset 12).

### The Solution: Direct Raw HID Writes
To control the backlight, the plugin opens the Interface 1 `hidraw` device (`path_int1`) directly and writes 513-byte raw reports:

1. **`COLOR` Configuration Report**:
   * **Byte 0**: `0x04` (Report ID)
   * **Bytes 1–3**: `b"CRT"`
   * **Bytes 4–5**: `0x00, 0x00` (Padding)
   * **Bytes 6–10**: `b"COLOR"`
   * **Byte 11**: Hardware Brightness (`150` for optimal, high-saturation LED current)
   * **Byte 12**: Red (`0` or `255`)
   * **Byte 13**: Green (`0` or `255`)
   * **Byte 14**: Blue (`0` or `255`)

2. **`CPOS` Commit Report** (sent immediately after to commit settings):
   * **Byte 0**: `0x04` (Report ID)
   * **Bytes 1–3**: `b"CRT"`
   * **Bytes 4–5**: `0x00, 0x00` (Padding)
   * **Bytes 6–9**: `b"CPOS"`
   * **Bytes 10–11**: `0x00, 0x00` (Padding)
   * **Byte 12**: `0x57` (`'W'` for Windows mode commit)

### Saturated Pure Color Snapping
To prevent the physical LEDs from displaying washed-out, pale, or incorrect intermediate shades (due to human perception limits and non-linear LED mixing on linear hardware), the plugin maps all input RGB colors to the nearest pure primary/secondary color using HSL/HSV classification:

* **White**: Chroma `< 40` (maps to `(255, 255, 255)`)
* **Red**: Hue `< 15` or `>= 345` (maps to `(255, 0, 0)`)
* **Yellow**: Hue `15..75` (maps to `(255, 255, 0)`)
* **Green**: Hue `75..165` (maps to `(0, 255, 0)`)
* **Cyan**: Hue `165..210` (maps to `(0, 255, 255)`)
* **Blue**: Hue `210..255` (maps to `(0, 0, 255)`)
* **Purple**: Hue `255..345` (maps to `(255, 0, 255)`)

All dimming/brightness shades are removed by locking the hardware brightness to a constant `150` and mapping dark/black states (`#000000`) to the closest active color to ensure the backlight is always saturated and rich.

