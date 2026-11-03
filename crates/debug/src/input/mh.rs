//! mh — paylasimli MinHook (GwynMod `dlls\bin\MinHook.x64.dll`, DL1; earlier `mods\bin`) dinamik yukleyici.
//! ER-Lab "MinHook standardi" (2026-10-08, kb/ecosystem/minhook_standard.md). Ayni dosya
//! ride_anything, AdjustCameraDistance ve ER-INTDebugger `debug` crate'inde kopya olarak durur
//! (bagimliliksiz; edition 2021 + 2024 uyumlu).
//!
//! Yukleme sirasi (Gransax RagdollScale.h ile ayni):
//!   1) surecte zaten yuklu `MinHook.x64.dll` (baska bir mod yukledi)
//!   2) `<bu DLL'in klasoru>\bin\MinHook.x64.dll`  (me3: GwynMod\dlls\bin)
//!   3) `mods\bin\MinHook.x64.dll` (oyun cwd'ye gore; techiew/AdjustCameraDistance yolu)
//! Bulunamazsa / export eksikse / MH_Initialize hata verirse: log + `None` -> cagiran hook KURMAZ.
//! `MH_ERROR_ALREADY_INITIALIZED` (baska mod zaten baslatti) basari sayilir. Panik yok.
//! MH_Uninitialize ASLA cagrilmaz (paylasimli durum; diger modlarin hook'larini da kaldirirdi).

#![allow(dead_code)]

use std::ffi::c_void;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const MH_OK: i32 = 0;
pub const MH_ERROR_ALREADY_INITIALIZED: i32 = 1;
pub const DLL_NAME: &str = "MinHook.x64.dll";

type InitFn = unsafe extern "system" fn() -> i32;
type CreateFn = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> i32;
type TargetFn = unsafe extern "system" fn(*mut c_void) -> i32;
type StrFn = unsafe extern "system" fn(i32) -> *const u8;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
}

#[derive(Clone, Copy)]
pub struct Api {
    create: CreateFn,
    enable: TargetFn,
    disable: TargetFn,
    remove: TargetFn,
    to_str: Option<StrFn>,
}

static API: OnceLock<Option<Api>> = OnceLock::new();

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

unsafe fn sym(h: *mut c_void, name: &[u8]) -> *mut c_void {
    unsafe { GetProcAddress(h, name.as_ptr()) }
}

/// MinHook'u bul/yukle, export'lari coz, MH_Initialize. Sonuc ilk cagridan sonra onbellekte.
/// `self_dir`: cagiran DLL'in klasoru. `tag`: log oneki.
///
/// # Safety
/// Loader lock altinda (DllMain icinde) cagirma; ayri thread'den cagir.
pub unsafe fn load(self_dir: Option<&Path>, tag: &str, log: &dyn Fn(&str)) -> Option<&'static Api> {
    API.get_or_init(|| unsafe { load_inner(self_dir, tag, log) }).as_ref()
}

