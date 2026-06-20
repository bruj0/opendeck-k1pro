id := "st.lynx.plugins.opendeck-k1pro.sdPlugin"
sdk_lib := "/home/bruj0/projects/k1pro/StreamDock-Device-SDK/CPP-SDK/src/Transport/TransportDLL/libtransport.so"
plugin_dir := "~/.config/opendeck/plugins/" + id

build:
    cargo build --release

install: build
    mkdir -p {{plugin_dir}}
    cp target/release/opendeck-k1pro {{plugin_dir}}/opendeck-k1pro-linux
    cp manifest.json {{plugin_dir}}/manifest.json
    cp -r assets {{plugin_dir}}/
    cp {{sdk_lib}} {{plugin_dir}}/libtransport.so
    @echo "Plugin installed successfully in {{plugin_dir}}"
