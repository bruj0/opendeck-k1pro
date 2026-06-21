# libtransport.so Disassembly and Protocol Analysis Report

This report provides a detailed reverse engineering analysis of `libtransport.so` (unstripped, ELF 64-bit x86-64), which forms the core of the StreamDock C++ and Python SDK. The analysis covers the C FFI exports, internal class layouts, byte buffer protocol structures, and architectural limitations/bugs identified during disassembling the library.

---

## 1. FFI Export Interface & C++ Class Mapping

`libtransport.so` exposes a C FFI layer (`extern "C"`) which wraps the C++ implementation. These FFI functions manage the lifecycle and invoke methods of the `Transport` class, which in turn orchestrates `TransportSession` and `TransportDevice` objects.

### Lifecycle & Configuration FFI
*   `transport_create(const hid_device_info *device_info, TransportHandle *out_handle)`
    *   Instantiates a C++ `Transport` object and returns its raw pointer as `TransportHandle`.
*   `transport_destroy(TransportHandle handle)`
    *   Deletes the `Transport` object and frees resources.
*   `transport_set_reportSize(TransportHandle handle, uint16_t input_size, uint16_t output_size, uint16_t feature_size)`
    *   Maps to `Transport::setReportSize(...)`. Updates dynamic report sizes in `TransportDevice`.
*   `transport_set_reportID(TransportHandle handle, uint8_t reportID)`
    *   Maps to `Transport::setReportID(...)`. Sets the report ID byte inside `TransportSession`.

---

## 2. Internal Support Data Structures

### `TransportByteBuffer`
`TransportByteBuffer` inherits from or directly wraps `std::vector<unsigned char>`. 

#### Automatic Bounds Expansion
Disassembly of `TransportByteBuffer::operator[](unsigned long index)` (at `0x13d40`) shows custom bounds-handling. If the requested index is greater than or equal to the current vector size, the operator calls `std::vector::_M_fill_insert` to resize the vector up to `index + 1`, padding the newly allocated slots with `0x00` (null bytes). 
This allows methods to initialize a short buffer (e.g. 8 bytes) and write directly to offset 10 or 11, causing the buffer to automatically expand to the required length.

### `TransportDevice` Offset Map
The `TransportDevice` class maintains the following internal structure:
*   `0x10`: Pointer to raw HID device handle (from `hidapi`).
*   `0x98`: `uint16_t` Input report size (`receiveSize`).
*   `0x9a`: `uint16_t` Output report size (`chunkSize`).
*   `0x9c`: `uint16_t` Feature report size.

### `TransportSession` Offset Map
*   `0x20` (in `Transport` object): `pthread_mutex_t` used to serialize transaction writes.
*   `0xc1`: `bool` Read-disabled flag (causes `read` to immediately return 0 bytes).
*   `0xc2`: `uint8_t` HID Report ID (used for feature/input/output transactions).

---

## 3. Protocol Command Buffers & Disassembly Analysis

All control commands sent to the device are packaged into a `TransportByteBuffer` with a prefix command string, followed by parameters inserted at specific offsets.

| Command ID | FFI Function | Initial Size | Final Size | Offset | Parameter Description |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `COLOR` | `transport_set_keyboard_rgb_backlight` | 13 | 13 | 10, 11, 12 | R, G, B color channels |
| `LLUM` | `transport_set_keyboard_backlight_brightness` | 11 | 11 | 10 | Brightness level (clamped 0–6) |
| `LMOD` | `transport_set_keyboard_lighting_effects` | 11 | 11 | 10 | Effect type ID (clamped 0–9) |
| `LMOD` | `transport_set_keyboard_lighting_speed` | 11 | 11 | 10 | Effect speed ID (clamped 0–7) |
| `MOD` | `transport_change_mode` | 8 | 11 | 10 | ASCII mode byte (`mode + 0x31`) |
| `M_V\x00` | `transport_change_page` | 4 | 4 | 3 | Raw page index byte |
| `SETLB` | `transport_set_led_color` | 10 | Variable | 10+ | Repeats R, G, B `count` times |
| `SETLB` | `transport_set_single_led_color` | 10 | Variable | 10+ | Linear array of unique R, G, B tuples |
| `DELED` | `transport_reset_led_color` | 10 | 10 | N/A | Clear/reset LED strip color |
| `CLE` | `transport_clear_all_keys` | 8 | 12 | 11 | Sets clear flag to `0xff` |
| `CLE` | `transport_clear_key` | 8 | 12 | 11 | Sets target key index to clear |
| `LIG` | `transport_set_key_brightness` | 8 | 11 | 10 | Key LCD brightness (validated <= 100) |
| `LBLIG` | `transport_set_led_brightness` | 10 | 11 | 10 | LED strip brightness |
| `HAN` | `transport_sleep` | 8 | 8 | N/A | Suspend LCD screen / sleep |
| `DIS` | `transport_wakeup_screen` | 8 | 8 | N/A | Wake LCD screen / display |
| `STP` | `transport_refresh` | 8 | 8 | N/A | Commit/Refresh LCD framebuffer |
| `CONNECT`| `transport_heartbeat` | 12 | 12 | N/A | Periodic connection keep-alive |

