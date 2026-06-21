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

## Standalone Mode (Internal Flash Profiles)

The K1 Pro plugin supports toggling the device between OpenDeck (host-controlled) mode and standalone (internal onboard flash) profiles.

### Quick Guide: Mode Switching
* **Enter Standalone Mode**: Click **Knob 1** (the top-left dial) once.
* **Return to OpenDeck Host Mode**: Click **Knob 1** twice (the first click returns the device to its home page, and the second click triggers the switch back to OpenDeck).

### Detailed Technical Behavior
* **Switching to Standalone Mode**: Press **Knob 1** once. The plugin will deregister the K1 Pro from the OpenDeck host, allowing the device to run profiles stored in its internal flash memory.
* **Switching Back to Host Mode**: Click **Knob 1** to reconnect the device back to OpenDeck.
  * **Firmware Hardcoded Behavior**: Once in standalone mode, the device's firmware requires the first click of Knob 1 to switch to the home page; the subsequent click then triggers the status report that reconnects the device back to host mode.
  * **Cooldown Behavior**: To prevent toggle loops and event races, there is a 1.5-second cooldown enforced immediately after entering standalone mode. If the user clicks Knob 1 immediately after entering standalone mode, they must wait at least a second and click it again to trigger the reconnection back to host mode.
  * Standard keypresses (e.g. key grid buttons) and knob turns are ignored for reconnection.

## Scenes, Pages, and Profile Switching

OpenDeck organizes layouts hierarchically into **Profiles**, **Scenes**, and **Pages**.
* **Profile**: A full layout file (e.g. `Default.json` or `page1.json`).
* **Scene**: A directory grouping multiple pages (e.g. `/scene1/` folder).
* **Page**: A single profile JSON file residing inside a scene directory (e.g. `/scene1/page1.json`).

### Configuration Directory Structure
Profiles are stored under `~/.config/opendeck/profiles/<device-id>/`:
```text
~/.config/opendeck/profiles/<device-id>/
├── Default.json         # The root "Default" profile
├── scene1/              # "scene1" Scene folder
│   ├── page1.json       # "page1" Page
│   └── page2.json       # "page2" Page
└── scene2/              # "scene2" Scene folder
    └── page1.json       # "page1" Page
```

### How to Configure Knob-Based Switching in the UI
1. **Open the OpenDeck Dashboard**: Open the OpenDeck UI in your browser or application window.
2. **Select the Device**: Choose your StreamDock K1 Pro from the device dropdown.
3. **Assign the "Switch Profile" Action**:
   * Navigate to the **Dials / Encoders** tab.
   * Drag the **Switch Profile** action (provided by the Starter Pack plugin) onto the dial/knob you want to map (e.g., **Knob 1**).
4. **Configure the Target Profiles**:
   * Click on the mapped dial to open its settings panel.
   * Under the **Clockwise** setting, select or type the target page (e.g., `scene1/page1`).
   * Under the **Anti-clockwise** setting, select or type the fallback page (e.g., `Default` to return to the root profile).
5. **Add Actions to Other Pages**:
   * Switch to the new page (e.g., `scene1/page1`) via the UI dropdown.
   * Map the dial on that page to switch to another page (e.g., clockwise to `scene2/page1`, anti-clockwise to `Default`) to build a complete navigation loop.

## Author

* **Rodrigo Leven** - <rodrigo.leven@gmail.com>

