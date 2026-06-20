# AI Agent Contribution Guidelines

This document provides context, conventions, and architectural constraints for AI agents contributing to `opendeck-k1pro`.

## Project Structure

* **`manifest.json`**: Plugin metadata. Defines `PluginUUID` (`st.lynx.plugins.opendeck-k1pro`) and `DeviceNamespace` (`k1`).
* **`src/transport.rs`**: Dynamic wrapper using `libloading` to bind symbols in `libtransport.so` at runtime.
* **`src/mappings.rs`**: Translates between OpenDeck logical layout coordinates and K1 Pro hardware codes.
* **`src/device.rs`**: Asynchronous read/write loops, keepalives (10s heartbeats), and JPEG/rotation transformation logic.
* **`src/watcher.rs`**: Polling loop verifying device discovery and handling hotplugging transitions.
* **`src/main.rs`**: Entry point orchestrating websocket event handlers via `openaction`.

---

## Architectural Constraints & Rules

### 1. Thread-Safe Futures (Send Boundary)
The async runtime spawns tasks which require all returned futures to implement `Send`. Because the plugin relies on raw pointer interaction with the FFI:
* **No raw pointers in future states**: Raw pointers like `*mut libc::c_void` or `*mut HidDeviceInfo` are not `Send`. If they are held in local variables across `.await` points, the Future becomes `!Send`.
* **Solutions**:
  1. Wrap FFI handles in transparent newtype structs implementing `Send + Sync` (e.g. `TransportHandle`).
  2. Perform all pointer-based FFI logic (traversal, instantiation) synchronously in safe blocks, then return `Send` structures (like `Arc<Device>`) before reaching any async await points.

### 2. C naming and Lint Suppressions
The dynamically loaded symbols from `libtransport.so` use non-standard casing (e.g., `transport_set_reportSize`). 
* Keep these bindings exactly matching the C exports.
* Suppress snake-case warnings on structure fields and functions using `#[allow(non_snake_case)]`.

### 3. Image Transformation
The K1 Pro screens require specific image formats:
* JPEG format.
* Resize exactly to `64x64`.
* Rotate -90 degrees (which is equivalent to `.rotate90()` in the `image` crate).
* Perform this conversion safely and make sure to lock the writing stream to avoid concurrent FFI collisions.

---

## Workflow & Development

1. **Modify code**: Make surgical edits matching existing style.
2. **Build and Check**: Run `cargo check` and `cargo build --release` to ensure 0 errors and 0 warnings.
3. **Local Deploy**: Run `just install` to copy the release artifacts to the OpenDeck plugins directory.
4. **Verification**: Verify the logs under `~/.local/share/opendeck/logs/plugins/st.lynx.plugins.opendeck-k1pro.sdPlugin.log` to confirm the plugin is communicating properly with the hardware.

---

## Profile & Page Switching (Host-Controlled)

The plugin behaves as a stateless input driver and does not contain hardcoded logic for switching scenes or pages:
* **Stateless Event Forwarding**: Rotations and clicks on Knobs 1, 2, and 3 (encoders 0, 1, and 2) are forwarded to the OpenDeck host immediately as standard `encoder_change` and keypress events.
* **UI Mapping**: All scene and page switching logic is configured dynamically by the user via the OpenDeck host dashboard (e.g. mapping Encoder 0/1 to switch profiles/pages). The host intercepts the encoder events and handles profile switching internally.
* **No Local State**: Do not implement custom layout-switching or directory-watching logic inside the plugin.