---

## 4. Deep-Dive Disassembly & Implementation Notes

### A. Keyboard RGB Backlight Control (`COLOR` Command)
*   **Symbol**: `Transport::setKeyboardRGBBacklight(unsigned char, unsigned char, unsigned char)` at `0x16410`.
*   **Disassembly**:
    ```assembly
    16460:  lea    0x9885(%rip),%rsi        # Loads string: "CRT\x00\x00COLOR\x00\x00\x00" (13 bytes)
    16467:  mov    $0xd,%edx                # Size = 13
    1646c:  mov    %rbp,%rdi
    1646f:  call   TransportByteBuffer::TransportByteBuffer(...)
    
    16474:  mov    $0xa,%esi                # Offset 10
    1647c:  call   TransportByteBuffer::operator[]
    16481:  mov    %r13b,(%rax)             # Writes Red (r)
    
    16484:  mov    $0xb,%esi                # Offset 11
    1648c:  call   TransportByteBuffer::operator[]
    16491:  mov    %r12b,(%rax)             # Writes Green (g)
    
    16494:  mov    $0xc,%esi                # Offset 12
    16499:  call   TransportByteBuffer::operator[]
    164a8:  mov    %r14b,(%rax)             # Writes Blue (b)
    ```
*   **Critical Bugs Confirmed**:
    1.  **Omits Blue Channel**: The K1 Pro device hardware firmware expects the `COLOR` report payload to be structured as:
        *   `Offset 10`: Brightness / Flag
        *   `Offset 11`: Red (0–255)
        *   `Offset 12`: Green (0–255)
        *   `Offset 13`: Blue (0–255)
        Because the FFI function only builds a 13-byte buffer and writes Red to `10`, Green to `11`, and Blue to `12`, the payload is misaligned, and the Blue channel (which should be at offset 13) is truncated/not sent.
    2.  **Color Mixing Mismatch**: The hardware interprets offset 10 as brightness, shifting Red to 11 and Green to 12. Consequently, writing Red to offset 10 controls brightness, and Blue is never illuminated. This explains why the FFI library was incapable of displaying custom/saturated colors like purple or cyan.

### B. Keyboard Backlight Brightness (`LLUM` Command)
*   **Symbol**: `Transport::setKeyboardBacklightBrightness(unsigned char)` at `0x15ff0`.
*   **Disassembly**:
    ```assembly
    16038:  lea    0x9c95(%rip),%rsi        # Loads string: "CRT\x00\x00LLUM\x00\x00" (11 bytes)
    1603f:  mov    $0xb,%edx                # Size = 11
    16047:  call   TransportByteBuffer::TransportByteBuffer(...)
    16059:  cmp    $0x6,%bl                 # Clamping check: param vs 6
    1605c:  mov    $0x6,%esi
    16065:  cmova  %esi,%ebx                # If > 6, clamp to 6
    16068:  mov    %bl,(%rax)               # Writes to offset 10
    ```
*   **Note**: Brightness range is limited strictly to `0–6`.

