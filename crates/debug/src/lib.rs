use hudhook::windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH};
use hudhook::{Hooks, Hudhook, ImguiRenderLoop, eject};
use tracing_panic::panic_hook;

mod clipboard;
mod display;
mod ext;
mod input;
mod state_map;

pub use clipboard::*;
pub use ext::*;
pub use input::*;
pub use state_map::*;

/// Initializes the debug tool as a DLL plugin, setting up a panic handler,
/// tracing, and the basic hooks into the underlying rendering system.
///
/// The `wait_for_system_init` callback should block until the core of the
/// underlying game systems are initialized.
///
/// This doesn't automatically set up any hotpatches; however, it is
/// hotpatch-safe, and won't re-run the core initialization logic when run from
/// a hotpatched DLL.
pub fn initialize<T>(
    hmodule: HINSTANCE,
    reason: u32,
    wait_for_system_init: impl FnOnce() + Send + 'static,
    render_loop: impl ImguiRenderLoop + Send + Sync + 'static,
) -> i32
where
    T: Hooks + 'static,
{
    if reason != DLL_PROCESS_ATTACH {
        return 1;
    }

    // Check if this DLL is being loaded by `libhotpatch` and skip initialization.
    if libhotpatch::is_hotpatched() {
        return 1;
    }

    std::panic::set_hook(Box::new(panic_hook));

    // ER-Lab 2026-10-07: log DLL'in yanina (cwd'ye degil).
    let log_dir = dll_dir(hmodule.0 as usize).unwrap_or_else(|| std::path::PathBuf::from("./"));
    // ER-Lab 2026-10-08 rename: dosya adi DLL adindan (`<dll adi>-chains.log`, ornek
    // `ER-INT-Debugger-chains.log`); ad cozulemezse eski `chains-debug.log`.
    let log_name = dll_stem(hmodule.0 as usize)
        .map(|s| format!("{s}-chains.log"))
        .unwrap_or_else(|| "chains-debug.log".to_string());
    let appender = tracing_appender::rolling::never(log_dir, log_name);
    tracing_subscriber::fmt().with_writer(appender).init();

    let hmodule_raw = hmodule.0 as usize;

    std::thread::spawn(move || {
        wait_for_system_init();

        if let Err(e) = Hudhook::builder()
            .with::<T>(render_loop)
            .with_hmodule(HINSTANCE(hmodule_raw as *mut _))
            .build()
            .apply()
        {
            tracing::error!("Couldn't apply hooks: {e:?}");
            eject();
        }
    });

    1
}

// ER-Lab 2026-10-07: DLL'in bulundugu klasor.
fn dll_dir(hmodule: usize) -> Option<std::path::PathBuf> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleFileNameW(h_module: usize, lp_filename: *mut u16, n_size: u32) -> u32;
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(hmodule, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])).parent().map(|d| d.to_path_buf())
}

/// ER-Lab 2026-10-08: DLL dosya adi (uzantisiz), log adini DLL adiyla eslemek icin.
fn dll_stem(hmodule: usize) -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleFileNameW(h_module: usize, lp_filename: *mut u16, n_size: u32) -> u32;
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(hmodule, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    std::path::Path::new(&String::from_utf16_lossy(&buf[..n]))
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}