unsafe fn load_inner(self_dir: Option<&Path>, tag: &str, log: &dyn Fn(&str)) -> Option<Api> {
    let mut h = unsafe { GetModuleHandleW(wide(DLL_NAME.as_ref()).as_ptr()) };
    if !h.is_null() {
        log(&format!("[{tag}] MinHook: {DLL_NAME} zaten yuklu ({h:p})"));
    }
    if h.is_null() {
        if let Some(d) = self_dir {
            let p = d.join("bin").join(DLL_NAME);
            h = unsafe { LoadLibraryW(wide(p.as_os_str()).as_ptr()) };
            log(&format!("[{tag}] MinHook: {} -> {}", p.display(), if h.is_null() { "YOK" } else { "yuklendi" }));
        }
    }
    if h.is_null() {
        let p = "mods\\bin\\MinHook.x64.dll";
        h = unsafe { LoadLibraryW(wide(p.as_ref()).as_ptr()) };
        log(&format!("[{tag}] MinHook: {p} (cwd) -> {}", if h.is_null() { "YOK" } else { "yuklendi" }));
    }
    if h.is_null() {
        let e = unsafe { GetLastError() };
        log(&format!("[{tag}] MinHook.x64.dll BULUNAMADI (GetLastError {e}) -> hook YOK"));
        return None;
    }
    let (i, c, en, di, rm, ts) = unsafe {
        (
            sym(h, b"MH_Initialize\0"),
            sym(h, b"MH_CreateHook\0"),
            sym(h, b"MH_EnableHook\0"),
            sym(h, b"MH_DisableHook\0"),
            sym(h, b"MH_RemoveHook\0"),
            sym(h, b"MH_StatusToString\0"),
        )
    };
    if i.is_null() || c.is_null() || en.is_null() || di.is_null() || rm.is_null() {
        log(&format!(
            "[{tag}] MinHook export eksik (Init {i:p} Create {c:p} Enable {en:p} Disable {di:p} Remove {rm:p}) -> hook YOK"
        ));
        return None;
    }
    let (init, api) = unsafe {
        (
            std::mem::transmute::<*mut c_void, InitFn>(i),
            Api {
                create: std::mem::transmute::<*mut c_void, CreateFn>(c),
                enable: std::mem::transmute::<*mut c_void, TargetFn>(en),
                disable: std::mem::transmute::<*mut c_void, TargetFn>(di),
                remove: std::mem::transmute::<*mut c_void, TargetFn>(rm),
                to_str: if ts.is_null() { None } else { Some(std::mem::transmute::<*mut c_void, StrFn>(ts)) },
            },
        )
    };
    let st = unsafe { init() };
    if st != MH_OK && st != MH_ERROR_ALREADY_INITIALIZED {
        log(&format!("[{tag}] MH_Initialize: {} -> hook YOK", api.status(st)));
        return None;
    }
    log(&format!("[{tag}] MH_Initialize: {} (paylasimli MinHook {h:p})", api.status(st)));
    Some(api)
}

impl Api {
    /// MH_STATUS -> metin (MH_StatusToString varsa).
    pub fn status(&self, st: i32) -> String {
        if let Some(f) = self.to_str {
            let p = unsafe { f(st) };
            if !p.is_null() {
                let s = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) };
                return s.to_string_lossy().into_owned();
            }
        }
        format!("MH_STATUS({st})")
    }

    /// MH_CreateHook -> trampoline'i `slot`a yayinla -> MH_EnableHook. Enable basarisizsa
    /// MH_RemoveHook ile geri alinir ve `slot` sifirlanir. Cagiran prolog dogrulamasini ONCE yapar.
    ///
    /// # Safety
    /// `target` gecerli bir fonksiyon girisi, `detour` ayni imzali `extern "system"` fonksiyon olmali.
    pub unsafe fn install(&self, target: usize, detour: usize, slot: &AtomicUsize) -> Result<(), String> {
        let mut orig: *mut c_void = std::ptr::null_mut();
        let st = unsafe { (self.create)(target as *mut c_void, detour as *mut c_void, &mut orig) };
        if st != MH_OK {
            return Err(format!("MH_CreateHook: {}", self.status(st)));
        }
        if orig.is_null() {
            let _ = unsafe { (self.remove)(target as *mut c_void) };
            return Err("MH_CreateHook: trampoline null".into());
        }
        slot.store(orig as usize, Ordering::SeqCst);
        let st = unsafe { (self.enable)(target as *mut c_void) };
        if st != MH_OK {
            let r = unsafe { (self.remove)(target as *mut c_void) };
            slot.store(0, Ordering::SeqCst);
            return Err(format!("MH_EnableHook: {} (MH_RemoveHook: {})", self.status(st), self.status(r)));
        }
        Ok(())
    }

    /// # Safety
    /// `target` bu Api ile kurulmus bir hook olmali.
    pub unsafe fn disable(&self, target: usize) -> Result<(), String> {
        let st = unsafe { (self.disable)(target as *mut c_void) };
        if st == MH_OK { Ok(()) } else { Err(format!("MH_DisableHook: {}", self.status(st))) }
    }

    /// # Safety
    /// `target` bu Api ile kurulmus bir hook olmali; trampoline artik cagrilmamali.
    pub unsafe fn remove(&self, target: usize) -> Result<(), String> {
        let st = unsafe { (self.remove)(target as *mut c_void) };
        if st == MH_OK { Ok(()) } else { Err(format!("MH_RemoveHook: {}", self.status(st))) }
    }
}