### C. Keyboard Lighting Effects & Speed (`LMOD` Command)
*   **Symbols**: `Transport::setKeyboardLightingEffects(unsigned char)` at `0x16150` and `Transport::setKeyboardLightingSpeed(unsigned char)` at `0x162b0`.
*   **Disassembly Details**:
    *   Both functions load the header `CRT\x00\x00LMOD\x00\x00` (11 bytes) from offset `0x1fce0`.
    *   Both functions write their parameter byte to offset 10 (`0x0a`).
    *   `setKeyboardLightingEffects` clamps the value to `9` using `cmova`.
    *   `setKeyboardLightingSpeed` clamps the value to `7` using `cmova`.
*   **Note**: Because both functions write to the same byte offset in the `LMOD` buffer, they cannot be configured independently in a single command, indicating they represent mutually exclusive registers or overwrite state inside the device.

### D. Change Mode (`MOD` Command)
*   **Symbol**: `Transport::changeMode(unsigned char)` at `0x18950`.
*   **Disassembly Details**:
    *   Loads header `CRT\x00\x00MOD` (8 bytes) from `0x1fd98`.
    *   Adds `0x31` (ASCII `'1'`) to the mode byte.
    *   Accesses offset 10 (`0x0a`), triggering `TransportByteBuffer` to automatically grow the buffer to 11 bytes.
    *   Writes `mode + 0x31` to offset 10.
    *   **Resulting Packet**: `CRT\x00\x00MOD\x00\x00<ASCII_mode>` (11 bytes).

### E. Change Page (`M_V` Command)
*   **Symbol**: `Transport::changePage(unsigned char)` at `0x18a60`.
*   **Disassembly Details**:
    *   Loads header `M_V\x00` (4 bytes) from `0x1fda1`.
    *   Writes the raw page number directly to offset 3 (`0x03`).
    *   **Resulting Packet**: `M_V<page_byte>` (4 bytes).

### F. Get Firmware Version
*   **Symbol**: `Transport::getFirmwareVesion[abi:cxx11](unsigned long)` at `0x139c0`.
*   **Disassembly Details**:
    *   Invokes `TransportDevice::open()`.
    *   Resolves the report ID by calling `Transport::reportID()`.
    *   Writes the report ID byte to the first byte of a temporary string buffer.
    *   Calls `hid_get_input_report` with the device handle, the buffer, and the input report size (obtained from offset `0x98` of `TransportDevice`).
    *   Returns the resulting version string populated by the device.

---

## 5. Image/Video Streaming & Touchscreen Framebuffer Protocol

Streaming static icons, background frames (GIFs/MP4s), and device skins (such as calculator/keyboard layouts on the StreamDock N1) uses a multi-stage chunked transport protocol. 

### A. Key Image Streaming (`CRT\x00\x00BAT` Command)
*   **Symbol**: `Transport::setKeyBitmap(std::string const&, unsigned char)` at `0x16e40` (called via `setKeyImgFileStream`).
*   **Protocol Structure**:
    1.  **Metadata Packet** (13 bytes):
        *   `Offsets 0–7`: ASCII Header `"CRT\x00\x00BAT\x00"` (8 bytes).
        *   `Offsets 8–11`: 32-bit big-endian length of the image data payload (e.g., JPEG or PNG size).
        *   `Offset 12`: Target hardware key index (1 byte).
        *   Sent via `TransportSession::transact`.
    2.  **Payload Streaming**:
        *   The C++ library breaks the image data payload into chunks of `chunkSize` (1024 or 1025 bytes, depending on device configuration) and streams the raw bytes sequentially over the transport without any added packet headers.
        *   After all chunks are transmitted, a `transport_refresh` (`STP`) command is issued to commit the frame.

### B. Background Frame Streaming (`CRT\x00\x00BGPIC` Command)
*   **Symbol**: `Transport::setBackgroundFrameStream(std::string const&, uint16_t, uint16_t, uint16_t, uint16_t, uint8_t)` at `0x17d20`.
*   **Protocol Structure**:
    1.  **Metadata Packet** (24 bytes):
        *   `Offsets 0–9`: ASCII Header `"CRT\x00\x00BGPIC"` (10 bytes).
        *   `Offsets 10–13`: 32-bit big-endian length of the JPEG image data payload.
        *   `Offsets 14–15`: 16-bit big-endian width of the frame.
        *   `Offsets 16–17`: 16-bit big-endian height of the frame.
        *   `Offsets 18–19`: 16-bit big-endian X-coordinate offset.
        *   `Offsets 20–21`: 16-bit big-endian Y-coordinate offset.
        *   `Offset 22`: Padding / Reserved (`0x00`).
        *   `Offset 23`: Framebuffer layer index (e.g. `0x00`–`0x03`).
        *   Sent via `TransportSession::transact`.
    2.  **Payload Streaming**:
        *   The raw JPEG bytes are chunked and streamed using the same dynamic `chunkSize` serialization pattern.

