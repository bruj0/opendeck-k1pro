use libloading::{Library, Symbol};
use std::path::Path;
use std::sync::Arc;

#[repr(C)]
#[derive(Debug)]
pub struct HidDeviceInfo {
    pub path: *const libc::c_char,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial_number: *const libc::wchar_t,
    pub release_number: u16,
    pub manufacturer_string: *const libc::wchar_t,
    pub product_string: *const libc::wchar_t,
    pub usage_page: u16,
    pub usage: u16,
    pub interface_number: libc::c_int,
    pub next: *mut HidDeviceInfo,
}

#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TransportHandle(pub *mut libc::c_void);

unsafe impl Send for TransportHandle {}
unsafe impl Sync for TransportHandle {}

#[allow(non_snake_case)]
pub struct TransportLib {
    _lib: Library,
    pub transport_hid_enumerate: Symbol<'static, unsafe extern "C" fn(u16, u16) -> *mut HidDeviceInfo>,
    pub transport_hid_free_enumeration: Symbol<'static, unsafe extern "C" fn(*mut HidDeviceInfo)>,
    pub transport_create: Symbol<'static, unsafe extern "C" fn(*const HidDeviceInfo, *mut TransportHandle) -> u32>,
    pub transport_destroy: Symbol<'static, unsafe extern "C" fn(TransportHandle) -> u32>,
    pub transport_set_reportSize: Symbol<'static, unsafe extern "C" fn(TransportHandle, u16, u16, u16) -> u32>,
    pub transport_set_reportID: Symbol<'static, unsafe extern "C" fn(TransportHandle, u8) -> u32>,
    pub transport_get_firmware_version: Symbol<'static, unsafe extern "C" fn(TransportHandle, *mut libc::c_char, usize) -> u32>,
    pub transport_set_key_brightness: Symbol<'static, unsafe extern "C" fn(TransportHandle, u8) -> u32>,
    pub transport_set_keyboard_backlight_brightness: Symbol<'static, unsafe extern "C" fn(TransportHandle, u8) -> u32>,
    pub transport_set_keyboard_rgb_backlight: Symbol<'static, unsafe extern "C" fn(TransportHandle, u8, u8, u8) -> u32>,
    pub transport_set_key_image_stream: Symbol<'static, unsafe extern "C" fn(TransportHandle, *const libc::c_char, usize, u8) -> u32>,
    pub transport_clear_all_keys: Symbol<'static, unsafe extern "C" fn(TransportHandle) -> u32>,
    pub fn_transport_clear_key: Symbol<'static, unsafe extern "C" fn(TransportHandle, u8) -> u32>,
    pub transport_refresh: Symbol<'static, unsafe extern "C" fn(TransportHandle) -> u32>,
    pub transport_disconnected: Symbol<'static, unsafe extern "C" fn(TransportHandle) -> u32>,
    pub transport_heartbeat: Symbol<'static, unsafe extern "C" fn(TransportHandle) -> u32>,
    pub transport_read: Symbol<'static, unsafe extern "C" fn(TransportHandle, *mut u8, *mut usize, i32) -> u32>,
}

impl TransportLib {
    #[allow(non_snake_case)]
    pub unsafe fn load(path: impl AsRef<Path>) -> Result<Arc<Self>, Box<dyn std::error::Error + Send + Sync>> {
        let lib = unsafe { Library::new(path.as_ref())? };

        // Helper macro to load a symbol and transmute its lifetime to 'static
        macro_rules! load_sym {
            ($name:expr, $type:ty) => {{
                let sym: Symbol<$type> = unsafe { lib.get($name)? };
                unsafe { std::mem::transmute::<Symbol<'_, $type>, Symbol<'static, $type>>(sym) }
            }};
        }

        let transport_hid_enumerate = load_sym!(b"transport_hid_enumerate\0", unsafe extern "C" fn(u16, u16) -> *mut HidDeviceInfo);
        let transport_hid_free_enumeration = load_sym!(b"transport_hid_free_enumeration\0", unsafe extern "C" fn(*mut HidDeviceInfo));
        let transport_create = load_sym!(b"transport_create\0", unsafe extern "C" fn(*const HidDeviceInfo, *mut TransportHandle) -> u32);
        let transport_destroy = load_sym!(b"transport_destroy\0", unsafe extern "C" fn(TransportHandle) -> u32);
        let transport_set_reportSize = load_sym!(b"transport_set_reportSize\0", unsafe extern "C" fn(TransportHandle, u16, u16, u16) -> u32);
        let transport_set_reportID = load_sym!(b"transport_set_reportID\0", unsafe extern "C" fn(TransportHandle, u8) -> u32);
        let transport_get_firmware_version = load_sym!(b"transport_get_firmware_version\0", unsafe extern "C" fn(TransportHandle, *mut libc::c_char, usize) -> u32);
        let transport_set_key_brightness = load_sym!(b"transport_set_key_brightness\0", unsafe extern "C" fn(TransportHandle, u8) -> u32);
        let transport_set_keyboard_backlight_brightness = load_sym!(b"transport_set_keyboard_backlight_brightness\0", unsafe extern "C" fn(TransportHandle, u8) -> u32);
        let transport_set_keyboard_rgb_backlight = load_sym!(b"transport_set_keyboard_rgb_backlight\0", unsafe extern "C" fn(TransportHandle, u8, u8, u8) -> u32);
        let transport_set_key_image_stream = load_sym!(b"transport_set_key_image_stream\0", unsafe extern "C" fn(TransportHandle, *const libc::c_char, usize, u8) -> u32);
        let transport_clear_all_keys = load_sym!(b"transport_clear_all_keys\0", unsafe extern "C" fn(TransportHandle) -> u32);
        let fn_transport_clear_key = load_sym!(b"transport_clear_key\0", unsafe extern "C" fn(TransportHandle, u8) -> u32);
        let transport_refresh = load_sym!(b"transport_refresh\0", unsafe extern "C" fn(TransportHandle) -> u32);
        let transport_disconnected = load_sym!(b"transport_disconnected\0", unsafe extern "C" fn(TransportHandle) -> u32);
        let transport_heartbeat = load_sym!(b"transport_heartbeat\0", unsafe extern "C" fn(TransportHandle) -> u32);
        let transport_read = load_sym!(b"transport_read\0", unsafe extern "C" fn(TransportHandle, *mut u8, *mut usize, i32) -> u32);

        Ok(Arc::new(Self {
            _lib: lib,
            transport_hid_enumerate,
            transport_hid_free_enumeration,
            transport_create,
            transport_destroy,
            transport_set_reportSize,
            transport_set_reportID,
            transport_get_firmware_version,
            transport_set_key_brightness,
            transport_set_keyboard_backlight_brightness,
            transport_set_keyboard_rgb_backlight,
            transport_set_key_image_stream,
            transport_clear_all_keys,
            fn_transport_clear_key,
            transport_refresh,
            transport_disconnected,
            transport_heartbeat,
            transport_read,
        }))
    }
}
