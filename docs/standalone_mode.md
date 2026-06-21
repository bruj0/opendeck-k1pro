# K1 Pro Standalone Mode Implementation

This document describes the design, implementation details, and hardware constraints resolved to support switching the StreamDock K1 Pro between OpenDeck (host-controlled) mode and the device's internal flash (standalone) profiles.

---

## 1. Hardware Interface Architecture

The StreamDock K1 Pro exposes two primary USB HID interfaces to the operating system:

| Interface | Default Linux Path (Example) | Purpose |
| :--- | :--- | :--- |
| **Interface 0** | `/dev/hidraw4` | Standard boot-protocol keyboard, system controls, and consumer/media control reports. |
| **Interface 1** | `/dev/hidraw5` | Custom vendor communication channel used for host control (handshake, screen streaming, button images, brightness, OS mode switches). |

### Host-Controlled Mode (OpenDeck)
* Communication occurs entirely on **Interface 1** using report ID `0x04` and report size `513`.
* The plugin receives raw events (button clicks, knob presses, and encoder rotations) and translates them into OpenDeck JSON messages via WebSockets.

### Standalone Mode (Internal Flash Profiles)
* The host-control mode is deactivated by calling the SDK FFI function `transport_disconnected`.
* The device resumes displaying and executing profiles stored in its onboard flash memory.
* Interactive actions (button presses and knob clicks) generate standard HID keycode reports on **Interface 0**.

---

## 2. Limitations of the SDK FFI

During implementation, we identified a critical limitation in the vendor SDK's `transport_read` FFI function:
* The SDK's read function is configured specifically for vendor host-controlled packets (expecting report ID `0x04`).
* When the device is in standalone mode, standard boot-protocol reports (such as a standard key press event) sent on **Interface 0** do not match the expected report ID or size.
* Consequently, calling `transport_read` on Interface 0 always returns a read timeout error (`0x05000302`), failing to register any user activity.

---

## 3. Design and Non-Blocking Implementation

To bypass FFI limitations and prevent system hangs, we implemented direct raw I/O reading with thread-safe transition cooldowns.

```mermaid
flowchart TD
    A[Host Mode] -->|Knob 1 Click| B[Deregister from OpenDeck]
    B --> C[Send transport_disconnected]
    C --> D[Enter Standalone Mode]
    D -->|Wait for 1.5s Cooldown| E{Monitor Interface 1}
    E -->|DEVCFG scr=0 AND last_non_zero_scr > 1.5s| F[Trigger Reconnect]
    E -->|DEVCFG scr > 0| G[Update last_non_zero_scr]
    F --> H[Perform Handshake]
    H --> I[Register back with OpenDeck]
    I --> A
```

### A. Non-Blocking Raw Reading to Prevent Thread-Pool Exhaustion
Because Linux character devices (like `/dev/hidraw`) do not support epoll-based asynchronous notifications, invoking standard blocking reads inside a Tokio task spawns blocking OS threads (`spawn_blocking`). When multiple timeouts occur, the pool exhausts quickly, hanging the plugin.

We resolved this by:
1. Opening `/dev/hidraw` devices directly as synchronous files with the `libc::O_NONBLOCK` flag.
2. Checking for data using standard `Read::read`.
3. If no data is available, handling `ErrorKind::WouldBlock` by sleeping asynchronously for `100ms` (for Interface 1) or `5ms` (for Interface 0), yielding execution back to Tokio.

### B. Capture of USB Paths in Watcher
The watcher task (`watcher.rs`) was updated to capture both interface paths:
* `path_int1` is extracted from the primary usage-page 65440 match.
* `path_int0` is captured by scanning for the matching serial number on `interface_number == 0`.

### C. Double-Transition Cooldowns
To prevent infinite toggle loops (e.g., when the user releases Knob 1 after switching, generating a trailing key release event), we added mutex-protected timestamp fields to the `Device` struct:
* `last_host_transition`: Cooldown window (1 second) to ignore Knob 1 click events immediately after entering host mode.
* `last_standalone_transition`: Cooldown window (1.5 seconds) to ignore reconnect attempts immediately after entering standalone mode.

### D. Page-Based Turn-vs-Click State Machine
In standalone mode, K1 Pro profiles use different page screens for button keycodes and encoder rotations:
* **Page 0 (`scr: 0`)**: The default active page containing click operations.
* **Page 1 (`scr: 1`) or Page 2 (`scr: 2`)**: The temporary overlay pages transitioned to during knob rotation.

To ensure that knob turns do not trigger reconnection back to host-controlled mode, while correctly detecting a Knob 1 click:
1. The plugin tracks the last time a non-zero screen (`scr > 0`) was active using the `last_non_zero_scr` timestamp in the `Device` struct.
2. When a `DEVCFG` report with `scr > 0` is read on Interface 1, `last_non_zero_scr` is updated to the current time.
3. When a `DEVCFG` report with `scr: 0` is read:
   - We check if the elapsed time since entering standalone mode (`last_standalone_transition`) is at least 1.5 seconds.
   - We check if the elapsed time since the last non-zero screen overlay (`last_non_zero_scr`) is at least 1.5 seconds.
   - If both conditions are met, it is recognized as a Knob 1 click, and the plugin reconnects. Otherwise (if the non-zero overlay screen recently faded, transitioning back to `scr: 0`), the event is ignored.
4. All Interface 0 standard keyboard activity (such as standard keystrokes) is ignored for reconnection purposes, and the logs are suppressed at `trace` level to eliminate log spam.

#### Reconnection Click Behavior (Firmware Hardcoded Limitation)
Due to a combination of device firmware behavior and a 1.5-second transition cooldown:
* **Firmware Behavior**: Once in standalone mode, the device's firmware requires the first click on Knob 1 to transition back to the home page. The subsequent click then sends the status report necessary for the plugin to detect the reconnection request.
* **Cooldown Protection**: To prevent loop races, the plugin enforces a 1.5-second cooldown after entering standalone mode. If Knob 1 is clicked immediately after entering, the user must wait at least 1.0 second and click it again to trigger reconnection.

### E. Re-registration Protocol
When a reconnection condition is met:
1. The raw file handles are closed to release the hidraw devices.
2. The standard connection handshake is run on the primary FFI handle.
3. The plugin sends a `deregister_device` and subsequent `register_device` event via the WebSocket client to the OpenDeck core, forcing a refresh/redrawing of all screen layouts.