### C. Clear Background Frame Stream (`CRT\x00\x00BGCLE` Command)
*   **Symbol**: `Transport::clearBackgroundFrameStream(unsigned char)` at `0x18220`.
*   **Protocol Structure**:
    *   **Packet** (11 bytes):
        *   `Offsets 0–9`: ASCII Header `"CRT\x00\x00BGCLE"` (10 bytes).
        *   `Offset 10`: Framebuffer layer index to clear (1 byte).
        *   Sent via `TransportSession::transact` to wipe dynamic layers on the touchscreen.

### D. N1 Skin Bitmap Control (`CRT\x00\xffLOG` Command)
*   **Symbol**: `Transport::setN1SkinBitmap(std::string const&, unsigned char, unsigned char, unsigned char, unsigned char, int)` at `0x18b70`.
*   **Protocol Structure**:
    1.  **Metadata Packet** (16 bytes):
        *   `Offsets 0–7`: ASCII Header `"CRT\x00\xffLOG"` (8 bytes; note that index 4 is explicitly set to `0xff`).
        *   `Offsets 8–11`: 32-bit big-endian length of the PNG data.
        *   `Offset 12`: Skin mode (1 byte; e.g. `0x11` for Keyboard, `0x1F` for Locked, `0xFF` for Calculator).
        *   `Offset 13`: Skin page index (1 byte, `1`–`5`).
        *   `Offset 14`: Skin status (1 byte; `0` for press, `1` for release).
        *   `Offset 15`: Target key index (1 byte; calculator range `1`–`18`, keyboard `1`–`15`).
        *   Sent via `TransportSession::transact`.
    2.  **Payload Streaming**:
        *   The PNG payload is chunked and sent using `chunkSize`.

---

## 6. SDK Animated GIF & Video Playback Architecture

The StreamDock Python SDK does not process animations on the hardware level. Instead, the host PC decodes and pushes pre-rendered frames sequentially.

### A. Frame Pre-Processing and Formatting
When a GIF or MP4 is loaded for playback on an LCD key or the touchscreen:
1.  **GIF Frame Extraction**: PIL `ImageSequence.Iterator` extracts each frame.
2.  **Delay Normalization**: Frame delay is extracted from the image metadata (`frame.info.get("duration")`) or falls back to 100ms.
3.  **Rotation & Resizing**: The frames are processed using the device's native layout:
    *   **K1 Pro Keys**: Rotated by `-90` degrees and scaled to `64x64`.
    *   **K1 Pro Touchscreen**: Rotated by `180` degrees and scaled to `800x480`.
4.  **Composition & Encoding**: Frames are pasted onto a black background (`RGBA` to `RGB`) and saved/cached in memory as raw JPEG bytes (`quality=80`).
5.  **Video Stream Decoding**: For MP4 files, the SDK loads OpenCV (`cv2.VideoCapture`), reads frames dynamically, converts `BGR` to `RGB`, and runs the same JPEG encoding on the fly.

### B. High-Precision Playback Thread
The animation loop (`GifController::_gif_work_loop`) runs on a daemon background thread:
1.  **Time Tracking**: Tracks elapsed duration using high-precision timers (`time.monotonic()`).
2.  **Frame Evaluation**: Iterates over all registered keys and background animations. If the elapsed milliseconds exceed the current frame's delay, the frame counter increments, and the cached JPEG data is queued.
3.  **Streaming & Commit**:
    *   For background layers, the thread invokes `transport_set_background_frame_stream`.
    *   For key icons, it invokes `transport_set_key_image_stream`.
    *   After rendering all updated frames for a given tick, it fires a synchronous `transport_refresh` (`STP`) call to update the physical screen.
