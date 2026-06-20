# OpenDeck StreamDock K1 Pro Plugin

A hardware support plugin for the **StreamDock K1 Pro** device in OpenDeck. It bridges the physical device inputs (keys and encoders) and LCD screens with the OpenDeck host application.

## Features

* **Hotplugging & Auto-Discovery**: Automatically detects connection/disconnection of the K1 Pro device.
* **6 LCD Display Keys**: Maps the 2x3 display key grid, handles automated downscaling (to 64x64), applies rotation (-90 degrees), and writes JPEG streams to the screens.
* **3 Rotary Encoders**: Fully maps encoder turn rotations (clockwise/counter-clockwise ticks) and encoder press actions (down/up).
* **Backlight Controls**: Configures the keyboard's ambient RGB lighting (defaulting to Cyan) and supports dynamic brightness adjustment.
* **Dynamic FFI Binding**: Bundles and loads the official precompiled `libtransport.so` dynamically, enabling clean cross-platform setups without system directory pollution.

## Prerequisites & Installation

### 1. Grant USB Permissions (udev rules)
To interact with the USB interface in user-space, copy the udev rules to your system configuration:
```bash
sudo cp 40-opendeck-k1pro.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

### 2. Build and Install
Compile the plugin in release mode and install it to the OpenDeck local plugins directory:
```bash
just install
```
This target:
1. Compiles the binary: `target/release/opendeck-k1pro`
2. Copies it to `~/.config/opendeck/plugins/st.lynx.plugins.opendeck-k1pro.sdPlugin/opendeck-k1pro-linux`
3. Copies `manifest.json`, the `assets/` folder, and the precompiled `libtransport.so` library.

---

## Testing the Plugin

1. **Ensure the udev rules are loaded** and the physical K1 Pro device is plugged in.
2. **Launch OpenDeck in development mode** from its workspace:
   ```bash
   npm run tauri dev
   ```
3. **Verify registration**: Check OpenDeck terminal output to verify registration of the `st.lynx.plugins.opendeck-k1pro.sdPlugin` plugin:
   ```log
   [opendeck::events][DEBUG] Registered plugin st.lynx.plugins.opendeck-k1pro.sdPlugin
   ```
4. **Monitor plugin activity**: Read the plugin logs to confirm connection to the physical hardware and initialization of the Cyan backlight:
   ```bash
   tail -f ~/.local/share/opendeck/logs/plugins/st.lynx.plugins.opendeck-k1pro.sdPlugin.log
   ```
   *Expected output:*
   ```log
   [INFO] Loading dynamic transport library from: "/home/bruj0/.config/opendeck/plugins/st.lynx.plugins.opendeck-k1pro.sdPlugin/libtransport.so"
   [INFO] Plugin initialized and watcher task spawned
   [INFO] Starting K1 Pro device watcher task
   [INFO] Discovered new K1 Pro device: k1-8730DB782625
   [INFO] Registered K1 Pro device. Firmware version: V3.010.03.013
   ```
5. **Interactive Testing**:
   * Assign an action image to any of the 6 display keys in the OpenDeck UI. The hardware key screen should update immediately.
   * Adjust the brightness slider in the OpenDeck settings. The display keys and ambient backlights should adjust accordingly.
   * Press physical keys or turn encoders. Verify that OpenDeck prints received input events.

## Author

* **Rodrigo Leven** - <rodrigo.leven@gmail.com>
