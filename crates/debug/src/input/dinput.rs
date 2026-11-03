//! IDirectInputDevice8W::GetDeviceState hook'lari (overlay girdiyi yutarken oyuna sifir durum).
//! ER-Lab 2026-10-08 MinHook standardi: ilhook (`hook_closure_retn`) -> paylasimli MinHook
//! (`mods\bin\MinHook.x64.dll`, dinamik; `mh.rs`). MinHook yoksa / hata: log + hook YOK, panik yok
//! (onceki `expect`/`assert_eq!` yollari hata dondurur). Eski kaynak: _to_delete/20261008_mhstd_pre/.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::core::{GUID, s};

use super::mh;
use crate::hooks::{HookError, is_blocked};
use crate::input::InputFlags;

const DIRECTINPUT_VERSION: u32 = 0x0800;

// {BF798031-483A-4DA2-AA99-5D64ED369700}
const IID_IDIRECTINPUT8W: GUID = GUID::from_values(
    0xbf798031,
    0x483a,
    0x4da2,
    [0xaa, 0x99, 0x5d, 0x64, 0xed, 0x36, 0x97, 0x00],
);
// {6F1D2B61-D5A0-11CF-BFC7-444553540000}
const GUID_SYS_KEYBOARD: GUID = GUID::from_values(
    0x6F1D2B61,
    0xD5A0,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);
// {6F1D2B60-D5A0-11CF-BFC7-444553540000}
const GUID_SYS_MOUSE: GUID = GUID::from_values(
    0x6F1D2B60,
    0xD5A0,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);

const VTBL_RELEASE: usize = 2;
const VTBL_CREATE_DEVICE: usize = 3;
const VTBL_GET_DEVICE_STATE: usize = 9;

type RawObj = *mut *const usize;

type DInput8CreateFn =
    unsafe extern "system" fn(usize, u32, *const GUID, *mut RawObj, usize) -> i32;
type CreateDeviceFn = unsafe extern "system" fn(RawObj, *const GUID, *mut RawObj, usize) -> i32;
type ReleaseFn = unsafe extern "system" fn(RawObj) -> u32;
// IDirectInputDevice8W::GetDeviceState(this, cbData: DWORD, lpvData: LPVOID) -> HRESULT
type GetStateFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> i32;

const E_FAIL: i32 = 0x8000_4005_u32 as i32;

static KB_TRAMP: AtomicUsize = AtomicUsize::new(0);
static MS_TRAMP: AtomicUsize = AtomicUsize::new(0);
static KB_FLAGS: AtomicU8 = AtomicU8::new(0);

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleExW(flags: u32, name: *const u16, out: *mut usize) -> i32;
    fn GetModuleFileNameW(h_module: usize, lp_filename: *mut u16, n_size: u32) -> u32;
}

unsafe fn vtable_fn<F: Copy>(obj: RawObj, slot: usize) -> F {
    unsafe { std::mem::transmute_copy(&*(*obj).add(slot)) }
}

/// Bu DLL'in klasoru (MinHook arama yolu 2: `<klasor>\bin\MinHook.x64.dll`).
fn self_dir() -> Option<std::path::PathBuf> {
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    let mut h = 0usize;
    let anchor = kb_get_state as GetStateFn as *const u16;
    if unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, anchor, &mut h) } == 0 || h == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(h, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])).parent().map(|d| d.to_path_buf())
}

/// Gecici bir DirectInput cihazi acip GetDeviceState adresini okur; hata panik degil Err.
unsafe fn probe_get_state(di8_create: DInput8CreateFn, hinstance: usize, guid: &GUID) -> Result<usize, HookError> {
    let mut di8: RawObj = std::ptr::null_mut();
    let hr = unsafe { di8_create(hinstance, DIRECTINPUT_VERSION, &IID_IDIRECTINPUT8W, &mut di8, 0) };
    if hr != 0 || di8.is_null() {
        return Err(HookError(format!("DirectInput8Create failed: {hr:#010x}")));
    }
    let release_di8: ReleaseFn = unsafe { vtable_fn(di8, VTBL_RELEASE) };
    let create_device: CreateDeviceFn = unsafe { vtable_fn(di8, VTBL_CREATE_DEVICE) };
    let mut device: RawObj = std::ptr::null_mut();
    let hr = unsafe { create_device(di8, guid, &mut device, 0) };
    if hr != 0 || device.is_null() {
        unsafe { release_di8(di8) };
        return Err(HookError(format!("IDirectInput8::CreateDevice({guid:?}) failed: {hr:#010x}")));
    }
    let get_state_addr = unsafe { *(*device).add(VTBL_GET_DEVICE_STATE) };
    let release_device: ReleaseFn = unsafe { vtable_fn(device, VTBL_RELEASE) };
    unsafe { release_device(device) };
    unsafe { release_di8(di8) };
    if get_state_addr == 0 {
        return Err(HookError(format!("GetDeviceState({guid:?}) null")));
    }
    Ok(get_state_addr)
}