4.  **Yielding**: Sleeps for 3ms between ticks to prevent thread starvation.

---

## 7. Exhaustive FFI Export Symbol Reference

The dynamic library exports exactly **66 symbols** (42 transport-specific control wrapper functions and 24 lower-level hidapi functions).

### A. Transport Control FFI Interface (`transport_*`)

| Exported C Function | Return Type | C Arguments | Description / Internal Action |
| :--- | :--- | :--- | :--- |
| `transport_create` | `uint32_t` | `const hid_device_info* dev, TransportHandle* out` | Instantiates `Transport` and opens the device context. |
| `transport_destroy` | `uint32_t` | `TransportHandle handle` | Closes device handle and deletes the `Transport` object. |
| `transport_set_reportSize` | `uint32_t` | `TransportHandle handle, uint16_t in, uint16_t out, uint16_t feat` | Sets report receive, chunk, and feature sizes on the device. |
| `transport_set_reportID` | `uint32_t` | `TransportHandle handle, uint8_t report_id` | Configures the default report ID inside `TransportSession`. |
| `transport_reportID` | `uint32_t` | `TransportHandle handle, uint8_t* out_id` | Retrieves the current report ID. |
| `transport_get_firmware_version` | `uint32_t` | `TransportHandle handle, char* buf, size_t len` | Reads the firmware version string from the device. |
| `transport_clear_task_queue` | `uint32_t` | `TransportHandle handle` | Clears all pending operations from the transaction queue. |
| `transport_can_write` | `uint32_t` | `TransportHandle handle, int* out_val` | Returns whether the transaction pool/queue is writable. |
| `transport_read` | `uint32_t` | `TransportHandle handle, uint8_t* buf, size_t* len, int32_t timeout` | Reads incoming reports/events (keys/dials) from the device. |
| `transport_wakeup_screen` | `uint32_t` | `TransportHandle handle` | Wakes the LCD touchscreen from standby (`DIS` packet). |
| `transport_sleep` | `uint32_t` | `TransportHandle handle` | Puts the LCD touchscreen into standby (`HAN` packet). |
| `transport_magnetic_calibration` | `uint32_t` | `TransportHandle handle` | Calibrates physical dials/magnetic rotary encoders on the device. |
| `transport_set_key_brightness` | `uint32_t` | `TransportHandle handle, uint8_t pct` | Sets key LCD brightness (clamped 0–100; `LIG` packet). |
| `transport_clear_all_keys` | `uint32_t` | `TransportHandle handle` | Clears images on all LCD keys (`CLE \xff` packet). |
| `transport_clear_key` | `uint32_t` | `TransportHandle handle, uint8_t key_idx` | Clears an image on a specific LCD key (`CLE` packet). |
| `transport_refresh` | `uint32_t` | `TransportHandle handle` | Commits framebuffers / refreshes the LCD (`STP` packet). |
| `transport_disconnected` | `uint32_t` | `TransportHandle handle` | Notifies the transport stack of a disconnection event. |
| `transport_heartbeat` | `uint32_t` | `TransportHandle handle` | Sends keep-alive heartbeat (`CONNECT` packet). |
| `transport_set_background_bitmap` | `uint32_t` | `TransportHandle handle, const char* data, size_t len, uint32_t timeout` | Streams full-screen raw bitmap background (`CRT\x00\x00LOG`). |
| `transport_set_key_image_stream` | `uint32_t` | `TransportHandle handle, const char* jpeg, size_t len, uint8_t key_idx` | Streams key image payload (`CRT\x00\x00BAT`). |
| `transport_set_background_image_stream`| `uint32_t` | `TransportHandle handle, const char* jpeg, size_t len, uint32_t timeout` | Streams JPEG touchscreen background (`CRT\x00\x00LOG`). |
| `transport_set_background_frame_stream`| `uint32_t` | `TransportHandle handle, const char* jpeg, size_t len, uint16_t w, uint16_t h, uint16_t x, uint16_t y, uint8_t layer` | Streams a JPEG frame to a layered coordinate (`CRT\x00\x00BGPIC`). |
| `transport_clear_background_frame_stream`| `uint32_t` | `TransportHandle handle, uint8_t layer` | Wipes a layered background coordinate (`CRT\x00\x00BGCLE`). |
| `transport_set_led_brightness` | `uint32_t` | `TransportHandle handle, uint8_t val` | Sets edge LED strip brightness (`LBLIG` packet). |
| `transport_set_led_color` | `uint32_t` | `TransportHandle handle, uint16_t count, uint8_t r, uint8_t g, uint8_t b` | Sets global edge LED color for N LEDs (`SETLB` packet). |
| `transport_set_single_led_color` | `uint32_t` | `TransportHandle handle, uint16_t count, const uint8_t (*colors)[3]` | Sets individual edge LED colors (`SETLB` packet). |
| `transport_reset_led_color` | `uint32_t` | `TransportHandle handle` | Clears/resets edge LED strip colors (`DELED` packet). |
| `transport_set_device_config` | `uint32_t` | `TransportHandle handle, const uint8_t* data, size_t len` | Transmits generic hardware configuration payload. |
| `transport_change_mode` | `uint32_t` | `TransportHandle handle, uint8_t mode` | Switches operation mode (`MOD` packet). |
| `transport_change_page` | `uint32_t` | `TransportHandle handle, uint8_t page` | Switches page on the touchscreen (`M_V` packet). |
| `transport_set_n1_skin_bitmap` | `uint32_t` | `TransportHandle handle, const char* png, size_t len, uint8_t mode, uint8_t page, uint8_t status, uint8_t key_idx, int32_t timeout` | Uploads calculator/keyboard layout skin PNG for N1 (`CRT\x00\xffLOG`). |
| `transport_raw_hid_last_error` | `uint32_t` | `TransportHandle handle, void* out, size_t* len` | Queries errors from the underlying `hidapi` connection. |
| `transport_disable_output` | `uint32_t` | `int8_t disable` | Toggles dynamic printing options in the FFI layer. |
| `transport_set_keyboard_backlight_brightness`| `uint32_t` | `TransportHandle handle, uint8_t val` | Sets keyboard LED backlight brightness (0–6; `LLUM` packet). |
| `transport_set_keyboard_lighting_effects`| `uint32_t` | `TransportHandle handle, uint8_t effect` | Sets keyboard animation effect (0–9; `LMOD` packet). |
| `transport_set_keyboard_lighting_speed`| `uint32_t` | `TransportHandle handle, uint8_t speed` | Sets keyboard animation speed (0–7; `LMOD` packet). |
| `transport_set_keyboard_rgb_backlight` | `uint32_t` | `TransportHandle handle, uint8_t r, uint8_t g, uint8_t b` | Sets keyboard RGB backlight color (`COLOR` packet). |
| `transport_keyboard_os_mode_switch` | `uint32_t` | `TransportHandle handle, uint8_t mode` | Switches the keyboard's host operating system profile. |
| `transport_get_last_error_info` | `uint32_t` | `TransportHandle handle, void* out_err` | Copies the last transport library error details to output. |
| `transport_hid_enumerate` | `hid_device_info*` | `uint16_t vid, uint16_t pid` | Helper wrapping `hid_enumerate` to list connected hardware. |
| `transport_hid_free_enumeration` | `void` | `hid_device_info* dev_list` | Helper wrapping `hid_free_enumeration`. |
| `transport_set_thread_error` | `uint32_t` | `TransportHandle handle, void* err` | Manually flags errors inside the thread context. |

### B. Bundled HIDAPI Symbols (`hid_*`)

The dynamic library statically compiles `hidapi`, exporting standard USB/Bluetooth interface wrappers:

| Symbol Name | Return Type | Arguments |
| :--- | :--- | :--- |
| `hid_init` / `hid_exit` | `int` | `void` |
| `hid_version` / `hid_version_str` | `const hid_api_version*` / `const char*` | `void` |
| `hid_enumerate` | `hid_device_info*` | `unsigned short vendor_id, unsigned short product_id` |
| `hid_free_enumeration` | `void` | `struct hid_device_info* devs` |
| `hid_open` | `hid_device*` | `unsigned short vendor_id, unsigned short product_id, const wchar_t* serial` |
| `hid_open_path` | `hid_device*` | `const char* path` |
| `hid_write` | `int` | `hid_device* dev, const unsigned char* data, size_t length` |
| `hid_read_timeout` | `int` | `hid_device* dev, unsigned char* data, size_t length, int milliseconds` |
| `hid_read` | `int` | `hid_device* dev, unsigned char* data, size_t length` |
| `hid_set_nonblocking` | `int` | `hid_device* dev, int nonblock` |
| `hid_send_feature_report` | `int` | `hid_device* dev, const unsigned char* data, size_t length` |
| `hid_get_feature_report` | `int` | `hid_device* dev, unsigned char* data, size_t length` |
| `hid_get_input_report` | `int` | `hid_device* dev, unsigned char* data, size_t length` |
| `hid_close` | `void` | `hid_device* dev` |
| `hid_get_manufacturer_string` | `int` | `hid_device* dev, wchar_t* string, size_t maxlen` |
| `hid_get_product_string` | `int` | `hid_device* dev, wchar_t* string, size_t maxlen` |
| `hid_get_serial_number_string` | `int` | `hid_device* dev, wchar_t* string, size_t maxlen` |
| `hid_get_indexed_string` | `int` | `hid_device* dev, int string_index, wchar_t* string, size_t maxlen` |
| `hid_get_device_info` | `hid_device_info*`| `hid_device* dev` |
| `hid_get_report_descriptor` | `int` | `hid_device* dev, unsigned char* buf, size_t maxlen` |
| hid_error | const wchar_t* | hid_device* dev |
| hid_read_error | int | hid_device* dev, unsigned char* data, size_t length |
| hid_send_output_report | int | hid_device* dev, const unsigned char* data, size_t length |