#[inline]
unsafe fn get_state(tramp: &AtomicUsize, flags: u8, this: *mut c_void, size: u32, data: *mut c_void) -> i32 {
    let t = tramp.load(Ordering::Acquire);
    if t == 0 {
        return E_FAIL; // olmamali: trampoline enable'dan once yayinlanir
    }
    let original: GetStateFn = unsafe { std::mem::transmute::<usize, GetStateFn>(t) };
    let hr = unsafe { original(this, size, data) };
    if hr == 0 && !data.is_null() && is_blocked(InputFlags::from_bits_retain(flags)) {
        unsafe { std::ptr::write_bytes(data as *mut u8, 0, size as usize) };
    }
    hr
}

unsafe extern "system" fn kb_get_state(this: *mut c_void, size: u32, data: *mut c_void) -> i32 {
    unsafe { get_state(&KB_TRAMP, KB_FLAGS.load(Ordering::Relaxed), this, size, data) }
}

unsafe extern "system" fn ms_get_state(this: *mut c_void, size: u32, data: *mut c_void) -> i32 {
    unsafe { get_state(&MS_TRAMP, InputFlags::Mouse.bits(), this, size, data) }
}

pub(super) unsafe fn install() -> Result<(), HookError> {
    let dinput8 = unsafe { GetModuleHandleA(s!("dinput8.dll")) }
        .map_err(|e| HookError(format!("dinput8.dll not loaded: {e:?}")))?;
    let di8_create: DInput8CreateFn = unsafe {
        std::mem::transmute(
            GetProcAddress(dinput8, s!("DirectInput8Create"))
                .ok_or_else(|| HookError("DirectInput8Create not found".into()))?,
        )
    };
    let hinstance = unsafe { GetModuleHandleA(None) }
        .map_err(|e| HookError(format!("GetModuleHandle failed: {e:?}")))?
        .0 as usize;

    let keyboard_addr = unsafe { probe_get_state(di8_create, hinstance, &GUID_SYS_KEYBOARD)? };
    let mouse_addr = unsafe { probe_get_state(di8_create, hinstance, &GUID_SYS_MOUSE)? };

    let dir = self_dir();
    let log = |m: &str| tracing::info!("{m}");
    let Some(api) = (unsafe { mh::load(dir.as_deref(), "debug/input", &log) }) else {
        return Err(HookError("paylasimli MinHook yok -> GetDeviceState hook YOK".into()));
    };

    // When both devices share the same GetDeviceState implementation, one hook
    // covers both. When they differ each hook handles its own device type only
    let kb_flags = if keyboard_addr == mouse_addr {
        InputFlags::Keyboard | InputFlags::Mouse
    } else {
        InputFlags::Keyboard
    };
    KB_FLAGS.store(kb_flags.bits(), Ordering::Relaxed);

    unsafe { api.install(keyboard_addr, kb_get_state as GetStateFn as usize, &KB_TRAMP) }
        .map_err(|e| HookError(format!("GetDeviceState(kb) {keyboard_addr:#x}: {e}")))?;
    tracing::info!("GetDeviceState(kb) hook {keyboard_addr:#x}: MinHook OK");

    if keyboard_addr != mouse_addr {
        unsafe { api.install(mouse_addr, ms_get_state as GetStateFn as usize, &MS_TRAMP) }
            .map_err(|e| HookError(format!("GetDeviceState(mouse) {mouse_addr:#x}: {e}")))?;
        tracing::info!("GetDeviceState(mouse) hook {mouse_addr:#x}: MinHook OK");
    }
    Ok(())
}