---

## 8. Page Configuration and Switching Protocols

Page setup and transitions within the StreamDock SDK fall into two categories: host-controlled active page switching (used on devices like the StreamDock N1) and internal firmware-level page state machines (used on the K1 Pro in standalone mode).

### A. Host-Controlled Active Page Switching (`M_V` Command)
Devices with multi-screen or calculator profiles (such as the StreamDock N1) support direct page transitions triggered by the host:
*   **Symbol**: `Transport::changePage(unsigned char)` at `0x18a60`.
*   **Protocol Packet** (4 bytes):
    *   `Offsets 0–2`: ASCII Command `"M_V"`
    *   `Offset 3`: Raw 8-bit page index (e.g., `0`–`4` corresponding to pages 1–5).
*   **Usage**: The host sends this packet via the FFI function `transport_change_page` to switch active layouts displayed on the LCD screens.

### B. Skin Page Layout Uploads (`CRT\x00\xffLOG` Command)
To support custom key designs on different pages, the transport library allows uploading PNG skin maps associated with specific page indexes:
*   **Symbol**: `Transport::setN1SkinBitmap(std::string const&, unsigned char, unsigned char, unsigned char, unsigned char, int)` at `0x18b70`.
*   **Protocol Packet Header** (16 bytes):
    *   `Offsets 0–7`: `"CRT\x00\xffLOG"`
    *   `Offsets 8–11`: 32-bit big-endian PNG size.
    *   `Offset 12`: Skin mode (`0x11` for keyboard, `0x1F` for locked keyboard, `0xFF` for calculator).
    *   `Offset 13`: Target page index (`1`–`5`).
    *   `Offset 14`: Button status (`0` for press, `1` for release).
    *   `Offset 15`: Hardware key index (`1`–`18` in calculator mode, `1`–`15` in keyboard mode).
*   **Payload**: Statically streams the raw PNG layout in chunks of `chunkSize` immediately following the header.

### C. Standalone Firmware Page State Machine (K1 Pro)
While the K1 Pro does not use active page switching commands in host-controlled mode, its internal firmware executes local page switches when disconnected:
*   **Firmware Pages**:
    *   **Page 0 (`scr: 0`)**: The default home screen page which logs button click events.
    *   **Page 1 / Page 2 (`scr: 1` / `scr: 2`)**: Temporary overlay pages displayed automatically by the firmware during physical knob rotations.
*   **Host Detection**: The OpenDeck driver tracks these page transitions by intercepting incoming `DEVCFG` reports (HID report ID 4) and extracting the page offset. Monitoring transitions back to `Page 0` enables the driver to detect Knob 1 double-clicks to safely exit standalone mode.
